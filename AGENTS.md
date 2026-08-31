# AGENTS.md

## Project Overview

**hallucinate-cli** is a re-implementation of the Pi Agent Harness — an AI coding agent tool for terminal-based interactive development — into Rust. It provides a terminal UI for interacting with LLMs (via OpenAI API-compatible endpoints), with built-in tool use (bash, file operations, search), session management, and multiple run modes (interactive, print, RPC).

This is a **Cargo workspace** with three crates:

| Crate | Path | Description | Publish |
| --- | --- | --- | --- |
| `hallucinate` | `crates/hallucinate` | CLI binary — entry point, argument parsing, run modes, session management, logging | `smalls-personal` |
| `hallucinate-agent` | `crates/hallucinate-agent` | Agent core library — LLM provider abstraction, agent runtime, tools, config, skills | `smalls-personal` |
| `hallucinate-tui` | `crates/hallucinate-tui` | Terminal UI library — ratatui-based rendering, editor, keyboard input, undo, markdown | `smalls-personal` |

**Dependency graph**: `hallucinate-tui` → `hallucinate-agent` · `hallucinate` → `hallucinate-agent` + `hallucinate-tui`

Key features:

- **LLM Provider Abstraction**: OpenAI API-compatible provider supporting any compatible endpoint (OpenAI, Azure, Ollama, vLLM, LM Studio, etc.) with multi-provider model support via `providers` config section
- **Agent Runtime**: Single `Agent` struct in `hallucinate-agent::agent::runtime` owns the production turn-loop, builds context, executes tool calls, and emits lifecycle events
- **Terminal UI**: Built on `ratatui` with alternate screen, multi-line editor with keyboard enhancements, undo support, model picker overlay, and markdown rendering
- **Multiple Modes**: Interactive (TUI), print (`-p "prompt"`), and RPC (newline-delimited JSON-RPC 2.0 over stdin/stdout)
- **Session Management**: JSONL-based session storage with CRUD operations
- **Skills System**: Load and execute skills from user config, project `.hallucinate`, shared `.agents` protocol directories, and `--skill` paths
- **Image Support**: Base64 image content and auto-resize via `image` crate
- **Auth CLI**: `auth set/remove/list` subcommands for provider API key management
- **Configurable Shell**: Bash tool supports explicit shell executable and full shell syntax (pipes, redirects, `&&`, `||`)

**Excluded from scope**: Plugins/extensions, server/client packages, evals framework, SQLite session backends, telemetry, NPM publish tooling, and CBOR protocol (removed).

## Key Technologies

| Layer | Technology |
| --- | --- |
| Language | Rust (edition 2024) |
| Workspace resolver | `"3"` (edition 2024) |
| Async Runtime | `tokio` (full features) |
| CLI Parsing | `clap` (derive, env, string features) |
| Terminal UI | `ratatui` 0.30 + `crossterm` 0.29 |
| HTTP Client | `reqwest` (json, stream features) |
| Serialization | `serde` + `serde_json` + `serde_yaml` + `toml` |
| Error Handling | `anyhow` (app-level) + `thiserror` (domain) |
| Markdown | `comrak` 0.54 |
| Images | `image` (bmp, gif, jpeg, png, webp) |
| Logging | `tracing` + `tracing-subscriber` (env-filter) |
| Paths | `directories` + `path-absolutize` + `shellexpand` |
| Other | `futures`, `tokio-stream`, `uuid`, `regex`, `glob`, `which`, `chrono` |

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
├── hallucinate/             # CLI binary crate
│   ├── src/
│   │   ├── main.rs          # CLI entry point
│   │   ├── cli/             # CLI commands (args, mod)
│   │   │   ├── args.rs      # Argument parsing (clap derive, subcommands)
│   │   │   └── mod.rs
│   │   ├── modes/           # Run modes
│   │   │   ├── mod.rs       # Model resolution, provider creation, shell resolution
│   │   │   ├── print.rs     # Print mode (-p "prompt")
│   │   │   ├── rpc.rs       # RPC mode (JSON-RPC)
│   │   │   ├── auth.rs      # Auth CLI subcommand handler
│   │   │   ├── commands.rs  # Slash command dispatch
│   │   │   └── interactive/ # Interactive mode (subdirectory)
│   │   │       ├── mod.rs       # Main UI, model picker, bracketed paste
│   │   │       ├── commands.rs  # Interactive slash commands
│   │   │       ├── input.rs     # Keyboard/mouse input handling
│   │   │       ├── render.rs    # TUI rendering logic
│   │   │       └── session.rs   # Session management in interactive mode
│   │   ├── session/         # Session management
│   │   │   ├── mod.rs
│   │   │   ├── manager.rs   # Session CRUD, listing
│   │   │   ├── storage.rs   # JSONL session storage
│   │   │   └── cwd.rs       # Session working directory
│   │   ├── logging.rs       # Tracing/logging setup
│   │   └── tests/
│   │       └── rpc.rs       # RPC integration tests
│   └── Cargo.toml
├── hallucinate-agent/       # Agent core library crate
│   ├── src/
│   │   ├── lib.rs           # Crate root, re-exports
│   │   ├── ai/              # LLM provider abstraction
│   │   │   ├── mod.rs
│   │   │   ├── types.rs     # Message, Tool, Model, event types
│   │   │   ├── provider.rs  # Provider trait + dispatch
│   │   │   ├── streaming.rs # Event stream protocol
│   │   │   ├── images.rs    # Image support (base64, auto-resize)
│   │   │   ├── auth.rs      # OAuth, credential store (multi-provider)
│   │   │   ├── model_catalog.rs # Model registry
│   │   │   └── providers/
│   │   │       ├── mod.rs
│   │   │       ├── openai_compat.rs  # OpenAI API-compatible provider
│   │   │       └── faux.rs          # Test provider (no real API calls)
│   │   ├── agent/           # Agent runtime
│   │   │   ├── mod.rs
│   │   │   ├── runtime.rs   # Agent struct, turn-loop, tool execution, events
│   │   │   ├── types.rs     # Agent-specific types
│   │   │   ├── compaction.rs    # Context window management
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
│   │   │       ├── read.rs      # File read
│   │   │       └── write.rs     # File write
│   │   └── config/          # Settings and paths
│   │       ├── mod.rs
│   │       ├── paths.rs     # Config directory paths (.agents protocol support)
│   │       └── settings.rs  # TOML settings manager with multi-provider config
│   └── Cargo.toml
└── hallucinate-tui/         # Terminal UI library crate
    ├── src/
    │   ├── lib.rs           # Crate root, re-exports
    │   ├── alt_screen.rs    # Alternate screen renderer
    │   ├── components/      # UI components
    │   │   ├── mod.rs
    │   │   ├── editor.rs    # Multi-line editor with cursor, undo, keyboard enhancements
    │   │   └── markdown.rs  # Markdown rendering
    │   ├── input.rs         # Keyboard/mouse input
    │   ├── keys.rs          # Key parsing
    │   └── undo.rs          # Undo stack
    └── Cargo.toml
```

### File Organization Conventions

- One public struct/enum per file, with a corresponding `mod.rs` or parent module exporting it
- Modules are organized by domain (ai, agent, tui, session, etc.)
- Sub-modules live in subdirectories (e.g., `tools/`, `providers/`, `components/`)
- Use `mod.rs` for module declarations within subdirectories
- Workspace-level dependencies are declared in root `Cargo.toml` under `[workspace.dependencies]`

### Note on Removed Modules

The `protocol/` directory (CBOR protocol implementation) has been removed. The RPC mode now uses newline-delimited JSON-RPC 2.0 instead. The `hallucinate-agent::agent::runtime::Agent` consolidates the former `agent.rs` and `agent_loop.rs` into a single runtime module.

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

### Artifacts

- Debug builds: `target/<target-triple>/debug/hallucinate`
- Release builds: `target/<target-triple>/release/hallucinate`
- Cross-compiled artifacts: `artifacts/<os>_<arch>/hallucinate`

### CI/CD

Forgejo CI workflow (`.forgejo/workflows/build.yml`) runs `make build` on push to `main` and on PRs.

## Pull Request Guidelines

- **Title format**: `[component] Brief description` (e.g., `[agent] Add tool execution loop`)
- **Branch workflow**: Uses Git Town (configured in `.git-branches.toml`)
- **Merge strategy**: Feature branches use merge strategy (`sync.feature-strategy = "merge"`)
- **CI**: Forgejo CI workflow runs `make build` on PRs and pushes to `main`
- **PR template**: Defined in `.forgejo/pull_request_template.md`

## Debugging and Troubleshooting

### Common Issues

1. **Cross-compilation failures on Linux**: Ensure `x86_64-linux-gnu-gcc` or `aarch64-linux-gnu-gcc` is installed
2. **Terminal rendering issues**: Check `TERM` environment variable; ensure terminal supports alternate screen
3. **LLM API errors**: Verify API key via `--api-key` flag, `HALLUCINATE_API_KEY` env var, or config file
4. **Keyboard enhancement failures**: If keyboard enhancement (Kitty protocol) fails, the TUI falls back gracefully — check `RUST_LOG=debug` for details
5. **Multi-line editor not working**: Shift+Enter inserts a newline; Enter submits. In terminals without Shift+Enter support, use Ctrl+J for newlines
6. **Bash tool quoting issues**: Commands are passed to the shell via `-c` (not split on whitespace) — this preserves quoting and shell operators like `|`, `&&`, `||`, `>`

### Logging

Logging is implemented via `tracing` + `tracing-subscriber`. Use `RUST_LOG=debug` to enable debug-level output.

### Cargo Registries

Custom registries are configured in `.cargo/config.toml`:

- `crates-io` (default)
- `smalls-personal`: `sparse+https://git.smalls.online/api/packages/smalls/cargo/`
- `smalls-online`: `sparse+https://git.smalls.online/api/packages/smallsonline/cargo/`

## Additional Notes

- The project is a reimplementation of the TypeScript `pi` agent harness.
- Configuration files: `.editorconfig`, `rustfmt.toml`, `renovate.json`
- Git workflow: Git Town for branch management, Forgejo for PRs

### Configuration File Format

Settings are stored in **TOML** format (`settings.toml`), not JSON. The config supports:

- **Multi-provider models**: Define providers with their own `base_url`, `api_key`, `headers`, and nested models
- **Credential store**: API keys stored per-provider in `[credential_store.credentials.<provider>]`
- **Configurable shell**: Optional `shell = "/bin/bash"` setting for the bash tool
- **Model picker**: Models can be selected interactively with `/model` or via `provider/model` selector

### Skills Discovery

Skills are loaded from multiple locations (first match wins):

1. `~/.hallucinate/skills` — user config directory
2. `<cwd>/.hallucinate/skills` — project-local
3. `~/.agents/skills` — shared `.agents` protocol (root files ignored, skills in `SKILL.md` or subdirectories)
4. `<cwd>/.agents/skills` (and ancestors up to git root) — shared `.agents` protocol per-project

### Multi-line Editor

The interactive editor supports multi-line input:

- **Shift+Enter** or **Ctrl+J**: Insert a new line
- **Enter**: Submit the complete message
- **Up/Down**: Move through lines in multi-line mode (scroll transcript in single-line mode)
- **Bracketed paste**: Enabled; line endings normalized to `\n`
- **Keyboard enhancement**: Kitty protocol enabled for enhanced key reporting (graceful fallback if unavailable)
