# Shorts Manager

Shorts Manager is a pure Rust batch driver and local MCP workflow over Shorts Factory's existing engine, library, native renderer, and Supabase/B2 cloud publisher. It opens a dedicated headless session and does not change an open editor. No FFmpeg is used for production renders.

Build from the desktop repository:

```powershell
cargo build --release -p filmcraft-cli --bins
```

The private plugin source is `plugins/shorts-manager`. Its MCP server is the actual built `target/release/shorts-manager-engine.exe mcp`, using stdio JSON-RPC. Connect that command directly in any MCP client, or load the plugin source using your client's local-plugin flow. The plugin depends on this repository checkout; keep its relative directory layout. Local processes are available only on this host.

Tools:

- `workflow_inventory {channel}`: brand, saved posting slots, source paths and cloud history. It fails closed when history needs pagination rather than making incomplete duplicate decisions.
- `workflow_plan`: strict typed editorial fields, with no arbitrary paths or commands. Titles <=100 characters, hooks <=90, captions <=2100, attribution <=120. Unknown keys are errors. Include an official HTTPS evidence URL, factual note, observed footage note and trim rationale. Times are integer ticks at 254016000000/second, exclusive end. Trims must be 3–60 seconds inside the native-probed source duration.
- `workflow_suggest`: optional constrained copy from a configured local OpenAI-compatible provider. The input is the same evidence object. The proposal is not persisted, rendered or scheduled; review its factual accuracy first.
- `workflow_render {channel,asset}`: native GPU/OS-media render with CPU fallback, saved editable project, thumbnail and real library record. The MCP tool returns a job immediately; poll `workflow_status` until rendered or failed. Its worker owns a separate headless session; each MCP process admits one render at a time. The engine command and batch runner block until complete. Existing completed records are reused. Use the explicit `studio.workflowRevise` command with a complete reviewed Editorial object to revise a completed, undelivered edit. The old library render is preserved, its superseded draft story is archived, and the revision is audited. `studio.workflowEvidence` corrects only source references, with an audit entry. Failed or interrupted rendering is held for reconciliation, never automatically recreated.
- `workflow_status {channel,asset}`: durable stage, plan, render, cloud ID, error.
- `workflow_schedule {channel,asset,account,at}`: validates matching workspace/cloud handle, approves the reviewed render, reserves/upload/finalizes through the existing cloud API, then checks the remote post and uploaded object status. `at` is a future ISO UTC Z timestamp. The persisted cloud key survives retries and process restarts.

The MCP session also retains the editor's existing command and inspection tools. Prefer the narrow workflow tools for this task. Do not run workflow rendering through a bridge connected to unsaved personal work.

For optional local drafting, configure these process environment variables (no model is assumed):

```powershell
$env:EDITORIAL_LLM_BASE_URL='http://127.0.0.1:1234/v1'
$env:EDITORIAL_LLM_MODEL='your-installed-model-id'
```

The provider must accept `/chat/completions` and JSON-object responses. Only loopback HTTP with an explicit port is accepted. Evidence is treated as untrusted data; the model may change only title, hook and caption. Responses are bounded and revalidated. The validator verifies structure and media timing, not the truth of a supplied factual assertion. A reviewing agent must inspect footage and verify official sources.

A batch manifest has `version:1`, an `items` array (at most 100), and each item contains `plan` (the strict editorial object), `account` (real cloud account UUID), and `at` (explicit UTC). The reviewed first-batch example is in `target/batch-2026-10-16/manifest.json`; it intentionally contains personal library IDs and is not part of the portable plugin.

Run a reviewed batch:

```powershell
.\target\release\shorts-manager.exe render .\path\manifest.json
# Inspect each preview and results-render.json before scheduling.
.\target\release\shorts-manager.exe schedule .\path\manifest.json
# For an already-reviewed batch, execute both stages:
.\target\release\shorts-manager.exe run .\path\manifest.json
```

Each item uses an isolated native CLI child process, bounded to two hours. Item logs and incrementally replaced `results-<mode>.json` live beside the manifest. A failed item does not silently discard later items; the final exit is nonzero when anything failed. Reuse the *same manifest* when resuming.

The durable SQLite tables `editorial_workflow` and `editorial_sources` record plans, source SHA-256 reservations, stages, errors, render IDs and retry keys in the real studio DB. Duplicate checks reconcile local publishing jobs, cloud schedules/history, script source IDs, and accompanying native projects for older records without source IDs. Identical files are deduplicated by SHA-256. Different trailers about the same story still require editorial review; do not treat a different file hash as proof of a new story.

Uncertain rendering is held because the library insertion and workflow state are separate operations. Inspect the saved project/MP4, scripts and render records before an operator resets that item. There is deliberately no automatic reset tool that could duplicate an interrupted insertion. Cloud retries preserve the key and reconcile remote state. Do not cancel or replace an uncertain publish job without checking its remote state.

Before scheduling, check cadence, occupied slots and account mapping from inventory. Use the intended IANA timezone when converting to UTC. This batch uses America/New_York; the underlying publisher records the host's configured IANA timezone. A successful cloud-confirmed post executes in the existing cloud worker with the PC closed. Local analysis and rendering still require the host. No recurring host automation is installed by this workflow.

Verification: `cargo test -p filmcraft-studio --lib`, `cargo test -p filmcraft-automation --lib`, `cargo test -p filmcraft-engine reel --lib`, and package-specific Clippy. MCP verification must include initialize, tools/list, strict-argument rejection and a real harmless inventory call. The first batch review records the actual checks and per-item outcomes; source code alone is not evidence of a successful cloud schedule.


For a JSON-configured MCP client, the verified local command is:

```json
{"mcpServers":{"shorts-manager":{"command":"E:\\AI\\IG_ReelAutomation_Desktop\\target\\release\\shorts-manager-engine.exe","args":["mcp"],"cwd":"E:\\AI\\IG_ReelAutomation_Desktop"}}}
```

The local drafting adapter was exercised against the installed Ollama service at `http://127.0.0.1:11434/v1` with the installed `qwen2.5:latest` model. This is a tested configuration, not a model default. Configure any compatible installed model using the environment variables above.
