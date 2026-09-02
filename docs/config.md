# Configuration Reference

Complete reference for all configuration options in `hallucinate`.

## Overview

`hallucinate` reads configuration from a **TOML** settings file and a set of environment variables. Provider credentials are stored in the same file but separated into a dedicated `credential_store` section.

| Setting | Type | Default | Description |
| --- | --- | --- | --- |
| `default_model` | `string` | — | Default model selector (`provider/model`). |
| `provider` | `string` | `"openai"` | Credential namespace for the default provider. |
| `base_url` | `string` | — | Global API endpoint for the default provider. |
| `shell` | `string` | `$SHELL` | Shell executable used by the bash tool. |
| `timeout_ms` | `number` | `600000` | Request timeout in milliseconds. |
| `max_retries` | `number` | `3` | Retries for transient LLM API failures. |
| `auto_resize_images` | `bool` | `true` | Automatically resize image attachments. |
| `max_turns` | `number` | `120` | Maximum conversation turns per prompt. |
| `enable_tools` | `bool` | `true` | Enable tool use by default. |
| `enable_compaction` | `bool` | `true` | Enable automatic context compaction. |
| `max_concurrent_tool_calls` | `number` | `4` | Maximum independent tool calls running at once. |
| `show_thinking` | `bool` | `false` | Show model reasoning output to the user. |
| `temperature` | `number` | — | Default sampling temperature. |
| `max_tokens` | `number` | — | Default maximum output tokens. |
| `providers` | `table` | — | Multi-provider model definitions. |
| `credential_store` | `table` | — | Stored API keys per provider. |
| `provider_headers` | `table` | — | Extra HTTP headers for the default provider. |

## Settings File Location

| Platform | Path |
| --- | --- |
| macOS | `~/Library/Application Support/com.hallucinate.hallucinate/settings.toml` |
| Linux | `~/.config/hallucinate/settings.toml` |
| Windows | `%APPDATA%\com\hallucinate\hallucinate\settings.toml` |

A **project-local override** may be placed at `.hallucinate/settings.toml` in the working directory. It is merged on top of the global file — nested tables are merged recursively, while scalar values and arrays are replaced entirely.

## Provider Configuration

Providers are defined under the `providers` section. Each provider groups a base URL, optional inline API key, custom headers, and a set of models.

```toml
[providers.openai]
base_url = "https://api.openai.com/v1"

[providers.openai.models.gpt-4o]
name = "GPT-4o"
api = "openai-completions"
reasoning = false
input = ["text", "image"]
context_window = 128000
max_tokens = 16384

[providers.openai.models.gpt-4o.cost]
input = 2.50
output = 10.00
```

### Provider-Level Fields

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `base_url` | `string` | — | OpenAI-compatible endpoint for this provider. |
| `api_key` | `string` | — | Inline API key (prefer the credential store instead). |
| `headers` | `table` | — | Additional HTTP headers sent with every request. |
| `models` | `table` | — | Models provided by this provider, keyed by model ID. |

### Model-Level Fields

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `name` | `string` | Model ID | Human-readable model name. |
| `api` | `string` | `"openai-completions"` | API variant: `openai-completions` or `openai-responses`. |
| `base_url` | `string` | Provider URL | Per-model endpoint override. |
| `reasoning` | `bool` | `false` | Whether the model supports reasoning/thinking output. |
| `input` | `array` | `["text"]` | Supported input modalities (e.g., `"text"`, `"image"`). |
| `context_window` | `number` | `128000` | Maximum context window in tokens. |
| `max_tokens` | `number` | `16384` | Maximum output tokens the model can produce. |
| `cost.input` | `number` | `0` | Cost per million input tokens. |
| `cost.output` | `number` | `0` | Cost per million output tokens. |
| `cost.cache_read` | `number` | `0` | Cost per million cache read tokens. |
| `cost.cache_write` | `number` | `0` | Cost per million cache write tokens. |

### API Variants

| Value | Description |
| --- | --- |
| `openai-completions` | Chat Completions wire format. |
| `openai-responses` | Responses API wire format. |

## Credential Store

API keys are stored in the `credential_store` section. Use the `hallucinate auth set <provider>` command to populate this section securely.

```toml
[credential_store.credentials.openrouter]
provider_id = "openrouter"
api_key = "sk-or-..."
stored_at = 1725120000
```

| Field | Description |
| --- | --- |
| `provider_id` | Provider identifier this credential belongs to. |
| `api_key` | The stored API key. |
| `stored_at` | Unix timestamp (seconds) when the credential was saved. |

## Global Settings

### `default_model`

Default model selector. Can be a bare model ID or `provider/model`.

```toml
default_model = "openai/gpt-4o"
```

### `provider`

Credential namespace for the default provider. Used to resolve keys from the credential store when no provider is explicitly specified. Defaults to `"openai"`.

```toml
provider = "openai"
```

### `base_url`

Global API endpoint for the default provider. Provider-level `base_url` takes precedence when a model is selected from a different provider.

```toml
base_url = "https://api.openai.com/v1"
```

### `shell`

Shell executable used by the bash tool. When unset, the `SHELL` environment variable (or the platform fallback) is used.

```toml
shell = "/bin/bash"
```

### `timeout_ms`

Request timeout in milliseconds.

```toml
timeout_ms = 600000
```

### `max_retries`

Number of retries for transient LLM API failures.

```toml
max_retries = 3
```

### `auto_resize_images`

Automatically resize image attachments to provider-friendly limits.

```toml
auto_resize_images = true
```

### `max_turns`

Maximum number of conversation turns per prompt. The agent stops with an error when this limit is reached.

```toml
max_turns = 120
```

### `enable_tools`

Enable tool use by default. When disabled, the agent operates in text-only mode.

```toml
enable_tools = true
```

### `enable_compaction`

Enable automatic context compaction. When the context window is exceeded, older messages are summarized and removed to make room for new ones.

```toml
enable_compaction = true
```

### `max_concurrent_tool_calls`

Maximum number of independent tool calls to run at once. Results remain in the model's original call order even when calls finish in a different order. Values less than one are treated as one.

```toml
max_concurrent_tool_calls = 4
```

### `show_thinking`

Show model reasoning/thinking output in user-facing responses.

```toml
show_thinking = true
```

### `temperature`

Default sampling temperature for LLM calls (typically 0.0–2.0).

```toml
temperature = 0.7
```

### `max_tokens`

Default maximum output tokens for LLM calls.

```toml
max_tokens = 4096
```

### `provider_headers`

Additional HTTP headers for the default (legacy) provider.

```toml
[provider_headers]
x-custom-header = "value"
```

## Environment Variables

| Variable | Description |
| --- | --- |
| `HALLUCINATE_API_KEY` | API key fallback (overridden by `--api-key`). |
| `OPENAI_API_KEY` | Fallback when the default provider is `"openai"`. |
| `HALLUCINATE_<PROVIDER>_API_KEY` | Provider-specific key (e.g., `HALLUCINATE_OPENROUTER_API_KEY`). |
| `SHELL` | Shell executable for the bash tool (overridden by `shell` setting). |

Key resolution order: `--api-key` flag → `HALLUCINATE_API_KEY` → `OPENAI_API_KEY` (for default provider) → `HALLUCINATE_<PROVIDER>_API_KEY` → inline `api_key` in provider settings → stored credential.

## System Prompt

### Global Override

Place a file at `SYSTEM.md` in the configuration directory to replace the built-in system prompt entirely.

| Platform | Path |
| --- | --- |
| macOS | `~/Library/Application Support/com.hallucinate.hallucinate/SYSTEM.md` |
| Linux | `~/.config/hallucinate/SYSTEM.md` |
| Windows | `%APPDATA%\com\hallucinate\hallucinate\SYSTEM.md` |

### Project Instructions

Place an `AGENTS.md` file at the project root. Its contents are appended under a `## Project Instructions` heading to every system prompt.

```text
<cwd>/AGENTS.md
```

## Skills

Skills are Markdown files with YAML frontmatter. They are discovered from multiple locations — the first match wins for duplicate names.

### Discovery Locations

| Priority | Location | Description |
| --- | --- | --- |
| 1 | `~/.hallucinate/skills` | User config directory |
| 2 | `<cwd>/.hallucinate/skills` | Project-local skills |
| 3 | `~/.agents/skills` | Shared `.agents` protocol |
| 4 | `<cwd>/.agents/skills` (and ancestors to git root) | Shared `.agents` protocol per-project |

### Command-Line Override

```bash
hallucinate --skill path/to/skill.md --skill another-skill/
```

### Disable Auto-Discovery

```bash
hallucinate --no-skills
```

### Skill Frontmatter

```yaml
---
name: git-helper
description: Help with Git tasks like branching and merging
disable-model-invocation: false
---
```

| Field | Description |
| --- | --- |
| `name` | Machine-readable skill name (lowercase, numbers, hyphens). |
| `description` | Brief explanation of when to use the skill (required). |
| `disable-model-invocation` | When `true`, the skill is only invocable via `/skill:<name>`. |

## Prompt Templates

Prompt templates are Markdown files with YAML frontmatter that expand slash commands with argument substitution.

### Discovery Locations

| Priority | Location | Description |
| --- | --- | --- |
| 1 | `~/.hallucinate/prompts` | User config directory |
| 2 | `<cwd>/.hallucinate/prompts` | Project-local templates |

### Command-Line Override

```bash
hallucinate --prompt-template path/to/template.md
```

### Disable Auto-Discovery

```bash
hallucinate --no-prompt-templates
```

### Template Frontmatter

```yaml
---
description: Review a code change
argument-hint: <path>
---
```

| Field | Description |
| --- | --- |
| `description` | Short description shown in `/templates`. |
| `argument-hint` | Usage hint for the user (e.g., `<path>`). |

### Argument Substitution

Templates support shell-like argument syntax:

| Syntax | Description |
| --- | --- |
| `$1`, `$2`, `@N` | Positional argument (1-indexed). |
| `$@`, `@`, `$ARGUMENTS` | All arguments joined. |
| `${N:-default}` | Positional argument with fallback default. |
| `${@:N}` | Slice from index N to end. |
| `${@:N:M}` | Slice of M arguments starting at N. |

## CLI Flags

| Flag | Description |
| --- | --- |
| `--model <selector>` | Model to use (`provider/model` or bare ID). |
| `--api-key <key>` | API key override. |
| `--base-url <url>` | API endpoint override. |
| `--shell <path>` | Shell executable override. |
| `-p, --print <prompt>` | Print mode: send prompt, print response, exit. |
| `--rpc` | Run in RPC mode (JSON-RPC over stdin/stdout). |
| `--interactive` | Run in interactive TUI mode (default). |
| `--session <id>` | Resume a persisted session. |
| `--max-tokens <n>` | Response max tokens override. |
| `--thinking` | Show model thinking output. |
| `--no-thinking` | Hide model thinking output. |
| `--temperature <n>` | Sampling temperature override. |
| `--system-prompt <text>` | Custom system prompt. |
| `--image <path>` | Attach an image (repeatable). |
| `--skill <path>` | Load a skill file or directory (repeatable). |
| `--no-skills` | Disable automatic skill discovery. |
| `--prompt-template <path>` | Load a prompt template file or directory (repeatable). |
| `--no-prompt-templates` | Disable automatic template discovery. |
| `--debug` | Write diagnostic logs to the cache directory. |

## Auth Subcommand

Manage stored provider API keys.

```bash
hallucinate auth set <provider> [--api-key <key>]
hallucinate auth remove <provider>
hallucinate auth list
```

## Interactive Slash Commands

| Command | Usage | Description |
| --- | --- | --- |
| `/help` | — | Show available commands. |
| `/auth` | `<list\|set\|remove> [provider] [key]` | Manage API keys. |
| `/new` | `[title]` | Create a new session. |
| `/sessions` | — | List saved sessions. |
| `/resume` | `<id>` | Resume a session. |
| `/fork` | — | Fork the current session. |
| `/delete` | `[id]` | Delete a session. |
| `/name` | `[name]` | Set or clear the session name. |
| `/session` | — | Show current session info. |
| `/prompts` | — | List prompt templates. |
| `/skills` | — | List loaded skills. |
| `/model` | `[provider/model]` | Open model picker or switch. |
| `/thinking` | `[on\|off]` | Toggle thinking output. |
| `/skill:<name>` | `[arguments]` | Invoke a loaded skill. |
| `/compact` | — | Compact the current context. |
| `/stop` | — | Cancel the running response. |
| `/quit` | — | Exit the interactive session. |

## Complete Example

```toml
default_model = "openai/gpt-5.6-luna"
provider = "openai"
base_url = "https://api.openai.com/v1"
shell = "/bin/bash"
timeout_ms = 60000
max_retries = 3
auto_resize_images = true
max_turns = 120
enable_tools = true
enable_compaction = true
show_thinking = false
temperature = 0.7
max_tokens = 32_768

[provider_headers]
x-request-source = "hallucinate"

[credential_store.credentials.openai]
provider_id = "openai"
api_key = "sk-..."
stored_at = 1725120000

[providers.openai]
base_url = "https://api.openai.com/v1"

[providers.openai.models."gpt-5.6-luna"]
name = "GPT-5.6 Luna"
api = "openai-responses"
reasoning = true
input = ["text", "image"]
context_window = 272_000
max_tokens = 128_000

[providers.openai.models."gpt-5.6-luna".cost]
input = 0.2
output = 1.2
cache_read = 0.02
cache_write = 0.25

[providers.openai.models.gpt-4o]
name = "GPT-4o"
api = "openai-completions"
reasoning = true
input = ["text", "image"]
context_window = 128_000
max_tokens = 16_384

[providers.openai.models.gpt-4o.cost]
input = 2.50
output = 10.00

[providers.local]
base_url = "http://localhost:1234/v1"

[providers.local.models.llama-3]
name = "Llama 3"
api = "openai-completions"
reasoning = false
input = ["text"]
context_window = 32_768
max_tokens = 4_096
```
