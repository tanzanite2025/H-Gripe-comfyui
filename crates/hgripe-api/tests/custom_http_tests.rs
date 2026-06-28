use hgripe_api::providers::custom_http::CustomHttpProvider;
use hgripe_api::{ApiBroker, ApiStatus, ApiTask};
use serde_json::json;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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

#[tokio::test]
async fn custom_http_saves_binary_response_output_file() {
    let body = b"\x89PNG\r\n\x1a\ncustom-http-binary";
    let url = spawn_once_binary_server(
        "HTTP/1.1 200 OK",
        "image/png",
        body,
        Some("local-binary-request"),
    )
    .await;
    let output_dir = temp_output_dir();

    let mut broker = ApiBroker::new();
    broker.register_provider(CustomHttpProvider::default());

    let mut task = ApiTask::new("custom_http", "request");
    task.id = "custom-http-save-binary-test".to_string();
    task.params.insert("method".into(), json!("GET"));
    task.params.insert("url".into(), json!(url));
    task.params.insert("save_response".into(), json!(true));
    task.params.insert(
        "output_dir".into(),
        json!(output_dir.to_string_lossy().to_string()),
    );

    let result = broker.execute(task).await.expect("HTTP task should run");

    assert_eq!(result.status, ApiStatus::Succeeded);
    assert_eq!(
        result.provider_request_id.as_deref(),
        Some("local-binary-request")
    );
    assert_eq!(result.output_files.len(), 1);
    let output_file = &result.output_files[0];
    assert!(output_file.path.ends_with(".png"));
    assert_eq!(output_file.mime_type.as_deref(), Some("image/png"));
    assert_eq!(output_file.size_bytes, Some(body.len() as u64));
    assert_eq!(fs::read(&output_file.path).unwrap(), body);

    let output_json = result.output_json.unwrap();
    assert_eq!(output_json["body"], json!(null));
    assert_eq!(output_json["body_saved"], json!(true));
    assert_eq!(output_json["body_size_bytes"], json!(body.len()));

    let _ = fs::remove_dir_all(output_dir);
}

#[tokio::test]
async fn custom_http_sends_multipart_file_and_fields() {
    let upload_file = write_temp_upload_file("png", b"\x89PNG\r\n\x1a\nupload-body");
    let (url, request_rx) = spawn_once_json_server_with_request(
        "HTTP/1.1 200 OK",
        r#"{"ok":true,"uploaded":true}"#,
        Some("local-multipart-request"),
    )
    .await;

    let mut broker = ApiBroker::new();
    broker.register_provider(CustomHttpProvider::default());

    let mut task = ApiTask::new("custom_http", "request");
    task.params.insert("method".into(), json!("POST"));
    task.params.insert("url".into(), json!(url));
    task.params.insert(
        "multipart_fields".into(),
        json!({
            "prompt": "make this sharper",
            "strength": 0.75
        }),
    );
    task.params.insert(
        "multipart_files".into(),
        json!([
            {
                "field": "image",
                "path": upload_file.to_string_lossy().to_string(),
                "filename": "input.png",
                "mime_type": "image/png"
            }
        ]),
    );

    let result = broker
        .execute(task)
        .await
        .expect("multipart HTTP task should run");
    let request = request_rx.await.expect("server should capture request");
    let _ = fs::remove_file(upload_file);

    assert_eq!(result.status, ApiStatus::Succeeded);
    assert_eq!(
        result.provider_request_id.as_deref(),
        Some("local-multipart-request")
    );
    assert_eq!(result.output_json.unwrap()["body"]["uploaded"], json!(true));
    assert!(request.contains("POST /multipart HTTP/1.1"));
    assert!(request
        .to_lowercase()
        .contains("content-type: multipart/form-data"));
    assert!(request.contains(r#"name="prompt""#));
    assert!(request.contains("make this sharper"));
    assert!(request.contains(r#"name="strength""#));
    assert!(request.contains("0.75"));
    assert!(request.contains(r#"name="image"; filename="input.png""#));
    assert!(request.contains("Content-Type: image/png"));
    assert!(request.contains("upload-body"));
}

#[tokio::test]
async fn custom_http_uses_credentials_ref_for_base_url_and_auth_headers() {
    let (absolute_url, request_rx) = spawn_once_json_server_with_request(
        "HTTP/1.1 200 OK",
        r#"{"ok":true,"credential":true}"#,
        Some("local-credential-request"),
    )
    .await;
    let base_url = absolute_url.trim_end_matches("/multipart").to_string();
    let credentials_file = write_temp_credentials_file(&base_url);

    let mut broker = ApiBroker::new();
    broker.register_provider(CustomHttpProvider::default());

    let mut task = ApiTask::new("custom_http", "request");
    task.credentials_ref = Some("local-custom-http".to_string());
    task.params.insert("method".into(), json!("GET"));
    task.params.insert("url".into(), json!("/secure"));
    task.params.insert(
        "credentials_file".into(),
        json!(credentials_file.to_string_lossy().to_string()),
    );

    let result = broker
        .execute(task)
        .await
        .expect("credential HTTP task should run");
    let request = request_rx.await.expect("server should capture request");
    let _ = fs::remove_file(credentials_file);

    assert_eq!(result.status, ApiStatus::Succeeded);
    assert_eq!(
        result.provider_request_id.as_deref(),
        Some("local-credential-request")
    );
    assert_eq!(
        result.output_json.unwrap()["body"]["credential"],
        json!(true)
    );
    assert!(request.contains("GET /secure HTTP/1.1"));
    assert!(request
        .to_lowercase()
        .contains("authorization: bearer credential-token"));
    assert!(request.contains("x-credential-test: yes"));
}

#[tokio::test]
async fn custom_http_async_job_polls_and_downloads_result() {
    let video_body = b"fake mp4 bytes from async job";
    let base_url = spawn_async_job_server(video_body).await;
    let output_dir = temp_output_dir();

    let mut broker = ApiBroker::new();
    broker.register_provider(CustomHttpProvider::default());

    let mut task = ApiTask::new("custom_http", "async_job");
    task.id = "custom-http-async-job-test".to_string();
    task.params.insert("method".into(), json!("POST"));
    task.params
        .insert("url".into(), json!(format!("{base_url}/submit")));
    task.params.insert(
        "json".into(),
        json!({
            "prompt": "make a short video"
        }),
    );
    task.params.insert(
        "poll_url".into(),
        json!(format!("{base_url}/jobs/{{job_id}}")),
    );
    task.params.insert("job_id_path".into(), json!("id"));
    task.params.insert("status_path".into(), json!("status"));
    task.params
        .insert("success_values".into(), json!(["succeeded"]));
    task.params
        .insert("failure_values".into(), json!(["failed"]));
    task.params.insert("max_polls".into(), json!(3));
    task.params.insert("poll_interval_ms".into(), json!(1));
    task.params.insert("result_path".into(), json!("result"));
    task.params.insert("download_result".into(), json!(true));
    task.params
        .insert("download_url_path".into(), json!("result.video_url"));
    task.params.insert("output_extension".into(), json!("mp4"));
    task.params.insert(
        "output_dir".into(),
        json!(output_dir.to_string_lossy().to_string()),
    );

    let result = broker
        .execute(task)
        .await
        .expect("async HTTP job should run");

    assert_eq!(result.status, ApiStatus::Succeeded);
    assert_eq!(
        result.provider_request_id.as_deref(),
        Some("local-async-poll-complete")
    );
    assert_eq!(result.output_files.len(), 1);
    let output_file = &result.output_files[0];
    assert!(output_file.path.ends_with(".mp4"));
    assert_eq!(output_file.mime_type.as_deref(), Some("video/mp4"));
    assert_eq!(output_file.size_bytes, Some(video_body.len() as u64));
    assert_eq!(fs::read(&output_file.path).unwrap(), video_body);

    let output_json = result.output_json.unwrap();
    assert_eq!(output_json["job_id"], json!("job-123"));
    assert_eq!(output_json["download_saved"], json!(true));
    assert_eq!(output_json["polling"]["poll_count"], json!(2));
    assert_eq!(
        output_json["result"]["video_url"],
        json!(format!("{base_url}/video.mp4"))
    );

    let _ = fs::remove_dir_all(output_dir);
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

async fn spawn_once_json_server_with_request(
    status_line: &'static str,
    body: &'static str,
    request_id: Option<&'static str>,
) -> (String, oneshot::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test server should bind");
    let addr = listener.local_addr().expect("test server should have addr");
    let (request_tx, request_rx) = oneshot::channel();

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("test server should accept");
        let request = read_http_request(&mut socket).await;
        let _ = request_tx.send(request);

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

    (format!("http://{addr}/multipart"), request_rx)
}

async fn spawn_once_binary_server(
    status_line: &'static str,
    content_type: &'static str,
    body: &'static [u8],
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
        let response_header = format!(
            "{status_line}\r\nContent-Type: {content_type}\r\n{request_header}Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );

        socket
            .write_all(response_header.as_bytes())
            .await
            .expect("test server should write response header");
        socket
            .write_all(body)
            .await
            .expect("test server should write response body");
    });

    format!("http://{addr}/binary")
}

async fn spawn_async_job_server(video_body: &'static [u8]) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test server should bind");
    let addr = listener.local_addr().expect("test server should have addr");
    let base_url = format!("http://{addr}");
    let server_base_url = base_url.clone();
    let poll_count = Arc::new(AtomicU64::new(0));

    tokio::spawn(async move {
        for _ in 0..4 {
            let (mut socket, _) = listener.accept().await.expect("test server should accept");
            let request = read_http_request(&mut socket).await;
            let request_line = request.lines().next().unwrap_or("");

            if request_line.starts_with("POST /submit ") {
                write_json_response(
                    &mut socket,
                    "HTTP/1.1 200 OK",
                    r#"{"id":"job-123"}"#,
                    Some("local-async-submit"),
                )
                .await;
            } else if request_line.starts_with("GET /jobs/job-123 ") {
                let count = poll_count.fetch_add(1, Ordering::SeqCst) + 1;
                if count == 1 {
                    write_json_response(
                        &mut socket,
                        "HTTP/1.1 200 OK",
                        r#"{"status":"running"}"#,
                        Some("local-async-poll-running"),
                    )
                    .await;
                } else {
                    let body = format!(
                        r#"{{"status":"succeeded","result":{{"video_url":"{server_base_url}/video.mp4"}}}}"#
                    );
                    write_json_response(
                        &mut socket,
                        "HTTP/1.1 200 OK",
                        Box::leak(body.into_boxed_str()),
                        Some("local-async-poll-complete"),
                    )
                    .await;
                }
            } else if request_line.starts_with("GET /video.mp4 ") {
                write_binary_response(
                    &mut socket,
                    "HTTP/1.1 200 OK",
                    "video/mp4",
                    video_body,
                    Some("local-async-download"),
                )
                .await;
            } else {
                write_json_response(
                    &mut socket,
                    "HTTP/1.1 404 Not Found",
                    r#"{"error":"not found"}"#,
                    None,
                )
                .await;
            }
        }
    });

    base_url
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
    status_line: &'static str,
    content_type: &'static str,
    body: &'static [u8],
    request_id: Option<&'static str>,
) {
    let request_header = request_id
        .map(|id| format!("X-Request-Id: {id}\r\n"))
        .unwrap_or_default();
    let response_header = format!(
        "{status_line}\r\nContent-Type: {content_type}\r\n{request_header}Content-Length: {}\r\nConnection: close\r\n\r\n",
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
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    buffer.len() >= header_end + 4 + content_length
}

fn find_subsequence(buffer: &[u8], needle: &[u8]) -> Option<usize> {
    buffer
        .windows(needle.len())
        .position(|window| window == needle)
}

fn write_temp_upload_file(extension: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("hgripe-upload-{}.{}", temp_suffix(), extension));
    fs::write(&path, bytes).expect("upload fixture should be written");
    path
}

fn write_temp_credentials_file(base_url: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "hgripe-custom-http-credentials-{}.json",
        temp_suffix()
    ));
    let document = json!({
        "local-custom-http": {
            "provider": "custom_http",
            "base_url": base_url,
            "api_key": "credential-token",
            "headers": {
                "X-Credential-Test": "yes"
            }
        }
    });
    fs::write(&path, serde_json::to_string_pretty(&document).unwrap())
        .expect("credentials fixture should be written");
    path
}

fn temp_output_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("hgripe-custom-http-output-{}", temp_suffix()))
}

fn temp_suffix() -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be valid")
        .as_nanos();
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{nonce}-{counter}")
}
