//! System prompt assembly.

/// Default system prompt template.
const DEFAULT_SYSTEM_PROMPT: &str = r#"You are a helpful AI coding assistant. You have access to tools that let you execute shell commands, read/write files, search code, and more.

When given a task:
1. Understand what the user wants
2. Plan your approach
3. Use tools as needed to accomplish the task
4. Explain what you did and show results
5. If something fails, try a different approach

Be concise in your explanations. Focus on solving the user's problem efficiently.

Remember: Always verify your work by reading back files or checking output after making changes. If you make an edit, read the file afterward to confirm it was applied correctly."#;

/// Build the system prompt with tool descriptions.
pub fn build_system_prompt(
    custom_prompt: Option<&str>,
) -> String {
    let base_prompt = custom_prompt.unwrap_or(DEFAULT_SYSTEM_PROMPT);

    base_prompt.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_system_prompt() {
        let prompt = build_system_prompt(None);
        assert_eq!(prompt, DEFAULT_SYSTEM_PROMPT.to_string());
    }

    #[test]
    fn test_build_system_prompt_with_custom() {
        let custom = "You are a custom assistant.";
        let prompt = build_system_prompt(Some(custom));
        assert_eq!(prompt, custom);
    }
}
