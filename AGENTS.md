# AGENTS.md

## Project Overview

**hallucinate-cli** is a re-implementation of the Pi Agent Harness — an AI coding agent tool for terminal-based interactive development — into Rust. It provides a terminal UI for interacting with LLMs (via OpenAI API-compatible endpoints), with built-in tool use (bash, file operations, search), session management, skills, prompt templates, and multiple run modes (interactive, print, RPC).

This is a **Cargo workspace** with four crates:

| Crate | Path | Description | Publish |
| --- | --- | --- | --- |
| `hallucinate-cli` | `crates/hallucinate-cli` | CLI binary — entry point, argument parsing, run modes, session management, logging | `smalls-personal` |
| `hallucinate-agent` | `crates/hallucinate-agent` | Agent core library — LLM provider abstraction, agent runtime, tools, config, skills, prompt templates | `smalls-personal` |
| `hallucinate-session` | `crates/hallucinate-session` | CLI-oriented JSONL session persistence and lifecycle archival | `smalls-personal` |
| `hallucinate-tui` | `crates/hallucinate-tui` | Terminal UI and interactive controller — ratatui rendering, editor, input, commands | `smalls-personal` |

**Dependency graph**: `hallucinate-tui` → `hallucinate-agent` · `hallucinate-cli` → `hallucinate-agent` + `hallucinate-tui` + `hallucinate-session`

Key features:

- **LLM Provider Abstraction**: OpenAI API-compatible provider supporting any compatible endpoint (OpenAI, Azure, Ollama, vLLM, LM Studio, etc.) with multi-provider model support via `providers` config section
- **Agent Runtime**: Single `Agent` struct in `hallucinate-agent::agent::runtime` owns the production turn-loop, builds context, executes tool calls, and emits lifecycle events
- **Context Compaction**: Context-history selection at turn boundaries with serialization for LLM-backed summarization; preserves complete user turns
- **Terminal UI**: Built on `ratatui` with alternate screen, multi-line editor with keyboard enhancements, undo support, model picker overlay, and markdown rendering
- **Multiple Modes**: Interactive (TUI), print (`-p "prompt"`), and RPC (newline-delimited JSON-RPC 2.0 over stdin/stdout)
- **Session Management**: CLI-oriented JSONL persistence with CRUD operations and lifecycle archival (via `hallucinate-session`)

- **Skills System**: Load and execute skills from user config, project `.hallucinate`, shared `.agents` protocol directories, and `--skill` paths
- **Prompt Templates**: Markdown files with YAML frontmatter that expand slash commands with argument substitution; discovered from user config and project-local directories
- **Image Support**: Base64 image content and auto-resize via `image` crate
- **Auth CLI**: `auth set/remove/list` subcommands for provider API key management
- **Configurable Shell**: Bash tool supports explicit shell executable and full shell syntax (pipes, redirects, `&&`, `||`)
- **System Prompt Override**: Place a `SYSTEM.md` in the config directory to replace the built-in system prompt; `AGENTS.md` at project root is appended under `## Project Instructions`
- **API Variants**: Both `openai-completions` (Chat Completions wire format) and `openai-responses` (Responses API wire format) are supported

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
cargo build --release --package hallucinate-cli

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
├── hallucinate-cli/         # CLI binary crate
│   ├── src/
│   │   ├── main.rs          # CLI entry point
│   │   ├── cli/             # CLI commands (args, mod)
│   │   │   ├── args.rs      # Argument parsing (clap derive, subcommands)
│   │   │   └── mod.rs
│   │   ├── modes/           # Run mode startup policy
│   │   │   ├── mod.rs
│   │   │   ├── print.rs     # Print mode (-p "prompt")
│   │   │   ├── rpc.rs       # RPC mode (JSON-RPC)
│   │   │   ├── auth.rs      # Auth CLI subcommand handler
│   │   │   └── interactive.rs # Interactive TUI startup
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
│   │   │   ├── title.rs     # Session title generation
│   │   │   ├── auth.rs      # OAuth, credential store (multi-provider)
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
│   │   │       └── write.rs     # File write
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
cargo test -p hallucinate-cli
cargo test -p hallucinate-agent
cargo test -p hallucinate-tui

# Run a specific test
cargo test -p hallucinate-cli specific_test_name

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

### CI/CD

Forgejo CI workflows:

- **Build** (`.forgejo/workflows/build.yml`): Runs on push to `main` and on PRs. Executes `make build`, `cargo check --all-targets`, and `cargo test --all-targets` on Ubuntu.
- **Publish** (`.forgejo/workflows/publish_release.yml`): Runs on tag push (`*.*.*` or `*.*.*-*`). Adds the `smalls-personal` cargo registry and publishes all workspace crates.

## Pull Request Guidelines

- **Title format**: `[component] Brief description` (e.g., `[agent] Add tool execution loop`)
- **Branch workflow**: Uses Git Town (configured in `.git-branches.toml`)
- **Merge strategy**: Feature branches use merge strategy (`sync.feature-strategy = "merge"`)
- **CI**: Forgejo CI workflow runs build, check, and test on PRs and pushes to `main`
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
- Full configuration reference: `docs/config.md`

### Configuration File Format

Settings are stored in **TOML** format (`settings.toml`), not JSON. A project-local override may be placed at `.hallucinate/settings.toml` (merged on top of the global file).

**Key settings**:

| Setting | Type | Default | Description |
| --- | --- | --- | --- |
| `default_model` | `string` | — | Default model selector (`provider/model`) |
| `provider` | `string` | `"openai"` | Credential namespace for the default provider |
| `base_url` | `string` | — | Global API endpoint for the default provider |
| `shell` | `string` | `$SHELL` | Shell executable used by the bash tool |
| `timeout_ms` | `number` | `600000` | Request timeout in milliseconds |
| `max_retries` | `number` | `3` | Retries for transient LLM API failures |
| `auto_resize_images` | `bool` | `true` | Automatically resize image attachments |
| `max_turns` | `number` | `120` | Maximum conversation turns per prompt |
| `enable_tools` | `bool` | `true` | Enable tool use by default |
| `enable_compaction` | `bool` | `true` | Enable automatic context compaction |
| `show_thinking` | `bool` | `false` | Show model reasoning output to the user |
| `temperature` | `number` | — | Default sampling temperature |
| `max_tokens` | `number` | — | Default maximum output tokens |
| `providers` | `table` | — | Multi-provider model definitions |
| `credential_store` | `table` | — | Stored API keys per provider |
| `provider_headers` | `table` | — | Extra HTTP headers for the default provider |

**Multi-provider models**: Define providers with their own `base_url`, `api_key`, `headers`, and nested models.

**Credential store**: API keys stored per-provider in `[credential_store.credentials.<provider>]`. Use `auth set <provider>` to populate.

**Configurable shell**: Optional `shell = "/bin/bash"` setting for the bash tool.

**Model picker**: Models can be selected interactively with `/model` or via `provider/model` selector.

### Skills Discovery

Skills are Markdown files with YAML frontmatter, discovered from multiple locations (first match wins):

| Priority | Location | Description |
| --- | --- | --- |
| 1 | `~/.hallucinate/skills` | User config directory |
| 2 | `<cwd>/.hallucinate/skills` | Project-local skills |
| 3 | `~/.agents/skills` | Shared `.agents` protocol |
| 4 | `<cwd>/.agents/skills` (and ancestors to git root) | Shared `.agents` protocol per-project |

**Command-line override**: `hallucinate --skill path/to/skill.md --skill another-skill/`

**Disable auto-discovery**: `hallucinate --no-skills`

### Prompt Templates

Prompt templates are Markdown files with YAML frontmatter that expand slash commands with argument substitution.

**Discovery locations**:

| Priority | Location | Description |
| --- | --- | --- |
| 1 | `~/.hallucinate/prompts` | User config directory |
| 2 | `<cwd>/.hallucinate/prompts` | Project-local templates |

**Command-line override**: `hallucinate --prompt-template path/to/template.md`

**Disable auto-discovery**: `hallucinate --no-prompt-templates`

**Argument substitution**: Templates support shell-like syntax: `$1`, `$2`, `@N` (positional), `$@`, `@`, `$ARGUMENTS` (all arguments), `${N:-default}` (default value).

### System Prompt

**Global override**: Place a `SYSTEM.md` in the config directory to replace the built-in system prompt entirely.

**Project instructions**: Place an `AGENTS.md` at the project root — its contents are appended under a `## Project Instructions` heading to every system prompt.

### Session Management

Sessions are stored as append-only JSONL files with CLI CRUD and lifecycle archival. `hallucinate-session` is intentionally CLI-oriented rather than a generic cross-frontend storage abstraction; legacy `.cwd.json` sidecars remain readable for migration.

### Multi-line Editor

The interactive editor supports multi-line input:

- **Shift+Enter** or **Ctrl+J**: Insert a new line
- **Enter**: Submit the complete message
- **Up/Down**: Move through lines in multi-line mode (scroll transcript in single-line mode)
- **Bracketed paste**: Enabled; line endings normalized to `\n`
- **Keyboard enhancement**: Kitty protocol enabled for enhanced key reporting (graceful fallback if unavailable)

### Environment Variables

| Variable | Description |
| --- | --- |
| `HALLUCINATE_API_KEY` | API key fallback (overridden by `--api-key`) |
| `OPENAI_API_KEY` | Fallback when the default provider is `"openai"` |
| `HALLUCINATE_<PROVIDER>_API_KEY` | Provider-specific key (e.g., `HALLUCINATE_OPENROUTER_API_KEY`) |
| `SHELL` | Shell executable for the bash tool (overridden by `shell` setting) |

Key resolution order: `--api-key` flag → `HALLUCINATE_API_KEY` → `OPENAI_API_KEY` (for default provider) → `HALLUCINATE_<PROVIDER>_API_KEY` → inline `api_key` in provider settings → stored credential.
