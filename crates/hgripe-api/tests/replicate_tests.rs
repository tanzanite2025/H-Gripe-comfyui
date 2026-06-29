use hgripe_api::providers::replicate::ReplicateProvider;
use hgripe_api::{ApiBroker, ApiStatus, ApiTask, CancellationToken, ProviderExecutionContext};
use serde_json::json;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

#[tokio::test]
async fn replicate_model_run_polls_and_downloads_outputs() {
    let png_a = b"\x89PNG\r\n\x1a\nreplicate-output-a";
    let png_b = b"\x89PNG\r\n\x1a\nreplicate-output-b";
    let base_url = spawn_replicate_server(png_a, png_b, "owner/widget", None).await;
    let output_dir = temp_output_dir();

    let mut broker = ApiBroker::new();
    broker.register_provider(ReplicateProvider::default());

    let mut task = ApiTask::new("replicate", "run");
    task.id = "replicate-model-run-test".to_string();
    task.params.insert("base_url".into(), json!(base_url));
    task.params.insert("model".into(), json!("owner/widget"));
    task.params
        .insert("input".into(), json!({"prompt": "a small robot"}));
    task.params.insert("max_polls".into(), json!(3));
    task.params.insert("poll_interval_ms".into(), json!(1));
    task.params.insert(
        "output_dir".into(),
        json!(output_dir.to_string_lossy().to_string()),
    );

    let result = broker
        .execute(task)
        .await
        .expect("replicate task should run");

    assert_eq!(result.status, ApiStatus::Succeeded);
    assert_eq!(result.provider_request_id.as_deref(), Some("pred-123"));
    assert_eq!(result.output_files.len(), 2);
    assert!(result.output_files[0].path.ends_with(".png"));
    assert_eq!(
        result.output_files[0].mime_type.as_deref(),
        Some("image/png")
    );
    assert_eq!(fs::read(&result.output_files[0].path).unwrap(), png_a);
    assert_eq!(fs::read(&result.output_files[1].path).unwrap(), png_b);

    let output_json = result.output_json.unwrap();
    assert_eq!(output_json["id"], json!("pred-123"));
    assert_eq!(output_json["status"], json!("succeeded"));
    assert_eq!(output_json["polling"]["poll_count"], json!(2));
    assert_eq!(
        output_json["output"][0],
        json!(format!("{base_url}/files/out-0.png"))
    );

    let _ = fs::remove_dir_all(output_dir);
}

#[tokio::test]
async fn replicate_version_run_uses_predictions_endpoint() {
    let png_a = b"\x89PNG\r\n\x1a\nreplicate-version-a";
    let png_b = b"";
    let (base_url, request_rx) =
        spawn_replicate_server_with_request(png_a, png_b, "", Some("v-abc")).await;
    let output_dir = temp_output_dir();

    let mut broker = ApiBroker::new();
    broker.register_provider(ReplicateProvider::default());

    let mut task = ApiTask::new("replicate", "run");
    task.id = "replicate-version-run-test".to_string();
    task.params.insert("base_url".into(), json!(base_url));
    task.params.insert("version".into(), json!("v-abc"));
    task.params
        .insert("input".into(), json!({"prompt": "a cat"}));
    task.params.insert("max_polls".into(), json!(3));
    task.params.insert("poll_interval_ms".into(), json!(1));
    task.params.insert("download_outputs".into(), json!(false));
    task.params.insert(
        "output_dir".into(),
        json!(output_dir.to_string_lossy().to_string()),
    );

    let result = broker
        .execute(task)
        .await
        .expect("replicate task should run");
    let submit_request = request_rx.await.expect("server should capture request");

    assert_eq!(result.status, ApiStatus::Succeeded);
    assert!(result.output_files.is_empty());
    assert!(submit_request.contains("POST /v1/predictions HTTP/1.1"));
    assert!(submit_request.contains(r#""version":"v-abc""#));
    assert!(submit_request.contains(r#""prompt":"a cat""#));

    let _ = fs::remove_dir_all(output_dir);
}

#[tokio::test]
async fn replicate_failed_prediction_returns_failed_result() {
    let base_url = spawn_replicate_failure_server().await;

    let mut broker = ApiBroker::new();
    broker.register_provider(ReplicateProvider::default());

    let mut task = ApiTask::new("replicate", "run");
    task.id = "replicate-failed-test".to_string();
    task.params.insert("base_url".into(), json!(base_url));
    task.params.insert("model".into(), json!("owner/widget"));
    task.params
        .insert("input".into(), json!({"prompt": "boom"}));
    task.params.insert("max_polls".into(), json!(3));
    task.params.insert("poll_interval_ms".into(), json!(1));
    task.retry_policy.max_attempts = 1;

    let result = broker
        .execute(task)
        .await
        .expect("replicate failure should return a result");

    assert_eq!(result.status, ApiStatus::Failed);
    let error = result.error.unwrap();
    assert_eq!(error.code, "failed");
    assert!(error.message.contains("model exploded"));
}

#[tokio::test]
async fn replicate_cancellation_sends_prediction_cancel_request() {
    let (base_url, first_poll_rx, cancel_request_rx) = spawn_cancelable_replicate_server().await;

    let mut broker = ApiBroker::new();
    broker.register_provider(ReplicateProvider::default());

    let mut task = ApiTask::new("replicate", "run");
    task.id = "replicate-cancel-test".to_string();
    task.params.insert("base_url".into(), json!(base_url));
    task.params.insert("model".into(), json!("owner/widget"));
    task.params
        .insert("input".into(), json!({"prompt": "cancel me"}));
    task.params.insert("max_polls".into(), json!(3));
    task.params.insert("poll_interval_ms".into(), json!(60_000));
    task.retry_policy.max_attempts = 1;

    let cancellation = CancellationToken::new();
    let context = ProviderExecutionContext::new(cancellation.clone());
    let run = tokio::spawn(async move { broker.execute_with_context(task, context).await });

    first_poll_rx
        .await
        .expect("server should observe the first poll");
    cancellation.cancel();

    let result = run
        .await
        .expect("broker task should join")
        .expect("cancelled prediction should return a result");
    let cancel_request = cancel_request_rx
        .await
        .expect("server should capture cancel request");

    assert_eq!(result.status, ApiStatus::Cancelled);
    assert_eq!(
        result.provider_request_id.as_deref(),
        Some("pred-cancel-request")
    );
    assert!(cancel_request.contains("POST /v1/predictions/pred-cancel/cancel HTTP/1.1"));
    assert_eq!(result.output_json.unwrap()["cancel"]["sent"], json!(true));
}

fn temp_output_dir() -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "hgripe-replicate-test-{}-{unique}",
        std::process::id()
    ))
}

async fn spawn_replicate_server(
    file_a: &'static [u8],
    file_b: &'static [u8],
    model: &str,
    version: Option<&str>,
) -> String {
    let (base_url, _request_rx) =
        spawn_replicate_server_with_request(file_a, file_b, model, version).await;
    base_url
}

async fn spawn_replicate_server_with_request(
    file_a: &'static [u8],
    file_b: &'static [u8],
    model: &str,
    _version: Option<&str>,
) -> (String, oneshot::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test server should bind");
    let addr = listener.local_addr().expect("test server should have addr");
    let base_url = format!("http://{addr}");
    let server_base_url = base_url.clone();
    let submit_path = if model.is_empty() {
        "POST /v1/predictions ".to_string()
    } else {
        format!("POST /v1/models/{model}/predictions ")
    };
    let poll_count = Arc::new(AtomicU64::new(0));
    let (request_tx, request_rx) = oneshot::channel();
    let request_tx = Arc::new(std::sync::Mutex::new(Some(request_tx)));

    tokio::spawn(async move {
        for _ in 0..5 {
            let (mut socket, _) = listener.accept().await.expect("test server should accept");
            let request = read_http_request(&mut socket).await;
            let request_line = request.lines().next().unwrap_or("").to_string();

            if request_line.starts_with(submit_path.trim_end()) {
                if let Some(tx) = request_tx.lock().unwrap().take() {
                    let _ = tx.send(request.clone());
                }
                let body = format!(
                    r#"{{"id":"pred-123","status":"starting","urls":{{"get":"{server_base_url}/v1/predictions/pred-123"}}}}"#
                );
                write_json_response(
                    &mut socket,
                    "HTTP/1.1 201 Created",
                    Box::leak(body.into_boxed_str()),
                    Some("pred-123"),
                )
                .await;
            } else if request_line.starts_with("GET /v1/predictions/pred-123 ") {
                let count = poll_count.fetch_add(1, Ordering::SeqCst) + 1;
                if count == 1 {
                    write_json_response(
                        &mut socket,
                        "HTTP/1.1 200 OK",
                        r#"{"id":"pred-123","status":"processing"}"#,
                        Some("pred-123"),
                    )
                    .await;
                } else {
                    let body = format!(
                        r#"{{"id":"pred-123","status":"succeeded","output":["{server_base_url}/files/out-0.png","{server_base_url}/files/out-1.png"]}}"#
                    );
                    write_json_response(
                        &mut socket,
                        "HTTP/1.1 200 OK",
                        Box::leak(body.into_boxed_str()),
                        Some("pred-123"),
                    )
                    .await;
                }
            } else if request_line.starts_with("GET /files/out-0.png ") {
                write_binary_response(&mut socket, "image/png", file_a).await;
            } else if request_line.starts_with("GET /files/out-1.png ") {
                write_binary_response(&mut socket, "image/png", file_b).await;
            } else {
                write_json_response(
                    &mut socket,
                    "HTTP/1.1 404 Not Found",
                    r#"{"detail":"not found"}"#,
                    None,
                )
                .await;
            }
        }
    });

    (base_url, request_rx)
}

async fn spawn_replicate_failure_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test server should bind");
    let addr = listener.local_addr().expect("test server should have addr");
    let base_url = format!("http://{addr}");
    let server_base_url = base_url.clone();

    tokio::spawn(async move {
        for _ in 0..3 {
            let (mut socket, _) = listener.accept().await.expect("test server should accept");
            let request = read_http_request(&mut socket).await;
            let request_line = request.lines().next().unwrap_or("");

            if request_line.starts_with("POST /v1/models/owner/widget/predictions ") {
                let body = format!(
                    r#"{{"id":"pred-err","status":"starting","urls":{{"get":"{server_base_url}/v1/predictions/pred-err"}}}}"#
                );
                write_json_response(
                    &mut socket,
                    "HTTP/1.1 201 Created",
                    Box::leak(body.into_boxed_str()),
                    None,
                )
                .await;
            } else if request_line.starts_with("GET /v1/predictions/pred-err ") {
                write_json_response(
                    &mut socket,
                    "HTTP/1.1 200 OK",
                    r#"{"id":"pred-err","status":"failed","error":"model exploded"}"#,
                    None,
                )
                .await;
            } else {
                write_json_response(
                    &mut socket,
                    "HTTP/1.1 404 Not Found",
                    r#"{"detail":"not found"}"#,
                    None,
                )
                .await;
            }
        }
    });

    base_url
}

async fn spawn_cancelable_replicate_server(
) -> (String, oneshot::Receiver<()>, oneshot::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test server should bind");
    let addr = listener.local_addr().expect("test server should have addr");
    let base_url = format!("http://{addr}");
    let server_base_url = base_url.clone();
    let (first_poll_tx, first_poll_rx) = oneshot::channel();
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let first_poll_tx = Arc::new(std::sync::Mutex::new(Some(first_poll_tx)));
    let cancel_tx = Arc::new(std::sync::Mutex::new(Some(cancel_tx)));

    tokio::spawn(async move {
        for _ in 0..3 {
            let (mut socket, _) = listener.accept().await.expect("test server should accept");
            let request = read_http_request(&mut socket).await;
            let request_line = request.lines().next().unwrap_or("");

            if request_line.starts_with("POST /v1/models/owner/widget/predictions ") {
                let body = format!(
                    r#"{{"id":"pred-cancel","status":"starting","urls":{{"get":"{server_base_url}/v1/predictions/pred-cancel"}}}}"#
                );
                write_json_response(
                    &mut socket,
                    "HTTP/1.1 201 Created",
                    Box::leak(body.into_boxed_str()),
                    Some("pred-cancel-submit"),
                )
                .await;
            } else if request_line.starts_with("GET /v1/predictions/pred-cancel ") {
                if let Some(tx) = first_poll_tx.lock().unwrap().take() {
                    let _ = tx.send(());
                }
                write_json_response(
                    &mut socket,
                    "HTTP/1.1 200 OK",
                    r#"{"id":"pred-cancel","status":"processing"}"#,
                    Some("pred-cancel-poll"),
                )
                .await;
            } else if request_line.starts_with("POST /v1/predictions/pred-cancel/cancel ") {
                if let Some(tx) = cancel_tx.lock().unwrap().take() {
                    let _ = tx.send(request.clone());
                }
                write_json_response(
                    &mut socket,
                    "HTTP/1.1 200 OK",
                    r#"{"id":"pred-cancel","status":"canceled"}"#,
                    Some("pred-cancel-request"),
                )
                .await;
            } else {
                write_json_response(
                    &mut socket,
                    "HTTP/1.1 404 Not Found",
                    r#"{"detail":"not found"}"#,
                    None,
                )
                .await;
            }
        }
    });

    (base_url, first_poll_rx, cancel_rx)
}

async fn write_json_response(
    socket: &mut tokio::net::TcpStream,
    status_line: &'static str,
    body: &'static str,
    request_id: Option<&'static str>,
) {
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
        .expect("test server should write JSON response");
}

async fn write_binary_response(
    socket: &mut tokio::net::TcpStream,
    content_type: &'static str,
    body: &'static [u8],
) {
    let response_header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    socket
        .write_all(response_header.as_bytes())
        .await
        .expect("test server should write binary response header");
    socket
        .write_all(body)
        .await
        .expect("test server should write binary response body");
}

async fn read_http_request(socket: &mut tokio::net::TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];

    loop {
        let read = socket
            .read(&mut chunk)
            .await
            .expect("test server should read request");
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);

        if request_is_complete(&buffer) {
            break;
        }
    }

    String::from_utf8_lossy(&buffer).into_owned()
}

fn request_is_complete(buffer: &[u8]) -> bool {
    let Some(header_end) = find_subsequence(buffer, b"\r\n\r\n") else {
        return false;
    };
    let headers = String::from_utf8_lossy(&buffer[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.trim().eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);

    buffer.len() >= header_end + 4 + content_length
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
