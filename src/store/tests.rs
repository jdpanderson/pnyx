use super::*;

type Store = Stored<u8, String>;

fn genesis() -> Acceptor<u8, String> {
    Acceptor::genesis(Chosen::genesis(1, "g".into()), ())
}

fn prepare(counter: u64) -> Request<u8, String> {
    Request::Prepare {
        config: 0,
        ballot: Ballot { counter, node: 2 },
        have: None,
    }
}

/// A store in a new directory, with its base path and its two copies.
fn new_store() -> (tempfile::TempDir, PathBuf, [PathBuf; 2], Store) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("acceptor");
    let store = Store::create(path.clone(), genesis()).unwrap();
    let copies = copy_paths(&path);
    (dir, path, copies, store)
}

/// Changes the bytes of one copy.
type Damage = fn(&mut Vec<u8>);
/// Changes the files of a store.
type Break = fn(&[PathBuf; 2]);

/// Reads and decodes one copy.
fn read(copy: &Path) -> io::Result<Found<Acceptor<u8, String>>> {
    decode(read_copy(copy)?)
}

fn is_valid(copy: &Path) -> bool {
    matches!(read(copy), Ok(Found::Valid { .. }))
}

/// A store whose newer copy is copy `newer`, with its base path and copies.
fn store_with_newer_copy(newer: usize) -> (tempfile::TempDir, PathBuf, [PathBuf; 2], Store) {
    let (dir, path, copies, mut store) = new_store();
    // `create` leaves copy 1 newer; one more save makes copy 0 newer.
    if newer == 0 {
        store.handle(prepare(3)).unwrap();
    }
    (dir, path, copies, store)
}

/// Whether the directory of a store holds no files.
fn is_empty(dir: &tempfile::TempDir) -> bool {
    fs::read_dir(dir.path()).unwrap().count() == 0
}

#[test]
fn stored_acceptor_survives_a_restart() {
    let (_dir, path, _, mut a) = new_store();
    a.handle(prepare(3)).unwrap();
    a.handle(prepare(4)).unwrap();

    let b = Store::open(path).unwrap();
    assert_eq!(a.acceptor(), b.acceptor());
}

#[test]
fn open_creates_an_empty_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("acceptor");
    let a = Store::open(path.clone()).unwrap();
    assert_eq!(a.acceptor(), &Acceptor::default());
    for copy in copy_paths(&path) {
        assert!(is_valid(&copy), "{}", copy.display());
    }
    assert_eq!(Store::open(path).unwrap().acceptor(), &Acceptor::default());
}

#[test]
fn open_existing_creates_nothing_when_no_file_exists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("acceptor");
    assert!(Store::open_existing(path.clone()).unwrap().is_none());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn open_existing_opens_a_store() {
    let (_dir, path, _, mut a) = new_store();
    a.handle(prepare(3)).unwrap();
    let b = Store::open_existing(path).unwrap().unwrap();
    assert_eq!(a.acceptor(), b.acceptor());
}

#[test]
fn open_existing_opens_a_store_with_one_copy_and_writes_the_other() {
    let (_dir, path, copies, a) = new_store();
    fs::remove_file(&copies[1]).unwrap();
    let b = Store::open_existing(path).unwrap().unwrap();
    assert_eq!(b.acceptor(), a.acceptor());
    assert!(is_valid(&copies[1]));
}

#[test]
fn open_existing_refuses_invalid_stores() {
    let (_dir, path, copies, _) = new_store();
    for copy in &copies {
        fs::write(copy, b"pnyx").unwrap();
    }
    let err = Store::open_existing(path).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{err}");
}

#[test]
fn copy_paths_add_a_suffix_to_the_path() {
    let dir = Path::new("dir");
    assert_eq!(
        copy_paths(&dir.join("acceptor")),
        [dir.join("acceptor.0"), dir.join("acceptor.1")]
    );
    // The suffix is added, not put in place of an extension.
    assert_eq!(
        copy_paths(&dir.join("acceptor.bin")),
        [dir.join("acceptor.bin.0"), dir.join("acceptor.bin.1")]
    );
}

#[test]
fn remove_deletes_a_store() {
    for newer in 0..2 {
        let (dir, path, _, _) = store_with_newer_copy(newer);
        remove(&path).unwrap();
        assert!(is_empty(&dir));
        assert!(Store::open_existing(path).unwrap().is_none());
    }
}

#[test]
fn remove_without_a_store_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("acceptor");
    remove(&path).unwrap();
    remove(&path).unwrap();
    assert!(is_empty(&dir));
}

#[test]
fn remove_invalidates_the_older_copy_first() {
    for newer in 0..2 {
        let (_dir, _, copies, _) = store_with_newer_copy(newer);
        let older = 1 - newer;
        assert_eq!(
            removal(&copies).unwrap(),
            [
                Step::Invalidate(older),
                Step::Invalidate(newer),
                Step::Delete(older),
                Step::Delete(newer),
                Step::SyncDir,
            ]
        );
    }
}

/// After a crash at any step of `remove`, a store opens as the state from
/// before, or as no store; and `remove` again finishes the removal.
#[test]
fn a_crash_during_remove_never_gives_an_older_state() {
    for newer in 0..2 {
        let (_dir, _, copies, _) = store_with_newer_copy(newer);
        let steps = removal(&copies).unwrap();
        for done in 0..=steps.len() {
            let (dir, path, copies, a) = store_with_newer_copy(newer);
            for step in &steps[..done] {
                step.run(&copies).unwrap();
            }
            let case = format!("newer copy {newer}, {done} steps done");
            match Store::open_existing(path.clone()).unwrap() {
                Some(b) => {
                    // Once one copy holds the removed marker, the store
                    // opens as no store.
                    assert_eq!(done, 0, "{case}");
                    assert_eq!(b.acceptor(), a.acceptor(), "{case}");
                }
                None => assert!(done > 0, "{case}"),
            }
            remove(&path).unwrap();
            assert!(is_empty(&dir), "{case}");
        }
    }
}

/// A crash while the removed marker is written leaves part of it over the
/// copy. The store then opens as the state from before, or as no store.
#[test]
fn a_torn_removed_marker_never_gives_an_older_state() {
    let marker = encode_removed().unwrap();
    for newer in 0..2 {
        for (done, torn) in [(0, 1 - newer), (1, newer)] {
            for written in 0..=marker.len() {
                let (dir, path, copies, a) = store_with_newer_copy(newer);
                let steps = removal(&copies).unwrap();
                for step in &steps[..done] {
                    step.run(&copies).unwrap();
                }
                let mut bytes = fs::read(&copies[torn]).unwrap();
                bytes[..written].copy_from_slice(&marker[..written]);
                fs::write(&copies[torn], bytes).unwrap();

                let case = format!("newer copy {newer}, copy {torn} torn at {written}");
                match Store::open_existing(path.clone()).unwrap() {
                    Some(b) => assert_eq!(b.acceptor(), a.acceptor(), "{case}"),
                    None => assert!(done == 1 || written == marker.len(), "{case}"),
                }
                remove(&path).unwrap();
                assert!(is_empty(&dir), "{case}");
            }
        }
    }
}

#[test]
fn open_creates_an_empty_store_after_a_crash_during_remove() {
    let (_dir, path, copies, _) = new_store();
    for step in &removal(&copies).unwrap()[..2] {
        step.run(&copies).unwrap();
    }
    assert_eq!(
        Store::open(path.clone()).unwrap().acceptor(),
        &Acceptor::default()
    );
    assert!(copies.iter().all(|c| is_valid(c)));
}

#[test]
fn remove_deletes_a_damaged_store() {
    let (dir, path, copies, _) = new_store();
    for copy in &copies {
        fs::write(copy, b"pnyx").unwrap();
    }
    remove(&path).unwrap();
    assert!(is_empty(&dir));
}

/// When which copy is newer is not known, no order of removal is safe.
#[test]
fn remove_refuses_a_store_without_a_known_newer_copy() {
    let cases: [(&str, Break); 2] = [
        ("the older copy has another version", |copies| {
            let mut bytes = fs::read(&copies[0]).unwrap();
            bytes[4..6].copy_from_slice(&2u16.to_le_bytes());
            fs::write(&copies[0], bytes).unwrap();
        }),
        ("the same sequence number in both", |copies| {
            fs::copy(&copies[1], &copies[0]).unwrap();
        }),
    ];
    for (case, break_store) in cases {
        let (_dir, path, copies, _) = new_store();
        break_store(&copies);
        let files = copies.each_ref().map(|c| fs::read(c).unwrap());
        let err = remove(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{case}: {err}");
        assert_eq!(
            copies.each_ref().map(|c| fs::read(c).unwrap()),
            files,
            "{case}"
        );
    }
}

#[test]
fn a_removed_marker_with_a_body_is_refused() {
    let (_dir, path, copies, _) = new_store();
    fs::write(&copies[0], encode(REMOVED, &"a body").unwrap()).unwrap();
    let err = Store::open_existing(path).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{err}");
}

#[test]
fn copy_layout() {
    let (_dir, _, copies, _) = new_store();
    for (copy, sequence) in copies.iter().zip([1u64, 2]) {
        let bytes = fs::read(copy).unwrap();
        assert_eq!(&bytes[..4], b"pnyx");
        assert_eq!(&bytes[4..6], &1u16.to_le_bytes());
        assert_eq!(&bytes[6..14], &sequence.to_le_bytes());
        let len = u64::from_le_bytes(bytes[14..22].try_into().unwrap()) as usize;
        let end = 22 + len;
        assert_eq!(bytes.len(), end + 4);
        assert_eq!(&bytes[end..], &crc32fast::hash(&bytes[..end]).to_le_bytes());
        assert_eq!(
            postcard::from_bytes::<Acceptor<u8, String>>(&bytes[22..end]).unwrap(),
            genesis()
        );
    }
}

#[test]
fn a_torn_copy_is_not_used_and_is_written_next() {
    let (_dir, path, copies, mut a) = new_store();
    let before = a.acceptor().clone();
    // This save goes to copy 0, which then holds the newest state.
    a.handle(prepare(3)).unwrap();
    let saved = fs::read(&copies[0]).unwrap();

    let damage: [(&str, Damage); 7] = [
        ("cut short", |b| b.truncate(b.len() / 2)),
        ("header cut short", |b| b.truncate(10)),
        ("body changed", |b| b[30] ^= 1),
        ("checksum cut short", |b| b.truncate(b.len() - 1)),
        ("magic number zeroed", |b| b[..4].fill(0)),
        // Lengths so large that the end of the body or of the checksum
        // can't be computed.
        ("largest length", |b| {
            b[14..22].copy_from_slice(&u64::MAX.to_le_bytes())
        }),
        ("length ending at the largest offset", |b| {
            b[14..22].copy_from_slice(&(u64::MAX - HEADER_LEN as u64).to_le_bytes())
        }),
    ];
    for (case, damage) in damage {
        let mut bytes = saved.clone();
        damage(&mut bytes);
        fs::write(&copies[0], bytes).unwrap();

        let mut b = Store::open(path.clone()).unwrap();
        assert_eq!(b.acceptor(), &before, "{case}");
        // The next save overwrites the torn copy.
        b.handle(prepare(5)).unwrap();
        assert!(is_valid(&copies[0]), "{case}");
        assert_eq!(
            Store::open(path.clone()).unwrap().acceptor(),
            b.acceptor(),
            "{case}"
        );
    }
}

#[test]
fn bytes_after_the_checksum_are_ignored() {
    let (_dir, path, copies, mut a) = new_store();
    a.handle(prepare(3)).unwrap();
    let mut file = OpenOptions::new().append(true).open(&copies[0]).unwrap();
    file.write_all(&[0xff; 100]).unwrap();
    assert_eq!(Store::open(path).unwrap().acceptor(), a.acceptor());
}

#[test]
fn a_missing_copy_is_created_again() {
    let (_dir, path, copies, a) = new_store();
    fs::remove_file(&copies[0]).unwrap();
    let mut b = Store::open(path.clone()).unwrap();
    assert_eq!(b.acceptor(), a.acceptor());
    assert!(is_valid(&copies[0]));
    // Saves don't create files, so both copies must exist for these.
    b.handle(prepare(3)).unwrap();
    b.handle(prepare(4)).unwrap();
    assert_eq!(Store::open(path).unwrap().acceptor(), b.acceptor());
}

#[test]
fn saves_never_create_files() {
    let (_dir, _, copies, mut a) = new_store();
    let before = a.acceptor().clone();
    fs::remove_file(&copies[0]).unwrap();
    let err = a.handle(prepare(3)).unwrap_err();
    assert!(
        matches!(&err, Error::Io(e) if e.kind() == io::ErrorKind::NotFound),
        "{err:?}"
    );
    assert_eq!(a.acceptor(), &before);
    assert!(!copies[0].exists());
}

#[test]
fn invalid_stores_are_refused() {
    let cases: [(&str, Break); 5] = [
        ("both copies torn", |copies| {
            for copy in copies {
                fs::write(copy, b"pnyx").unwrap();
            }
        }),
        ("one copy torn and one missing", |copies| {
            fs::write(&copies[0], b"pn").unwrap();
            fs::remove_file(&copies[1]).unwrap();
        }),
        // The newer copy is valid, but the older one may be from a newer
        // version of pnyx, so neither can be trusted.
        ("the older copy has another version", |copies| {
            let mut bytes = fs::read(&copies[0]).unwrap();
            bytes[4..6].copy_from_slice(&2u16.to_le_bytes());
            fs::write(&copies[0], bytes).unwrap();
        }),
        ("the same sequence number in both", |copies| {
            fs::copy(&copies[1], &copies[0]).unwrap();
        }),
        ("a valid copy of other types", |copies| {
            fs::write(&copies[1], encode(9, &"not an acceptor").unwrap()).unwrap();
        }),
    ];
    for (case, break_store) in cases {
        let (_dir, path, copies, _) = new_store();
        break_store(&copies);
        let err = Store::open(path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{case}: {err}");
    }
}

#[test]
fn create_replaces_any_state() {
    let (_dir, path, copies, mut a) = new_store();
    for counter in 3..10 {
        a.handle(prepare(counter)).unwrap();
    }
    fs::write(&copies[0], b"not a copy").unwrap();
    let b = Store::create(path.clone(), genesis()).unwrap();
    assert_eq!(b.acceptor(), &genesis());
    assert_eq!(Store::open(path).unwrap().acceptor(), &genesis());
}

#[test]
fn failed_writes_preserve_memory_and_disk_and_can_be_retried() {
    let (_dir, path, copies, _) = new_store();
    let before = genesis();
    let ballot = Ballot {
        counter: 3,
        node: 2,
    };
    let mut value = before.learned().unwrap().value.clone();
    value.version += 1;
    value.value = "next".into();
    let chosen = Chosen {
        value: value.clone(),
        ballot: Some(ballot.clone()),
    };
    for operation in 0..4 {
        let mut a = Store::create(path.clone(), before.clone()).unwrap();
        let apply = |a: &mut Store| -> Result<(), Error> {
            match operation {
                0 => a.handle(prepare(3)).map(drop),
                1 => a.endorse(ballot.clone(), value.clone()),
                2 => a
                    .handle_proven(
                        Request::Accept {
                            config: 0,
                            ballot: ballot.clone(),
                            value: value.clone(),
                        },
                        (),
                    )
                    .map(drop),
                3 => match a.learn(chosen.clone(), ()) {
                    Ok(learned) => {
                        assert!(learned);
                        Ok(())
                    }
                    Err(e) => Err(Error::from(e)),
                },
                _ => unreachable!(),
            }
        };
        // The next save goes to copy 0. A directory in its place makes the
        // save fail.
        let saved = fs::read(&copies[0]).unwrap();
        fs::remove_file(&copies[0]).unwrap();
        fs::create_dir(&copies[0]).unwrap();
        assert!(matches!(apply(&mut a), Err(Error::Io(_))));
        assert_eq!(a.acceptor(), &before);
        assert!(matches!(
            read(&copies[1]).unwrap(),
            Found::Valid { acceptor, .. } if acceptor == before
        ));

        fs::remove_dir(&copies[0]).unwrap();
        fs::write(&copies[0], saved).unwrap();
        apply(&mut a).unwrap();
        assert_ne!(a.acceptor(), &before);
        assert_eq!(Store::open(path.clone()).unwrap().acceptor(), a.acceptor());
    }

    // Learning an older value doesn't need a write, even with a bad disk.
    let mut a = Store::create(path, before.clone()).unwrap();
    assert!(a.learn(chosen.clone(), ()).unwrap());
    for copy in &copies {
        fs::remove_file(copy).unwrap();
        fs::create_dir(copy).unwrap();
    }
    assert!(!a.learn(before.learned().unwrap().clone(), ()).unwrap());
    // Learning the same value again doesn't need a write either.
    assert!(!a.learn(chosen, ()).unwrap());
    assert_eq!(a.acceptor().learned().unwrap().state(), "next");
}

#[test]
fn an_unreadable_copy_is_an_error_not_an_empty_store() {
    let (_dir, path, copies, _) = new_store();
    fs::remove_file(&copies[0]).unwrap();
    fs::create_dir(&copies[0]).unwrap();
    assert!(Store::open(path).is_err());
}

#[test]
fn a_store_in_a_missing_directory_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing").join("acceptor");
    assert!(Store::open(path.clone()).is_err());
    assert!(Store::create(path, genesis()).is_err());
}

#[test]
fn refused_requests_save_nothing() {
    let (_dir, _, copies, mut a) = new_store();
    let before = a.acceptor().clone();
    let files = copies.each_ref().map(|c| fs::read(c).unwrap());

    // Another value at the ballot that genesis accepted its value at.
    let genesis = before.learned().unwrap();
    let ballot = genesis.ballot.clone().unwrap();
    let mut other = genesis.value.clone();
    other.value = "other".into();
    let accept = Request::Accept {
        config: 0,
        ballot: ballot.clone(),
        value: other.clone(),
    };
    assert!(matches!(a.handle(accept.clone()), Err(Error::Invalid(_))));
    assert!(matches!(
        a.handle_proven(accept, ()),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(a.endorse(ballot, other), Err(Error::Invalid(_))));

    assert_eq!(a.acceptor(), &before);
    assert_eq!(copies.each_ref().map(|c| fs::read(c).unwrap()), files);
}

#[test]
fn a_save_fails_when_no_sequence_numbers_are_left() {
    let (_dir, path, copies, _) = new_store();
    fs::write(&copies[1], encode(u64::MAX, &genesis()).unwrap()).unwrap();
    let mut a = Store::open(path).unwrap();
    let err = a.handle(prepare(3)).unwrap_err();
    assert!(matches!(err, Error::Io(_)), "{err:?}");
    assert_eq!(a.acceptor(), &genesis());
}

#[cfg(unix)]
#[test]
fn a_path_without_a_directory_syncs_the_current_directory() {
    sync_dir(Path::new("acceptor")).unwrap();
}

mod properties {
    use proptest::prelude::*;

    use super::*;

    /// Bytes in place of a copy: random, or a valid start of a copy
    /// (magic number and version) followed by random bytes, so that they get
    /// past the first checks.
    fn damage() -> impl Strategy<Value = Vec<u8>> {
        (any::<bool>(), prop::collection::vec(any::<u8>(), 0..120)).prop_map(
            |(with_header, rest)| {
                let mut bytes = Vec::new();
                if with_header {
                    bytes.extend_from_slice(MAGIC);
                    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
                }
                bytes.extend(rest);
                bytes
            },
        )
    }

    proptest! {
        /// Whatever bytes replace one or both copies, opening never panics,
        /// and never loads a state that wasn't saved: it loads the intact
        /// copy, or fails with `InvalidData`.
        #[test]
        fn damaged_copies_never_load_as_another_state(
            first in damage(),
            second in prop::option::of(damage()),
            which in 0usize..2,
        ) {
            let (_dir, path, copies, a) = new_store();
            fs::write(&copies[which], first).unwrap();
            if let Some(second) = second {
                fs::write(&copies[1 - which], second).unwrap();
            }
            match Store::open(path) {
                Ok(b) => prop_assert_eq!(b.acceptor(), a.acceptor()),
                Err(e) => prop_assert_eq!(e.kind(), io::ErrorKind::InvalidData),
            }
        }
    }
}
