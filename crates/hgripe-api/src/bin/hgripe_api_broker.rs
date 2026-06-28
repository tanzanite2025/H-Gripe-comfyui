use hgripe_api::providers::custom_http::CustomHttpProvider;
use hgripe_api::providers::mock::MockProvider;
use hgripe_api::providers::openai_compatible::OpenAiCompatibleProvider;
use hgripe_api::providers::replicate::ReplicateProvider;
use hgripe_api::{record_task_failure, record_task_result, ApiBroker, ApiTask};
use serde_json::json;
use std::io::{self, Read};
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

async fn run() -> Result<(), String> {
    let mut payload = String::new();
    io::stdin()
        .read_to_string(&mut payload)
        .map_err(|err| format!("failed to read stdin: {err}"))?;

    let task: ApiTask =
        serde_json::from_str(&payload).map_err(|err| format!("invalid ApiTask JSON: {err}"))?;
    let history_task = task.clone();

    let mut broker = ApiBroker::new();
    broker.register_provider(CustomHttpProvider::default());
    broker.register_provider(MockProvider);
    broker.register_provider(OpenAiCompatibleProvider::default());
    broker.register_provider(ReplicateProvider::default());

    match broker.execute(task).await {
        Ok(result) => {
            if let Err(err) = record_task_result(&history_task, &result) {
                eprintln!("warning: failed to record task history: {err}");
            }
            let encoded = serde_json::to_string_pretty(&result)
                .map_err(|err| format!("failed to encode ApiResult: {err}"))?;
            println!("{encoded}");
            Ok(())
        }
        Err(err) => {
            if let Err(history_err) = record_task_failure(&history_task, err.to_string()) {
                eprintln!("warning: failed to record task history: {history_err}");
            }
            let encoded = serde_json::to_string_pretty(&json!({
                "status": "failed",
                "error": {
                    "message": err.to_string()
                }
            }))
            .map_err(|encode_err| format!("failed to encode broker error: {encode_err}"))?;
            println!("{encoded}");
            Err(err.to_string())
        }
    }
}
