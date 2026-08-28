//! Catalog and resolution for models supported by the OpenAI-compatible API.

use crate::ai::types::{Api, Model};

/// Return the built-in OpenAI model metadata.
pub fn openai_models() -> Vec<Model> {
    vec![
        Model::new(
            "gpt-4o",
            "GPT-4o",
            Api::OpenaiCompletions,
            "openai",
            "https://api.openai.com/v1",
            false,
            128_000,
            16_384,
        ),
        Model::new(
            "gpt-4o-mini",
            "GPT-4o Mini",
            Api::OpenaiCompletions,
            "openai",
            "https://api.openai.com/v1",
            false,
            128_000,
            16_384,
        ),
        Model::new(
            "o1",
            "o1",
            Api::OpenaiCompletions,
            "openai",
            "https://api.openai.com/v1",
            true,
            200_000,
            100_000,
        ),
        Model::new(
            "o1-mini",
            "o1 Mini",
            Api::OpenaiCompletions,
            "openai",
            "https://api.openai.com/v1",
            true,
            128_000,
            65_536,
        ),
        Model::new(
            "o3-mini",
            "o3 Mini",
            Api::OpenaiCompletions,
            "openai",
            "https://api.openai.com/v1",
            true,
            200_000,
            100_000,
        ),
    ]
}

/// Return all built-in models. Custom OpenAI-compatible model IDs are accepted
/// by the CLI even when they are not in this small convenience catalog.
pub fn all_models() -> Vec<Model> {
    openai_models()
}

pub fn find_model(model_id: &str) -> Option<Model> {
    all_models().into_iter().find(|model| model.id == model_id)
}

pub fn find_model_by_provider(provider_model: &str) -> Option<Model> {
    if let Some((provider, model_id)) = provider_model.split_once('/') {
        all_models()
            .into_iter()
            .find(|model| model.id == model_id && model.provider == provider)
    } else {
        find_model(provider_model)
    }
}

pub fn resolve_model(model_str: &str) -> Option<Model> {
    find_model_by_provider(model_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_contains_openai_models() {
        let models = openai_models();
        assert!(models.iter().any(|model| model.id == "gpt-4o"));
        assert!(models.iter().any(|model| model.id == "gpt-4o-mini"));
        assert_eq!(all_models().len(), models.len());
    }

    #[test]
    fn model_resolution_supports_ids_and_provider_prefixes() {
        assert_eq!(find_model("gpt-4o").unwrap().name, "GPT-4o");
        assert_eq!(
            find_model_by_provider("openai/gpt-4o").unwrap().provider,
            "openai"
        );
        assert!(find_model("claude-sonnet-4").is_none());
        assert!(find_model_by_provider("openai/missing").is_none());
    }
}
