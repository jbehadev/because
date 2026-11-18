# Plugin Interfaces

Implements decision **D3**: Rust traits now, with signatures chosen so a WASM
host can be added later without reworking the interfaces.

## 1. The four extension points

| Trait | Swaps out | First impl |
|---|---|---|
| `StorageEngine` | where records live | SQLite |
| `Sectioner` | how a segment is sliced | Haiku, structured output |
| `Summarizer` | how a change is described | Haiku, structured output |
| `AgentBackend` | who runs the conversation | Claude CLI, then Messages API |

`AgentBackend` is documented in `04-agent-host.md`; it obeys the same ABI rules
below.

## 2. ABI-representable discipline

The goal is that today's trait can become tomorrow's WASM interface without a
rewrite. That imposes rules now, while they are cheap:

1. **Owned data across the boundary.** No `&'a str`, no `&[T]`, no lifetimes in
   trait method signatures. `String`, `Vec<u8>`, owned structs.
2. **No generics or associated types on the trait.** Generic methods have no
   ABI; monomorphization does not cross a module boundary.
3. **Every parameter and return type is `Serialize + Deserialize`.** If it
   cannot round-trip through canonical JSON it cannot cross a WASM boundary.
4. **Errors are a concrete enum, not `Box<dyn Error>` or `anyhow::Error`.**
   Errors must serialize too, and callers need to branch on kind (retryable vs
   not) rather than on a message string.
5. **No callbacks or trait objects in arguments.** Streaming is expressed as
   "return a batch and a cursor", not "take a closure".
6. **Coarse-grained calls.** A per-row call is fine in-process and ruinous
   across a serialization boundary, so batch at the interface: `put_records`,
   not `put_record`.
7. **No `Instant`, `SystemTime`, `PathBuf`, or file handles in signatures.**
   Timestamps are RFC 3339 strings; paths are repo-relative `String`s.

Rule 7 also removes an accidental portability bug: repo-relative string paths
mean a record exported on Windows replays correctly on Linux.

## 3. Trait sketches

```rust
// because-store/src/lib.rs
#[async_trait]
pub trait StorageEngine: Send + Sync {
    fn id(&self) -> String;
    fn schema_version(&self) -> u32;

    async fn migrate(&self) -> Result<(), StoreError>;

    /// Idempotent by record id. Returns the ids actually newly applied.
    async fn put_records(&self, records: Vec<Record>) -> Result<Vec<String>, StoreError>;

    async fn query_sessions(&self, f: SessionFilter) -> Result<Page<SessionRow>, StoreError>;
    async fn query_segments(&self, f: SegmentFilter)       -> Result<Page<SegmentRow>, StoreError>;
    async fn query_sections(&self, segment_id: String)  -> Result<Vec<SectionRow>, StoreError>;
    async fn query_changes(&self, f: ChangeFilter)   -> Result<Page<ChangeRow>, StoreError>;
    async fn search(&self, q: String, limit: u32)    -> Result<Vec<SearchHit>, StoreError>;

    async fn get_blob(&self, digest: String) -> Result<Option<Vec<u8>>, StoreError>;
    async fn put_blob(&self, data: Vec<u8>)  -> Result<String, StoreError>;

    // durable job queue
    async fn claim_jobs(&self, kinds: Vec<String>, max: u32) -> Result<Vec<Job>, StoreError>;
    async fn settle_job(&self, id: String, outcome: JobOutcome) -> Result<(), StoreError>;
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum StoreError {
    Conflict { detail: String },
    NotFound { id: String },
    SchemaTooNew { found: u32, supported: u32 },
    Io { detail: String },
    Corrupt { detail: String },
}

// Pagination as an explicit cursor — no iterators across the boundary.
#[derive(Serialize, Deserialize)]
pub struct Page<T> { pub items: Vec<T>, pub next: Option<String> }
```

```rust
// ts-summarize/src/lib.rs
#[async_trait]
pub trait Sectioner: Send + Sync {
    fn id(&self) -> String;
    fn prompt_ver(&self) -> String;
    async fn slice(&self, req: SectionRequest) -> Result<SectionResponse, SummarizeError>;
}

#[async_trait]
pub trait Summarizer: Send + Sync {
    fn id(&self) -> String;
    fn prompt_ver(&self) -> String;
    /// One call per file change; the pool decides parallelism, not the impl.
    async fn summarize_change(&self, req: ChangeRequest) -> Result<ChangeSummary, SummarizeError>;
    async fn rollup_segment(&self, req: RollupRequest) -> Result<SegmentSummary, SummarizeError>;
}

#[derive(Serialize, Deserialize)]
pub struct ChangeRequest {
    pub prompt: String,          // the user prompt — the grounding for "why"
    pub section_title: Option<String>,
    pub section_intent: Option<String>,
    pub path: String,            // repo-relative
    pub language: Option<String>,
    pub change_type: String,
    pub hunks: Vec<HunkInput>,   // header + patch text, already redacted
    pub prior_summary: Option<String>,  // for incremental re-summarization
}

#[derive(Serialize, Deserialize)]
pub enum SummarizeError {
    RateLimited { retry_after_ms: Option<u64> },
    Transport   { detail: String },
    BadResponse { detail: String },
    Refused     { category: Option<String> },
    Oversized   { tokens: u64, limit: u64 },
    Redacted    { reason: String },
}
```

Note what the interface enforces: the `Summarizer` receives **already redacted**
hunks. Redaction runs in `ts-summarize`'s pipeline *before* the trait call, so a
third-party plugin can never be the thing that leaks a secret, and cannot
accidentally bypass the gate. That is a direct consequence of designing the
boundary for untrusted code from the start.

## 4. Registry and configuration

```toml
# .because/config.toml   (committed; contains no secrets)
[storage]
engine = "sqlite"
[storage.sqlite]
path = ".because/cache/slicer.db"

[summarize]
sectioner  = "haiku"
summarizer = "haiku"
model = "claude-haiku-4-5"
escalate_model = "claude-sonnet-5"
escalate_lines = 400
max_concurrency = 8

[agent]
backend = "claude-cli"          # or "messages-api"
model = "claude-opus-5"
effort = "xhigh"
thinking_display = "summarized" # "omitted" captures no reasoning at all

[retention]
tier = "narration"              # summaries | narration | full
warn_records_over_mb = 200

[redaction]
extra_patterns = []
allow_paths = []                # explicit opt-in overrides, reviewable in a diff
```

Registry is a `HashMap<String, Box<dyn …>>` built at startup from compiled-in
implementations selected by cargo features:

```toml
[features]
default        = ["store-sqlite", "sum-haiku", "agent-cli"]
store-sqlite   = ["dep:rusqlite"]
store-postgres = ["dep:tokio-postgres"]
sum-haiku      = []
sum-local      = ["dep:llama-cpp-rs"]   # a local model as a summarizer
agent-cli      = []
agent-api      = ["dep:reqwest", "dep:eventsource-stream"]
plugins-wasm   = ["dep:wasmtime"]
```

A local-model summarizer (`sum-local`) is the plugin that most justifies the
abstraction existing: some teams will not send diffs to a hosted API at all, and
that is a config change rather than a fork.

## 5. The WASM path, later

When plugins need to be third-party and untrusted, add a host behind
`plugins-wasm`:

- **`wasmtime` 49** for a hand-rolled host (maximum control, more plumbing), or
  **`extism` 1.30** for a plugin framework that already provides the host-function
  plumbing, memory handling and multi-language PDKs. Start with extism; drop to
  raw wasmtime only if its model gets in the way.
- Transport: canonical JSON in, canonical JSON out, one function per trait
  method, named `ts_storage_put_records` etc. Rules 1–7 above mean the Rust
  traits map to this directly.
- Capabilities are explicit and deny-by-default: no filesystem, no network, no
  clock unless granted. A storage plugin gets one granted host function for its
  own data directory; a summarizer plugin gets one for an HTTP POST to a
  configured host and nothing else.
- Fuel metering and a wall-clock timeout per call, so a bad plugin degrades one
  job rather than hanging the app.
- Blocking DB I/O inside WASM is the known awkward part. Mitigation: coarse
  batched calls (rule 6) and running plugin calls on `spawn_blocking`.

A `WasmStorageEngine` that implements `StorageEngine` by forwarding to a module
is then just one more entry in the registry, and nothing upstream changes.

### Alternative already available for tool plugins

For *tool* plugins specifically (as opposed to storage or summarizers), MCP over
stdio via `rmcp` 3.4.0 is a better fit than WASM: the protocol exists, servers
can be written in any language, process isolation is free, and users may already
have servers they want to use. Use MCP for tools, WASM for storage/summarizers,
traits for everything in-tree.
