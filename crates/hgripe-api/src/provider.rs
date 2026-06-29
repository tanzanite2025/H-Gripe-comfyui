use crate::model::{ApiResult, ApiTask};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Notify;
use tokio::time::{sleep, Duration};

#[derive(Debug, Error)]
pub enum BrokerError {
    #[error("provider '{0}' is not registered")]
    ProviderNotFound(String),
    #[error("provider '{provider}' does not support operation '{operation}'")]
    UnsupportedOperation { provider: String, operation: String },
    #[error("task cancelled")]
    Cancelled,
    #[error("provider error: {0}")]
    Provider(String),
    #[error("task failed after {attempts} attempt(s): {message}")]
    RetryExhausted { attempts: u32, message: String },
}

pub type BrokerResult<T> = Result<T, BrokerError>;

#[derive(Clone, Default)]
pub struct CancellationToken {
    inner: Arc<CancellationState>,
}

#[derive(Default)]
struct CancellationState {
    cancelled: AtomicBool,
    notify: Notify,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::SeqCst) {
            self.inner.notify.notify_waiters();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    pub fn check_cancelled(&self) -> BrokerResult<()> {
        if self.is_cancelled() {
            Err(BrokerError::Cancelled)
        } else {
            Ok(())
        }
    }

    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        self.inner.notify.notified().await;
    }

    pub async fn sleep(&self, duration: Duration) -> BrokerResult<()> {
        tokio::select! {
            _ = sleep(duration) => Ok(()),
            _ = self.cancelled() => Err(BrokerError::Cancelled),
        }
    }
}

#[derive(Clone, Default)]
pub struct ProviderExecutionContext {
    cancellation: CancellationToken,
}

impl ProviderExecutionContext {
    pub fn new(cancellation: CancellationToken) -> Self {
        Self { cancellation }
    }

    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    pub fn check_cancelled(&self) -> BrokerResult<()> {
        self.cancellation.check_cancelled()
    }

    pub async fn sleep(&self, duration: Duration) -> BrokerResult<()> {
        self.cancellation.sleep(duration).await
    }
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &'static str;
    fn supports(&self, operation: &str) -> bool;
    async fn execute(&self, task: &ApiTask) -> BrokerResult<ApiResult>;

    async fn execute_with_context(
        &self,
        task: &ApiTask,
        context: &ProviderExecutionContext,
    ) -> BrokerResult<ApiResult> {
        context.check_cancelled()?;
        self.execute(task).await
    }
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
