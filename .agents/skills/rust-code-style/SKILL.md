---
name: rust-code-style
description: >
  Write, review, or refactor Rust in the user's established style. Use this skill for any request that creates or changes Rust source: application or library code, async code, CLI programs, APIs, database code, serialization models, tests, macros, terminal UI code, or Rust code review. Apply it even when the user asks only to "implement," "fix," or "clean up" Rust and does not explicitly mention style.
license: MIT
metadata:
  author: "Tim Small"
  version: "1.0.0"
---

# Rust Code Style

Write Rust that is direct, explicit about behavior and failure, and pleasant to maintain. This style favors clear domain vocabulary, small named helpers, documented public APIs, and code paths whose success and failure cases are easy to follow. It is informed by two related bodies of code:

- a newer async CLI application with a relatively conventional, rustfmt-oriented style; and
- an established web/server workspace whose framework macros and model layer have a more expansive documentation and formatting style.

Follow the conventions already present in the file and crate being changed. In a new project, use the **default/current style** below. Do not mechanically preserve incidental historical choices (for example, missing trailing commas, needless clones, or non-rustfmt layout) when they make new code worse. Run `cargo fmt` when available.

All examples in this guide are invented and intentionally generic. Adapt the names, types, framework attributes, and error types to the surrounding project.

## Working principles

1. **Start from the surrounding code.** Reuse the crate's error type, result alias, module layout, framework conventions, and existing terminology before introducing a new abstraction.
2. **Make contracts visible.** Use types, serde attributes, validation, and narrow helper functions to express what is permitted and what happens when it is not.
3. **Keep the happy path linear.** Extract parsing, conversion, and repeated policy into helpers. Within an operation, bind meaningful intermediate values, use `?`, and return the final value explicitly.
4. **Handle bad inputs deliberately.** Never silently discard malformed persisted data, failed I/O, or invalid external responses when that could yield an incorrect result. Include useful operation, identifier, and location context in errors.
5. **Optimize for understandable behavior, not cleverness.** A straightforward `match`, explicit query branches, or a locally named collection is preferable to compressed but opaque iterator gymnastics.
6. **Comment the reason for a non-obvious decision.** Explain invariants, ordering, compatibility, ownership, or protocol behavior—not the syntax a Rust reader can already see.

## File and module organization

### Module headers and imports

- For a substantive module, begin with a short `//!` module doc that says what the module owns. Add a brief second paragraph when its responsibilities or constraints need explanation.
- Keep very small module declaration files minimal: declarations and deliberate re-exports only.
- Put imports after the module docs. Group them by origin, with blank lines between groups:
  1. `std`, usually consolidated with braces;
  2. third-party crates;
  3. local `crate` imports.
- Prefer grouped imports when several items share a root. Keep a single imported item ungrouped when that reads better. Let `rustfmt` choose wrapping.
- In a module that is strongly framework-shaped (web handlers, components, Diesel models), organize imports so framework items are visibly grouped together.
- Place module-level constants, lazy regexes, and type aliases immediately after imports. Give public or semantically important constants a doc comment.

```rust
//! Request parsing and validation for the catalog API.
//!
//! This module keeps wire-format validation at the HTTP boundary.

use std::{collections::HashMap, time::Duration};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{error::ApiError, store::CatalogStore};

const DEFAULT_PAGE_SIZE: usize = 25;
```

### Order declarations by how readers use them

A typical module order is:

1. module docs, imports, constants, aliases, and statics;
2. public data types and their implementations;
3. public operation functions or trait implementations;
4. private helpers, kept near the code that uses them when practical;
5. `#[cfg(test)] mod tests` at the end.

For a large conceptual module, use quiet visual section dividers such as `// ── Parsing ──` or `// ── Event Stream ──`. Use them to delineate real concepts, not before every function.

## Naming and API shape

### Names

- Use `snake_case` for functions, methods, local variables, modules, and fields; `PascalCase` for types, traits, enum variants, and framework-required component names; `SCREAMING_SNAKE_CASE` for constants.
- Prefer concrete domain nouns: `session_entries`, `request_headers`, `retained_messages`, `blog_post_count`, `image_items`, `terminal_reason`. Avoid vague names such as `data`, `thing`, or `result` when the result has a meaningful identity.
- Use action-oriented names for operations: `load_*`, `save_*`, `list_*`, `get_*_by_*`, `insert_new_*`, `update_*`, `delete_*`, `parse_*`, `format_*`, `build_*`, `create_*`, and `resolve_*`.
- Use boolean names that read as predicates: `is_error`, `is_terminal`, `only_published`, `done_seen`, `enable_tools`.
- Call a parsed or transformed value by the same domain name after shadowing when it improves the linear story:

```rust
let record_id = RecordId::parse(raw_record_id)?;
let record = repository.get_record(&record_id)?;
```

Do not introduce artificial suffixes merely to avoid shadowing.

### Visibility

- Make items `pub` when they are part of the module or crate API. Keep implementation details private.
- Use `pub(crate)` for a helper intentionally shared within the crate but not part of its external API.
- Data-transfer, model, configuration, and event structs commonly expose public fields because their shape is the API. Keep coordinating object state private unless callers genuinely need direct access.
- Use a type alias when it makes a recurring concept or a complex signature more legible. Avoid aliases that only hide a simple standard type.

### Constructors, builders, and conversions

- Give data types an idiomatic `new` constructor when it establishes defaults, invariants, or required metadata. For uncomplicated fields that come from callers, accept `impl Into<String>` to make call sites flexible.
- Use builder-style `with_*` methods that take `mut self` and return `Self` for optional configuration. They should make a sequence of settings read naturally.
- Use `Default` for a meaningful baseline configuration or empty state. Delegate `new()` to `Self::default()` when it adds no behavior.
- Implement `From` for clear, lossless domain conversions. Use named conversion functions or `TryFrom` when conversion can fail or obscures a meaningful operation.
- Keep constructors honest: clone a borrowed value only when the new owned object needs it; accept ownership when that is the natural API.

```rust
impl SearchOptions {
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
}
```

## Types, derives, and serialization

### Data model conventions

- Derive only what the type needs, commonly `Debug`, `Clone`, `PartialEq`, `Eq`, `Default`, `Serialize`, and `Deserialize`. Keep the derive list in a stable, readable order.
- Use structured enums for mutually exclusive states or protocol variants. Give variants meaningful payloads rather than parallel optional fields.
- For serde-backed external shapes, use `#[serde(tag = "...")]`, `rename_all`, field `rename`, `default`, and `skip_serializing_if` to make the wire contract explicit.
- Prefer `Option<T>` for genuinely absent fields. Do not encode absence as an empty string unless the external format requires it.
- Use `#[serde(default)]` for fields that must deserialize cleanly from older or partial configurations. Combine it with `skip_serializing_if` when omission is the desired serialized form.
- Use `type_` when a Rust field would otherwise conflict with a reserved or awkward wire name, and map it explicitly with serde when necessary.
- In persistence or framework models, use conditional derives/attributes (`cfg_attr`) when the type is deliberately usable with and without an optional feature. Keep the feature-specific schema information beside the type declaration.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running { started_at: u64 },
    Failed { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}
```

### Documentation

- Document public modules, types, public fields in shape-heavy structs, public enum variants, traits, and public functions/methods with `///` comments.
- Start with a concise sentence in the present tense: “Load…”, “Return…”, “Configuration for…”, “An event emitted when…”.
- Expand documentation for boundary-facing, persistence, parsing, protocol, or framework code. Explain semantics, precedence, ownership, side effects, and unusual failure behavior.
- For a function with several meaningful parameters, use a blank line plus `## Arguments`, bulleting each parameter as ``* `name` - …``. Use `## Returns` when the return meaning needs clarification. Do not produce boilerplate sections for a trivial getter whose signature is self-evident.
- Use intra-doc links where they materially help navigation (for related types, functions, or crate concepts).
- For fields, say what the stored value represents, not merely that it “is a string.”

```rust
/// Load the most recent checkpoint for a workspace.
///
/// A missing checkpoint is normal and returns `Ok(None)`; an unreadable or
/// malformed checkpoint is reported to avoid resuming from incorrect state.
///
/// ## Arguments
///
/// * `workspace_id` - Identifier of the workspace to inspect.
pub fn load_checkpoint(workspace_id: &str) -> Result<Option<Checkpoint>, AppError> {
    // ...
}
```

## Formatting and layout

- Use four-space indentation and rustfmt-compatible layout.
- Write multi-line function signatures with one parameter per line when the signature is non-trivial, and include trailing commas. The same applies to long calls, enum variants, struct literals, match arms with substantial expressions, imports, and attributes.
- Prefer vertical structure over lines that require horizontal scanning. Break a method chain so each operation is visible.
- Insert blank lines between logical phases: validation, conversion, I/O/query, transformation, and return. Do not pad tiny functions with arbitrary whitespace.
- Use a local binding for a non-trivial expression before returning it; return direct expressions for a clear one-step operation.
- Prefer explicit `Self { ... }` in constructors and conversions. In struct literals, use field shorthand where it remains clear.
- Use named format captures (`format!("failed to load {path}")`) or named formatting arguments when they improve readability. Use readable separators, units, and numeric underscores in constants such as `128_000`.

```rust
let entries = source
    .iter()
    .filter(|entry| entry.is_active)
    .map(format_entry)
    .collect::<Vec<_>>();

Ok(entries)
```

## Control flow and collections

### Favor linear, explicit branching

- Validate preconditions close to the top of an operation and use early returns for failure, missing resources, empty input, cancellation, and simple terminal states.
- Use `let Some(value) = ... else { ... };` when the absent case exits and the present value drives the rest of the function.
- Use `if let` or `while let` for a single interesting variant; use `match` for multiple meaningful alternatives or when each alternative needs a distinct result.
- It is natural in this style to bind the outcome of a `match` to a descriptive variable, then return or use it afterward. Nested matches are acceptable when modeling genuinely nested state, though extracting a helper is better once the branches become large.
- Use guards, `matches!`, `then_some`, and `filter` to express small predicates without constructing unnecessary temporary state.
- Match explicitly rather than relying on a broad fallback when a new enum variant would need a policy decision.

```rust
let page_size = requested_page_size.unwrap_or(DEFAULT_PAGE_SIZE);

if page_size == 0 {
    return Err(AppError::InvalidPageSize);
}

let mode = match request.mode.as_deref() {
    Some("summary") => RenderMode::Summary,
    Some("full") | None => RenderMode::Full,
    Some(other) => return Err(AppError::InvalidMode(other.to_string())),
};
```

### Iteration and transformations

- Use iterator chains for a contained transformation: filter/map/collect, simple lookup, or computing a derived collection.
- Use a `for` loop when handling errors, logging/skipping selected items, mutation, sequencing, or multi-step work. This makes policy visible.
- Give intermediate collections descriptive names when they have domain meaning or are reused. Collect into an explicit `Vec<_>`, `HashMap<_, _>`, or other target when inference would obscure intent.
- Sort intentionally when output order is user-visible or tests rely on determinism. State the ordering reason in a comment if it is not obvious.
- Avoid N+1 work. Prefer loading related records in one query or pass, grouping them into a map, then reconstructing the desired ordered result.
- Use `entry(...).or_default()` and `remove(...).unwrap_or_default()` for uncomplicated grouping and consumption of a temporary map.

## Errors, fallibility, and invariants

### Choose the error boundary intentionally

- Use a domain-specific `thiserror` enum at library, database, parsing, protocol, and web boundaries where callers need to distinguish failure categories.
- Use `Result<T, anyhow::Error>` in application orchestration where errors are being propagated to a user-facing boundary and a bespoke type adds little value.
- Use `?` for normal propagation. Use `map_err` or `anyhow!` to add context that a caller needs: what failed, for which ID/path/line, or why the operation is unsafe to continue.
- Favor error messages that name the operation and relevant input. Preserve the source error through `#[from]`, a typed variant, or formatting.
- Use `anyhow::bail!` for a concise validation failure that terminates the current operation.
- Convert framework, database, and parsing errors near the boundary where the semantic category is known.

```rust
let content = std::fs::read_to_string(path).map_err(|error| {
    anyhow::anyhow!("failed to read settings file '{}': {error}", path.display())
})?;
```

### Options and recoverable conditions

- Use `unwrap_or`, `unwrap_or_default`, `map_or`, `ok_or_else`, and `Option` chaining for clear local defaults.
- Return `Ok(None)`, an empty collection, or a boolean for ordinary absence only when that is part of the function's documented contract.
- Do not treat malformed persistent input as ordinary absence. Reject it with contextual error information.
- Do not use `unwrap` or `expect` for recoverable production input. They are appropriate in tests and for narrow, documented internal invariants that would represent a programming bug if violated (for example, a mutex lock that is not expected to be poisoned or an index built by the same type).
- Use `unreachable!` only after the code structure proves the alternative impossible, and make its message state the invariant.
- Add a small explanatory comment before a non-obvious default, compatibility fallback, or intentionally ignored error.

### Error types

- Derive `Debug` and `thiserror::Error` for typed errors.
- Name variants by category or operation, such as `Configuration`, `NotFound`, `DatabaseOperation`, `InvalidInput`, or `ChannelClosed`.
- Give variants an informative `#[error(...)]` string and preserve a source error with `#[from]` where appropriate.
- At an HTTP boundary, map internal typed errors to the framework's response type and log diagnostic detail while returning a safe client message.

## Async, stateful, and external-service code

- Use `async fn` for natural asynchronous operations. When a trait or framework requires a boxed future, write the explicit `Pin<Box<dyn Future<Output = Result<...>> + Send + 'a>>` signature and contain the async implementation in `Box::pin(async move { ... })`.
- Keep mutable state transitions explicit. Use named state fields and methods such as `set_state`, `emit`, `finish`, or `fail` rather than scattering mutations across unrelated branches.
- For multi-step workflows, make lifecycle phases apparent: validate/build context, start I/O, process events, record results, then finalize state.
- In `tokio::select!`, handle cancellation or shutdown as a first-class branch at each wait point where it matters.
- For streaming and protocol parsers, separate byte buffering, one-record parsing, record application, and finalization into small helpers. Test fragmented input and terminal conditions.
- Use `Arc<Mutex<_>>` only for genuinely shared mutable state. A lock `unwrap()` is acceptable only where poisoning is an invariant violation; otherwise surface a typed error.
- Build external request values in clearly named helpers (`request_headers`, `convert_messages`, `retry_delay`). Validate external data carefully and distinguish allowed incomplete states from malformed states.
- Use bounded retries for explicitly transient failures and cap external retry hints/delays. Do not retry invalid requests or unknown failures without a policy.

## Logging and observability

- Use `tracing` in application/server code. Log the operation and useful non-secret identifiers before or around meaningful external work.
- Use `trace!` for high-volume implementation details, `debug!` for diagnostic steps, `info!` for normal operations worth recording, and `error!` at a boundary where failure is being handled or reported.
- Never log secrets, credentials, or raw sensitive payloads.
- Favor direct, useful messages. Include an ID, path, URL fragment, or count when it helps diagnosis.
- Keep user-facing errors and diagnostic logs distinct: detailed logs can retain error structure; HTTP/UI messages should be safe and actionable.

## Common application patterns

### Configuration and files

- Model settings as serde types with deliberate defaults. Keep default-producing functions named and centralized when attributes need to reference them.
- Merge layered configuration explicitly: load the base, load the overlay, recursively merge structured maps/tables, then deserialize once. State which layer wins in the docs.
- Create parent directories before atomic writes. Write to a temporary sibling path, flush it, then rename it when data integrity matters.
- Normalize or validate paths and identifiers before using them. Preserve meaningful display forms for error messages.
- For append-only formats, read line by line; skip deliberately blank lines but report malformed content with its line number rather than silently continuing.

### CLI code

- Represent CLI arguments as a documented `clap` struct with a derived parser. Use `Option<T>` for omitted optional flags, `Vec<T>` plus an append action for repeatable flags, and a nested enum for subcommands.
- Document constraints, precedence, environment fallbacks, and security advice directly on flags when users need it.
- Test parser behavior with `try_parse_from`, covering normal combinations and conflicts.

### Database code

- Keep data models, connection/pool setup, errors, schema, and operation functions in clear modules.
- Name data access operations with their domain action and lookup fields. Accept a mutable connection and return a typed database result.
- Build a Diesel query one semantic clause per line. Bind the retrieved value to a precise name, map the database error to the domain error, and return `Ok(value)`.
- Use `.optional()` for absence that is a normal result. Use count/existence queries when a boolean contract is clearer.
- Make ordering explicit, particularly for presentation and pagination.
- Avoid repeated per-row queries; batch-load relationships, group by foreign key, and assemble the ordered result afterward.

### HTTP/API code

- Keep an API handler focused on boundary work: extract/parse parameters, obtain request context/resources, validate authorization or existence, log the operation, call a data/service function, and return the framework response.
- Parse path/query IDs near their extraction point and propagate conversion errors with `?`.
- Check permission/role requirements at an API layer or macro designed for that purpose, not ad hoc throughout data access.
- Use framework route/layer attributes and path-parameter macros consistently with adjacent handlers.
- For expected missing resources, return the framework's not-found response. Map internal errors to a safe server error at the error boundary.

### Macro- and view-oriented code

- When working in a macro-driven web UI framework, preserve its signature and naming conventions even where they differ from ordinary Rust (for example, component names that must be `PascalCase`). Add a narrow `#[allow(...)]` on the affected item rather than disabling lints broadly.
- Build non-trivial derived values above the view macro; keep the view body focused on layout, conditional branches, iteration, and component composition.
- Use local `view!` fragments for reusable page sections such as previous/next controls. Name them for what they render.
- Prefer semantic names and explicit classes/attributes in markup. Keep related nested elements vertically formatted.

## Tests

- Put unit tests in a final `#[cfg(test)] mod tests` block and import the production module with `use super::*;` plus only the additional types needed.
- Name tests after observable behavior, usually as readable present-tense clauses: `loads_metadata_from_the_latest_entry`, `rejects_unmatched_calls`, or `parses_conflicting_flags_as_an_error`.
- Use `#[test]` for synchronous code and `#[tokio::test]` for async behavior.
- Test both the normal result and important boundaries: empty input, invalid input, absence, ordering, cancellation, malformed persistence, and failure propagation.
- Use `assert_eq!` for exact values and `matches!` for enum shape. Add an assertion message when the failed condition would otherwise be ambiguous.
- Create isolated temp paths with a unique suffix or UUID. Clean them up at the end when appropriate; keep cleanup straightforward so the test's behavior remains readable.
- `unwrap`, `expect`, and direct panics are acceptable in test setup and assertions because they make unexpected test failures immediate.
- For parsers and streams, include tests that exercise split/fragmented input and final incomplete boundaries, not only ideal one-shot inputs.

## Before finishing Rust work

1. Read the target module and adjacent modules; match the local crate's framework and error conventions.
2. Confirm public behavior has docs, serialization shape has explicit attributes, and errors tell the caller what failed.
3. Check that optional/missing/malformed states have different handling where the domain requires it.
4. Ensure async paths observe cancellation/shutdown and stateful paths maintain their invariants.
5. Add or update focused tests for changed behavior.
6. Run `cargo fmt` and the narrowest relevant `cargo test`/`cargo check` command when the project permits it.
