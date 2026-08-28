//! Agent Skills discovery and system-prompt formatting.
//!
//! Skills are Markdown files with YAML frontmatter. They are discovered from
//! the user configuration directory, the current project's `.hallucinate`
//! directory, the shared `.agents` protocol locations, and optional paths
//! passed on the command line.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::config::paths;

const MAX_NAME_LENGTH: usize = 64;
const MAX_DESCRIPTION_LENGTH: usize = 1024;

/// A loaded skill that can be advertised to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// Machine-readable skill name.
    pub name: String,
    /// Brief explanation of when to use the skill.
    pub description: String,
    /// Absolute path to the Markdown file.
    pub file_path: PathBuf,
    /// Directory against which paths in the skill are resolved.
    pub base_dir: PathBuf,
    /// Whether the skill may only be invoked explicitly.
    pub disable_model_invocation: bool,
}

/// A non-fatal problem encountered while loading a skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDiagnostic {
    pub message: String,
    pub path: PathBuf,
}

/// The result of skill discovery.
#[derive(Debug, Default)]
pub struct SkillLoadResult {
    pub skills: Vec<Skill>,
    pub diagnostics: Vec<SkillDiagnostic>,
}

/// Options controlling skill discovery.
#[derive(Debug, Clone)]
pub struct SkillLoadOptions {
    /// Working directory whose project-local skills should be loaded.
    pub cwd: PathBuf,
    /// Extra skill files or directories, normally from `--skill`.
    pub skill_paths: Vec<PathBuf>,
    /// Whether to load the standard user and project skill directories,
    /// including the shared `.agents` locations.
    pub include_defaults: bool,
}

#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
    #[serde(rename = "disable-model-invocation", default)]
    disable_model_invocation: bool,
}

/// Load skills from the configured default locations and explicit paths.
pub fn load_skills(options: &SkillLoadOptions) -> SkillLoadResult {
    let mut result = SkillLoadResult::default();
    let mut seen_paths = HashSet::new();
    let mut seen_names = HashSet::new();

    if options.include_defaults {
        // Preserve the existing built-in locations first. This keeps the
        // traditional first-loaded-skill-wins merge behavior when a skill is
        // present in more than one location.
        if let Some(directory) = paths::skills_dir() {
            load_skills_from_dir(&directory, &mut result, &mut seen_paths, &mut seen_names);
        }
        load_skills_from_dir(
            &paths::project_skills_dir(&options.cwd),
            &mut result,
            &mut seen_paths,
            &mut seen_names,
        );

        // `.agents/skills` is a shared convention used by multiple agent
        // harnesses. Unlike the built-in locations, root Markdown files in
        // these directories are not skills; skills are declared by SKILL.md
        // files or Markdown files below a grouping directory.
        if let Some(directory) = paths::agents_skills_dir() {
            load_skills_from_dir_with_root_files(
                &directory,
                false,
                &mut result,
                &mut seen_paths,
                &mut seen_names,
            );
        }
        for directory in project_agents_skills_dirs(&options.cwd) {
            load_skills_from_dir_with_root_files(
                &directory,
                false,
                &mut result,
                &mut seen_paths,
                &mut seen_names,
            );
        }
    }

    for path in &options.skill_paths {
        let path = expand_path(path, &options.cwd);
        if path.is_dir() {
            load_skills_from_dir(&path, &mut result, &mut seen_paths, &mut seen_names);
        } else if path.is_file() && is_markdown_file(&path) {
            load_skill_file(&path, &mut result, &mut seen_paths, &mut seen_names);
        } else {
            result.diagnostics.push(SkillDiagnostic {
                message: if path.exists() {
                    "skill path must be a Markdown file or directory".to_string()
                } else {
                    "skill path does not exist".to_string()
                },
                path,
            });
        }
    }

    result
}

/// Expand an explicitly invoked `/skill:name` command.
///
/// Unknown skills are returned unchanged so slash commands owned by a
/// provider remain usable. Explicit invocation includes the complete skill
/// body and appends any arguments after it.
pub fn expand_skill_command(
    text: &str,
    skills: &[Skill],
) -> Result<String, anyhow::Error> {
    let input = text.trim_start();
    let Some(command) = input.strip_prefix("/skill:") else {
        return Ok(text.to_string());
    };
    let end = command.find(char::is_whitespace).unwrap_or(command.len());
    let name = &command[..end];
    let Some(skill) = skills.iter().find(|skill| skill.name == name) else {
        return Ok(text.to_string());
    };

    let content = fs::read_to_string(&skill.file_path)
        .map_err(|error| anyhow::anyhow!("failed to read skill '{}': {error}", name))?;
    let body = skill_body(&content).trim();
    let block = format!(
        "<skill name=\"{}\" location=\"{}\">\nReferences are relative to {}.\n\n{}\n</skill>",
        escape_xml(&skill.name),
        escape_xml(&skill.file_path.to_string_lossy()),
        escape_xml(&skill.base_dir.to_string_lossy()),
        body
    );
    let arguments = command[end..].trim();
    Ok(if arguments.is_empty() {
        block
    } else {
        format!("{block}\n\n{arguments}")
    })
}

/// Format model-invocable skills according to the Agent Skills convention.
pub fn format_skills_for_prompt(skills: &[Skill]) -> String {
    let visible: Vec<_> = skills
        .iter()
        .filter(|skill| !skill.disable_model_invocation)
        .collect();
    if visible.is_empty() {
        return String::new();
    }

    let mut output = String::from(
        "\n\nThe following skills provide specialized instructions for specific tasks.\n\
Use the read tool to load a skill's file when the task matches its description.\n\
When a skill file references a relative path, resolve it against the skill directory \
(parent of SKILL.md) and use that absolute path in tool commands.\n\n<available_skills>",
    );

    for skill in visible {
        output.push_str("\n  <skill>\n");
        output.push_str(&format!("    <name>{}</name>\n", escape_xml(&skill.name)));
        output.push_str(&format!(
            "    <description>{}</description>\n",
            escape_xml(&skill.description)
        ));
        output.push_str(&format!(
            "    <location>{}</location>\n",
            escape_xml(&skill.file_path.to_string_lossy())
        ));
        output.push_str("  </skill>");
    }
    output.push_str("\n</available_skills>");
    output
}

fn load_skills_from_dir(
    directory: &Path,
    result: &mut SkillLoadResult,
    seen_paths: &mut HashSet<PathBuf>,
    seen_names: &mut HashSet<String>,
) {
    load_skills_from_dir_with_root_files(directory, true, result, seen_paths, seen_names);
}

fn load_skills_from_dir_with_root_files(
    directory: &Path,
    include_root_files: bool,
    result: &mut SkillLoadResult,
    seen_paths: &mut HashSet<PathBuf>,
    seen_names: &mut HashSet<String>,
) {
    if !directory.is_dir() {
        return;
    }

    // A directory containing SKILL.md is itself a skill and is not a container.
    let root_skill = directory.join("SKILL.md");
    if root_skill.is_file() {
        load_skill_file(&root_skill, result, seen_paths, seen_names);
        return;
    }

    let Ok(entries) = fs::read_dir(directory) else {
        result.diagnostics.push(SkillDiagnostic {
            message: "failed to read skills directory".to_string(),
            path: directory.to_path_buf(),
        });
        return;
    };

    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || matches!(name.as_ref(), "node_modules" | "target") {
            continue;
        }
        if path.is_dir() {
            // Root files are special only for the directory supplied by the
            // caller. Markdown files inside grouping directories are valid
            // `.agents` skills.
            load_skills_from_dir_with_root_files(&path, true, result, seen_paths, seen_names);
        } else if include_root_files && is_markdown_file(&path) {
            load_skill_file(&path, result, seen_paths, seen_names);
        }
    }
}

fn load_skill_file(
    path: &Path,
    result: &mut SkillLoadResult,
    seen_paths: &mut HashSet<PathBuf>,
    seen_names: &mut HashSet<String>,
) {
    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !seen_paths.insert(canonical_path) {
        return;
    }

    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            result.diagnostics.push(SkillDiagnostic {
                message: format!("failed to read skill: {error}"),
                path: path.to_path_buf(),
            });
            return;
        },
    };
    let frontmatter = match parse_frontmatter(&content) {
        Ok(frontmatter) => frontmatter,
        Err(message) => {
            if path.file_name().is_some_and(|name| name == "SKILL.md") {
                result.diagnostics.push(SkillDiagnostic {
                    message,
                    path: path.to_path_buf(),
                });
            }
            return;
        },
    };

    let Some(description) = frontmatter
        .description
        .filter(|value| !value.trim().is_empty())
    else {
        result.diagnostics.push(SkillDiagnostic {
            message: "description is required".to_string(),
            path: path.to_path_buf(),
        });
        return;
    };
    let fallback_name = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();
    let name = frontmatter.name.unwrap_or(fallback_name);

    for message in validate_skill(&name, &description) {
        result.diagnostics.push(SkillDiagnostic {
            message,
            path: path.to_path_buf(),
        });
    }
    if !seen_names.insert(name.clone()) {
        result.diagnostics.push(SkillDiagnostic {
            message: format!("skill name collision: {name:?}; keeping the first skill"),
            path: path.to_path_buf(),
        });
        return;
    }

    result.skills.push(Skill {
        name,
        description,
        file_path: path.canonicalize().unwrap_or_else(|_| path.to_path_buf()),
        base_dir: path.parent().unwrap_or(Path::new(".")).to_path_buf(),
        disable_model_invocation: frontmatter.disable_model_invocation,
    });
}

fn parse_frontmatter(content: &str) -> Result<SkillFrontmatter, String> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let Some(rest) = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))
    else {
        return Err("skill is missing YAML frontmatter".to_string());
    };
    let Some(end) = rest.find("\n---\n").or_else(|| rest.find("\n---\r\n")) else {
        return Err("skill frontmatter is not terminated".to_string());
    };
    serde_yaml::from_str(&rest[..end])
        .map_err(|error| format!("invalid skill frontmatter: {error}"))
}

fn skill_body(content: &str) -> &str {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let Some(rest) = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))
    else {
        return content;
    };
    let Some(end) = rest.find("\n---\n").or_else(|| rest.find("\n---\r\n")) else {
        return content;
    };
    let body_start = end
        + if rest[end..].starts_with("\n---\r\n") {
            6
        } else {
            5
        };
    &rest[body_start..]
}

fn validate_skill(
    name: &str,
    description: &str,
) -> Vec<String> {
    let mut messages = Vec::new();
    if name.len() > MAX_NAME_LENGTH {
        messages.push(format!("name exceeds {MAX_NAME_LENGTH} characters"));
    }
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || name.starts_with('-')
        || name.ends_with('-')
        || name.contains("--")
    {
        messages.push(
            "name must use lowercase letters, numbers, and single internal hyphens".to_string(),
        );
    }
    if description.len() > MAX_DESCRIPTION_LENGTH {
        messages.push(format!(
            "description exceeds {MAX_DESCRIPTION_LENGTH} characters"
        ));
    }
    messages
}

fn is_markdown_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

fn project_agents_skills_dirs(cwd: &Path) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    let mut directory = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let git_root = find_git_root(&directory);

    loop {
        directories.push(paths::project_agents_skills_dir(&directory));
        if git_root.as_ref().is_some_and(|root| root == &directory) {
            break;
        }

        let Some(parent) = directory.parent() else {
            break;
        };
        if parent == directory {
            break;
        }
        directory = parent.to_path_buf();
    }

    directories
}

fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut directory = start.to_path_buf();
    loop {
        if directory.join(".git").exists() {
            return Some(directory);
        }

        let parent = directory.parent()?.to_path_buf();
        if parent == directory {
            return None;
        }
        directory = parent;
    }
}

fn expand_path(
    path: &Path,
    cwd: &Path,
) -> PathBuf {
    let path = path.to_string_lossy();
    let expanded = shellexpand::tilde(&path);
    let path = PathBuf::from(expanded.as_ref());
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "hallucinate-skills-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_skill(
        path: &Path,
        content: &str,
    ) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn loads_nested_skill_directories() {
        let directory = temp_dir("nested");
        write_skill(
            &directory.join("git/SKILL.md"),
            "---\nname: git-helper\ndescription: Help with Git tasks\n---\n# Git",
        );
        let result = load_skills(&SkillLoadOptions {
            cwd: directory.clone(),
            skill_paths: vec![directory.clone()],
            include_defaults: false,
        });
        assert_eq!(result.skills.len(), 1);
        assert_eq!(result.skills[0].name, "git-helper");
        assert_eq!(result.skills[0].base_dir, directory.join("git"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn skill_root_is_preferred_to_nested_skills() {
        let directory = temp_dir("root");
        write_skill(
            &directory.join("SKILL.md"),
            "---\ndescription: Root skill\n---\n",
        );
        write_skill(
            &directory.join("child/SKILL.md"),
            "---\ndescription: Child skill\n---\n",
        );
        let result = load_skills(&SkillLoadOptions {
            cwd: directory.clone(),
            skill_paths: vec![directory.clone()],
            include_defaults: false,
        });
        assert_eq!(result.skills.len(), 1);
        assert_eq!(
            result.skills[0].name,
            directory.file_name().unwrap().to_string_lossy()
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn loads_agents_skills_and_ignores_root_markdown() {
        let directory = temp_dir("agents");
        write_skill(
            &directory.join("root-file.md"),
            "---\nname: root-file\ndescription: Root file\n---\n",
        );
        write_skill(
            &directory.join("group/nested.md"),
            "---\nname: nested\ndescription: Nested skill\n---\n",
        );
        write_skill(
            &directory.join("declared/SKILL.md"),
            "---\nname: declared\ndescription: Declared skill\n---\n",
        );

        let mut result = SkillLoadResult::default();
        let mut seen_paths = HashSet::new();
        let mut seen_names = HashSet::new();
        load_skills_from_dir_with_root_files(
            &directory,
            false,
            &mut result,
            &mut seen_paths,
            &mut seen_names,
        );

        assert_eq!(
            result
                .skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            vec!["declared", "nested"]
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn loads_project_agents_directories_from_cwd_to_repo_root() {
        let directory = temp_dir("agents-ancestors");
        let cwd = directory.join("packages").join("feature");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir(directory.join(".git")).unwrap();
        write_skill(
            &directory.join(".agents/skills/repo/SKILL.md"),
            "---\nname: repo\ndescription: Repository skill\n---\n",
        );
        write_skill(
            &directory.join("packages/.agents/skills/package/SKILL.md"),
            "---\nname: package\ndescription: Package skill\n---\n",
        );

        let directories = project_agents_skills_dirs(&cwd);
        let canonical_cwd = cwd.canonicalize().unwrap();
        let canonical_root = directory.canonicalize().unwrap();
        assert_eq!(directories[0], canonical_cwd.join(".agents/skills"));
        assert!(directories.contains(&canonical_root.join(".agents/skills")));

        let mut result = SkillLoadResult::default();
        let mut seen_paths = HashSet::new();
        let mut seen_names = HashSet::new();
        for skill_directory in directories {
            load_skills_from_dir_with_root_files(
                &skill_directory,
                false,
                &mut result,
                &mut seen_paths,
                &mut seen_names,
            );
        }

        assert_eq!(result.skills.len(), 2);
        assert!(result.skills.iter().any(|skill| skill.name == "package"));
        assert!(result.skills.iter().any(|skill| skill.name == "repo"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_missing_description() {
        let directory = temp_dir("description");
        write_skill(&directory.join("bad/SKILL.md"), "---\nname: bad\n---\n");
        let result = load_skills(&SkillLoadOptions {
            cwd: directory.clone(),
            skill_paths: vec![directory.clone()],
            include_defaults: false,
        });
        assert!(result.skills.is_empty());
        assert!(result.diagnostics[0].message.contains("description"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn deduplicates_skills_merged_from_multiple_directories() {
        let directory = temp_dir("dedupe");
        let first = directory.join("first");
        let second = directory.join("second");
        write_skill(
            &first.join("shared/SKILL.md"),
            "---\nname: shared\ndescription: First skill\n---\n",
        );
        write_skill(
            &second.join("shared/SKILL.md"),
            "---\nname: shared\ndescription: Second skill\n---\n",
        );

        let result = load_skills(&SkillLoadOptions {
            cwd: directory.clone(),
            skill_paths: vec![first.clone(), second],
            include_defaults: false,
        });

        assert_eq!(result.skills.len(), 1);
        assert_eq!(result.skills[0].description, "First skill");
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("skill name collision"))
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn expands_explicit_skill_commands_with_arguments() {
        let directory = temp_dir("expand");
        let path = directory.join("review/SKILL.md");
        write_skill(
            &path,
            "---\nname: review\ndescription: Review code\n---\nCheck the diff",
        );
        let result = load_skills(&SkillLoadOptions {
            cwd: directory.clone(),
            skill_paths: vec![directory.clone()],
            include_defaults: false,
        });
        let expanded = expand_skill_command("/skill:review be strict", &result.skills).unwrap();
        assert!(expanded.contains("<skill name=\"review\""));
        assert!(expanded.contains("Check the diff"));
        assert!(expanded.ends_with("be strict"));
        assert_eq!(
            expand_skill_command("/skill:missing", &result.skills).unwrap(),
            "/skill:missing"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn formats_only_model_invocable_skills_and_escapes_xml() {
        let skills = vec![
            Skill {
                name: "normal".to_string(),
                description: "Use <this> & that".to_string(),
                file_path: PathBuf::from("/tmp/normal/SKILL.md"),
                base_dir: PathBuf::from("/tmp/normal"),
                disable_model_invocation: false,
            },
            Skill {
                name: "manual".to_string(),
                description: "Manual only".to_string(),
                file_path: PathBuf::from("/tmp/manual/SKILL.md"),
                base_dir: PathBuf::from("/tmp/manual"),
                disable_model_invocation: true,
            },
        ];
        let prompt = format_skills_for_prompt(&skills);
        assert!(prompt.contains("normal"));
        assert!(prompt.contains("Use &lt;this&gt; &amp; that"));
        assert!(!prompt.contains("manual"));
    }
}
