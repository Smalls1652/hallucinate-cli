//! Interactive mode and the terminal application lifecycle.
//!
//! Input handling, command dispatch, session synchronization, and rendering
//! live in focused child modules so this module only owns shared state and
//! terminal setup.

use anyhow::{Context, Result};
use hallucinate_agent::{agent::runtime::Agent, config::Settings};
use hallucinate_session::SessionManager;
use hallucinate_tui::interactive::InteractiveTui;

use crate::cli::Cli;

/// Run interactive mode.
pub async fn interactive_command(cli: &Cli) -> Result<()> {
    let settings = &Settings::load()?;

    // Ensure directories exist
    hallucinate_agent::config::ensure_directories()?;
    let discarded_sessions = SessionManager::discard_historical_empty_sessions()?;
    if discarded_sessions > 0 {
        tracing::debug!(discarded_sessions, "discarded historical empty sessions");
    }

    let runtime_overrides = hallucinate_agent::config::RuntimeOverrides::from(cli);

    let model = hallucinate_agent::config::resolve_model(
        settings,
        runtime_overrides.model.as_ref(),
        runtime_overrides.base_url.as_ref(),
    )?;

    let provider =
        hallucinate_agent::config::create_provider(settings, &runtime_overrides, &model)?;

    let agent_config =
        hallucinate_agent::config::agent_config(settings, &model, &runtime_overrides);

    // Discover skills before creating the agent so their metadata can be
    // included in the system prompt. Invalid skill files do not prevent the
    // session from starting.
    let cwd = std::env::current_dir().context("Failed to determine working directory")?;

    let skills = hallucinate_agent::agent::skills::load_skills(
        &hallucinate_agent::agent::skills::SkillLoadOptions {
            cwd: cwd.clone(),
            skill_paths: cli.skills.clone(),
            include_defaults: !cli.no_skills,
        },
    );

    for diagnostic in &skills.diagnostics {
        eprintln!(
            "Warning: could not load skill {}: {}",
            hallucinate_agent::config::display_path(&diagnostic.path, &cwd),
            diagnostic.message
        );
    }

    let prompt_template_options =
        hallucinate_agent::agent::prompt_templates::PromptTemplateLoadOptions {
            cwd: cwd.clone(),
            prompt_paths: cli.prompt_templates.clone(),
            include_defaults: !cli.no_prompt_templates,
        };

    let prompt_templates =
        hallucinate_agent::agent::prompt_templates::load_prompt_templates(&prompt_template_options);

    for diagnostic in &prompt_templates.diagnostics {
        eprintln!(
            "Warning: could not load prompt template {}: {}",
            hallucinate_agent::config::display_path(&diagnostic.path, &cwd),
            diagnostic.message
        );
    }

    // Create agent with all built-in tools.
    let loaded_skills = skills.skills;

    let mut agent = Agent::new(provider, model.clone(), agent_config)
        .with_cwd(cwd.to_string_lossy().into_owned())
        .with_skills(loaded_skills.clone());

    // Load built-in tools.
    agent.add_builtin_tools(settings.tools.builtin.to_tool_list());

    // Load MCP servers.
    agent.add_mcp_servers(&settings.tools.mcp_servers).await?;

    // Load in any script tools the user has defined.
    let script_tools = hallucinate_agent::agent::tools::script_tool::load_script_tools(
        &cwd,
        hallucinate_agent::config::resolve_shell_timeout(settings, None),
    );

    for diagnostic in script_tools.diagnostics {
        eprintln!(
            "Warning: could not load script tool {}: {}",
            hallucinate_agent::config::display_path(&diagnostic.path, &cwd),
            diagnostic.message
        );
    }

    for tool in script_tools.tools {
        if agent.add_tool_checked(std::sync::Arc::new(tool)).is_err() {
            eprintln!(
                "Warning: could not add script tool because its ID conflicts with an existing tool"
            );
        }
    }

    agent.disable_tools()?;

    let loaded_tools = agent.list_tools();

    // Apply prompt overrides. Provider credentials and endpoint are resolved
    // when `create_provider` constructs the provider.
    if let Some(system_prompt) = &cli.system_prompt {
        agent = agent.with_system_prompt(system_prompt.clone());
    }

    // Every interactive run has a concrete session before the UI starts, so
    // user and assistant messages can never be silently discarded. `--session`
    // restores validated history and the persisted cwd when it still exists.
    let mut session_manager = SessionManager::new();

    let is_new_session = cli.session.is_none();

    if let Some(session_id) = &cli.session {
        let session = session_manager.resume(session_id)?;
        let storage = session_manager
            .current_storage()
            .expect("resumed session has storage");
        let messages = storage.load_messages()?;
        let checkpoint = storage.load_compaction_checkpoint()?;

        agent.load_history(messages)?;
        if let Some((summary, archived_message_count)) = checkpoint {
            agent.restore_compaction_checkpoint(summary, archived_message_count)?;
        }
        agent.set_cwd(session.cwd);
    } else {
        session_manager.create(hallucinate_session::manager::SessionCreateOptions {
            title: None,
            parent_session_id: None,
            cwd: Some(cwd.to_string_lossy().into_owned()),
        })?;
    }

    let token_count = session_manager
        .current_storage()
        .expect("created or resumed session has storage")
        .load_latest_context_token_count()?
        .unwrap_or_default();

    // Subscribe to agent events
    let event_rx = agent.subscribe();

    // Create TUI
    let mut models = hallucinate_agent::config::available_models(settings);
    if !model.hidden
        && !models
            .iter()
            .any(|candidate| candidate.provider == model.provider && candidate.id == model.id)
    {
        models.push(model.clone());
        models.sort_by(|left, right| {
            left.provider
                .cmp(&right.provider)
                .then_with(|| left.id.cmp(&right.id))
        });
    }

    let mut tui = InteractiveTui::new(
        agent,
        event_rx,
        session_manager,
        prompt_templates.templates,
        prompt_template_options,
        loaded_skills,
        loaded_tools,
        runtime_overrides,
        cli.thinking,
        settings.clone(),
        model,
        models,
        settings.session_title.clone(),
        is_new_session,
        token_count,
    )?;

    // Run the TUI
    tui.run().await
}
