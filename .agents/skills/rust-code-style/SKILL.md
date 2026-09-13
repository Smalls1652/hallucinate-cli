---
name: rust-code-style
description: |
    Rust code style for creating, changing, refactoring, debugging, or reviewing Rust source. Use whenever work touches `.rs` code, Cargo crates, Rust tests, libraries, CLIs, APIs, async services, database access, serialization, procedural macros, or terminal/web UI code—even if the request only says to implement, fix, clean up, or review.
license: MIT
metadata:
  author: "Tim Small"
  version: "2.0.0"
---

# Rust Code Style

Write **explicit, linear Rust**: domain names make data and operations clear; small helpers isolate policy and conversion; each success and failure path is visible. The code should be easy to extend without guessing what an omitted, invalid, or failed value means.

## Local code is authoritative

1. Read the target module, its siblings, and the crate's error and formatting setup before editing. Follow their vocabulary, public API shape, framework patterns, and error boundary.
2. Run the crate's formatter configuration with `cargo fmt`. Its `rustfmt.toml` determines imports, wrapping, function parameter layout, item ordering, and trailing commas. Preserve its output rather than applying a repository-wide preference by hand.
3. Keep the change focused. New code should match its immediate surroundings; improve nearby code only when required to make the changed behavior correct, clear, or consistently formatted.
4. For a new crate or a module with no strong local precedent, apply the conventions below.

## Structure and layout

- Give substantive modules a `//!` summary when the crate uses module docs. Keep `mod.rs` files to declarations and intentional re-exports.
- Put module docs before imports. Group imports as `std`, external crates, then `crate`/`super`, separated by blank lines when the formatter and surrounding module do so. Group items from a common root with braces.
- Place constants, statics, and type aliases directly after imports. Keep public models and their implementations before operations; keep private helpers near their callers or after the public API; put tests last.
- Use blank lines to separate real phases: validate, acquire/parse, perform I/O, transform, and return. A compact one-step function stays compact.
- Use rustfmt's vertical signatures and calls. In non-trivial function bodies, separate distinct phases with blank lines—for example, validation, context/resource acquisition, primary work, transformation, and return—so the operation scans clearly. Keep a compact body when it contains one small cohesive action. Separate struct fields with a blank line, including compact data types, so each property reads as a distinct part of the contract. Long attributes, struct literals, match arms, chains, and macro invocations should scan vertically, with the punctuation prescribed by the local formatter.
- Build view/macro inputs and non-trivial derived values above the macro body. Keep markup focused on its layout, branches, iteration, and composition.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestOptions {
    /// Destination used for the request.
    pub endpoint: Url,

    /// Optional timeout override in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,

    /// Whether the operation may retry transient failures.
    #[serde(default)]
    pub retry_enabled: bool,
}
```

```rust
pub fn load_active_records(
    store: &Store,
    requested_limit: Option<usize>,
) -> Result<Vec<Record>, AppError> {
    let limit = requested_limit.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 {
        return Err(AppError::InvalidLimit);
    }

    let records = store.load_active_records(limit)?;
    let records = records
        .into_iter()
        .filter(|record| record.is_visible)
        .collect::<Vec<_>>();

    Ok(records)
}
```

A short cohesive operation needs no artificial spacing:

```rust
fn is_enabled(settings: &Settings) -> bool {
    settings.enabled
}
```

## Names and API shape

Use concrete domain nouns for values: `pending_requests`, `completed_count`, `database_connection`, `relative_path`, `retry_delay`. Operations state their intent: `load_*`, `get_*_by_*`, `list_*`, `insert_new_*`, `update_*`, `delete_*`, `parse_*`, `resolve_*`, `collect_*`, `create_*`.

- Use conventional Rust casing. Framework-required component and macro names retain the framework's casing.
- Name booleans as predicates such as `is_published`, `is_error`, `exists`, or `show_thinking`.
- Shadow a value after a clear transformation when it makes the story linear: parse `record_id`, then use `record_id`. Introduce a distinct name only when both representations remain live.
- Make implementation details private. Use `pub(crate)` for intentional crate-internal sharing. Public DTOs, models, configuration, and event shapes commonly have public fields; coordinating state remains private.
- Use `new` where construction establishes a meaningful invariant or default. Use `Default` for an ordinary baseline; builder-style `with_*` methods take `mut self` and return `Self` for optional configuration.
- Use `From` for obvious, lossless conversions; use named conversion functions or `TryFrom` when conversion can fail or hides a meaningful action.

## Models, contracts, and documentation

Make boundary contracts visible in types, derives, serde attributes, and docs.

- Derive only traits the type uses, in the local ordering. Use structured enum variants for distinct states rather than parallel optional fields.
- Model genuine absence as `Option<T>`. Use serde `default`, `rename`, `rename_all`, `tag`, and `skip_serializing_if` when they express an external or compatibility contract. Keep feature-gated derives and schema attributes co-located with the model.
- Document public modules, types, fields in shape-heavy public structs, enum variants, traits, and public operations. Start with what the item does or represents. Expand boundary code docs with semantics, constraints, precedence, side effects, and meaningful absence/error behavior.
- Use `## Arguments` and `## Returns` for public operations whose inputs or return contract need explanation. State the domain meaning, rather than restating the Rust type.
- Comments explain a reason: an invariant, externally required ordering, compatibility choice, security property, or deliberately handled failure. Keep syntax that already reads clearly uncommented.

```rust
/// Load the most recent saved state for a workspace.
///
/// A missing saved state is ordinary and returns `Ok(None)`. Invalid persisted
/// state is an error because continuing with incorrect state is not.
pub fn load_saved_state(workspace_id: &str) -> Result<Option<SavedState>, AppError> {
    // ...
}
```

## Linear control flow

Keep the happy path visible and make exceptional paths exit early.

- Validate inputs and terminal state near the top. Use `let Some(value) = ... else { ... };` when absence exits; use `if let` for one significant variant; use `match` when alternatives each have a policy.
- Bind meaningful intermediate values, use `?` for ordinary propagation, and return a named final value after a non-trivial query or transformation.
- Prefer an explicit `match` to a catch-all when every enum variant needs a policy decision. Binding a `match` result to a domain-named value is often clearer than hiding the decision inside another expression.
- Use iterator chains for contained transformations and lookups. Use `for` loops when ordering, mutation, logging, error handling, or multi-step work matters. Name derived collections that have domain meaning.
- Make user-visible and test-significant ordering explicit. For relational data, batch-load, group in a map, and rebuild the required order instead of issuing per-row lookups.

```rust
let record_id = RecordId::parse(request.path_parameter("record_id"))?;

let database_connection = context
    .database_pool()
    .acquire()
    .map_err(ApiError::Database)?;

let record = repository::load_by_id(
    &mut database_connection,
    &record_id,
)?;

Ok(Json(record))
```

## Errors and invariants

Choose the error type at the boundary where its meaning becomes known.

- Use a focused `thiserror` enum for library, model, parsing, database, protocol, and HTTP boundaries where callers distinguish categories. Preserve the source with `#[from]` or a typed variant.
- Use `anyhow::Result` in application orchestration and commands when errors are headed to a user-facing boundary and category matching adds no value. Add context naming the failed operation and relevant ID, path, line, or input.
- Treat absent, invalid, and failed input as separate states whenever the domain does. Return `Ok(None)`, an empty collection, or `false` only for documented ordinary absence. Persisted malformed data carries contextual error information rather than becoming absence.
- `unwrap` and `expect` belong in tests or narrow internal invariants. An invariant panic message identifies the invariant. Use `unreachable!` only where the structure proves that branch impossible.
- At server/UI boundaries, log diagnostic detail and return a safe actionable error. Keep credentials, authorization values, and sensitive raw payloads out of logs and errors.

## Async, I/O, and external services

- Let lifecycle phases show: validate/build context, begin I/O, process the result or events, record state, then finalize.
- In `tokio::select!`, give cancellation/shutdown its own branch at each wait where it changes the operation's outcome.
- Isolate stream framing, parsing, record application, and finalization into helpers once they are independently meaningful or testable.
- Name request construction and external conversions (`request_headers`, `convert_messages`, `retry_delay`). Validate external responses before relying on them. Retry only specifically transient failures with a bounded, documented policy.
- For configuration and durable files, use serde models with centralized defaults. Merge layers explicitly. Append-only files report malformed lines with their location; integrity-sensitive replacement writes use a temporary sibling and rename after the write succeeds.
- Use `tracing` for significant application work. Select the level by value: `trace!` for high-volume detail, `debug!` for diagnostics, `info!` for normal operations, and `error!` at handled failure boundaries. Include useful, non-secret identifiers.

## Framework-shaped code

- Keep CLI flags in documented `clap` argument structs. Encode optional, repeatable, environment-backed, required, and conflicting arguments in the argument type and attributes.
- Keep API handlers at the boundary: extract and parse input, obtain context, enforce authorization, log the operation, call the domain/database function, and map the result to the framework response.
- Keep database access functions domain-named, accepting the connection and returning the crate's typed database result. Build one query clause per line; use `.optional()` for ordinary missing records; specify order intentionally.
- Respect framework macro signatures and component naming. Place a narrow lint allowance on the item that requires it.

## Tests and completion

- Put unit tests in a final `#[cfg(test)] mod tests` with `use super::*` and only required extra imports. Name tests for observable behavior, such as `loads_the_most_recent_saved_state` or `malformed_entries_are_reported_with_their_line_number`.
- Test changed behavior and its important boundary: absence, invalid input, ordering, malformed persistence, cancellation, or failure propagation as appropriate. Tests may use `unwrap`, `expect`, and direct panics for setup and assertions.
- Before finishing, ensure every changed public/boundary contract is documented where the surrounding crate expects it; error context distinguishes the failed operation; focused tests cover the behavior; then run `cargo fmt` and the narrowest relevant `cargo test` or `cargo check`.
