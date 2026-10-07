//! User-defined script tools loaded from TOML files.

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{
    limits::ToolLimits,
    types::{Tool, ToolOutputSink, ToolResult},
};
use crate::{agent::tools::shell_exec::run_shell_command, config::paths};

/// A definition file which could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptToolDiagnostic {
    /// Definition file path.
    pub path: PathBuf,

    /// Reason the definition was rejected.
    pub message: String,
}

/// Result of discovering script tool definition files.
#[derive(Debug, Default)]
pub struct ScriptToolLoadResult {
    /// Successfully loaded tools.
    pub tools: Vec<ScriptTool>,

    /// Invalid definitions which were skipped.
    pub diagnostics: Vec<ScriptToolDiagnostic>,
}

#[cfg_attr(not(feature = "schemas"), derive(Debug, Deserialize))]
#[cfg_attr(feature = "schemas", derive(Debug, Deserialize, schemars::JsonSchema))]
/// A parsed TOML definition for a script tool.
pub struct ScriptToolDefinition {
    /// Shell command to execute.
    command: String,

    /// Human-readable description of what the tool does.
    description: String,

    #[serde(default)]
    /// Optional shell override (e.g. `["bash", "-c"]`).
    shell: Option<Vec<String>>,

    #[serde(default)]
    /// Maximum execution time in seconds. Defaults to the configured shell
    /// timeout.
    timeout: Option<u64>,

    #[serde(default)]
    /// Named parameters accepted by the tool.
    parameters: BTreeMap<String, ScriptToolParameter>,
}

#[cfg_attr(not(feature = "schemas"), derive(Debug, Deserialize))]
#[cfg_attr(feature = "schemas", derive(Debug, Deserialize, schemars::JsonSchema))]
/// A single parameter definition for a script tool.
pub struct ScriptToolParameter {
    /// Description of the parameter.
    description: String,

    #[serde(default = "default_input_type")]
    /// JSON schema type for the parameter (e.g. "string", "int", "bool").
    input_type: String,

    #[serde(default)]
    /// Whether this parameter is required.
    required: bool,

    #[serde(default)]
    /// Optional constraints enforced for values supplied to this parameter.
    constraints: Option<ScriptToolParameterConstraints>,
}

#[cfg_attr(not(feature = "schemas"), derive(Debug, Serialize, Deserialize))]
#[cfg_attr(
    feature = "schemas",
    derive(Debug, Deserialize, Serialize, schemars::JsonSchema)
)]
/// Value constraints for a script tool parameter.
pub struct ScriptToolParameterConstraints {
    /// Values accepted by the parameter.
    #[serde(default, rename = "enum")]
    enum_values: Option<Vec<Value>>,

    /// Inclusive lower bound for numeric parameters.
    #[serde(default)]
    minimum: Option<f64>,

    /// Inclusive upper bound for numeric parameters.
    #[serde(default)]
    maximum: Option<f64>,
}

/// A model-invocable tool backed by a user-defined shell command.
#[derive(Debug)]
pub struct ScriptTool {
    /// Unique identifier for the tool.
    name: String,

    /// Human-readable description of what the tool does.
    description: String,

    /// Shell command to execute.
    command: String,

    /// Optional shell override (e.g. `["bash", "-c"]`).
    shell: Option<Vec<String>>,

    /// Named parameters accepted by the tool.
    parameters: BTreeMap<String, ScriptToolParameter>,

    /// Maximum execution time in seconds.
    timeout: u64,

    /// Output size and resource limits.
    limits: ToolLimits,
}

impl ScriptTool {
    /// Parse a `ScriptTool` from its TOML definition, validating all fields.
    fn from_definition(
        name: String,
        definition: ScriptToolDefinition,
        _definition_path: &Path,
        timeout: u64,
    ) -> Result<Self, anyhow::Error> {
        if !valid_name(&name, true) {
            anyhow::bail!("tool ID must contain only letters, numbers, underscores, or hyphens");
        }

        if definition.command.trim().is_empty() {
            anyhow::bail!("command must contain a non-empty executable");
        }

        if definition.description.trim().is_empty() {
            anyhow::bail!("description cannot be empty");
        }

        if definition
            .shell
            .as_ref()
            .is_some_and(|shell| shell.first().is_none_or(|value| value.trim().is_empty()))
        {
            anyhow::bail!("shell must contain a non-empty executable when configured");
        }

        if definition.timeout.is_some_and(|timeout| timeout == 0) {
            anyhow::bail!("timeout must be greater than zero when configured");
        }

        for (name, parameter) in &definition.parameters {
            if !valid_name(name, false) {
                anyhow::bail!(
                    "parameter '{name}' must contain only letters, numbers, or underscores"
                );
            }

            if parameter.description.trim().is_empty() {
                anyhow::bail!("parameter '{name}' has an empty description");
            }

            if json_schema_type(&parameter.input_type).is_none() {
                anyhow::bail!(
                    "parameter '{name}' has unsupported input_type '{}'",
                    parameter.input_type
                );
            }

            validate_parameter_constraints(name, parameter)?;
        }

        Ok(Self {
            name,
            description: definition.description,
            command: definition.command,
            shell: definition.shell,
            parameters: definition.parameters,
            timeout: definition.timeout.unwrap_or(timeout),
            limits: ToolLimits::default(),
        })
    }

    /// Render the command with template parameters substituted from arguments.
    fn render_command(
        &self,
        arguments: &Map<String, Value>,
    ) -> Result<String, anyhow::Error> {
        for (name, parameter) in &self.parameters {
            match arguments.get(name) {
                Some(value) if value_matches_type(value, &parameter.input_type) => {
                    if !value_matches_constraints(value, parameter.constraints.as_ref()) {
                        anyhow::bail!("parameter '{name}' does not satisfy its constraints");
                    }
                },

                Some(_) => anyhow::bail!("parameter '{name}' must be a {}", parameter.input_type),

                None if parameter.required => anyhow::bail!("Missing required field: {name}"),

                None => {},
            }
        }

        Ok(minijinja::Environment::new().render_str(&self.command.trim(), arguments)?)
    }

    /// Execute the script tool with the given arguments.
    async fn execute_inner(
        &self,
        arguments: &Value,
        cwd: Option<&str>,
        output: Option<ToolOutputSink>,
    ) -> Result<ToolResult, anyhow::Error> {
        let arguments = arguments
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("tool arguments must be an object"))?;

        let command = self.render_command(arguments)?;

        let execution_directory = cwd
            .map(PathBuf::from)
            .map(Ok)
            .unwrap_or_else(std::env::current_dir)
            .map_err(|error| {
                anyhow::anyhow!("failed to determine the current directory: {error}")
            })?;

        let command_output = match run_shell_command(
            self.shell.as_deref(),
            &command,
            &execution_directory,
            self.timeout,
            self.limits.max_tool_result_bytes,
            output,
        )
        .await
        {
            Ok(value) => value,
            Err(error) => return Ok(ToolResult::error("", &self.name, error.to_string())),
        };

        Ok(if command_output.success {
            ToolResult::success("", &self.name, command_output.content)
        } else {
            ToolResult::error("", &self.name, command_output.content)
        })
    }
}

impl Tool for ScriptTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> Value {
        let mut properties = Map::new();

        let required = self
            .parameters
            .iter()
            .filter(|(_, parameter)| parameter.required)
            .map(|(name, _)| Value::String(name.clone()))
            .collect::<Vec<_>>();

        for (name, parameter) in &self.parameters {
            let mut property = Map::from_iter([
                (
                    "type".to_string(),
                    json!(json_schema_type(&parameter.input_type).expect("validated type")),
                ),
                (
                    "description".to_string(),
                    Value::String(parameter.description.clone()),
                ),
            ]);

            if let Some(constraints) = &parameter.constraints {
                if let Some(enum_values) = &constraints.enum_values {
                    property.insert("enum".to_string(), json!(enum_values));
                }

                if let Some(minimum) = constraints.minimum {
                    property.insert("minimum".to_string(), json!(minimum));
                }

                if let Some(maximum) = constraints.maximum {
                    property.insert("maximum".to_string(), json!(maximum));
                }
            }

            properties.insert(name.clone(), Value::Object(property));
        }

        let mut schema = Map::from_iter([
            ("type".to_string(), json!("object")),
            ("properties".to_string(), Value::Object(properties)),
        ]);

        if !required.is_empty() {
            schema.insert("required".to_string(), Value::Array(required));
        }

        Value::Object(schema)
    }

    fn execute<'a>(
        &'a self,
        arguments: &'a Value,
        cwd: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, anyhow::Error>> + Send + 'a>> {
        Box::pin(self.execute_inner(arguments, cwd, None))
    }

    fn execute_with_output<'a>(
        &'a self,
        arguments: &'a Value,
        cwd: Option<&'a str>,
        output: Option<ToolOutputSink>,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, anyhow::Error>> + Send + 'a>> {
        Box::pin(self.execute_inner(arguments, cwd, output))
    }
}

/// Load global tools first and project-local tools second. Project definitions
/// replace global definitions with the same ID.
pub fn load_script_tools(
    cwd: impl AsRef<Path>,
    timeout: u64,
) -> ScriptToolLoadResult {
    let mut result = ScriptToolLoadResult::default();

    let mut directories = paths::script_tools_dir().into_iter().collect::<Vec<_>>();
    directories.push(paths::project_script_tools_dir(cwd));

    let mut positions = HashMap::new();

    for directory in directories {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };

        let mut files = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
            })
            .collect::<Vec<_>>();
        files.sort();

        for path in files {
            let Some(name) = path
                .file_stem()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
            else {
                result.diagnostics.push(ScriptToolDiagnostic {
                    path,
                    message: "tool file name is not valid UTF-8".to_string(),
                });

                continue;
            };

            let loaded = fs::read_to_string(&path)
                .map_err(anyhow::Error::from)
                .and_then(|content| toml::from_str(&content).map_err(anyhow::Error::from))
                .and_then(|definition| {
                    ScriptTool::from_definition(name.clone(), definition, &path, timeout)
                });

            match loaded {
                Ok(tool) => {
                    if let Some(position) = positions.get(&name).copied() {
                        result.tools[position] = tool;
                    } else {
                        positions.insert(name, result.tools.len());
                        result.tools.push(tool);
                    }
                },

                Err(error) => result.diagnostics.push(ScriptToolDiagnostic {
                    path,
                    message: error.to_string(),
                }),
            }
        }
    }

    result
}

/// Return the default input type for script tool parameters.
fn default_input_type() -> String {
    "string".to_string()
}

/// Validate that a name contains only allowed characters.
fn valid_name(
    name: &str,
    allow_hyphen: bool,
) -> bool {
    !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || character == '_'
                || (allow_hyphen && character == '-')
        })
}

/// Map an input type string to its JSON schema type, if supported.
fn json_schema_type(input_type: &str) -> Option<&'static str> {
    match input_type {
        "string" => Some("string"),
        "int" | "integer" => Some("integer"),
        "float" | "number" => Some("number"),
        "bool" | "boolean" => Some("boolean"),
        "array" => Some("array"),
        "object" => Some("object"),
        _ => None,
    }
}

/// Check whether a JSON value matches the given input type.
fn value_matches_type(
    value: &Value,
    input_type: &str,
) -> bool {
    match json_schema_type(input_type) {
        Some("string") => value.is_string(),
        Some("integer") => value.as_i64().is_some() || value.as_u64().is_some(),
        Some("number") => value.is_number(),
        Some("boolean") => value.is_boolean(),
        Some("array") => value.is_array(),
        Some("object") => value.is_object(),
        _ => false,
    }
}

/// Validate constraints declared for a parameter definition.
fn validate_parameter_constraints(
    name: &str,
    parameter: &ScriptToolParameter,
) -> Result<(), anyhow::Error> {
    let Some(constraints) = &parameter.constraints else {
        return Ok(());
    };

    if constraints.enum_values.as_ref().is_some_and(Vec::is_empty) {
        anyhow::bail!("parameter '{name}' constraint enum cannot be empty");
    }

    if let Some(enum_values) = &constraints.enum_values {
        if enum_values
            .iter()
            .any(|value| !value_matches_type(value, &parameter.input_type))
        {
            anyhow::bail!(
                "parameter '{name}' constraint enum values must match input_type '{}'",
                parameter.input_type
            );
        }
    }

    let has_range = constraints.minimum.is_some() || constraints.maximum.is_some();
    if has_range
        && !matches!(
            json_schema_type(&parameter.input_type),
            Some("integer" | "number")
        )
    {
        anyhow::bail!("parameter '{name}' minimum and maximum require a numeric input_type");
    }

    if constraints.minimum.is_some_and(|value| !value.is_finite())
        || constraints.maximum.is_some_and(|value| !value.is_finite())
    {
        anyhow::bail!("parameter '{name}' minimum and maximum must be finite numbers");
    }

    if constraints
        .minimum
        .zip(constraints.maximum)
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        anyhow::bail!("parameter '{name}' minimum cannot exceed maximum");
    }

    Ok(())
}

/// Check whether a JSON value satisfies its declared constraints.
fn value_matches_constraints(
    value: &Value,
    constraints: Option<&ScriptToolParameterConstraints>,
) -> bool {
    let Some(constraints) = constraints else {
        return true;
    };

    if constraints
        .enum_values
        .as_ref()
        .is_some_and(|values| !values.contains(value))
    {
        return false;
    }

    let Some(number) = value.as_f64() else {
        return true;
    };

    constraints.minimum.is_none_or(|minimum| number >= minimum)
        && constraints.maximum.is_none_or(|maximum| number <= maximum)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn schema_and_template_use_declared_parameters() {
        let definition: ScriptToolDefinition = toml::from_str("command = \"\"\"\necho \"{{ name }}\"\n\"\"\"\ndescription = \"test\"\ntimeout = 30\n[parameters.name]\ndescription = \"Name\"\nrequired = true").unwrap();

        let tool = ScriptTool::from_definition(
            "example".to_string(),
            definition,
            Path::new("/tmp/example.toml"),
            60,
        )
        .unwrap();

        assert_eq!(tool.timeout, 30);
        assert_eq!(tool.parameters()["required"], json!(["name"]));
        assert_eq!(
            tool.render_command(&Map::from_iter([(String::from("name"), json!("Ada"))]))
                .unwrap(),
            "echo \"Ada\""
        );
    }

    #[test]
    fn includes_and_enforces_parameter_constraints() {
        let definition: ScriptToolDefinition = toml::from_str(
            "command = \"\"\"echo \"{{ format }} {{ quality }}\"\"\"\"\n\
             description = \"test\"\n\
             [parameters.format]\n\
             description = \"Format\"\n\
             [parameters.format.constraints]\n\
             enum = [\"json\", \"text\"]\n\
             [parameters.quality]\n\
             description = \"Quality\"\n\
             input_type = \"integer\"\n\
             [parameters.quality.constraints]\n\
             minimum = 1\n\
             maximum = 100",
        )
        .unwrap();

        let tool = ScriptTool::from_definition(
            "example".to_string(),
            definition,
            Path::new("/tmp/example.toml"),
            60,
        )
        .unwrap();

        assert_eq!(
            tool.parameters()["properties"]["format"]["enum"],
            json!(["json", "text"])
        );
        assert_eq!(tool.parameters()["properties"]["quality"]["minimum"], 1.0);
        assert_eq!(tool.parameters()["properties"]["quality"]["maximum"], 100.0);
        assert!(
            tool.render_command(&Map::from_iter([
                (String::from("format"), json!("json")),
                (String::from("quality"), json!(80)),
            ]))
            .is_ok()
        );
        assert!(
            tool.render_command(&Map::from_iter([
                (String::from("format"), json!("xml")),
                (String::from("quality"), json!(80)),
            ]))
            .unwrap_err()
            .to_string()
            .contains("format")
        );
        assert!(
            tool.render_command(&Map::from_iter([
                (String::from("format"), json!("json")),
                (String::from("quality"), json!(101)),
            ]))
            .unwrap_err()
            .to_string()
            .contains("quality")
        );
    }

    #[test]
    fn rejects_constraints_incompatible_with_parameter_type() {
        let definition: ScriptToolDefinition = toml::from_str(
            "command = \"echo\"\ndescription = \"test\"\n\
             [parameters.enabled]\ndescription = \"Enabled\"\ninput_type = \"boolean\"\n\
             [parameters.enabled.constraints]\nminimum = 0",
        )
        .unwrap();

        let error = ScriptTool::from_definition(
            "example".to_string(),
            definition,
            Path::new("/tmp/example.toml"),
            60,
        )
        .unwrap_err();

        assert!(error.to_string().contains("require a numeric input_type"));
    }

    #[test]
    fn rejects_a_zero_timeout() {
        let definition: ScriptToolDefinition =
            toml::from_str("command = \"echo\"\ndescription = \"test\"\ntimeout = 0").unwrap();

        let error = ScriptTool::from_definition(
            "example".to_string(),
            definition,
            Path::new("/tmp/example.toml"),
            60,
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("timeout must be greater than zero")
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn streams_stdout_before_the_script_finishes() {
        let definition: ScriptToolDefinition = toml::from_str(
            "command = \"\"\"sh -c \"printf first; sleep 1; printf second\"\"\"\"\n\
             description = \"Stream stdout\"",
        )
        .unwrap();

        let tool = ScriptTool::from_definition(
            "stream-stdout".to_string(),
            definition,
            Path::new("/tmp/script-tools/stream-stdout.toml"),
            60,
        )
        .unwrap();

        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();

        let output = ToolOutputSink::new(sender, "call-1", "stream-stdout");

        let arguments = json!({});

        let execution = tool.execute_with_output(&arguments, None, Some(output));
        tokio::pin!(execution);

        let first_output = tokio::select! {
            output = receiver.recv() => output.expect("output channel should remain open"),

            result = &mut execution => panic!("script finished before emitting output: {result:?}"),

            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                panic!("stdout should arrive before the script completes");
            },
        };

        assert_eq!(first_output.tool_call_id, "call-1");
        assert_eq!(first_output.output, "first");
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut execution)
                .await
                .is_err()
        );

        let result = execution.await.unwrap();
        assert_eq!(result.content, "firstsecond");
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn timeout_kills_spawned_commands() {
        let marker = std::env::temp_dir().join(format!("script-timeout-{}", uuid::Uuid::new_v4()));

        let definition: ScriptToolDefinition = toml::from_str(&format!(
            "command = \"(sleep 2; printf orphan > '{}') & wait\"\ndescription = \"test\"\ntimeout = 1",
            marker.display()
        ))
        .unwrap();

        let tool = ScriptTool::from_definition(
            "timeout-test".to_string(),
            definition,
            Path::new("/tmp/timeout-test.toml"),
            60,
        )
        .unwrap();

        let result = tool.execute(&json!({}), None).await.unwrap();

        assert!(result.content.contains("timed out"));

        tokio::time::sleep(Duration::from_millis(1300)).await;
        assert!(!marker.exists(), "background script survived timeout");
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn dropping_execution_kills_spawned_commands() {
        let marker = std::env::temp_dir().join(format!("script-cancel-{}", uuid::Uuid::new_v4()));

        let definition: ScriptToolDefinition = toml::from_str(&format!(
            "command = \"(sleep 1; printf orphan > '{}') & printf ready; wait\"\ndescription = \"test\"",
            marker.display()
        ))
        .unwrap();

        let tool = ScriptTool::from_definition(
            "cancel-test".to_string(),
            definition,
            Path::new("/tmp/cancel-test.toml"),
            60,
        )
        .unwrap();

        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let output = ToolOutputSink::new(sender, "call-1", "cancel-test");
        let arguments = json!({});

        let mut execution = Box::pin(tool.execute_with_output(&arguments, None, Some(output)));

        tokio::select! {
            output = receiver.recv() => assert_eq!(output.unwrap().output, "ready"),
            result = &mut execution => panic!("script finished early: {result:?}"),
        }

        drop(execution);

        tokio::time::sleep(Duration::from_millis(1300)).await;
        assert!(!marker.exists(), "background script survived cancellation");
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn executes_in_the_current_directory_by_default() {
        let definition: ScriptToolDefinition =
            toml::from_str("command = \"pwd\"\ndescription = \"Print the working directory\"")
                .unwrap();
        let tool = ScriptTool::from_definition(
            "print-directory".to_string(),
            definition,
            Path::new("/tmp/script-tools/print-directory.toml"),
            60,
        )
        .unwrap();

        let result = tool.execute(&json!({}), None).await.unwrap();
        let expected_directory = std::env::current_dir().unwrap();

        assert_eq!(result.content.trim(), expected_directory.to_string_lossy());
    }
}
