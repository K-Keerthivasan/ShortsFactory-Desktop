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

## Where the add-ons live (so updates don't collide with them)

Almost everything Shorts Factory adds is in files FilmCraft does not have, so a FilmCraft
update cannot conflict with them:

| Our files (never in upstream) | What they hold |
|---|---|
| `crates/reel/` | the 21 templates, brand and card model |
| `crates/studio/` | library, accounts, AI, yt-dlp, Instagram / YouTube / cloud, the `studio.*` MCP commands |
| `crates/engine/src/{reel,reel_tests,host}.rs` | `reel.*` commands; the hook other crates register commands through |
| `crates/ui-egui/src/panels/{reel,reel_ai}.rs` | Create, Design, Downloads, Reel Studio, AI, Export ▸ Library / Publish, Plugin Settings |
| `assets/app-icon/shorts-factory-*`, `packaging/shorts-factory/`, `docs/{SHORTS-FACTORY,MCP,UPSTREAM}.md` | branding and docs |

FilmCraft's own files are touched only at small hook points (about 30 files, a few lines
each; `git diff <fork point>..main -- <file>` shows them). Only these can conflict:

| Upstream file | Our change |
|---|---|
| `Cargo.toml`, `xtask/src/main.rs` | the `reel` and `studio` crates and their layer rules |
| `crates/engine/src/{lib,commands}.rs` | `mod reel; mod host;` and adding their commands |
| `crates/project/src/lib.rs` | `Project::reel` (one optional field) |
| `crates/ui-egui/src/{dock,menus,lib,state}.rs`, `panels/{mod,export_mode}.rs` | panel kinds, the Reels workspace, the Reel menu, the per-frame poll |
| `crates/ui-egui/src/i18n/*.tsv` | our menu words, appended at the end of each file |
| `apps/*`, `crates/ui-egui/src/{brand,header,links}.rs`, `panels/dialogs.rs`, `README.md`, `NOTICE` | the name, icon, About box, data folder and command registration |

Rules that keep it that way:

- New features go in our files. When FilmCraft code must change, add a hook (as
  `filmcraft_engine::host::register` does for `studio.*`) rather than editing its logic.
- Never reformat or reorder upstream code; changes to upstream files stay small inserts.
- FilmCraft's own tests run unchanged against FilmCraft's defaults (the library still opens
  the Editing workspace; only the Shorts Factory binary opens Reels). After an update, a failing
  upstream test means a real break, not our layout.
- Merge, never rebase, so the history shows which FilmCraft release we are on.

After the merge, also check by hand: the Reels workspace, a New Reel with its sound, Design,
Export ▸ Library / Publish, the AI panel and `filmcraft-cli commands studio`. Changes in FilmCraft's
`CommandSpec`, `Session::execute`, the `graphic_text` / `graphic_shape` layers, `export.quick`,
the dock / panel registry or the MCP server may need matching changes in our crates; the
compiler points at them.

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
