use anyhow::Result;
use clap::{Parser, ValueEnum};
use hallucinate_agent::{agent::tools::script_tool::ScriptToolDefinition, config::Settings};
use schemars::schema_for;

pub const CLI_NAME: &str = "hallucinate-agent-schemagen";

/// Schema generator for hallucinate.
#[derive(Parser, Debug, Clone)]
#[command(name = CLI_NAME, version, about)]
struct Cli {
    pub schema: SchemaName,
}

#[derive(ValueEnum, Debug, Clone)]
enum SchemaName {
    /// Settings schema
    #[value(name = "settings")]
    Settings,

    /// Script Tool schema
    #[value(name = "script-tool")]
    ScriptTool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let schema_value = match cli.schema {
        SchemaName::Settings => schema_for!(Settings),
        SchemaName::ScriptTool => schema_for!(ScriptToolDefinition),
    };

    let schema_value = serde_json::to_string_pretty(&schema_value)?;

    println!("{}", schema_value);

    Ok(())
}
