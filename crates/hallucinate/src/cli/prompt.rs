//! Print mode: single-shot prompt/response.

use std::{
    io::{IsTerminal, Write},
    sync::Arc,
};

use anyhow::{Context as _, Result, anyhow};
use clap::{ArgAction, Args};
use hallucinate_agent::{
    agent::runtime::Agent,
    ai::types::{AssistantContent, Context, Message, ReasoningEffort, StreamOptions, UserMessage},
    config::Settings,
};
use hallucinate_tui::components::markdown_to_ansi;

use super::stream_output::StreamOutput;

/// Arguments for prompt mode.
#[derive(Args, Debug, Clone)]
pub struct PromptArgs {
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

    /// Maximum number of tokens in the response.
    #[arg(long = "max-tokens")]
    pub max_tokens: Option<u32>,

    /// Temperature for sampling (0.0 - 2.0).
    #[arg(long = "temperature")]
    pub temperature: Option<f64>,

    /// Reasoning effort for models that support it.
    #[arg(long = "reasoning-effort", value_parser = parse_reasoning_effort)]
    pub reasoning_effort: Option<ReasoningEffort>,

    /// System prompt to use.
    #[arg(long = "system-prompt")]
    pub system_prompt: Option<String>,

    /// Attach an image to a print-mode prompt. May be repeated.
    #[arg(long = "image", action = ArgAction::Append)]
    pub images: Vec<std::path::PathBuf>,

    /// Stream an agent response with configured tools and skills, without
    /// saving a session.
    #[arg(long = "stream", conflicts_with = "pretty")]
    pub stream: bool,

    /// Shell executable for the shell tool in streaming mode.
    #[arg(long = "shell", requires = "stream")]
    pub shell: Option<String>,

    /// Load a skill Markdown file or directory in streaming mode. May be
    /// repeated.
    #[arg(long = "skill", action = ArgAction::Append, requires = "stream")]
    pub skills: Vec<std::path::PathBuf>,

    /// Disable automatic skill discovery in streaming mode.
    #[arg(long = "no-skills", requires = "stream")]
    pub no_skills: bool,

    /// Pretty print the output (not available with streaming).
    #[arg(long = "pretty")]
    pub pretty: bool,

    /// The prompt to send to the LLM.
    pub prompt: String,
}

impl From<&PromptArgs> for hallucinate_agent::config::RuntimeOverrides {
    fn from(args: &PromptArgs) -> Self {
        Self {
            model: args.model.clone(),
            api_key: args.api_key.clone(),
            base_url: args.base_url.clone(),
            shell: args
                .shell
                .as_ref()
                .map(|shell| shell.split(' ').map(str::to_string).collect::<Vec<_>>()),
            max_tokens: args.max_tokens,
            temperature: args.temperature,
            reasoning_effort: args.reasoning_effort,
            system_prompt: args.system_prompt.clone(),
        }
    }
}

/// Parse a CLI reasoning-effort value with a user-facing validation error.
fn parse_reasoning_effort(value: &str) -> Result<ReasoningEffort, String> {
    value.parse()
}

/// Run prompt mode with the given CLI arguments and settings.
pub async fn prompt_command(args: &PromptArgs) -> Result<()> {
    let mut settings = Settings::load()?;
    if args.stream {
        let mut stdout = std::io::stdout();
        let mut stderr = std::io::stderr();

        let render_markdown = stdout.is_terminal();
        let color_tools = stderr.is_terminal()
            && std::env::var_os("NO_COLOR").is_none()
            && std::env::var("TERM").ok().as_deref() != Some("dumb");

        return stream_prompt(
            args,
            &settings,
            &mut stdout,
            &mut stderr,
            render_markdown,
            color_tools,
        )
        .await;
    }

    settings.tools.enable = false;

    // Resolve model
    let model = hallucinate_agent::config::resolve_model(
        &settings,
        args.model.as_ref(),
        args.base_url.as_ref(),
    )?;

    // Build context
    let images = futures::future::try_join_all(args.images.iter().map(|path| {
        hallucinate_agent::ai::images::load_image_file(
            path,
            settings.tools.builtin.read.auto_resize_images,
        )
    }))
    .await?;

    let cwd = std::env::current_dir()?;

    let mut context = Context::new(vec![Message::User(UserMessage::with_images(
        &args.prompt,
        images,
    ))]);

    let system_prompt = hallucinate_agent::agent::system_prompt::build_system_prompt(
        args.system_prompt.as_deref(),
        Some(&cwd),
        false,
    );

    context = context.with_system_prompt(system_prompt);

    // Build stream options
    let mut options = StreamOptions::new();

    if let Some(include_usage) = settings
        .providers
        .get(&model.provider)
        .and_then(|provider| provider.include_usage)
    {
        options = options.with_include_usage(include_usage);
    }

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

    if model.supports_reasoning_effort {
        let effort = args
            .reasoning_effort
            .unwrap_or_else(ReasoningEffort::default_for_supported_model);
        options = options.with_reasoning_effort(effort);
    }

    // Create provider for the model's provider group.
    let provider = hallucinate_agent::config::create_provider_for_model(
        &settings,
        args.base_url.as_ref(),
        args.api_key.as_ref(),
        &model,
    )?;

    let response = provider.complete(&model, &context, &options).await?;

    let response_text = response
        .content
        .iter()
        .filter_map(|item| {
            if let AssistantContent::Text(text) = item {
                Some(text.text.clone())
            } else {
                None
            }
        })
        .collect::<Vec<String>>();

    let response_text = match response_text.first() {
        Some(value) => value.as_ref(),
        None => return Err(anyhow!("No response text found")),
    };

    let output = match args.pretty {
        true => markdown_to_ansi(response_text),
        false => response_text.to_string(),
    };

    print!("{}", output);

    Ok(())
}

/// Run a transient agent and stream assistant text and tool status.
async fn stream_prompt(
    args: &PromptArgs,
    settings: &Settings,
    output: &mut impl Write,
    diagnostics: &mut impl Write,
    render_markdown: bool,
    color_tools: bool,
) -> Result<()> {
    let overrides = hallucinate_agent::config::RuntimeOverrides::from(args);

    let model = hallucinate_agent::config::resolve_model(
        settings,
        overrides.model.as_ref(),
        overrides.base_url.as_ref(),
    )?;

    let provider = hallucinate_agent::config::create_provider(settings, &overrides, &model)?;
    let config = hallucinate_agent::config::agent_config(settings, &model, &overrides);
    let cwd = std::env::current_dir().context("Failed to determine working directory")?;

    let skills = hallucinate_agent::agent::skills::load_skills(
        &hallucinate_agent::agent::skills::SkillLoadOptions {
            cwd: cwd.clone(),
            skill_paths: args.skills.clone(),
            include_defaults: hallucinate_agent::config::resolve_enable_skills(
                settings,
                args.no_skills,
            ),
            skill_ids: settings.skills.available_skills.clone(),
        },
    );

    for diagnostic in &skills.diagnostics {
        eprintln!(
            "Warning: could not load skill {}: {}",
            hallucinate_agent::config::display_path(&diagnostic.path, &cwd),
            diagnostic.message
        );
    }

    let mut agent = Agent::new(provider, model, config)
        .with_cwd(cwd.to_string_lossy().into_owned())
        .with_skills(skills.skills);

    if let Some(prompt) = &args.system_prompt {
        agent = agent.with_system_prompt(prompt.clone());
    }

    let mut builtin_tools = settings.tools.builtin.clone();
    builtin_tools.shell.shell = hallucinate_agent::config::resolve_shell(settings, overrides.shell);
    agent.add_builtin_tools(builtin_tools.to_tool_list());
    agent.add_mcp_servers(&settings.tools.mcp_servers).await?;

    let script_tools = hallucinate_agent::agent::tools::script_tool::load_script_tools(&cwd);
    for diagnostic in script_tools.diagnostics {
        eprintln!(
            "Warning: could not load script tool {}: {}",
            hallucinate_agent::config::display_path(&diagnostic.path, &cwd),
            diagnostic.message
        );
    }

    for tool in script_tools.tools {
        if agent.add_tool_checked(Arc::new(tool)).is_err() {
            eprintln!(
                "Warning: could not add script tool because its ID conflicts with an existing tool"
            );
        }
    }
    agent.disable_tools();

    let images = futures::future::try_join_all(args.images.iter().map(|path| {
        hallucinate_agent::ai::images::load_image_file(
            path,
            settings.tools.builtin.read.auto_resize_images,
        )
    }))
    .await?;

    agent.load_history(vec![Message::User(UserMessage::with_images(
        &args.prompt,
        images,
    ))])?;

    stream_agent(
        &mut agent,
        output,
        diagnostics,
        render_markdown,
        color_tools,
    )
    .await
}

async fn stream_agent(
    agent: &mut Agent,
    output: &mut impl Write,
    diagnostics: &mut impl Write,
    render_markdown: bool,
    color_tools: bool,
) -> Result<()> {
    let mut presentation = StreamOutput::new(output, diagnostics, render_markdown, color_tools);
    let mut events = agent.subscribe();

    let run = agent.run();
    tokio::pin!(run);

    let result = loop {
        tokio::select! {
            event = events.recv() => {
                if let Some(event) = event {
                    presentation.handle(event)?;
                }
            },

            result = &mut run => break result,
        }
    };

    // The run can finish before the receiver has processed its final events.
    while let Ok(event) = events.try_recv() {
        presentation.handle(event)?;
    }
    presentation.finish()?;

    result?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use serde_json::json;

    use super::*;
    use crate::cli::{Cli, CliCommands};

    #[test]
    fn stream_options_are_parsed_and_pretty_is_rejected() {
        let cli = Cli::try_parse_from([
            "hallucinate",
            "prompt",
            "--stream",
            "--skill",
            "skill.md",
            "--no-skills",
            "hello",
        ])
        .unwrap();

        let Some(CliCommands::Prompt(args)) = cli.command else {
            panic!("expected prompt")
        };

        assert!(args.stream);
        assert!(args.no_skills);
        assert_eq!(args.skills, vec![std::path::PathBuf::from("skill.md")]);
        assert!(
            Cli::try_parse_from(["hallucinate", "prompt", "--stream", "--pretty", "hello"])
                .is_err()
        );
        assert!(
            Cli::try_parse_from(["hallucinate", "prompt", "--skill", "skill.md", "hello"]).is_err()
        );
    }

    #[tokio::test]
    async fn streaming_propagates_provider_errors() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/chat/completions")
            .with_status(400)
            .with_body("provider failed")
            .create_async()
            .await;

        let cli = Cli::try_parse_from([
            "hallucinate",
            "prompt",
            "--stream",
            "--model",
            "test",
            "--base-url",
            &server.url(),
            "--api-key",
            "test",
            "--no-skills",
            "hello",
        ])
        .unwrap();

        let Some(CliCommands::Prompt(args)) = cli.command else {
            panic!("expected prompt")
        };

        let mut settings = Settings::default();
        settings
            .providers
            .insert("openai".to_string(), Default::default());
        settings.max_retries = 0;

        let mut output = Vec::new();
        let mut diagnostics = Vec::new();
        assert!(
            stream_prompt(
                &args,
                &settings,
                &mut output,
                &mut diagnostics,
                false,
                false
            )
            .await
            .is_err()
        );
        assert!(output.is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn streaming_sends_prompt_skills_and_tool_results_without_reasoning() {
        let mut server = mockito::Server::new_async().await;

        let temp =
            std::env::temp_dir().join(format!("hallucinate-prompt-skill-{}", std::process::id()));
        std::fs::create_dir_all(&temp).unwrap();

        let skill = temp.join("SKILL.md");
        std::fs::write(
            &skill,
            "---\nname: prompt-test-skill\ndescription: Test streamed skills\n---\nInstructions",
        )
        .unwrap();

        let first_body = format!(
            "data: {}\n\ndata: [DONE]\n",
            json!({"choices":[{"delta":{"reasoning_content":"secret-thinking", "tool_calls":[{"index":0,"id":"call-1","type":"function","function":{"name":"shell","arguments":"{\"command\":\"printf '%s%s' tool- secret\"}"}}]},"finish_reason":"tool_calls"}]})
        );
        let second_body = format!(
            "data: {}\n\ndata: [DONE]\n",
            json!({"choices":[{"delta":{"content":"answer"},"finish_reason":"stop"}]})
        );

        let first = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("prompt-test-skill".to_string()),
                mockito::Matcher::Regex("project-marker".to_string()),
                mockito::Matcher::Regex("\\\"name\\\":\\\"shell\\\"".to_string()),
            ]))
            .with_status(200)
            .with_body(first_body)
            .expect(1)
            .create_async()
            .await;

        let second = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("tool-secret".to_string()))
            .with_status(200)
            .with_body(second_body)
            .expect(1)
            .create_async()
            .await;

        let cli = Cli::try_parse_from([
            "hallucinate",
            "prompt",
            "--stream",
            "--model",
            "test",
            "--base-url",
            &server.url(),
            "--api-key",
            "test",
            "--skill",
            skill.to_str().unwrap(),
            "--no-skills",
            "hello",
        ])
        .unwrap();

        let Some(CliCommands::Prompt(mut args)) = cli.command else {
            panic!("expected prompt")
        };
        args.system_prompt = Some("project-marker".to_string());

        let mut settings = Settings::default();
        settings
            .providers
            .insert("openai".to_string(), Default::default());

        let mut output = Vec::new();
        let mut diagnostics = Vec::new();

        stream_prompt(
            &args,
            &settings,
            &mut output,
            &mut diagnostics,
            false,
            false,
        )
        .await
        .unwrap();

        assert_eq!(output, b"answer");

        let diagnostics = String::from_utf8(diagnostics).unwrap();
        assert!(diagnostics.contains("[shell]\n"));
        assert!(diagnostics.contains("  Tool result: ✅"));
        assert!(diagnostics.contains("printf '%s%s' tool- secret"));
        assert!(!diagnostics.contains("tool-secret"));
        assert!(!diagnostics.contains("secret-thinking"));

        first.assert_async().await;
        second.assert_async().await;

        std::fs::remove_dir_all(temp).unwrap();
    }
}
