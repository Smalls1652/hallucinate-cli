# Configuration Reference

Complete reference for all configuration options in `hallucinate`.

## Overview

`hallucinate` reads configuration from a **TOML** settings file and a set of environment variables. Provider credentials are stored in the operating system's secure credential store (keyring), not in the settings file.

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
| `session_title.enabled` | `bool` | `true` | Generate a title from a session's first user message. |
| `session_title.model` | `string` | Active model | Model selector used to generate session titles. |
| `session_title.generation_mode` | `string` | `"parallel"` | Generate alongside (`parallel`) or before (`before`) the first response. |
| `providers` | `table` | — | Multi-provider model definitions. |
| `mcp_servers` | `table` | — | Local stdio or remote HTTP MCP servers whose discovered tools are added to the agent. |
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
include_usage = true

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
| `api_key` | `string` | — | Inline API key (prefer the platform keyring or environment variables). |
| `headers` | `table` | — | Additional HTTP headers sent with every request. |
| `include_usage` | `bool` | — | Include `{ "stream_options": { "include_usage": value } }` in streaming requests. |
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

## Global Settings

### `default_model`

Default model selector. Can be a bare model ID or `provider/model`.

```toml
default_model = "openai/gpt-4o"
```

### `provider`

Credential namespace for the default provider. Used to resolve keys from the platform keyring when no provider is explicitly specified. Defaults to `"openai"`.

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

### `session_title`

Controls automatic titles for interactive sessions. A title is generated only
for the first user message in a session; existing titles and later messages
are not changed. Failures leave the current title unchanged and are shown as a
non-blocking notice.

```toml
[session_title]
enabled = true
model = "openai/gpt-4o-mini"
generation_mode = "parallel" # or "before"
```

`model` defaults to the model selected for the conversation. With
`generation_mode = "parallel"` (the default), title generation and the first
agent response run concurrently. `"before"` saves the generated title before
starting the first agent response.

### `mcp_servers`

MCP servers may be either local stdio processes (`command`) or remote streamable HTTP endpoints (`url`). Exactly one transport field is required. Hallucinate starts or connects to each configured server when an interactive or RPC agent is created, initializes it, and adds the tools returned by `tools/list`. By default, tools retain their remote names after non-alphanumeric characters are converted to `_`. If an MCP tool name collides with a built-in tool or a tool from another MCP server, startup fails; set `tool_prefix` to disambiguate the server's tools.

```toml
# Local stdio server
[mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/workspace"]
env = { LOG_LEVEL = "warn" }
tool_prefix = "fs"

# Containerized local stdio server. `-i` preserves JSON-RPC stdin and
# `--rm` removes the container when Hallucinate stops the runtime command.
[mcp_servers.filesystem_container]
command = "docker"
args = [
  "run", "-i", "--rm",
  "--mount", "type=bind,src=/path/to/workspace,dst=/workspace",
  "mcp/filesystem", "/workspace",
]
tool_prefix = "container_fs"

# Remote streamable HTTP server with no authentication
[mcp_servers.example]
url = "https://mcp.example.com/mcp"
auth = { type = "no-auth" }
tool_prefix = "example"

# Remote server using an Authorization: Bearer API key
[mcp_servers.protected]
url = "https://mcp.protected.example/mcp"
auth = { type = "api-key", api_key = "replace-me" }
tool_prefix = "protected"

# API keys can also use a vendor-specific header and no prefix.
[mcp_servers.vendor]
url = "https://mcp.vendor.example/mcp"
auth = { type = "api-key", api_key = "replace-me", header = "X-API-Key", prefix = "" }
```

| Field | Type | Description |
| --- | --- | --- |
| `command` | `string` | Executable used to start a local server over stdio, including a container runtime such as `docker`, `podman`, or `nerdctl`. Mutually exclusive with `url`. |
| `args` | `array` | Command-line arguments passed to a local server. |
| `env` | `table` | Environment variables added to a local server process. |
| `url` | `string` | `http` or `https` streamable HTTP endpoint for a remote server. Mutually exclusive with `command`. |
| `tool_prefix` | `string` | Optional prefix prepended to each tool name. Set this when unprefixed names would collide. |
| `auth.type` | `"no-auth"` or `"api-key"` | Remote authentication mode. Defaults to `"no-auth"`. |
| `auth.api_key` | `string` | API key sent when `auth.type = "api-key"`. |
| `auth.header` | `string` | API-key HTTP header; defaults to `Authorization`. |
| `auth.prefix` | `string` | Text prepended to the key; defaults to `Bearer`. |

MCP server processes inherit the application's current environment except for values explicitly overridden by `env`. A local command is executed directly rather than through a shell, so put every container-runtime option in `args`; shell quoting, pipes, and variable expansion are not applied. For a containerized stdio server, use the runtime's stdin/attach option (for Docker-compatible runtimes, `run -i`) and avoid allocating a TTY (`-t`), which can alter the JSON-RPC byte stream. Use the runtime's cleanup option where available (for Docker-compatible runtimes, `--rm`).

The `env` table sets variables for the local command process. Container runtimes do not generally forward them to the container automatically; pass them explicitly through runtime arguments such as `-e NAME` or `--env-file` (and arrange any required host environment value). Remote servers support no authentication (the default) or API-key authentication. API-key authentication sends `Authorization: Bearer <key>` by default; configure `auth.header` and `auth.prefix` for services using a different header format. API keys are currently stored in the settings file, so protect its file permissions.

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

Key resolution order: `--api-key` flag → `HALLUCINATE_API_KEY` → `OPENAI_API_KEY` (for default provider) → `HALLUCINATE_<PROVIDER>_API_KEY` → inline `api_key` in provider settings → stored credential in the platform keyring.

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

The CLI uses a **subcommand-based** architecture. Global options apply to all subcommands; some subcommands accept additional options.

### Global Options

| Flag | Description |
| --- | --- |
| `-m, --model <selector>` | Model to use (`provider/model` or bare ID). |
| `--api-key <key>` | API key override. |
| `--base-url <url>` | API endpoint override. |
| `--shell <path>` | Shell executable override. |
| `--session <id>` | Resume a persisted session in interactive mode. |
| `--max-tokens <n>` | Response max tokens override. |
| `--thinking` | Show model thinking output. |
| `--temperature <n>` | Sampling temperature override. |
| `--system-prompt <text>` | Custom system prompt. |
| `--image <path>` | Attach an image (repeatable). |
| `--skill <path>` | Load a skill file or directory (repeatable). |
| `--no-skills` | Disable automatic skill discovery. |
| `--prompt-template <path>` | Load a prompt template file or directory (repeatable). |
| `--no-prompt-templates` | Disable automatic template discovery. |
| `--debug` | Write diagnostic logs to the cache directory. |

### Subcommands

| Subcommand | Description |
| --- | --- |
| `hallucinate` | Run in interactive TUI mode (default, no subcommand required). |
| `hallucinate print <prompt>` | Print mode: send prompt, print response, exit. |
| `hallucinate rpc` | Run in RPC mode (JSON-RPC over stdin/stdout). |
| `hallucinate auth` | Manage stored provider API keys (`set`, `remove`, `list`). |

## Auth Subcommand

Manage stored provider API keys in the platform keyring.

```bash
hallucinate auth set <provider> [--api-key <key>]
hallucinate auth remove <provider>
hallucinate auth list
```

`auth set` accepts an optional `--api-key` flag; when omitted, the command falls back to the `HALLUCINATE_<PROVIDER>_API_KEY` environment variable. If no environment variable is set, the command prompts for a masked key. Keys are stored in the platform-native keyring (macOS keychain, Linux keyutils, Windows keyring) and are never written to the settings file.

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

[mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]

show_thinking = false
temperature = 0.7
max_tokens = 32_768

[session_title]
enabled = true
model = "openai/gpt-4o"
generation_mode = "parallel"

[provider_headers]
x-request-source = "hallucinate"

[providers.openai]
base_url = "https://api.openai.com/v1"
include_usage = true

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
