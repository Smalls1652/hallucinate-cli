//! File-backed prompt templates and slash-command expansion.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use regex::Regex;
use serde::Deserialize;

use crate::config::paths;

/// A reusable prompt loaded from a Markdown file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptTemplate {
    /// Slash-command name, derived from the Markdown filename.
    pub name: String,
    /// A short description from frontmatter or the first body line.
    pub description: String,
    /// Optional usage text from `argument-hint` frontmatter.
    pub argument_hint: Option<String>,
    /// Template body, excluding the YAML frontmatter.
    pub content: String,
    /// Absolute path to the source file.
    pub file_path: PathBuf,
}

/// A non-fatal error from prompt-template discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptTemplateDiagnostic {
    pub message: String,
    pub path: PathBuf,
}

/// The templates and diagnostics returned by discovery.
#[derive(Debug, Default)]
pub struct PromptTemplateLoadResult {
    pub templates: Vec<PromptTemplate>,
    pub diagnostics: Vec<PromptTemplateDiagnostic>,
}

/// Options controlling prompt-template discovery.
#[derive(Debug, Clone)]
pub struct PromptTemplateLoadOptions {
    pub cwd: PathBuf,
    pub prompt_paths: Vec<PathBuf>,
    pub include_defaults: bool,
}

#[derive(Debug, Default, Deserialize)]
struct PromptFrontmatter {
    description: Option<String>,
    #[serde(rename = "argument-hint")]
    argument_hint: Option<String>,
}

/// Load prompt templates from standard locations and explicit file paths.
pub fn load_prompt_templates(options: &PromptTemplateLoadOptions) -> PromptTemplateLoadResult {
    let mut result = PromptTemplateLoadResult::default();
    let mut names = HashSet::new();
    let mut files = HashSet::new();

    if options.include_defaults {
        if let Some(directory) = paths::prompts_dir() {
            load_directory(&directory, &mut result, &mut names, &mut files);
        }
        load_directory(
            &paths::project_prompts_dir(&options.cwd),
            &mut result,
            &mut names,
            &mut files,
        );
    }

    for path in &options.prompt_paths {
        let path = expand_path(path, &options.cwd);
        if path.is_dir() {
            load_directory(&path, &mut result, &mut names, &mut files);
        } else if is_markdown_file(&path) {
            load_file(&path, &mut result, &mut names, &mut files);
        } else {
            result.diagnostics.push(PromptTemplateDiagnostic {
                message: if path.exists() {
                    "prompt template path must be a Markdown file or directory".to_string()
                } else {
                    "prompt template path does not exist".to_string()
                },
                path,
            });
        }
    }

    result
}

/// Expand a `/template-name arguments` command, returning input unchanged when
/// no matching template exists.
pub fn expand_prompt_template(
    text: &str,
    templates: &[PromptTemplate],
) -> String {
    let input = text.trim_start();
    let Some(command) = input.strip_prefix('/') else {
        return text.to_string();
    };
    let mut parts = command.splitn(2, char::is_whitespace);
    let Some(name) = parts.next().filter(|name| !name.is_empty()) else {
        return text.to_string();
    };
    let Some(template) = templates.iter().find(|template| template.name == name) else {
        return text.to_string();
    };

    substitute_args(
        &template.content,
        &parse_command_args(parts.next().unwrap_or_default()),
    )
}

/// Parse shell-like command arguments, including single and double quotes.
pub fn parse_command_args(arguments: &str) -> Vec<String> {
    let mut parsed = Vec::new();
    let mut current = String::new();
    let mut quote = None;

    for character in arguments.chars() {
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            } else {
                current.push(character);
            }
        } else if matches!(character, '\'' | '"') {
            quote = Some(character);
        } else if character.is_whitespace() {
            if !current.is_empty() {
                parsed.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if !current.is_empty() {
        parsed.push(current);
    }
    parsed
}

/// Substitute positional and aggregate arguments in a template body.
pub fn substitute_args(
    content: &str,
    arguments: &[String],
) -> String {
    let pattern = Regex::new(
        r"\$\{(\d+|ARGUMENTS|@):-([^}]*)\}|\$\{@:(\d+)(?::(\d+))?\}|\$(ARGUMENTS|@|\d+)",
    )
    .expect("template substitution pattern is valid");
    let all_arguments = arguments.join(" ");
    pattern
        .replace_all(content, |captures: &regex::Captures<'_>| {
            if let Some(target) = captures.get(1) {
                let value = argument_value(target.as_str(), arguments, &all_arguments);
                return if value.is_empty() {
                    captures
                        .get(2)
                        .map_or("", |default| default.as_str())
                        .to_string()
                } else {
                    value.to_string()
                };
            }
            if let Some(start) = captures.get(3) {
                let start = start
                    .as_str()
                    .parse::<usize>()
                    .unwrap_or(1)
                    .saturating_sub(1);
                let end = captures
                    .get(4)
                    .and_then(|length| length.as_str().parse::<usize>().ok())
                    .map_or(arguments.len(), |length| start.saturating_add(length));
                return arguments[start.min(arguments.len())..end.min(arguments.len())].join(" ");
            }
            argument_value(captures.get(5).unwrap().as_str(), arguments, &all_arguments).to_string()
        })
        .into_owned()
}

fn load_directory(
    directory: &Path,
    result: &mut PromptTemplateLoadResult,
    names: &mut HashSet<String>,
    files: &mut HashSet<PathBuf>,
) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_file() && is_markdown_file(&path) {
            load_file(&path, result, names, files);
        }
    }
}

fn load_file(
    path: &Path,
    result: &mut PromptTemplateLoadResult,
    names: &mut HashSet<String>,
    files: &mut HashSet<PathBuf>,
) {
    let file_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !files.insert(file_path.clone()) {
        return;
    }
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) => {
            result.diagnostics.push(PromptTemplateDiagnostic {
                message: format!("failed to read prompt template: {error}"),
                path: path.to_path_buf(),
            });
            return;
        },
    };
    let (frontmatter, content) = match split_frontmatter(&raw) {
        Ok(parts) => parts,
        Err(message) => {
            result.diagnostics.push(PromptTemplateDiagnostic {
                message,
                path: path.to_path_buf(),
            });
            return;
        },
    };
    let name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();
    if name.is_empty() {
        return;
    }
    if !names.insert(name.clone()) {
        result.diagnostics.push(PromptTemplateDiagnostic {
            message: format!(
                "prompt template name collision: {name:?}; keeping the first template"
            ),
            path: path.to_path_buf(),
        });
        return;
    }
    let description = frontmatter
        .description
        .filter(|description| !description.trim().is_empty())
        .unwrap_or_else(|| first_line_description(&content));
    result.templates.push(PromptTemplate {
        name,
        description,
        argument_hint: frontmatter.argument_hint,
        content,
        file_path,
    });
}

fn split_frontmatter(raw: &str) -> Result<(PromptFrontmatter, String), String> {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let Some(rest) = raw
        .strip_prefix("---\n")
        .or_else(|| raw.strip_prefix("---\r\n"))
    else {
        return Ok((PromptFrontmatter::default(), raw.to_string()));
    };
    let Some(end) = rest.find("\n---\n").or_else(|| rest.find("\n---\r\n")) else {
        return Err("prompt template frontmatter is not terminated".to_string());
    };
    let frontmatter = serde_yaml::from_str(&rest[..end])
        .map_err(|error| format!("invalid prompt template frontmatter: {error}"))?;
    let body_start = end
        + if rest[end..].starts_with("\n---\r\n") {
            6
        } else {
            5
        };
    Ok((frontmatter, rest[body_start..].to_string()))
}

fn argument_value<'a>(
    target: &str,
    arguments: &'a [String],
    all_arguments: &'a str,
) -> &'a str {
    if matches!(target, "@" | "ARGUMENTS") {
        return all_arguments;
    }
    target
        .parse::<usize>()
        .ok()
        .and_then(|index| index.checked_sub(1))
        .and_then(|index| arguments.get(index))
        .map_or("", String::as_str)
}

fn first_line_description(content: &str) -> String {
    let line = content
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default();
    if line.chars().count() <= 60 {
        line.to_string()
    } else {
        format!("{}...", line.chars().take(60).collect::<String>())
    }
}

fn is_markdown_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

fn expand_path(
    path: &Path,
    cwd: &Path,
) -> PathBuf {
    let value = shellexpand::tilde(&path.to_string_lossy()).into_owned();
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template(
        name: &str,
        content: &str,
    ) -> PromptTemplate {
        PromptTemplate {
            name: name.to_string(),
            description: String::new(),
            argument_hint: None,
            content: content.to_string(),
            file_path: PathBuf::new(),
        }
    }

    #[test]
    fn expands_positional_defaults_and_slices() {
        let content = "$1 $2 $@ ${3:-fallback} ${@:2} ${@:1:2}";
        assert_eq!(
            substitute_args(
                content,
                &["one".to_string(), "two".to_string(), "three".to_string()]
            ),
            "one two one two three three two three one two"
        );
    }

    #[test]
    fn parses_quoted_command_arguments() {
        assert_eq!(
            parse_command_args("one 'two words' \"three words\""),
            ["one", "two words", "three words"]
        );
    }

    #[test]
    fn expands_only_known_slash_commands() {
        let templates = vec![template("review", "Review $1: $@")];
        assert_eq!(
            expand_prompt_template("/review src 'be strict'", &templates),
            "Review src: src be strict"
        );
        assert_eq!(
            expand_prompt_template("/unknown value", &templates),
            "/unknown value"
        );
        assert_eq!(
            expand_prompt_template("  /review src", &templates),
            "Review src: src"
        );
    }

    #[test]
    fn frontmatter_is_removed_from_template_content() {
        let (frontmatter, body) = split_frontmatter(
            "---\ndescription: Review code\nargument-hint: <path>\n---\nReview $1",
        )
        .unwrap();
        assert_eq!(frontmatter.description.as_deref(), Some("Review code"));
        assert_eq!(frontmatter.argument_hint.as_deref(), Some("<path>"));
        assert_eq!(body, "Review $1");
    }

    #[test]
    fn loads_only_top_level_templates_from_an_explicit_directory() {
        let directory =
            std::env::temp_dir().join(format!("hallucinate-prompts-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(directory.join("nested")).unwrap();
        fs::write(
            directory.join("review.md"),
            "---\ndescription: Review a change\n---\nReview $1",
        )
        .unwrap();
        fs::write(directory.join("nested/ignored.md"), "This is ignored").unwrap();

        let result = load_prompt_templates(&PromptTemplateLoadOptions {
            cwd: directory.clone(),
            prompt_paths: vec![directory.clone()],
            include_defaults: false,
        });
        assert_eq!(result.templates.len(), 1);
        assert_eq!(result.templates[0].name, "review");
        assert_eq!(result.templates[0].description, "Review a change");
        fs::remove_dir_all(directory).unwrap();
    }
}
