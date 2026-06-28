use crate::model::{ApiResult, ApiTask};
use crate::provider::{BrokerResult, Provider};
use async_trait::async_trait;
use serde_json::json;

#[derive(Default)]
pub struct MockProvider;

#[async_trait]
impl Provider for MockProvider {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn supports(&self, operation: &str) -> bool {
        matches!(operation, "echo" | "image.generate" | "text.generate")
    }

    async fn execute(&self, task: &ApiTask) -> BrokerResult<ApiResult> {
        Ok(ApiResult::succeeded(
            task.id.clone(),
            Some(json!({
                "provider": task.provider,
                "operation": task.operation,
                "inputs": task.inputs,
                "params": task.params,
            })),
        ))
    }
}
