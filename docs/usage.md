# CLI Usage Guide

Practical guide to using the `hallucinate` CLI across all run modes.

## Overview

`hallucinate` uses a **subcommand-based** architecture. Running the binary without a subcommand enters interactive TUI mode by default.

```bash
hallucinate              # Interactive TUI (default)
hallucinate print <msg>  # Print mode — one-shot prompt
hallucinate rpc          # RPC mode — JSON-RPC over stdin/stdout
hallucinate auth         # Auth management subcommand
```

Global options are available on every invocation and subcommand.

---

## Global Options

These flags work with any subcommand (including the default interactive mode).

| Flag | Description |
| --- | --- |
| `-m, --model <MODEL>` | Model selector — a configured model ID or `provider/model` pair. |
| `--api-key <KEY>` | API key override. Falls back to `HALLUCINATE_API_KEY` or `OPENAI_API_KEY`. |
| `--base-url <URL>` | Base URL for the API (OpenAI-compatible providers). |
| `--shell <PATH>` | Shell executable for the bash tool. Overrides the settings file and `$SHELL`. |
| `--session <ID>` | Resume a persisted session (interactive mode). |
| `--max-tokens <N>` | Maximum number of tokens in the response. |
| `--thinking` | Show model thinking/reasoning output. Overrides the settings file. |
| `--temperature <N>` | Sampling temperature (0.0 – 2.0). |
| `--system-prompt <TEXT>` | Custom system prompt to use. |
| `--image <PATH>` | Attach an image (repeatable). |
| `--skill <PATH>` | Load a skill Markdown file or directory (repeatable). |
| `--no-skills` | Disable automatic skill discovery. |
| `--prompt-template <PATH>` | Load a prompt template file or directory (repeatable). |
| `--no-prompt-templates` | Disable automatic template discovery. |
| `--debug` | Write diagnostic logs to the application cache directory. |

---

## Interactive Mode

Running `hallucinate` without a subcommand launches the full terminal UI with:

- Syntax-highlighted markdown rendering
- Multi-line editor with word-level cursor navigation, undo/redo, and Shift+Enter for newlines
- Keyboard-driven session picker and model picker overlays
- Slash-command autocomplete (accept with Enter)
- Session management with fork support

### Starting a Session

```bash
hallucinate
```

The agent loads your configured `default_model` and any project-local instructions from `AGENTS.md`.

### Resuming a Session

```bash
hallucinate --session <session-id>
```

Or use the `/resume` slash command inside the TUI (without arguments it lists saved sessions).

### Slash Commands

All commands are entered in the editor input prefixed with `/`. The first whitespace-delimited token after `/` is the command name; everything after is the argument.

| Command | Usage | Description |
| --- | --- | --- |
| `/help` | — | Show this list of commands. |
| `/auth` | `<list\|set\|remove> [provider] [key]` | Manage API keys. |
| `/new` | `[title]` | Create and switch to a new session. |
| `/resume` | `[id]` | List saved sessions or resume one by ID. |
| `/fork` | — | Fork the current session. |
| `/delete` | `[id]` | Delete a saved session. |
| `/name` | `[name]` | Set or clear the current session name. |
| `/session` | — | Show current session information. |
| `/prompts` | — | List prompt templates. |
| `/skills` | — | List loaded skills. |
| `/model` | `[provider/model]` | Open the model picker or switch directly. |
| `/thinking` | `[on\|off]` | Toggle model thinking/reasoning output. |
| `/skill:<name>` | `[arguments]` | Invoke a loaded skill. |
| `/compact` | — | Compact the current context. |
| `/reload` | — | Reload prompt templates and system prompt files. |
| `/stop` | — | Cancel the running response. |
| `/quit` | — | Exit the interactive session. |

### Keyboard Shortcuts

#### Editor Input

| Shortcut | Action |
| --- | --- |
| **Enter** | Send the message. |
| **Shift+Enter / Alt+Enter** | Insert a newline in the editor (send with Enter). |
| **Ctrl+U** | Undo. |
| **Ctrl+Left / Ctrl+Right** | Move cursor by word. |
| **Esc** | Cancel the editor input (clears it). |

#### Slash-Command Autocomplete

When you type `/` the editor shows matching commands, templates, and skills. While the completion popup is active:

| Shortcut | Action |
| --- | --- |
| **Tab / Enter** | Accept the selected completion. |
| **Up / Down** | Navigate the completion list. |

#### Transcript Navigation

| Shortcut | Action |
| --- | --- |
| **Up arrow** | Scroll transcript up (when editor is not multiline). |
| **Down arrow** | Scroll transcript down (when editor is not multiline). |
| **Ctrl+End** | Jump to the bottom of the transcript and follow new messages. |
| **Mouse wheel / trackpad** | Scroll the transcript. |
| **Click + drag** | Select transcript text. |
| **Ctrl+C** (when text is selected) | Copy the selected transcript text to the host clipboard. |

> On macOS, **Cmd+C** copies selected transcript text instead of interrupting the response.

#### Global Shortcuts

| Shortcut | Action |
| --- | --- |
| **Alt+M** | Open the model picker overlay. |
| **Alt+R** | Open the session picker overlay. |
| **Alt+P** | Open the prompt template picker overlay. |
| **Alt+T** | Toggle model thinking/reasoning output. |
| **Alt+O** | Toggle full / compact tool output display. |
| **Shift+Alt+N** | Create a new empty session. |
| **Alt+K** | Navigate to the previous message in the editor history. |
| **Alt+J** | Navigate to the next message in the editor history. |
| **Ctrl+C** | Cancel the running agent response (does not quit). |
| **Esc** (while response is running) | Cancel the running response, keeping editor contents intact. |

#### Picker Overlays

While any picker (model, session, or prompt) is open, you can start typing to filter items in the picker. To navigate the picker, use these keys:

| Shortcut | Action |
| --- | --- |
| **Up / Down** | Navigate the picker list. |
| **Enter** | Select the highlighted item and close the picker. |
| **Esc** | Close the picker without selecting. |

---

## Print Mode

Print mode sends a prompt and prints the full response to stdout, then exits. Ideal for scripting and CI pipelines.

```bash
hallucinate print "What files are in the current directory?"
```

### Additional Print Options

```bash
hallucinate print -m openai/gpt-4o "Summarize the key points"
hallucinate print --max-tokens 1024 --temperature 0.3 "Write a commit message"
hallucinate print --image screenshot.png "What do you see in this image?"
hallucinate print --skill ./skills/git/SKILL.md "Review this diff"
```

The `--skill` and `--no-skills` flags work in print mode to load skills for the duration of the prompt.

---

## RPC Mode

RPC mode exposes a **JSON-RPC 2.0** interface over newline-delimited JSON on stdin/stdout. Each line is a complete JSON-RPC request; responses are written as a single line per request.

### Starting RPC Mode

```bash
hallucinate rpc
```

### Supported Methods

| Method | Description |
| --- | --- |
| `ping` | Health check — returns `{"pong": true}`. |
| `list_sessions` | List all persisted sessions. |
| `create_session` | Create a new session. |
| `chat` | Send a message and receive the full response. |

### Request Format

Every request follows the JSON-RPC 2.0 structure:

```json
{
  "jsonrpc": "2.0",
  "method": "<method-name>",
  "params": { ... },
  "id": <string | number | null>
}
```

The `id` field is echoed back in the response and may be any string, number, or null.

### Method Details

#### `ping`

```json
{"jsonrpc": "2.0", "method": "ping", "id": 1}
```

Response:

```json
{"jsonrpc": "2.0", "result": {"pong": true}, "id": 1}
```

#### `list_sessions`

```json
{"jsonrpc": "2.0", "method": "list_sessions", "id": 2}
```

Response:

```json
{
  "jsonrpc": "2.0",
  "result": {
    "sessions": [
      {
        "id": "a1b2c3d4",
        "modified": "2025-01-15T10:30:00Z",
        "path": "/path/to/session.jsonl"
      }
    ]
  },
  "id": 2
}
```

#### `create_session`

```json
{
  "jsonrpc": "2.0",
  "method": "create_session",
  "params": {
    "title": "My session",
    "cwd": "/path/to/project"
  },
  "id": 3
}
```

Response:

```json
{
  "jsonrpc": "2.0",
  "result": {
    "session_id": "a1b2c3d4",
    "cwd": "/path/to/project"
  },
  "id": 3
}
```

#### `chat`

```json
{
  "jsonrpc": "2.0",
  "method": "chat",
  "params": {
    "message": "Explain how Rust ownership works",
    "session_id": "a1b2c3d4",
    "thinking": true,
    "cwd": "/path/to/project"
  },
  "id": 4
}
```

| Parameter | Type | Description |
| --- | --- | --- |
| `message` | string (required) | The user message to send. |
| `session_id` | string (optional) | Resume an existing session. A new session is created if omitted. |
| `thinking` | boolean (optional) | Include thinking output in the response. Defaults to the settings file value. |
| `cwd` | string (optional) | Working directory for the session. |

Response:

```json
{
  "jsonrpc": "2.0",
  "result": {
    "session_id": "a1b2c3d4",
    "message": "Rust ownership is a set of rules that govern how memory is managed..."
  },
  "id": 4
}
```

The `chat` method is **non-streaming** — the response is written only after the agent reaches a terminal answer. Session and lifecycle events are persisted even though they are not sent as live notifications.

### Error Responses

Errors follow the JSON-RPC 2.0 error format:

```json
{
  "jsonrpc": "2.0",
  "error": {
    "code": -32600,
    "message": "Invalid Request",
    "data": {
      "detail": "jsonrpc must be '2.0' and method is required"
    }
  },
  "id": 1
}
```

| Error Code | Meaning |
| --- | --- |
| `-32700` | Parse error — invalid JSON. |
| `-32600` | Invalid Request — missing `jsonrpc` or `method`. |
| `-32601` | Method not found — unknown method name. |
| `-32602` | Invalid params — missing or malformed parameters. |
| `-32603` | Internal error — provider, session, or tool failure. |

### Example: Chat via Shell

```bash
echo '{"jsonrpc":"2.0","method":"chat","params":{"message":"Hello"},"id":1}' \
  | hallucinate rpc
```

---

## Authentication

### Setting a Key

```bash
# Recommended: use an environment variable so the key isn't in shell history
hallucinate auth set openai
# or explicitly pass the key
hallucinate auth set openai --api-key "sk-..."
```

The `auth set` command falls back to `HALLUCINATE_<PROVIDER>_API_KEY` (e.g., `HALLUCINATE_OPENAI_API_KEY`) when `--api-key` is omitted.

### Listing and Removing

```bash
hallucinate auth list
hallucinate auth remove openai
```

### Inside the TUI

The same operations are available via the `/auth` slash command:

```text
/auth set openai sk-...
/auth list
/auth remove openai
```

---

## Tips and Best Practices

### Model Selection

- Use a bare model ID if it's configured under the default provider: `--model gpt-4o`
- Use `provider/model` syntax for multi-provider setups: `--model openrouter/meta-llama/llama-3.1-70b`

### Image Attachments

Images are automatically resized when `auto_resize_images` is enabled. Supported formats: bmp, gif, jpeg, png, webp.

```bash
hallucinate print --image photo1.jpg --image photo2.png "Compare these two images"
```

### Skills in Print/RPC Mode

Skills are auto-discovered from user and project directories by default. Override with:

```bash
hallucinate print --skill ./skills/git/SKILL.md "Review this PR"
hallucinate print --no-skills "Plain prompt without skills"
```

### Prompt Templates

Similarly, templates are auto-discovered. Load specific ones:

```bash
hallucinate print --prompt-template ./prompts/review.md "src/main.rs"
hallucinate print --no-prompt-templates "No templates"
```

### Debugging

Enable verbose logging:

```bash
RUST_LOG=debug hallucinate print "Hello"
# or use the --debug flag to write logs to the cache directory
hallucinate --debug print "Hello"
```

### Shell Configuration

Override the shell used by the bash tool:

```bash
hallucinate print --shell /bin/zsh "What shell am I using?"
```
