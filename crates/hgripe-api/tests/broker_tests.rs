use hgripe_api::model::OutputType;
use hgripe_api::providers::mock::MockProvider;
use hgripe_api::{ApiBroker, ApiStatus, ApiTask};
use serde_json::json;

#[tokio::test]
async fn executes_registered_provider() {
    let mut broker = ApiBroker::new();
    broker.register_provider(MockProvider);

    let mut task = ApiTask::new("mock", "echo");
    task.output_type = OutputType::Json;
    task.inputs.insert("prompt".into(), json!("hello"));

    let result = broker
        .execute(task)
        .await
        .expect("mock provider should run");

    assert_eq!(result.status, ApiStatus::Succeeded);
    assert!(!result.cache_hit);
    assert_eq!(
        result.output_json.unwrap()["inputs"]["prompt"],
        json!("hello")
    );
}

#[tokio::test]
async fn returns_cached_result_for_same_payload() {
    let mut broker = ApiBroker::new();
    broker.register_provider(MockProvider);

    let mut first = ApiTask::new("mock", "echo");
    first.inputs.insert("prompt".into(), json!("cache me"));

    let mut second = ApiTask::new("mock", "echo");
    second.inputs.insert("prompt".into(), json!("cache me"));

    let first_result = broker.execute(first).await.expect("first run should pass");
    let second_result = broker
        .execute(second)
        .await
        .expect("second run should pass");

    assert_eq!(first_result.status, ApiStatus::Succeeded);
    assert_eq!(second_result.status, ApiStatus::Cached);
    assert!(second_result.cache_hit);
}

#[tokio::test]
async fn rejects_unknown_provider() {
    let broker = ApiBroker::new();
    let task = ApiTask::new("missing", "echo");

    let error = broker.execute(task).await.expect_err("provider is absent");

    assert!(error.to_string().contains("not registered"));
}
