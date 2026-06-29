use crate::model::{ApiResult, ApiStatus, ApiTask};
use crate::provider::{
    BrokerError, BrokerResult, Provider, ProviderExecutionContext, ProviderRegistry,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tokio::time::Duration;

#[derive(Default)]
pub struct ApiBroker {
    registry: ProviderRegistry,
    cache: Arc<Mutex<HashMap<String, ApiResult>>>,
}

impl ApiBroker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_registry(registry: ProviderRegistry) -> Self {
        Self {
            registry,
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn register_provider<P>(&mut self, provider: P)
    where
        P: Provider + 'static,
    {
        self.registry.register(provider);
    }

    pub fn providers(&self) -> Vec<String> {
        self.registry.names()
    }

    pub async fn execute(&self, task: ApiTask) -> BrokerResult<ApiResult> {
        self.execute_with_context(task, ProviderExecutionContext::default())
            .await
    }

    pub async fn execute_with_context(
        &self,
        task: ApiTask,
        context: ProviderExecutionContext,
    ) -> BrokerResult<ApiResult> {
        context.check_cancelled()?;
        let cache_key = self.cache_key(&task)?;
        if task.cache_policy.enabled {
            if let Some(mut cached) = self.cache.lock().await.get(&cache_key).cloned() {
                cached.id = task.id.clone();
                cached.status = ApiStatus::Cached;
                cached.cache_hit = true;
                return Ok(cached);
            }
        }

        let provider = self
            .registry
            .get(&task.provider)
            .ok_or_else(|| BrokerError::ProviderNotFound(task.provider.clone()))?;

        if !provider.supports(&task.operation) {
            return Err(BrokerError::UnsupportedOperation {
                provider: task.provider.clone(),
                operation: task.operation.clone(),
            });
        }

        let started = Instant::now();
        let max_attempts = task.retry_policy.max_attempts.max(1);
        let mut last_error: Option<BrokerError> = None;

        for attempt in 1..=max_attempts {
            context.check_cancelled()?;
            match provider.execute_with_context(&task, &context).await {
                Ok(mut result) => {
                    result.duration_ms = started.elapsed().as_millis();
                    if task.cache_policy.enabled && result.status == ApiStatus::Succeeded {
                        self.cache.lock().await.insert(cache_key, result.clone());
                    }
                    return Ok(result);
                }
                Err(err) => {
                    if matches!(err, BrokerError::Cancelled) {
                        return Err(err);
                    }
                    last_error = Some(err);
                    if attempt < max_attempts {
                        context
                            .sleep(Duration::from_millis(task.retry_policy.backoff_ms))
                            .await?;
                    }
                }
            }
        }

        let message = last_error
            .map(|err| err.to_string())
            .unwrap_or_else(|| "unknown provider failure".to_string());

        Err(BrokerError::RetryExhausted {
            attempts: max_attempts,
            message,
        })
    }

    fn cache_key(&self, task: &ApiTask) -> BrokerResult<String> {
        if let Some(key) = &task.cache_policy.key {
            return Ok(key.clone());
        }

        let payload = serde_json::json!({
            "provider": task.provider,
            "operation": task.operation,
            "inputs": task.inputs,
            "params": task.params,
            "output_type": task.output_type,
        });
        let encoded =
            serde_json::to_vec(&payload).map_err(|err| BrokerError::Provider(err.to_string()))?;
        let digest = Sha256::digest(encoded);
        Ok(format!("{digest:x}"))
    }
}
