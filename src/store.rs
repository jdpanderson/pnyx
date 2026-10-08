//! An acceptor whose state is kept in two files, so that a crash during a save
//! leaves the state before the save.
//!
//! [`Stored`] keeps two copies of the state, `<path>.0` and `<path>.1`. Each
//! save overwrites the copy that doesn't hold the current state, and syncs it
//! to disk. Each copy holds a sequence number and a checksum, so a copy that
//! was being written during a crash is found and not used. Saves don't rename
//! or create files, so they are durable on every system with a working sync of
//! file data, including Windows.
//!
//! Files are created only when a store is created, and their creation is
//! durable once the directory is synced. On Unix, the directory is synced
//! then. Windows has no documented way to sync a directory, so a power loss
//! right after a store is created on Windows can undo the creation of its
//! files.
//!
//! The functions here block on file I/O, including a sync to disk. In async
//! code, call them where blocking is allowed, such as in
//! `tokio::task::spawn_blocking`.

use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Serialize, de::DeserializeOwned};

use crate::{Acceptor, Agreed, Ballot, Chosen, InvalidRequest, Reply, Request};

/// The start of each copy: a magic number and the format version. The version
/// changes when the layout of a copy or the stored form of [`Acceptor`]
/// changes.
const MAGIC: &[u8; 4] = b"pnyx";
const FORMAT_VERSION: u16 = 1;

/// A copy of the state, with all numbers little-endian:
///
/// | Bytes      | Field                                    |
/// | ---------- | ---------------------------------------- |
/// | 0..4       | [`MAGIC`]                                |
/// | 4..6       | [`FORMAT_VERSION`]                       |
/// | 6..14      | sequence number, one more for each save  |
/// | 14..22     | body length                              |
/// | 22..end    | body: the acceptor, in postcard          |
/// | end..end+4 | CRC-32 of bytes 0..end                   |
///
/// Any bytes after the checksum are ignored.
const HEADER_LEN: usize = 22;

/// An acceptor whose state is saved to disk before each reply.
///
/// The state is kept in two files, `<path>.0` and `<path>.1` (see the
/// [module documentation](self)). A copy with another format version makes
/// [`Stored::open`] fail with [`io::ErrorKind::InvalidData`].
///
/// Each change blocks until the state is synced to disk.
#[derive(Debug)]
pub struct Stored<N, V, P = ()> {
    acceptor: Acceptor<N, V, P>,
    /// The two copies.
    paths: [PathBuf; 2],
    /// Which copy holds the current state.
    current: usize,
    /// The current state's sequence number.
    sequence: u64,
}

/// Why a [`Stored`] acceptor refused or failed a change.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The acceptor refused the request; see [`InvalidRequest`].
    #[error(transparent)]
    Invalid(#[from] InvalidRequest),
    /// The new state could not be saved. The state is left as it was.
    #[error("can't save the acceptor state")]
    Io(#[from] io::Error),
}

impl<N, V, P> Stored<N, V, P>
where
    N: Clone + Ord + Serialize + DeserializeOwned,
    V: Clone + PartialEq + Serialize + DeserializeOwned,
    P: Clone + Serialize + DeserializeOwned,
{
    /// Opens the acceptor state in `<path>.0` and `<path>.1`. If neither file
    /// exists, it creates them with an empty acceptor.
    ///
    /// # Errors
    ///
    /// Fails if a file can't be read or written, or with
    /// [`io::ErrorKind::InvalidData`] if neither copy is valid or a copy has
    /// another format version.
    pub fn open(path: PathBuf) -> io::Result<Self> {
        let paths = copy_paths(path);
        let copies = [read_copy(&paths[0])?, read_copy(&paths[1])?];
        if copies.iter().all(|c| matches!(c, Found::Missing)) {
            return Self::create_at(paths, Acceptor::default());
        }
        let valid = copies.iter().enumerate().filter_map(|(i, c)| match c {
            Found::Valid { sequence, .. } => Some((*sequence, i)),
            _ => None,
        });
        let Some((sequence, current)) = valid.max() else {
            return Err(invalid_data(format!(
                "no valid copy of the acceptor state in {} or {}",
                paths[0].display(),
                paths[1].display(),
            )));
        };
        let other = 1 - current;
        if matches!(&copies[other], Found::Valid { sequence: s, .. } if *s == sequence) {
            return Err(invalid_data(format!(
                "both copies of the acceptor state in {} have sequence number {sequence}",
                paths[0].display(),
            )));
        }
        let missing = matches!(copies[other], Found::Missing);
        let [a, b] = copies;
        let Found::Valid { acceptor, .. } = (if current == 0 { a } else { b }) else {
            unreachable!("the current copy is valid");
        };
        let mut stored = Stored {
            acceptor,
            paths,
            current,
            sequence,
        };
        if missing {
            // A crash while the store was created left only one file. Create
            // the other now, so that saves never create files.
            stored.save(&stored.acceptor.clone(), Mode::Create)?;
            sync_dir(&stored.paths[0])?;
        }
        Ok(stored)
    }

    /// Saves `acceptor` as the state in `<path>.0` and `<path>.1`, in place of
    /// any state there.
    ///
    /// # Errors
    ///
    /// Fails if a file can't be written.
    pub fn create(path: PathBuf, acceptor: Acceptor<N, V, P>) -> io::Result<Self> {
        Self::create_at(copy_paths(path), acceptor)
    }

    fn create_at(paths: [PathBuf; 2], acceptor: Acceptor<N, V, P>) -> io::Result<Self> {
        // Copy 1 is written last, so until the store is created, a valid
        // copy 1 from before keeps the state from before.
        let mut stored = Stored {
            acceptor,
            paths,
            current: 1,
            sequence: 0,
        };
        let acceptor = stored.acceptor.clone();
        stored.save(&acceptor, Mode::Create)?;
        stored.save(&acceptor, Mode::Create)?;
        sync_dir(&stored.paths[0])?;
        Ok(stored)
    }

    /// The acceptor, as last saved.
    pub fn acceptor(&self) -> &Acceptor<N, V, P> {
        &self.acceptor
    }

    /// Answers a request, once the new state is on disk (see
    /// [`Acceptor::handle`]).
    ///
    /// # Errors
    ///
    /// Fails if the request is invalid, or if the new state can't be saved.
    /// In both cases, the state is left as it was.
    pub fn handle(&mut self, req: Request<N, V>) -> Result<Reply<N, V>, Error> {
        self.update(|next| Ok(next.handle(req)?))
    }

    /// Records an endorsement, once the new state is on disk (see
    /// [`Acceptor::endorse`]).
    ///
    /// # Errors
    ///
    /// Fails if the endorsement is invalid, or if the new state can't be
    /// saved. In both cases, the state is left as it was.
    pub fn endorse(&mut self, ballot: Ballot<N>, value: Agreed<N, V>) -> Result<(), Error> {
        self.update(|next| {
            next.endorse(ballot, value)?;
            Ok(((), true))
        })
    }

    /// Answers a request with its proof, once the new state is on disk (see
    /// [`Acceptor::handle_proven`]).
    ///
    /// # Errors
    ///
    /// Fails if the request is invalid, or if the new state can't be saved.
    /// In both cases, the state is left as it was.
    pub fn handle_proven(&mut self, req: Request<N, V>, proof: P) -> Result<Reply<N, V>, Error> {
        self.update(|next| Ok(next.handle_proven(req, proof)?))
    }

    /// Records an agreed value (see [`Acceptor::learn`]).
    ///
    /// # Errors
    ///
    /// Fails if the new state can't be saved. The state is then left as it
    /// was.
    pub fn learn(&mut self, chosen: Chosen<N, V>, proof: P) -> io::Result<()> {
        self.update(|next| Ok(((), next.learn(chosen, proof))))
    }

    /// Every mutation uses the same save-before-publish boundary. Changes
    /// become visible in memory only after their save succeeds.
    fn update<R, E: From<io::Error>>(
        &mut self,
        change: impl FnOnce(&mut Acceptor<N, V, P>) -> Result<(R, bool), E>,
    ) -> Result<R, E> {
        let mut next = self.acceptor.clone();
        let (result, changed) = change(&mut next)?;
        if changed {
            self.save(&next, Mode::Overwrite)?;
            self.acceptor = next;
        }
        Ok(result)
    }

    /// Writes `acceptor` over the copy that doesn't hold the current state,
    /// and makes that copy the current one.
    ///
    /// If the write fails, the current copy is still the current one, and
    /// the next save writes the other copy again.
    fn save(&mut self, acceptor: &Acceptor<N, V, P>, mode: Mode) -> io::Result<()> {
        let next = 1 - self.current;
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| io::Error::other("no sequence numbers are left"))?;
        let bytes = encode(sequence, acceptor)?;
        write_copy(&self.paths[next], &bytes, mode)?;
        self.current = next;
        self.sequence = sequence;
        Ok(())
    }
}

/// How [`write_copy`] opens its file.
#[derive(Clone, Copy)]
enum Mode {
    /// Create the file if it doesn't exist.
    Create,
    /// Write only a file that exists. Saves use this, because a new file is
    /// durable only once its directory is synced.
    Overwrite,
}

fn copy_paths(path: PathBuf) -> [PathBuf; 2] {
    let with = |suffix: &str| {
        let mut p = OsString::from(path.as_os_str());
        p.push(suffix);
        PathBuf::from(p)
    };
    [with(".0"), with(".1")]
}

fn encode<T: Serialize>(sequence: u64, value: &T) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(256);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&sequence.to_le_bytes());
    // The body length, set once the body is written.
    bytes.extend_from_slice(&[0; 8]);
    let mut bytes = postcard::to_extend(value, bytes).map_err(io::Error::other)?;
    let len = (bytes.len() - HEADER_LEN) as u64;
    bytes[14..HEADER_LEN].copy_from_slice(&len.to_le_bytes());
    let checksum = crc32fast::hash(&bytes);
    bytes.extend_from_slice(&checksum.to_le_bytes());
    Ok(bytes)
}

/// What [`read_copy`] found in one file.
enum Found<T> {
    Missing,
    /// Not a complete copy: a write to it was interrupted, or the file is
    /// damaged.
    Torn,
    Valid {
        sequence: u64,
        acceptor: T,
    },
}

fn read_copy<T: DeserializeOwned>(path: &Path) -> io::Result<Found<T>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Found::Missing),
        Err(e) => return Err(e),
    };
    if bytes.get(..MAGIC.len()) != Some(MAGIC) {
        return Ok(Found::Torn);
    }
    let Some(version) = field(&bytes, 4).map(u16::from_le_bytes) else {
        return Ok(Found::Torn);
    };
    // A torn write doesn't change these bytes: both the old and the new
    // contents have the same magic number and version.
    if version != FORMAT_VERSION {
        return Err(invalid_data(format!(
            "{} has format version {version}; this version of pnyx reads only version {FORMAT_VERSION}",
            path.display()
        )));
    }
    let (Some(sequence), Some(len)) = (
        field(&bytes, 6).map(u64::from_le_bytes),
        field(&bytes, 14).map(u64::from_le_bytes),
    ) else {
        return Ok(Found::Torn);
    };
    let Some(end) = usize::try_from(len)
        .ok()
        .and_then(|len| HEADER_LEN.checked_add(len))
    else {
        return Ok(Found::Torn);
    };
    let (Some(covered), Some(checksum)) =
        (bytes.get(..end), field(&bytes, end).map(u32::from_le_bytes))
    else {
        return Ok(Found::Torn);
    };
    if crc32fast::hash(covered) != checksum {
        return Ok(Found::Torn);
    }
    // The checksum matches, so these are the bytes that were written. If
    // they don't decode, the caller's types don't match the file.
    let acceptor = postcard::from_bytes(&covered[HEADER_LEN..])
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Found::Valid { sequence, acceptor })
}

/// `N` bytes of `bytes`, starting at `at`.
fn field<const N: usize>(bytes: &[u8], at: usize) -> Option<[u8; N]> {
    bytes.get(at..at.checked_add(N)?)?.try_into().ok()
}

/// Writes `bytes` from the start of the file at `path`, and syncs it.
fn write_copy(path: &Path, bytes: &[u8], mode: Mode) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(matches!(mode, Mode::Create))
        .truncate(false)
        .open(path)?;
    file.write_all(bytes)?;
    // Bytes after the checksum are ignored, so this only keeps the file
    // small.
    file.set_len(bytes.len() as u64)?;
    file.sync_all()
}

/// Makes the creation of files in the directory of `path` durable.
#[cfg(unix)]
fn sync_dir(path: &Path) -> io::Result<()> {
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    fs::File::open(dir)?.sync_all()
}

/// Windows has no documented way to sync a directory (see the
/// [module documentation](self)).
#[cfg(not(unix))]
fn sync_dir(_: &Path) -> io::Result<()> {
    Ok(())
}

fn invalid_data(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests;
