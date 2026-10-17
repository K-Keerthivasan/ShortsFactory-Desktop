# Studio workflow additions

The existing `studio.*` command registry also exposes the resumable editorial workflow documented in [shorts-manager.md](../../docs/shorts-manager.md). All product code is Rust and uses the existing native rendering and cloud scheduling paths. The workflow ledger is separate from the user's open editor session.

Known limitation: interrupted render/library insertion requires reconciliation before a manual reset; the workflow holds the item instead of risking duplicate creation. Different trailers covering the same story require editorial review. Evidence fields carry reviewed facts, not automatic fact verification.
