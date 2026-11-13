# Configuration Reference

Complete reference for all configuration options for `hallucinate`.

## Overview

`hallucinate` reads configuration from a **TOML** settings file and a set of environment variables. Provider credentials are stored in the operating system's secure credential store (keyring).

> [!NOTE]
> Provider credentials *can* be added to the settings file; however, it is **strongly advised** to only use it as a last resort. There are some valid use-cases for setting them in the settings file, but it comes with the caveat that they are stored in plain text.

| Setting | Type | Default | Description |
| --- | --- | --- | --- |
| `base_url` | `string \| null` | — | Default base URL for the provider. |
| `default_model` | `string \| null` | — | Default model to use. |
| `enable_compaction` | `boolean` | `true` | Enable context compaction. |
| `max_concurrent_tool_calls` | `integer` | `4` | Maximum number of independent tool calls to execute at once. |
| `max_retries` | `integer` | `3` | Number of retries for transient LLM API failures. |
| `max_tokens` | `integer \| null` | — | Default max tokens. |
| `max_turns` | `integer` | `120` | Maximum number of turns. |
| `provider` | `string` | `"openai"` | Credential namespace for the OpenAI-compatible provider (usually `"openai"`). |
| `provider_headers` | `object` | — | Provider-specific headers for the default provider. |
| `providers` | `object` | — | Providers and their models. |
| `session_title` | `object` | — | Settings for generating a session title. |
| `show_thinking` | `boolean` | `false` | Show model thinking/reasoning in user-facing output. |
| `skills` | `object` | — | Settings for skills. |
| `temperature` | `number \| null` | — | Default temperature. |
| `timeout_ms` | `integer` | `600000` | Request timeout in milliseconds. |
| `tools` | `object` | — | Settings for tools. |

## Settings File Location

| Platform | Path |
| --- | --- |
| macOS | `~/Library/Application Support/online.smalls.hallucinate/settings.toml` |
| Linux | `~/.config/hallucinate/settings.toml` |
| Windows | `%APPDATA%\com\hallucinate\hallucinate\settings.toml` |

A **project-local override** may be placed at `.hallucinate/settings.toml` in the working directory. It is merged on top of the global file — nested tables are merged recursively, while scalar values and arrays are replaced entirely.

## Global Settings

### `base_url`

Default base URL for the provider.

```toml
base_url = "https://api.openai.com/v1"
```

### `default_model`

Default model to use. Can be a bare model ID or `provider/model`.

```toml
default_model = "openai/gpt-6-sol"
```

### `provider`

Credential namespace for the default provider. Used to resolve keys from the platform keyring when no provider is explicitly specified. Defaults to `"openai"`.

```toml
provider = "openai"
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

### `max_turns`

Maximum number of consecutive agent turns in a row. The agent stops with an error when this limit is reached.

```toml
max_turns = 120
```

### `enable_compaction`

Enable automatic context compaction. When the context window is exceeded, older messages are summarized and removed to make room for new ones.

```toml
enable_compaction = true
```

### `max_concurrent_tool_calls`

Maximum number of independent tool calls to execute at once. Values less than one are treated as one.

```toml
max_concurrent_tool_calls = 4
```

### `show_thinking`

Show model thinking/reasoning output in user-facing responses.

```toml
show_thinking = false
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

Additional HTTP headers for the default provider.

```toml
[provider_headers]
x-custom-header = "value"
```

## Provider Configuration

Providers are defined under the `providers` section. Each provider groups a base URL, optional inline API key, custom headers, and a set of models.

```toml
[providers.openai]
base_url = "https://api.openai.com/v1"
include_usage = true
headers = { "X-Custom-Header" = "Value" }

[providers.openai.models.gpt-6-luna]
name = "GPT-6 Luna"
api = "openai-responses"
reasoning = true
supports_reasoning_effort = true
input = ["text", "image"]
context_window = 128_000
max_tokens = 16_384

[providers.openai.models.gpt-6-luna.cost]
input = 0.10
output = 0.50
cache_read = 0.01
cache_write = 0.125

[providers.openai.models.gpt-6-sol]
name = "GPT-6 Sol"
api = "openai-responses"
reasoning = true
supports_reasoning_effort = true
input = [
  "text",
  "image",
]
context_window = 1_050_000
max_tokens = 32_000

[providers.openai.models.gpt-6-sol.cost]
input = 2
output = 10
cache_read = 0.20
cache_write = 2.5
```

Cost configuration is optional. Omit the entire `cost` table for models whose pricing is unavailable (For example, when using local or remote self-hosted model); their usage cost is reported as zero.

| Field | Type | Description |
| --- | --- | --- |
| `base_url` | `string \| null` | OpenAI-compatible endpoint for this provider. |
| `api_key` | `string \| null` | Inline API key (prefer the platform keyring or environment variables). |
| `headers` | `object` | Additional HTTP headers sent with every request. |
| `include_usage` | `boolean \| null` | Whether streaming requests include usage data in `stream_options`. |
| `models` | `object` | Models provided by this provider, keyed by model ID. |

### Model-Level Fields

| Field | Type | Description |
| --- | --- | --- |
| `api` | `string` | API variant: `openai-completions` or `openai-responses`. |
| `base_url` | `string \| null` | Per-model endpoint override. |
| `context_window` | `integer` | Maximum context window size in tokens. |
| `cost` | `object \| null` | Optional cost rates per million tokens. |
| `hidden` | `boolean` | Hide the model from the interactive model picker. |
| `input` | `array` | Supported input modalities (e.g., `["text"]`). |
| `max_tokens` | `integer` | Maximum output tokens the model can produce. |
| `name` | `string \| null` | Human-readable model name. |
| `reasoning` | `boolean` | Whether the model supports reasoning/thinking output. |
| `supports_reasoning_effort` | `boolean` | Whether the model accepts the Responses API `reasoning.effort` option. |

#### Cost Fields

| Field | Type | Description |
| --- | --- | --- |
| `input` | `number \| null` | Optional cost rate per million input tokens. |
| `output` | `number \| null` | Optional cost rate per million output tokens. |
| `cache_read` | `number \| null` | Optional cost rate per million cache read tokens. |
| `cache_write` | `number \| null` | Optional cost rate per million cache write tokens. |

## Tools Configuration

Settings for managing tools, including built-in tools and MCP servers.

```toml
[tools]
enable = true
disable_tools = ["edit"]

[tools.builtin.shell]
timeout = 69
max_tool_result_bytes = 100_000_000
shell = ["/bin/bash", "-c"]

[tools.builtin.find]
max_results = 100
max_files = 10_000
max_directory_entries = 20_000

[tools.builtin.grep]
max_results = 25

[tools.builtin.ls]
max_results = 42
max_files = 1_337
max_directory_entries = 69_420

[tools.mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/workspace"]
env = { LOG_LEVEL = "warn" }
tool_prefix = "fs"

[tools.mcp_servers.remote_server]
url = "https://mcp.example.com/mcp"
auth = { type = "api-key", api_key = "replace-me", header = "X-API-Key", prefix = "" }
```

### Built-in Tools

Built-in tools can be enabled or disabled. If `enable_tools` is set, it overrides `disable_tools`.

#### `shell` Fields

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `timeout` | `integer` | `60` | Maximum execution timeout in seconds. |
| `shell` | `array \| null` | — | Executable and arguments (e.g., `["/bin/bash", "-c"]`). |
| `max_tool_result_bytes` | `integer` | `100000` | Max bytes in a returned tool result. |

#### `edit` Fields

> [!NOTE]
> No fields for this tool.

#### `find` Fields

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `max_directory_entries` | `integer` | `20000` | Max directory entries to walk. |
| `max_files` | `integer` | `10000` | Max files to consider. |
| `max_results` | `integer` | `100` | Max results to return. |

#### `grep` Fields

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `max_directory_entries` | `integer` | `20000` | Max directory entries to walk. |
| `max_file_scan_bytes` | `integer` | `1000000` | Max bytes scanned per candidate. |
| `max_files` | `integer` | `10000` | Max files to consider. |
| `max_line_bytes` | `integer` | `65536` | Max accepted line length. |
| `max_results` | `integer` | `100` | Max results to return. |
| `max_search_bytes` | `integer` | `20971520` | Max bytes scanned per request. |
| `max_tool_result_bytes` | `integer` | `100000` | Max bytes in a returned tool result. |

#### `ls` Fields

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `max_directory_entries` | `integer` | `20000` | Max directory entries to walk. |
| `max_files` | `integer` | `10000` | Max files to consider. |
| `max_results` | `integer` | `100` | Max results to return. |

#### `read` Fields

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `auto_resize_images` | `boolean` | `true` | Auto-resize images when reading. |
| `max_image_source_bytes` | `integer` | `20971520` | Max image source bytes. |
| `max_text_file_bytes` | `integer` | `1000000` | Max bytes for text files. |

#### `write` Fields

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `max_text_file_bytes` | `integer` | `1000000` | Max bytes written to text files. |
| `max_tool_result_bytes` | `integer` | `100000` | Max bytes in a returned tool result. |

### MCP Servers

MCP servers can be local stdio processes (`command`) or remote HTTP endpoints (`url`). Exactly one of `command` or `url` is required.

| Field | Type | Description |
| --- | --- | --- |
| `command` | `string \| null` | Executable for a local stdio server. Mutually exclusive with `url`. |
| `args` | `array` | Arguments passed to `command`. |
| `env` | `object` | Environment variables for the local server. |
| `url` | `string \| null` | HTTP endpoint for a remote server. Mutually exclusive with `command`. |
| `tool_prefix` | `string \| null` | Optional prefix for discovered tools. |
| `auth` | `object` | Authentication for a remote MCP server. |

**Auth Modes:**

- `type = "no-auth"`: No authentication.
- `type = "api-key"`: Sends an API key. Requires `api_key`.
  - **Optional properties:**
    - `header` (default `Authorization`)
    - `prefix` (default `Bearer`)

### Session Title Configuration

Controls automatic titles for interactive sessions.

```toml
[session_title]
enabled = true
model = "openai/gpt-6-luna"
generation_mode = "parallel"
```

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `enabled` | `boolean` | `true` | Whether new sessions receive an LLM-generated title. |
| `generation_mode` | `string` | `"parallel"` | `parallel` (runs alongside first response) or `before` (before first response). |
| `model` | `string \| null` | — | Model used for title generation. If omitted, the active model is used. |

## Skills Configuration

Restrict which skills are loaded by specifying a list of allowed skill IDs. When the list is non-empty, only skills whose IDs appear in the list are loaded; all others are silently skipped.

```toml
[skills]
available_skills = ["git-helper", "code-review"]
```

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `available_skills` | `array` | `[]` | List of skill IDs to enable. When empty, all discovered skills are loaded. |

## Environment Variables

| Variable | Description |
| --- | --- |
| `HALLUCINATE_API_KEY` | API key fallback (overridden by `--api-key`). |
| `OPENAI_API_KEY` | Fallback when the default provider is `"openai"`. |
| `HALLUCINATE_<PROVIDER>_API_KEY` | Provider-specific key (e.g., `HALLUCINATE_OPENROUTER_API_KEY`). |
| `SHELL` | Shell executable for the shell tool (overridden by `tools.builtin.shell.shell`). |

Key resolution order: `--api-key` flag $\rightarrow$ `HALLUCINATE_API_KEY` $\rightarrow$ `OPENAI_API_KEY` (for default provider) $\rightarrow$ `HALLUCINATE_<PROVIDER>_API_KEY` $\rightarrow$ inline `api_key` in provider settings $\rightarrow$ stored credential in the platform keyring.

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

## Skills

Skills are Markdown files with YAML frontmatter. They are discovered from multiple locations.

### Skills Discovery Locations

| Priority | Location | Description |
| --- | --- |
| 1 | `~/.hallucinate/skills` | User config directory |
| 2 | `<cwd>/.hallucinate/skills` | Project-local skills |
| 3 | `~/.agents/skills` | Shared `.agents` protocol |
| 4 | `<cwd>/.agents/skills` | Shared `.agents` protocol per-project |

### CLI Skills Override

```bash
hallucinate --skill path/to/skill.md --skill another-skill/
```

### Disable Skills Auto-Discovery

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
| `---` | — |

## Prompt Templates

Prompt templates are Markdown files with YAML frontmatter that expand slash commands with argument substitution.

### Prompt Templates Discovery Locations

| Priority | Location | Description |
| --- | --- |
| 1 | `~/.hallucinate/prompts` | User config directory |
| 2 | `<cwd>/.hallucinate/prompts` | Project-local templates |

### CLI Prompt Templates Override

```bash
hallucinate --prompt-template path/to/template.md
```

### Disable Prompt Templates Auto-Discovery

```bash
hallucinate --no-prompt-templates
```

### Prompt Template Frontmatter

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

## Complete Example

```toml
default_model = "openai/gpt-5.6-luna"
provider = "openai"
base_url = "https://api.openai.com/v1"
timeout_ms = 60_000
max_retries = 3
max_turns = 120
enable_compaction = true
max_concurrent_tool_calls = 4
show_thinking = false
temperature = 0.7
max_tokens = 32_768

[skills]
available_skills = []

[session_title]
enabled = true
model = "openai/gpt-6-luna"
generation_mode = "parallel"

[providers.openai.models.gpt-6-luna]
name = "GPT-6 Luna"
api = "openai-responses"
reasoning = true
supports_reasoning_effort = true
input = ["text", "image"]
context_window = 128_000
max_tokens = 16_384

[providers.openai.models.gpt-6-luna.cost]
input = 0.10
output = 0.50
cache_read = 0.01
cache_write = 0.125

[providers.openai.models.gpt-6-sol]
name = "GPT-6 Sol"
api = "openai-responses"
reasoning = true
supports_reasoning_effort = true
input = [
  "text",
  "image",
]
context_window = 1_050_000
max_tokens = 32_000

[providers.openai.models.gpt-6-sol.cost]
input = 2
output = 10
cache_read = 0.20
cache_write = 2.5

[providers.local]
base_url = "http://localhost:1234/v1"

[providers.local.models.gemma4]
name = "Gemma 4"
api = "openai-completions"
reasoning = true
supports_reasoning_effort = false
input = ["text", "image"]
context_window = 128_000
max_tokens = 16_000
```
