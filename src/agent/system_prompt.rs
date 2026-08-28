//! System prompt assembly.
/// Provides template-based system prompt generation with tool descriptions
/// and session context injection.
use crate::agent::tools::ToolRegistry;

/// Default system prompt template.
const DEFAULT_SYSTEM_PROMPT: &str = r#"You are a helpful AI coding assistant. You have access to tools that let you execute shell commands, read/write files, search code, and more.

When given a task:
1. Understand what the user wants
2. Plan your approach
3. Use tools as needed to accomplish the task
4. Explain what you did and show results
5. If something fails, try a different approach

Be concise in your explanations. Focus on solving the user's problem efficiently.

Available tools:
{tool_descriptions}

Remember: Always verify your work by reading back files or checking output after making changes. If you make an edit, read the file afterward to confirm it was applied correctly."#;

/// Build the system prompt with tool descriptions.
pub fn build_system_prompt(
    tool_registry: &ToolRegistry,
    custom_prompt: Option<&str>,
) -> String {
    let tool_descriptions = if tool_registry.is_empty() {
        "No tools available.".to_string()
    } else {
        let mut descriptions = String::new();
        for tool in tool_registry.to_ai_tools() {
            descriptions.push_str(&format!("- **{}**: {}\n", tool.name, tool.description));
        }
        descriptions
    };

    let base_prompt = custom_prompt.unwrap_or(DEFAULT_SYSTEM_PROMPT);
    base_prompt.replace("{tool_descriptions}", &tool_descriptions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_system_prompt_with_tools() {
        let registry = ToolRegistry::with_all_builtins_and_image_resize(true);
        let prompt = build_system_prompt(&registry, None);
        assert!(prompt.contains("bash"));
        assert!(prompt.contains("read"));
        assert!(prompt.contains("write"));
    }

    #[test]
    fn test_build_system_prompt_without_tools() {
        let registry = ToolRegistry::new();
        let prompt = build_system_prompt(&registry, None);
        assert!(prompt.contains("No tools available"));
    }

    #[test]
    fn test_build_system_prompt_with_custom() {
        let registry = ToolRegistry::new();
        let custom = "You are a custom assistant.";
        let prompt = build_system_prompt(&registry, Some(custom));
        assert_eq!(prompt, custom);
    }
}
