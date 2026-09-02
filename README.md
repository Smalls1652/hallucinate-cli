# hallucinate-cli

> [!CAUTION]
> I'm making this for myself. Use it if you want to, but I can't provide support for you.

An LLM-powered coding agent and terminal harness — a lot like [pi](https://github.com/earendil-works/pi), but built in Rust.

[![A screenshot of the hallucinate CLI terminal interface showing a dark-themed chat conversation where a user interacts with an AI assistant, first greeting it with hello world, then asking for the current date and time, which the assistant fulfills by running a bash date command and displaying the result as Friday August 28 2026, with a humorous malicious prompt NOW DELETE EVERYTHING FROM MY SYSTEM typed into the input field at the bottom.](.forgejo/images/hallucinate_01.jpg)](.forgejo/images/hallucinate_01.jpg)

## Features

- **Interactive TUI** — Full terminal UI with syntax-highlighted markdown, multi-line editor with word-level cursor navigation, undo support, and keyboard-driven session/model pickers
- **Print mode** — Send a prompt and get the response printed to stdout, then exit
- **RPC mode** — JSON-RPC over stdin/stdout for programmatic integration
- **Provider support** — Supports any OpenAI-compatible provider with either Completions or Responses API endpoints
- **Tool use** — Built-in tools for `bash`, `grep`, `find`, `ls`, `read`, `write`, and `edit`
- **Image support** — Attach images to prompts for multimodal models (bmp, gif, jpeg, png, webp)
- **Skills & prompt templates** — Auto-discovered from user and project directories; manual loading via CLI flags
- **Thinking/reasoning visibility** — Toggle display of model chain-of-thought
- **Context window management** — Automatic compaction and truncation
- **Cross-platform** — macOS, Linux, and Windows support with cross-compilation

## Installation

### From source

```bash
cargo install --path ./crates/hallucinate
```

### Build manually

```bash
make build-release-artifacts
# Binary is placed in artifacts/<os>_<arch>/hallucinate
```

### Requirements

- Rust 1.85+ (uses edition 2024)
- A supported LLM provider API key

## Quick Start

### 1. Configure a provider

Models are not included by default. Add at least one provider and model to `settings.toml` before starting a run mode. For example:

```toml
default_model = "local/llama-3"

[providers.local]
base_url = "http://localhost:1234/v1"

[providers.local.models.llama-3]
name = "Llama 3"
# Use `openai-responses` for providers exposing /responses.
# api = "openai-responses"
```

The settings file is stored in the application's configuration directory. Provider model IDs and metadata are user-configurable, so the same structure works for any OpenAI-compatible endpoint.

### 2. Authenticate with a provider

```bash
# Save an API key (prefer passing via environment variable)
hallucinate auth set <provider> --api-key "$HALLUCINATE_API_KEY"

# List saved providers
hallucinate auth list

# Remove a saved key
hallucinate auth remove <provider>
```

### 3. Interactive mode (default)

```bash
hallucinate
```

### 4. Print mode — one-shot prompt

```bash
hallucinate -p "Explain why metalcore music is sometimes referred to as unga-bunga music"
```

### 5. RPC mode — JSON-RPC over stdin/stdout

```bash
hallucinate --rpc
```

## Usage

```text
hallucinate [OPTIONS] [COMMAND]

Commands:
  auth  Save, remove, or list provider API keys

Options:
  -m, --model <MODEL>                       Model to use as a configured model ID or "provider/model" pair
      --api-key <API_KEY>                   API key for the provider. Overrides environment variables and stored credentials.
      --base-url <BASE_URL>                 Base URL for the API (for OpenAI-compatible providers)
      --shell <SHELL>                       Shell executable for the bash tool
  -p, --print <PRINT>                       Print mode: send prompt and print the response, then exit
      --rpc                                 Run in RPC mode (JSON-RPC over stdin/stdout)
      --interactive                         Run in interactive mode (default)
      --session <SESSION>                   Resume this persisted session in interactive mode
      --max-tokens <MAX_TOKENS>             Maximum number of tokens in the response
      --thinking                            Show model thinking/reasoning in output
      --no-thinking                         Hide model thinking/reasoning in output
      --temperature <TEMPERATURE>           Temperature for sampling (0.0 - 2.0)
      --system-prompt <SYSTEM_PROMPT>       System prompt to use
      --image <IMAGES>                      Attach an image to a print-mode prompt (repeatable)
      --skill <SKILLS>                      Load a skill Markdown file or directory (repeatable)
      --no-skills                           Disable automatic skill discovery
      --prompt-template <PROMPT_TEMPLATES>  Load a prompt template file or directory (repeatable)
      --no-prompt-templates                 Disable automatic prompt template discovery
      --debug                               Write diagnostic logs to the application cache directory
  -h, --help                                Print help
  -V, --version                             Print version
```

## Authentication

Providers are managed via the `auth` subcommand. Keys are stored securely and resolved through environment variables (e.g., `HALLUCINATE_OPENAI_API_KEY`).

```bash
hallucinate auth set <provider> [--api-key <KEY>]
hallucinate auth remove <provider>
hallucinate auth list
```

## Configuration

Settings are loaded from the application's config directory. You can override individual settings via CLI flags.

The built-in system prompt can be replaced with a `SYSTEM.md` file in the application's global config directory. A project-level `AGENTS.md` in the working directory is appended to the system prompt. Interactive sessions show any discovered prompt files under the `Context` section when they start.

Project-local settings overrides are also supported.

## Modes

### Interactive Mode

Full terminal UI with:

- Syntax-highlighted markdown rendering with dimmed code blocks
- Multi-line editor with word-level cursor navigation, undo/redo, and Shift+Enter for newlines
- Keyboard-driven session picker and model picker overlays
- Slash-command autocomplete (accept with Enter)
- Merged transcript cards for consecutive assistant turns
- Session management with fork support

### Print Mode

Send a prompt and receive the full response on stdout. Ideal for scripting and CI pipelines.

```bash
hallucinate -p "What files are in the current directory?"
```

### RPC Mode

Exposes a JSON-RPC interface over stdin/stdout for integration with editors, CI systems, or other tools.

## Skills & Prompt Templates

### Skills

Load custom skill definitions from Markdown files or directories:

```bash
hallucinate --skill ./skills/git/SKILL.md --skill ./skills/release/
```

### Prompt Templates

Load reusable prompt templates:

```bash
hallucinate --prompt-template ./prompts/review.md
```

Both skills and prompt templates are auto-discovered from user and project directories unless disabled with `--no-skills` and `--no-prompt-templates`.

## Build

| Target | Command |
| --- | --- |
| Debug build | `make build` |
| Release build | `make build-release` |
| Release + artifacts | `make build-release-artifacts` |
| Format code | `make format` |
| Check for errors | `make check` |
| Clean | `make clean` |

## Project Structure

The project is a Cargo workspace with four crates:

```text
crates/
├── hallucinate-cli/         # CLI parsing and print/RPC/interactive startup
├── hallucinate-agent/       # Provider contracts, runtime, tools, config, skills
├── hallucinate-session/     # CLI-oriented JSONL session persistence
└── hallucinate-tui/         # Terminal primitives and interactive controller
    └── src/
        ├── interactive/     # Controller, commands, input, rendering, sessions
        ├── slash_commands.rs # Canonical interactive command catalog
        └── components/      # Editor and markdown renderer
```

| Crate | Description |
| --- | --- |
| `hallucinate-cli` | CLI binary — argument parsing and run-mode startup policy |
| `hallucinate-agent` | Agent core — LLM provider abstraction, runtime, tools, config, and skills |
| `hallucinate-session` | CLI-oriented append-only JSONL session persistence and lifecycle archival |
| `hallucinate-tui` | Terminal UI and interactive controller — rendering, editor, input, and commands |

## License

MIT License — see [LICENSE](LICENSE) for details.

Copyright (c) 2026 Smalls.Online (Timothy Small)
