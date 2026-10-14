# Keeping Shorts Factory Desktop current

This repository is a GitHub fork of `storytold/filmcraft`. `origin` is your
`K-Keerthivasan/ShortsFactory-Desktop` repository; `upstream` is FilmCraft.
The desktop additions live on `main` on top of FilmCraft's history.

Check for patches from PowerShell in this checkout:

```powershell
git fetch upstream
git log --oneline main..upstream/main
git diff main...upstream/main --stat
```

Integrate them on a review branch so your working desktop version stays available:

```powershell
git switch main
git switch -c codex/update-filmcraft
git merge upstream/main
# Resolve conflicts, keeping the Shorts Factory branding and Reel Studio integration.
cargo xtask ci
cargo build --release -p filmcraft --bin shorts-factory-desktop
git push -u origin codex/update-filmcraft
```

Review that branch in a pull request to **your fork's main**, then merge it after
checking the editor, templates and shared library. Never replace your modified
main with upstream/main or force-push it to obtain updates: that removes your
desktop additions. A GitHub "Sync fork" conflict needs the same reviewed merge.

On this PC, bound compiler concurrency to avoid exhausting RAM when testing the
whole workspace: `cargo test --workspace -j 2 -- --test-threads=2`. Set
`$env:CARGO_BUILD_JOBS='2'` before other large checks if necessary. The desktop
release build remains optimized; normal-profile tests avoid linking an optimized
copy of every integration-test executable.

The earlier Electron application's repository is `K-Keerthivasan/ShortsFactory`.
Its code uses a different language and renderer; updates there must be ported and
checked against `docs/SHORTS-FACTORY.md`, rather than merged as FilmCraft patches.

Builds stay in `target/release`, outside Git. Keep credentials, the library database,
downloaded footage and runtime `.env` files out of this public source repository.
