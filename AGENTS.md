# AGENTS.md

## Project Overview

**hallucinate-cli** is a Rust CLI application implementing the Pi Agent Harness — an AI coding agent tool for terminal-based interactive development. It provides a terminal UI for interacting with LLMs (via OpenAI API-compatible endpoints), with built-in tool use (bash, file operations, search), session management, and multiple run modes (interactive, print, RPC).

This is a single-binary Rust crate (not a Cargo workspace), organized into modules that mirror the TypeScript/JavaScript source from the `pi` project. Key features:

- **LLM Provider Abstraction**: OpenAI API-compatible provider supporting any compatible endpoint (OpenAI, Azure, Ollama, vLLM, LM Studio, etc.) with multi-provider model support via `providers` config section
- **Agent Runtime**: Single `Agent` struct in `agent::runtime` owns the production turn-loop, builds context, executes tool calls, and emits lifecycle events
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

# Build debug
cargo build

# Build release
make build-release
# or
cargo build --release

# Run the binary
cargo run
cargo run --release

# Cross-compile (Makefile detects platform automatically)
make build
```

## Development Workflow

### Module Structure

```plain
src/
├── main.rs              # CLI entry point
├── cli/                 # CLI commands (args, mod)
│   ├── args.rs          # Argument parsing (clap derive, subcommands)
│   └── mod.rs
├── ai/                  # LLM provider abstraction (pi-ai)
│   ├── types.rs         # Message, Tool, Model, event types
│   ├── provider.rs      # Provider trait + dispatch
│   ├── providers/       # Provider implementations
│   │   ├── mod.rs
│   │   ├── openai_compat.rs  # OpenAI API-compatible provider
│   │   └── faux.rs          # Test provider (no real API calls)
│   ├── auth.rs          # OAuth, credential store (multi-provider)
│   ├── model_catalog.rs # Model registry
│   ├── streaming.rs     # Event stream protocol
│   └── images.rs        # Image support (base64, auto-resize)
├── agent/               # Agent runtime (agent-core)
│   ├── runtime.rs       # Agent struct, turn-loop, tool execution, events
│   ├── tools/           # Tool implementations
│   │   ├── mod.rs
│   │   ├── bash.rs      # Shell execution (configurable shell, full syntax)
│   │   ├── read.rs      # File read
│   │   ├── write.rs     # File write
│   │   ├── edit.rs      # Intelligent edit
│   │   ├── find.rs      # File search (glob)
│   │   ├── grep.rs      # Pattern-based search
│   │   ├── ls.rs        # Directory listing
│   │   └── types.rs     # Tool trait, results
│   ├── compaction.rs    # Context window management
│   ├── system_prompt.rs # System prompt assembly
│   ├── prompt_templates.rs # Prompt template system
│   ├── skills.rs        # Skills loading and execution (.agents protocol)
│   └── types.rs         # Agent-specific types
├── tui/                 # Terminal UI (pi-tui)
│   ├── alt_screen.rs    # Alternate screen renderer
│   ├── components/      # UI components
│   │   ├── mod.rs
│   │   └── editor.rs    # Multi-line editor with cursor, undo, keyboard enhancements
│   ├── input.rs         # Keyboard/mouse input
│   ├── keys.rs          # Key parsing
│   ├── mod.rs
│   └── undo.rs          # Undo stack
├── session/             # Session management
│   ├── mod.rs
│   ├── manager.rs       # Session CRUD, listing
│   ├── storage.rs       # JSONL session storage
│   └── cwd.rs           # Session working directory
├── config/              # Settings and paths
│   ├── mod.rs
│   ├── paths.rs         # Config directory paths (.agents protocol support)
│   └── settings.rs      # TOML settings manager with multi-provider config
├── modes/               # Run modes
│   ├── mod.rs           # Model resolution, provider creation, shell resolution
│   ├── interactive.rs   # Interactive mode (main UI, model picker, bracketed paste)
│   ├── print.rs         # Print mode (-p "prompt")
│   ├── rpc.rs           # RPC mode (JSON-RPC)
│   ├── auth.rs          # Auth CLI subcommand handler
│   └── commands.rs      # Slash command dispatch
└── logging.rs           # Tracing/logging setup
```

### File Organization Conventions

- One public struct/enum per file, with a corresponding `mod.rs` or parent module exporting it
- Modules are organized by domain (ai, agent, tui, session, etc.)
- Sub-modules live in subdirectories (e.g., `tools/`, `providers/`, `components/`)
- Use `mod.rs` for module declarations within subdirectories

### Note on Removed Modules

The `protocol/` directory (CBOR protocol implementation) has been removed. The RPC mode now uses newline-delimited JSON-RPC 2.0 instead. The `agent::runtime::Agent` consolidates the former `agent.rs` and `agent_loop.rs` into a single runtime module.

### Adding Dependencies

Update `Cargo.toml` and run `cargo build` to verify. The project uses Renovate for automated dependency updates (configured in `renovate.json`).

## Code Style

The project uses `rustfmt` with a custom configuration (`rustfmt.toml`):

```bash
# Format all code
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

- **Unit tests**: Inline `#[cfg(test)]` modules in source files for AI providers (mock HTTP), agent loop (faux provider), TUI components (headless), JSON/TOML serialization, tools, and protocol
- **HTTP mocking**: Use `mockito::Server` for provider tests
- **Async tests**: Use `#[tokio::test]`
- **Test utilities**: `tokio-test` for async stream testing
- **Bash tool tests**: Verify shell syntax support (pipes, redirects, `&&`, `||`) and configurable shell execution
- **Editor tests**: Multi-line input, Shift+Enter/Ctrl+J for newlines, vertical cursor movement, bracketed paste normalization

### Running Tests

```bash
# Run all tests
cargo test

# Run tests in a specific module
cargo test -p hallucinate-cli module_name

# Run a specific test
cargo test -p hallucinate-cli specific_test_name

# Run tests with output
cargo test -- --nocapture
```

### Adding Tests

- Place tests in the same file as the code using `#[cfg(test)]` modules
- Use `#[tokio::test]` for async test functions
- Use `mockito::Server` for mocking HTTP responses in provider tests

## Build and Deployment

### Build Targets

The Makefile supports cross-compilation for Linux (amd64/arm64), macOS (amd64/arm64), and Windows (amd64/arm64):

```bash
# Show detected platform config
make show-config

# Build for current platform
make build

# Build release
make build-release

# Build with nightly toolchain
make build-nightly

# Build with verbose output
make build-verbose

# Clean build artifacts
make clean

# Check for errors
make check
```

### Artifacts

- Debug builds: `target/<target-triple>/debug/hallucinate-cli`
- Release builds: `target/<target-triple>/release/hallucinate-cli`

### CI/CD

GitHub Actions workflow (`.forgejo/workflows/build.yml`) runs `make build` on push to `main` and on PRs.

## Pull Request Guidelines

- **Title format**: `[component] Brief description` (e.g., `[agent] Add tool execution loop`)
- **Branch workflow**: Uses Git Town (configured in `.git-branches.toml`)
- **Merge strategy**: Feature branches use merge strategy (`sync.feature-strategy = "merge"`)
- **CI**: GitHub Actions workflow runs `make build` on PRs and pushes to `main`
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
