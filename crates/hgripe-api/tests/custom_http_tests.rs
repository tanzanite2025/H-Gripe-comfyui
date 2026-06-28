use hgripe_api::providers::custom_http::CustomHttpProvider;
use hgripe_api::{ApiBroker, ApiStatus, ApiTask};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tokio::test]
async fn custom_http_gets_json_response() {
    let url = spawn_once_json_server(
        "HTTP/1.1 200 OK",
        r#"{"ok":true,"message":"hello"}"#,
        Some("local-test-request"),
    )
    .await;

    let mut broker = ApiBroker::new();
    broker.register_provider(CustomHttpProvider::default());

    let mut task = ApiTask::new("custom_http", "request");
    task.params.insert("method".into(), json!("GET"));
    task.params.insert("url".into(), json!(url));

    let result = broker.execute(task).await.expect("HTTP task should run");

    assert_eq!(result.status, ApiStatus::Succeeded);
    assert_eq!(
        result.provider_request_id.as_deref(),
        Some("local-test-request")
    );
    assert_eq!(result.output_json.unwrap()["body"]["ok"], json!(true));
}

#[tokio::test]
async fn custom_http_reports_client_error_as_failed_result() {
    let url =
        spawn_once_json_server("HTTP/1.1 404 Not Found", r#"{"error":"missing"}"#, None).await;

    let mut broker = ApiBroker::new();
    broker.register_provider(CustomHttpProvider::default());

    let mut task = ApiTask::new("custom_http", "request");
    task.params.insert("url".into(), json!(url));

    let result = broker
        .execute(task)
        .await
        .expect("4xx should return a result");

    assert_eq!(result.status, ApiStatus::Failed);
    assert_eq!(result.error.unwrap().code, "404");
    assert_eq!(
        result.output_json.unwrap()["body"]["error"],
        json!("missing")
    );
}

async fn spawn_once_json_server(
    status_line: &'static str,
    body: &'static str,
    request_id: Option<&'static str>,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test server should bind");
    let addr = listener.local_addr().expect("test server should have addr");

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("test server should accept");
        let mut buffer = [0_u8; 4096];
        let _ = socket.read(&mut buffer).await;

        let request_header = request_id
            .map(|id| format!("X-Request-Id: {id}\r\n"))
            .unwrap_or_default();
        let response = format!(
            "{status_line}\r\nContent-Type: application/json\r\n{request_header}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );

        socket
            .write_all(response.as_bytes())
            .await
            .expect("test server should write response");
    });

    format!("http://{addr}/test")
}
