//! Model catalog and resolution helpers.
//!
//! Models are supplied by configured providers. The application intentionally
//! does not ship a built-in model catalog; provider and model selections come
//! from `settings.toml` or an explicit CLI selector for a configured provider.

use crate::ai::types::Model;

/// Return all built-in models.
///
/// This is intentionally empty. In particular, OpenAI models are not added to
/// the available model list implicitly; models must come from a configured
/// provider.
pub fn all_models() -> Vec<Model> {
    Vec::new()
}

/// Find a model by its ID.
///
/// Returns `None` if no model with the given ID is registered.
pub fn find_model(model_id: &str) -> Option<Model> {
    all_models().into_iter().find(|model| model.id == model_id)
}

/// Find a model by provider/model string (e.g. `"openai/gpt-4o"`).
///
/// If the string contains a `/`, it is split into provider and model ID.
/// Otherwise, it is treated as a bare model ID.
pub fn find_model_by_provider(provider_model: &str) -> Option<Model> {
    if let Some((provider, model_id)) = provider_model.split_once('/') {
        all_models()
            .into_iter()
            .find(|model| model.id == model_id && model.provider == provider)
    } else {
        find_model(provider_model)
    }
}

/// Resolve a model string to a `Model`.
///
/// Delegates to [`find_model_by_provider`].
pub fn resolve_model(model_str: &str) -> Option<Model> {
    find_model_by_provider(model_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_no_built_in_models() {
        assert!(all_models().is_empty());
        assert!(find_model("gpt-4o").is_none());
        assert!(find_model_by_provider("openai/gpt-4o").is_none());
        assert!(resolve_model("gpt-4o").is_none());
    }
}
