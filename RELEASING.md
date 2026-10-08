# Releasing

## Each release

1. Update `version` in `Cargo.toml`, and run `cargo check` to update
   `Cargo.lock`.
2. In `CHANGELOG.md`, rename `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD`
   and start a new empty `## [Unreleased]` section above it.
3. Merge this to `main` and wait for CI to pass.
4. Tag the commit on `main`, and push the tag:

   ```sh
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```

The [release workflow](.github/workflows/release.yml) then:

- checks that the tag matches the version, and that `CHANGELOG.md` has a
  section for it,
- runs the tests,
- runs `cargo semver-checks` against the latest version on crates.io, and
- publishes to crates.io.

If a check fails, nothing is published. Fix the problem, then move the tag:
delete it (`git push origin :refs/tags/vX.Y.Z`) and tag again.

## Setup, once

Trusted publishing lets the workflow publish without a stored API token.
crates.io only allows it for a crate that exists, so the first version is
published by hand.

1. Publish the first version by hand, from a clean checkout of `main`, and
   tag that commit:

   ```sh
   cargo publish --locked
   git tag v0.1.0
   git push origin v0.1.0
   ```

   The release workflow runs for this tag too, and fails at the publish step,
   because the version is already on crates.io. That is expected.

2. On GitHub, under **Settings → Environments**, create an environment named
   `release`. Add required reviewers if a person should approve each
   publish.
3. On crates.io, under the crate's **Settings → Trusted Publishing**, add a
   GitHub publisher with:
   - repository owner `jdpanderson` and repository name `pnyx`,
   - workflow file name `release.yml`,
   - environment `release`.
4. Optionally, set the crate on crates.io to accept only trusted publishing,
   so that an API token can't publish it. Check the crate's settings page for
   this option.
