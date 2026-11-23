//! The real [`LlmClient`]: the configured providers, routed by the provider half of a
//! [`ModelRef`].

use base::cancel::CancelToken;
use model::ModelRef;

use crate::config::{Config, ProviderConfig, ProviderKind};
use crate::{BlobSource, ChatRequest, LlmClient, LlmError, ProviderInfo, StreamEvent, anthropic, openai};

/// Every configured provider, ready to stream.
#[derive(Clone, Debug)]
pub struct Providers {
    config: Config,
}

impl Providers {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// The configuration this client was built from.
    pub fn config(&self) -> &Config {
        &self.config
    }

    fn provider(&self, id: &str) -> Result<&ProviderConfig, LlmError> {
        self.config.provider(id).ok_or_else(|| LlmError::UnknownModel(id.to_string()))
    }
}

impl LlmClient for Providers {
    fn stream(
        &self,
        model: &ModelRef,
        request: &ChatRequest,
        blobs: &dyn BlobSource,
        cancel: &CancelToken,
        on_event: &mut dyn FnMut(StreamEvent),
    ) -> Result<(), LlmError> {
        let provider = self.config.provider(&model.provider).filter(|_| !model.is_empty());
        let Some(provider) = provider else {
            return Err(LlmError::UnknownModel(model.to_string()));
        };
        if cancel.is_cancelled() {
            return Err(LlmError::Cancelled);
        }
        match provider.kind {
            ProviderKind::Anthropic => anthropic::stream(provider, model, request, blobs, cancel, on_event),
            ProviderKind::OpenAi => openai::stream(provider, model, request, blobs, cancel, on_event),
        }
    }

    fn list_models(&self, provider: &str) -> Result<Vec<String>, LlmError> {
        let provider = self.provider(provider)?;
        match provider.kind {
            ProviderKind::Anthropic => anthropic::list_models(provider),
            ProviderKind::OpenAi => openai::list_models(provider),
        }
    }

    fn providers(&self) -> Vec<ProviderInfo> {
        self.config
            .providers
            .iter()
            .map(|p| ProviderInfo {
                id: p.id.clone(),
                kind: p.kind.as_str().to_string(),
                base_url: p.base_url.clone(),
                models: p.models.clone(),
                has_key: p.api_key.is_some(),
            })
            .collect()
    }

    /// The file's `default_model`, else the first model named in the file.
    fn default_model(&self) -> Option<ModelRef> {
        self.config.default_model.clone().or_else(|| {
            self.config
                .providers
                .iter()
                .find_map(|p| p.models.first().map(|m| ModelRef::new(p.id.clone(), m.clone())))
        })
    }
}

#[cfg(test)]
mod tests;
