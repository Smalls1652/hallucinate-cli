//! Print mode: single-shot prompt/response.

use std::io::{IsTerminal, Write};

use anyhow::Result;
use clap::{ArgAction, Args};
use futures::StreamExt;
use hallucinate_agent::{
    ai::types::{Context, Message, StreamOptions, UserMessage},
    config::settings::Settings,
};
use hallucinate_tui::components::{markdown_to_ansi, markdown_to_text};

/// Arguments for print mode.
#[derive(Args, Debug, Clone)]
pub struct PrintArgs {
    /// Model to use as a configured model ID or "provider/model" pair.
    #[arg(short = 'm', long = "model")]
    pub model: Option<String>,

    /// API key for the provider. Overrides environment variables and stored
    /// credentials. Environment fallback also accepts OPENAI_API_KEY.
    #[arg(long = "api-key", env = "HALLUCINATE_API_KEY")]
    pub api_key: Option<String>,

    /// Base URL for the API (for OpenAI-compatible providers).
    #[arg(long = "base-url")]
    pub base_url: Option<String>,

    /// Shell executable for the bash tool. Overrides the settings file and
    /// SHELL environment variable.
    #[arg(long = "shell")]
    pub shell: Option<String>,

    /// Maximum number of tokens in the response.
    #[arg(long = "max-tokens")]
    pub max_tokens: Option<u32>,

    /// Temperature for sampling (0.0 - 2.0).
    #[arg(long = "temperature")]
    pub temperature: Option<f64>,

    /// System prompt to use.
    #[arg(long = "system-prompt")]
    pub system_prompt: Option<String>,

    /// Attach an image to a print-mode prompt. May be repeated.
    #[arg(long = "image", action = ArgAction::Append)]
    pub images: Vec<std::path::PathBuf>,

    /// Load a skill Markdown file or a directory of skills. May be repeated.
    #[arg(long = "skill", action = ArgAction::Append)]
    pub skills: Vec<std::path::PathBuf>,

    /// Disable automatic discovery of user and project skills.
    #[arg(long = "no-skills")]
    pub no_skills: bool,

    /// The prompt to send to the LLM.
    pub prompt: String,
}

impl From<&PrintArgs> for hallucinate_agent::config::RuntimeOverrides {
    fn from(args: &PrintArgs) -> Self {
        Self {
            model: args.model.clone(),
            api_key: args.api_key.clone(),
            base_url: args.base_url.clone(),
            shell: args.shell.clone(),
            max_tokens: args.max_tokens,
            temperature: args.temperature,
            system_prompt: args.system_prompt.clone(),
        }
    }
}

/// Run print mode with the given CLI arguments and settings.
pub async fn print_command(
    args: &PrintArgs,
    settings: &Settings,
) -> Result<()> {
    // Resolve model
    let model = hallucinate_agent::config::resolve_model(
        settings,
        args.model.as_ref(),
        args.base_url.as_ref(),
    )?;

    // Build context
    let images = futures::future::try_join_all(args.images.iter().map(|path| {
        hallucinate_agent::ai::images::load_image_file(path, settings.auto_resize_images)
    }))
    .await?;

    let cwd = std::env::current_dir()?;

    let mut context = Context::new(vec![Message::User(UserMessage::with_images(
        &args.prompt,
        images,
    ))]);

    let system_prompt = hallucinate_agent::agent::system_prompt::build_system_prompt_for_cwd(
        args.system_prompt.as_deref(),
        Some(&cwd),
    );

    context = context.with_system_prompt(system_prompt);

    // Build stream options
    let mut options = StreamOptions::new();
    if let Some(api_key) = args.api_key.as_deref().filter(|key| !key.is_empty()) {
        options = options.with_api_key(api_key);
    }

    if let Some(max_tokens) =
        hallucinate_agent::config::resolve_max_tokens(&settings, args.max_tokens, &model)
    {
        options = options.with_max_tokens(max_tokens);
    }

    if let Some(temperature) = args.temperature.or(settings.temperature) {
        options = options.with_temperature(temperature);
    }

    // Create provider for the model's provider group.
    let provider = hallucinate_agent::config::create_provider_for_model(
        settings,
        args.base_url.as_ref(),
        args.api_key.as_ref(),
        &model,
    )?;

    // Stream the response
    let mut stream = provider.stream(&model, &context, &options).await?;

    // Markdown needs the complete response in order to parse fenced code
    // blocks and other constructs correctly. Collect deltas and render once
    // the stream is complete instead of printing raw Markdown as it arrives.
    let show_thinking = hallucinate_agent::config::resolve_show_thinking(settings, false);

    let mut response = String::new();
    let mut thinking = String::new();

    while let Some(event) = stream.next().await {
        match event {
            hallucinate_agent::ai::streaming::AssistantMessageEvent::TextDelta {
                delta, ..
            } => {
                response.push_str(&delta);
            },

            hallucinate_agent::ai::streaming::AssistantMessageEvent::ThinkingDelta {
                delta,
                ..
            } => {
                thinking.push_str(&delta);
            },

            hallucinate_agent::ai::streaming::AssistantMessageEvent::Done { message, .. } => {
                // Some providers include final content only in the Done
                // message, so use it as a fallback when no deltas arrived.
                if response.is_empty() {
                    response = message
                        .content
                        .iter()
                        .filter_map(|content| match content {
                            hallucinate_agent::ai::types::AssistantContent::Text(text) => {
                                Some(text.text.as_str())
                            },
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");
                }

                if thinking.is_empty() {
                    thinking = message
                        .content
                        .iter()
                        .filter_map(|content| match content {
                            hallucinate_agent::ai::types::AssistantContent::Thinking(thinking) => {
                                Some(thinking.thinking.as_str())
                            },
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                }

                let output = visible_output(&response, &thinking, show_thinking);

                let rendered = if std::io::stdout().is_terminal() {
                    markdown_to_ansi(&output)
                } else {
                    markdown_to_text(&output)
                };

                print!("{rendered}");

                std::io::stdout().flush()?;
            },

            hallucinate_agent::ai::streaming::AssistantMessageEvent::Error { error, .. } => {
                eprintln!(
                    "\n[Error: {}]",
                    error.error_message.as_deref().unwrap_or("unknown error")
                );

                return Err(anyhow::anyhow!(
                    "LLM error: {}",
                    error.error_message.as_deref().unwrap_or("unknown error")
                ));
            },

            hallucinate_agent::ai::streaming::AssistantMessageEvent::ToolcallEnd {
                tool_call,
                ..
            } => {
                println!("\n[Tool call: {}]", tool_call.human_readable());
            },
            _ => {},
        }
    }

    Ok(())
}

/// Add a clear, Markdown-friendly heading when reasoning output is enabled.
fn visible_output(
    response: &str,
    thinking: &str,
    show_thinking: bool,
) -> String {
    if !show_thinking || thinking.is_empty() {
        return response.to_string();
    }

    if response.is_empty() {
        format!("**Thinking**\n\n{thinking}")
    } else {
        format!("**Thinking**\n\n{thinking}\n\n{response}")
    }
}

#[cfg(test)]
mod tests {
    use super::visible_output;

    #[test]
    fn thinking_output_is_optional_and_precedes_the_answer() {
        assert_eq!(visible_output("answer", "plan", false), "answer");
        assert_eq!(
            visible_output("answer", "plan", true),
            "**Thinking**\n\nplan\n\nanswer"
        );
    }
}
