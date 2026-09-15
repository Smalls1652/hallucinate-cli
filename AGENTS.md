# AGENTS.md

## Project Overview

**hallucinate** is a re-implementation of the Pi Agent Harness — an AI coding agent tool for terminal-based interactive development — into Rust. It provides a terminal UI for interacting with LLMs (via OpenAI API-compatible endpoints), with built-in tool use (bash, file operations, search, user-defined script tools), session management, skills, prompt templates, and multiple run modes (interactive, print, RPC).

This is a **Cargo workspace** with four crates:

| Crate | Path | Description | Publish |
| --- | --- | --- | --- |
| `hallucinate` | `crates/hallucinate` | CLI binary — entry point, argument parsing, run modes, session management, logging | `smalls-personal` |
| `hallucinate-agent` | `crates/hallucinate-agent` | Agent core library — LLM provider abstraction, agent runtime, tools, config, skills, prompt templates | `smalls-personal` |
| `hallucinate-session` | `crates/hallucinate-session` | CLI-oriented JSONL session persistence and lifecycle archival | `smalls-personal` |
| `hallucinate-tui` | `crates/hallucinate-tui` | Terminal UI and interactive controller — ratatui rendering, editor, input, commands | `smalls-personal` |

**Dependency graph**: `hallucinate-tui` → `hallucinate-agent` · `hallucinate` → `hallucinate-agent` + `hallucinate-tui` + `hallucinate-session`

Key features:

- **LLM Provider Abstraction**: OpenAI API-compatible provider supporting any compatible endpoint (OpenAI, Azure, Ollama, vLLM, LM Studio, etc.) with multi-provider model support via `providers` config section
- **Agent Runtime**: Single `Agent` struct in `hallucinate-agent::agent::runtime` owns the production turn-loop, builds context, executes tool calls, and emits lifecycle events
- **Context Compaction**: Context-history selection at turn boundaries with serialization for LLM-backed summarization; preserves complete user turns
- **Terminal UI**: Built on `ratatui` with alternate screen, multi-line editor with keyboard enhancements, undo support, model picker overlay, markdown rendering, thinking/reasoning display, tool result rendering, and status bar
- **Multiple Modes**: Interactive (TUI), print (`-p "prompt"`), and RPC (newline-delimited JSON-RPC 2.0 over stdin/stdout)
- **Session Management**: CLI-oriented JSONL persistence with CRUD operations and lifecycle archival (via `hallucinate-session`)

- **Skills System**: Load and execute skills from user config, project `.hallucinate`, shared `.agents` protocol directories, and `--skill` paths
- **Prompt Templates**: Markdown files with YAML frontmatter that expand slash commands with argument substitution; discovered from user config and project-local directories
- **Image Support**: Base64 image content and auto-resize via `image` crate
- **Auth CLI**: `auth set/remove/list` subcommands for provider API key management; keys are stored in the platform-native keyring (macOS keychain, Linux keyutils, Windows keyring), not in the settings file
- **Configurable Shell**: Bash tool supports explicit shell executable and full shell syntax (pipes, redirects, `&&`, `||`)
- **Script Tools**: User-defined tools backed by shell commands — define TOML files in `script_tools/` directories to expose any executable as an LLM-invocable tool with typed parameters and `{{parameter}}` template substitution. Loaded from user config and project-local `.hallucinate/script_tools` (project overrides user by tool ID). See `docs/script_tools.md` for the full reference.
- **System Prompt Override**: Place a `SYSTEM.md` in the config directory to replace the built-in system prompt; `AGENTS.md` at project root is appended under `## Project Instructions`
- **API Variants**: Both `openai-completions` (Chat Completions wire format) and `openai-responses` (Responses API wire format) are supported
- **Credential Resolution**: API keys are resolved in order: CLI override → environment variables (global, provider-specific, or fallback) → inline `api_key` in provider TOML settings → platform keyring. The `HALLUCINATE_<PROVIDER>_API_KEY` env var is supported for multi-provider setups

**Excluded from scope**: Plugins/extensions, server/client packages, evals framework, telemetry, NPM publish tooling, and CBOR protocol (removed).

## Key Technologies

| Layer | Technology |
| --- | --- |
| Language | Rust (edition 2024) |
| Workspace resolver | `"3"` (edition 2024) |
| Async Runtime | `tokio` (default-features = false) |
| CLI Parsing | `clap` (derive, env, string features) |
| Terminal UI | `ratatui` 0.30 + `crossterm` 0.29 |
| HTTP Client | `reqwest` (json, stream features) |
| Serialization | `serde` + `serde_json` + `serde_yaml` + `toml` |
| Error Handling | `anyhow` (app-level) + `thiserror` (domain) |
| Markdown | `comrak` 0.54 |
| Images | `image` (default-features = false) |
| Logging | `tracing` + `tracing-subscriber` (env-filter) |
| Paths | `directories` + `path-absolutize` + `shellexpand` |
| Other | `futures`, `tokio-stream`, `uuid`, `regex`, `glob`, `which`, `chrono`, `base64` |

## Setup Commands

```bash
# Install Rust (if not installed)
# Use rustup: https://rustup.rs/

# Build all workspace crates (debug)
cargo build

# Build release
make build-release
# or
cargo build --release --package hallucinate

# Run the binary
cargo run
cargo run --release

# Cross-compile (Makefile detects platform automatically)
make build

# Run checks without building
make check
```

## Development Workflow

### Workspace Structure

```plain
crates/
├── hallucinate/         # CLI binary crate (library + binary)
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs           # Library crate root, re-exports
│   │   ├── bin/
│   │   │   └── hallucinate.rs  # CLI entry point
│   │   ├── cli/             # CLI commands (args, subcommands, modes)
│   │   │   ├── mod.rs       # Argument parsing (clap derive, subcommands)
│   │   │   ├── auth.rs      # Auth CLI subcommand handler
│   │   │   ├── completions.rs  # Shell completions generation
│   │   │   ├── interactive.rs # Interactive TUI startup
│   │   │   ├── print.rs     # Print mode (-p "prompt")
│   │   │   └── rpc.rs       # RPC mode (JSON-RPC)
│   │   └── logging.rs       # Tracing/logging setup
│   └── tests/
│       └── rpc.rs           # RPC integration tests
├── hallucinate-agent/       # Agent core library crate
│   ├── src/
│   │   ├── lib.rs           # Crate root, re-exports
│   │   ├── ai/              # LLM provider abstraction
│   │   │   ├── mod.rs
│   │   │   ├── types.rs     # Message, Tool, Model, event types
│   │   │   ├── provider.rs  # Provider trait + dispatch
│   │   │   ├── streaming.rs # Event stream protocol
│   │   │   ├── images.rs    # Image support (base64, auto-resize)
│   │   │   ├── title.rs     # Session title generation
│   │   │   ├── auth.rs      # Platform keyring credential storage (OS-native keyring)
│   │   │   └── providers/
│   │   │       ├── mod.rs
│   │   │       ├── openai_compat.rs  # OpenAI API-compatible provider
│   │   │       └── faux.rs          # Test provider (no real API calls)
│   │   ├── agent/           # Agent runtime
│   │   │   ├── mod.rs
│   │   │   ├── runtime.rs   # Agent struct, turn-loop, tool execution, events
│   │   │   ├── types.rs     # Agent-specific types
│   │   │   ├── compaction.rs    # Context-history selection and serialization
│   │   │   ├── system_prompt.rs # System prompt assembly
│   │   │   ├── prompt_templates.rs # Prompt template system
│   │   │   ├── skills.rs        # Skills loading and execution (.agents protocol)
│   │   │   └── tools/
│   │   │       ├── mod.rs
│   │   │       ├── types.rs     # Tool trait, results
│   │   │       ├── bash.rs      # Shell execution (configurable shell, full syntax)
│   │   │       ├── edit.rs      # Intelligent edit
│   │   │       ├── find.rs      # File search (glob)
│   │   │       ├── grep.rs      # Pattern-based search
│   │   │       ├── ls.rs        # Directory listing
│   │   │       ├── paths.rs     # Shared filesystem path resolution
│   │   │       ├── limits.rs    # Tool output size limits
│   │   │       ├── walker.rs    # Filesystem traversal
│   │   │       ├── read.rs      # File read
│   │   │       ├── write.rs     # File write
│   │   │       └── script_tool.rs # User-defined script tools (TOML-backed shell commands)
│   │   └── config/          # Settings and paths
│   │       ├── mod.rs
│   │       ├── paths.rs     # Config directory paths (.agents protocol support)
│   │       └── settings.rs  # TOML settings manager with multi-provider config
│   └── Cargo.toml
├── hallucinate-session/     # CLI-oriented JSONL session persistence
│   ├── src/
│   │   ├── lib.rs           # Crate root, re-exports
│   │   ├── manager.rs       # Session lifecycle and event archival
│   │   ├── storage.rs       # JSONL records and metadata
│   │   ├── cwd.rs           # Legacy .cwd.json sidecar migration
│   │   └── events.rs        # Session event types and serialization
│   └── Cargo.toml
└── hallucinate-tui/         # Terminal UI and interactive controller
    ├── src/
    │   ├── lib.rs           # Crate root, re-exports
    │   ├── interactive/     # Controller, commands, input, render, session sync
    │   │   ├── mod.rs
    │   │   ├── commands.rs  # Interactive command handling
    │   │   ├── input.rs     # Interactive input processing
    │   │   ├── render.rs    # Interactive rendering
    │   │   └── session.rs   # Session sync and management
    │   ├── slash_commands.rs # Canonical interactive command catalog
    │   ├── alt_screen.rs    # Alternate screen renderer
    │   ├── components/      # UI components
    │   │   ├── mod.rs
    │   │   ├── editor.rs    # Multi-line editor with cursor, undo, keyboard enhancements
    │   │   ├── markdown.rs  # Markdown rendering
    │   │   ├── picker_overlays.rs  # Model picker and other overlay widgets
    │   │   ├── status_bar.rs   # Status bar rendering
    │   │   ├── thinking.rs     # Thinking/reasoning display
    │   │   └── tool_results.rs # Tool call and result rendering
    │   ├── input.rs         # Keyboard/mouse input
    │   ├── keys.rs          # Key parsing
    │   ├── undo.rs          # Undo stack
    │   └── utils.rs         # Shared utilities (line wrapping, truncation, prefixing)
    └── Cargo.toml
```

### File Organization Conventions

- One public struct/enum per file, with a corresponding `mod.rs` or parent module exporting it
- Modules are organized by domain (ai, agent, tui, session, etc.)
- Sub-modules live in subdirectories (e.g., `tools/`, `providers/`, `components/`)
- Use `mod.rs` for module declarations within subdirectories
- Workspace-level dependencies are declared in root `Cargo.toml` under `[workspace.dependencies]`

### Adding Dependencies

Update the root `Cargo.toml` under `[workspace.dependencies]` and reference via `.workspace = true` in the target crate's `Cargo.toml`. Run `cargo build` to verify. The project uses Renovate for automated dependency updates (configured in `renovate.json`).

## Code Style

The project uses `rustfmt` with a custom configuration (`rustfmt.toml`):

```bash
# Format all code in the workspace
make format
# or
cargo +nightly fmt -- --emit files
```

**Key conventions** (from `rustfmt.toml`):

- Imports: grouped (std → external → crate), crate-level granularity, horizontal-vertical layout
- Modules: auto-reordered
- Comments: wrapped at 80 characters
- Functions: vertical parameter layout
- Structs: impl items auto-reordered
- Trailing commas: vertical (including match blocks)

## Testing Instructions

### Test Strategy

- **Unit tests**: Inline `#[cfg(test)]` modules in source files for AI providers (mock HTTP), agent loop (faux provider), TUI components (headless), JSON/TOML serialization, tools
- **HTTP mocking**: Use `mockito::Server` for provider tests
- **Async tests**: Use `#[tokio::test]`
- **Test utilities**: `tokio-test` for async stream testing
- **Bash tool tests**: Verify shell syntax support (pipes, redirects, `&&`, `||`) and configurable shell execution
- **Editor tests**: Multi-line input, Shift+Enter/Ctrl+J for newlines, vertical cursor movement, bracketed paste normalization

### Running Tests

```bash
# Run all tests across the workspace
cargo test

# Run tests in a specific crate
cargo test -p hallucinate
cargo test -p hallucinate-agent
cargo test -p hallucinate-tui

# Run a specific test
cargo test -p hallucinate specific_test_name

# Run tests with output
cargo test -- --nocapture
```

### Adding Tests

- Place tests in the same file as the code using `#[cfg(test)]` modules
- Use `#[tokio::test]` for async test functions
- Use `mockito::Server` for mocking HTTP responses in provider tests
- Integration tests go in the crate's `tests/` directory

## Build and Deployment

### Build Targets

The Makefile supports cross-compilation for Linux (amd64/arm64), macOS (amd64/arm64), and Windows (amd64/arm64):

```bash
# Show detected platform config
make show-config

# Build for current platform (debug)
make build

# Build release for current platform
make build-release

# Build release and copy to artifacts/
make build-release-artifacts

# Build with nightly toolchain
make build-nightly

# Build with nightly + backtrace
make build-nightly-backtrace

# Build with verbose output
make build-verbose

# Clean build artifacts
make clean

# Check for errors
make check

# Check with nightly
make check-nightly
```

### Release Profile

The release profile uses size optimization (`opt-level = "s"`) and LTO:

```toml
[profile.release]
opt-level = "s"
lto = true
```

### Artifacts

- Debug builds: `target/<target-triple>/debug/hallucinate`
- Release builds: `target/<target-triple>/release/hallucinate`
- Cross-compiled artifacts: `artifacts/<os>_<arch>/hallucinate`

## Pull Request Guidelines

- **Title format**: `[component] Brief description` (e.g., `[agent] Add tool execution loop`)
- **Branch workflow**: Uses Git Town (configured in `.git-branches.toml`)
- **Merge strategy**: Feature branches use merge strategy (`sync.feature-strategy = "merge"`)
- **CI**: Forgejo CI workflow runs build, check, and test on PRs and pushes to `main`
- **PR template**: Defined in `.forgejo/pull_request_template.md`

## Debugging and Troubleshooting

### Logging

Logging is implemented via `tracing` + `tracing-subscriber`. Use `RUST_LOG=debug` to enable debug-level output.

### Cargo Registries

Custom registries are configured in `.cargo/config.toml`:

- `crates-io` (default)
- `smalls-personal`: `sparse+https://git.smalls.online/api/packages/smalls/cargo/`
- `smalls-online`: `sparse+https://git.smalls.online/api/packages/smallsonline/cargo/`

## Additional Notes

- Configuration files: `.editorconfig`, `rustfmt.toml`, `renovate.json`
- Git workflow: Git Town for branch management, Forgejo for PRs
- Full configuration reference: `docs/config.md`
