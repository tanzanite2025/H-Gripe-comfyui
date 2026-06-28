use crate::model::{ApiResult, ApiTask};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BrokerError {
    #[error("provider '{0}' is not registered")]
    ProviderNotFound(String),
    #[error("provider '{provider}' does not support operation '{operation}'")]
    UnsupportedOperation { provider: String, operation: String },
    #[error("provider error: {0}")]
    Provider(String),
    #[error("task failed after {attempts} attempt(s): {message}")]
    RetryExhausted { attempts: u32, message: String },
}

pub type BrokerResult<T> = Result<T, BrokerError>;

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &'static str;
    fn supports(&self, operation: &str) -> bool;
    async fn execute(&self, task: &ApiTask) -> BrokerResult<ApiResult>;
}

#[derive(Default)]
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn Provider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<P>(&mut self, provider: P)
    where
        P: Provider + 'static,
    {
        self.providers
            .insert(provider.name().to_string(), Arc::new(provider));
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Provider>> {
        self.providers.get(name).cloned()
    }

    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.providers.keys().cloned().collect();
        names.sort();
        names
    }
}
