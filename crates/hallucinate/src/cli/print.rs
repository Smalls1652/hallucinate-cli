//! Print mode: single-shot prompt/response.

use anyhow::{Result, anyhow};
use clap::{ArgAction, Args};
use hallucinate_agent::{
    ai::types::{AssistantContent, Context, Message, ReasoningEffort, StreamOptions, UserMessage},
    config::settings::Settings,
};
use hallucinate_tui::components::markdown_to_ansi;

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

    /// Pretty print the output
    #[arg(long = "pretty")]
    pub pretty: bool,

    /// The prompt to send to the LLM.
    pub prompt: String,
}

impl From<&PrintArgs> for hallucinate_agent::config::RuntimeOverrides {
    fn from(args: &PrintArgs) -> Self {
        Self {
            model: args.model.clone(),
            api_key: args.api_key.clone(),
            base_url: args.base_url.clone(),
            shell: None,
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

/// Run print mode with the given CLI arguments and settings.
pub async fn print_command(args: &PrintArgs) -> Result<()> {
    let mut settings = Settings::load()?;
    settings.enable_tools = false;

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
            settings.builtin_tools.read.auto_resize_images,
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
