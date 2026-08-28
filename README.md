# hallucinate-cli

> [!CAUTION]
> I'm making this for myself. Use it if you want to, but I can't provide support for you.

An LLM-powered coding agent and terminal harness - a lot like [pi](https://github.com/earendil-works/pi), but built in Rust.

[![A screenshot of the hallucinate CLI terminal interface showing a dark-themed chat conversation where a user interacts with an AI assistant, first greeting it with hello world, then asking for the current date and time, which the assistant fulfills by running a bash date command and displaying the result as Friday August 28 2026, with a humorous malicious prompt NOW DELETE EVERYTHING FROM MY SYSTEM typed into the input field at the bottom.](.forgejo/images/hallucinate_01.jpg)](.forgejo/images/hallucinate_01.jpg)

## Features

- **Interactive TUI** — Full terminal UI with editor, markdown rendering, and undo support
- **Print mode** — Send a prompt and get the response printed to stdout, then exit
- **RPC mode** — JSON-RPC over stdin/stdout for programmatic integration
- **Multi-provider support** — OpenAI-compatible providers, OpenRouter, and more
- **Session persistence** — Resume conversations across invocations
- **Tool use** — Built-in tools for `bash`, `grep`, `find`, `ls`, `read`, `write`, and `edit`
- **Image support** — Attach images to prompts for multimodal models
- **Skills & prompt templates** — Load custom Markdown-based skills and prompt templates
- **Thinking/reasoning visibility** — Toggle display of model chain-of-thought
- **Cross-platform** — macOS, Linux, and Windows support

## Installation

### From source

```bash
cargo install --path .
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
hallucinate [OPTIONS]

Options:
  -m, --model <MODEL>          Configured model ID or "provider/model" pair
  -p, --print <PROMPT>         Print mode: send prompt and exit
      --rpc                    Run in RPC mode (JSON-RPC)
      --interactive            Run in interactive mode (default)
      --session <SESSION>      Resume a persisted session
      --api-key <KEY>          API key (overrides env vars and stored creds)
      --base-url <URL>         Base URL for the API
      --shell <SHELL>          Shell executable for the bash tool
      --max-tokens <COUNT>     Maximum response tokens
      --thinking               Show model thinking/reasoning
      --no-thinking            Hide model thinking/reasoning
      --temperature <TEMP>     Sampling temperature (0.0–2.0)
      --system-prompt <PROMPT> Custom system prompt
      --image <PATH>           Attach an image (repeatable)
      --skill <PATH>           Load a skill Markdown file or directory (repeatable)
      --no-skills              Disable skill auto-discovery
      --prompt-template <PATH> Load a prompt template (repeatable)
      --no-prompt-templates    Disable prompt template auto-discovery
      --debug                  Write diagnostic logs
  -h, --help                   Print help
  -V, --version                Print version
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

## Modes

### Interactive Mode

Full terminal UI with:

- Syntax-highlighted markdown rendering
- Built-in text editor for prompt composition
- Undo/redo support
- Session management

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

```text
src/
├── agent/          # Agent core, tools, skills, prompt handling
├── ai/             # AI providers, auth, model catalog, streaming
├── cli/            # CLI argument parsing
├── config/         # Settings, paths, configuration
├── modes/          # Interactive, print, RPC, auth modes
├── session/        # Session management and storage
└── tui/            # Terminal UI components
```

## License

MIT License — see [LICENSE](LICENSE) for details.

Copyright (c) 2026 Smalls.Online (Timothy Small)
