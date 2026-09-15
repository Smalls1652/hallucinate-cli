# Script Tools

User-defined tools backed by shell commands. Define a TOML file to expose any executable as an LLM-invocable tool with typed parameters and template substitution.

## Overview

Script tools let you create tools for custom scripts, CLI binaries, or workflows in your project. Each tool is defined by a single TOML file placed in a discovery directory. The agent receives the tool's name, description, and JSON Schema parameters — it then executes the command with the provided arguments and returns stdout (on success) or stderr (on failure).

## Discovery Locations

Script tools are discovered from two locations. Project-local definitions override user-level definitions with the same tool ID.

| Priority | Location | Description |
| --- | --- | --- |
| 1 | `~/.config/hallucinate/script_tools` (Linux) / `~/Library/Application Support/com.hallucinate.hallucinate/script_tools` (macOS) | User-level tools |
| 2 | `<cwd>/.hallucinate/script_tools` | Project-local tools |

| Platform | User-level Path |
| --- | --- |
| macOS | `~/Library/Application Support/com.hallucinate.hallucinate/script_tools/` |
| Linux | `~/.config/hallucinate/script_tools/` |
| Windows | `%APPDATA%\com\hallucinate\hallucinate\script_tools\` |

## Definition Format

Each tool is defined in a TOML file named `<tool-id>.toml`. The file stem becomes the tool ID (letters, numbers, underscores, or hyphens). The TOML structure uses the following keys:

```toml
command = ["echo", "Hello {{name}}"]
description = "Print a greeting to a named person"

[parameters.name]
description = "The person's name"
required = true
input_type = "string"
```

### Required Fields

| Field | Type | Description |
| --- | --- | --- |
| `command` | `array` | Executable and arguments. The first element is the path to the script/binary; remaining elements are arguments. Supports `{{parameter}}` template substitution. |
| `description` | `string` | Human-readable description of what the tool does (becomes the tool description sent to the LLM). |

### Optional Fields

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `shell` | `array` | `$SHELL` (Unix) / `%COMSPEC%` (Windows) | Shell executable and arguments used to run the command. Overrides the global `shell` setting for this tool only. |
| `timeout` | `integer` | Configured `shell_timeout` (60 seconds by default) | Maximum execution time in seconds. Must be greater than zero. |

### Parameters

Parameters are defined under `[parameters.<name>]` tables. Each parameter becomes a property in the tool's JSON Schema.

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `description` | `string` | — | Brief explanation of the parameter (required). |
| `input_type` | `string` | `"string"` | JSON Schema type for the parameter. |
| `required` | `bool` | `false` | Whether the parameter must be provided. |
| `constraints` | `table` | — | Optional value constraints. Supports `enum`, `minimum`, and `maximum`. |

### Parameter Constraints

Add a `constraints` table to limit values accepted by the tool. Constraints are included in the JSON Schema sent to the LLM and are enforced again before the command executes.

| Field | Type | Applies to | Description |
| --- | --- | --- | --- |
| `enum` | `array` | All input types | Allowed values. Every value must match the parameter's `input_type`. |
| `minimum` | `number` | `"int"`, `"integer"`, `"float"`, `"number"` | Inclusive numeric lower bound. |
| `maximum` | `number` | `"int"`, `"integer"`, `"float"`, `"number"` | Inclusive numeric upper bound. |

`enum` cannot be empty, and `minimum` cannot be greater than `maximum`.

```toml
[parameters.format]
description = "Output format"
input_type = "string"
required = true

[parameters.format.constraints]
enum = ["json", "csv", "text"]

[parameters.quality]
description = "JPEG quality"
input_type = "integer"
required = true

[parameters.quality.constraints]
minimum = 1
maximum = 100
```


| Value | JSON Schema | Example |
| --- | --- | --- |
| `"string"` | `string` | `"hello"` |
| `"int"`, `"integer"` | `integer` | `42` |
| `"float"`, `"number"` | `number` | `3.14` |
| `"bool"`, `"boolean"` | `boolean` | `true` |
| `"array"` | `array` | `["a", "b"]` |
| `"object"` | `object` | `{"key": "value"}` |

## Template Substitution

Command arguments support `{{parameter_name}}` template substitution. Parameters are replaced with their string representation (or the raw value for strings). All templates must resolve — unresolved `{{` delimiters cause execution to fail.

```toml
command = ["git", "log", "--since={{since}}", "--until={{until}}"]

[parameters.since]
description = "Start date (e.g., 2024-01-01)"
required = true
input_type = "string"

[parameters.until]
description = "End date (e.g., 2025-01-01)"
input_type = "string"
```

## Execution Behavior

- **Working directory**: The command runs in the agent's current working directory. The agent may override the CWD per invocation.
- **Timeout**: Commands are terminated after the timeout specified in the tool definition. Definitions without `timeout` use the configured shell timeout (default: 60 seconds). Timed-out commands return an error.
- **Shell quoting**: All arguments are single-quoted with inner quotes escaped (`'\''`), preventing injection even when parameter values contain special characters.
- **Output**: On success, stdout is returned. On failure, stderr is returned. Output is truncated to the configured tool result limit.
- **Exit code**: The tool reports success or failure based on the process exit code.

## Examples

### Simple Utility

```toml
command = ["echo", "{{message}}"]
description = "Echo a message back"

[parameters.message]
description = "The message to echo"
required = true
```

### Git Log with Date Range

```toml
command = ["git", "log", "--since={{since}}", "--until={{until}}", "--pretty=format:%h %s"]
description = "Show git commits within a date range"

[parameters.since]
description = "Start date in YYYY-MM-DD format"
required = true

[parameters.until]
description = "End date in YYYY-MM-DD format"
required = true
```

### Script with Custom Shell

```toml
command = ["python3", "{{script}}", "--output={{output}}"]
description = "Run a Python data processing script"
shell = ["/usr/bin/python3"]
timeout = 300

[parameters.script]
description = "Path to the Python script"
required = true

[parameters.output]
description = "Output format (json, csv, text)"
required = true
```

### Integer and Boolean Parameters

```toml
command = ["convert", "{{input}}", "-resize", "{{width}}x{{height}}", "-quality", "{{quality}}", "{{output}}"]
description = "Resize and compress an image"

[parameters.input]
description = "Input file path"
required = true

[parameters.output]
description = "Output file path"
required = true

[parameters.width]
description = "Target width in pixels"
required = true
input_type = "integer"

[parameters.height]
description = "Target height in pixels"
required = true
input_type = "integer"

[parameters.quality]
description = "JPEG quality (1-100)"
required = true
input_type = "integer"

[parameters.quality.constraints]
minimum = 1
maximum = 100
```

### Array Parameter

```toml
command = ["echo", "{{items}}"]
description = "Join and print a list of items"

[parameters.items]
description = "List of items to join"
required = true
input_type = "array"
```

## Loading Order

User-level tools are loaded first, then project-local tools. When a project-local tool has the same ID as a user-level tool, the project-local definition **replaces** the user-level one. This allows projects to override or extend shared tools with project-specific behavior.

## Diagnostics

Invalid definitions are silently skipped and logged as diagnostics. Common reasons for rejection:

- Tool ID contains invalid characters (only letters, numbers, underscores, and hyphens allowed)
- `command` is empty or has no executable
- `description` is empty
- `shell` is configured but has no executable
- `timeout` is zero
- Parameter name contains invalid characters (only letters, numbers, and underscores allowed)
- Parameter `description` is empty
- Parameter `input_type` is not a supported type
- Parameter constraints are incompatible with the input type, empty, or have an invalid range
