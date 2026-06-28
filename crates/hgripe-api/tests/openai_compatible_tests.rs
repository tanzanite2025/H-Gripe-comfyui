use hgripe_api::providers::openai_compatible::OpenAiCompatibleProvider;
use hgripe_api::{ApiBroker, ApiStatus, ApiTask};
use serde_json::json;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

#[tokio::test]
async fn openai_compatible_chat_generates_text() {
    let (base_url, request_rx) = spawn_once_json_server(
        "HTTP/1.1 200 OK",
        r#"{"id":"chatcmpl_test","choices":[{"message":{"role":"assistant","content":"hello from provider"},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":3,"total_tokens":7}}"#,
        Some("openai-chat-test"),
    )
    .await;

    let mut broker = ApiBroker::new();
    broker.register_provider(OpenAiCompatibleProvider::default());

    let mut task = ApiTask::new("openai_compatible", "chat.completions");
    task.params.insert("base_url".into(), json!(base_url));
    task.params.insert("api_key".into(), json!("test-key"));
    task.params.insert("model".into(), json!("test-model"));
    task.inputs.insert("prompt".into(), json!("say hello"));

    let result = broker.execute(task).await.expect("chat task should run");
    let request = request_rx.await.expect("server should capture request");

    assert_eq!(result.status, ApiStatus::Succeeded);
    assert_eq!(
        result.provider_request_id.as_deref(),
        Some("openai-chat-test")
    );
    assert_eq!(
        result.output_json.unwrap()["text"],
        json!("hello from provider")
    );
    assert!(request.contains("POST /chat/completions HTTP/1.1"));
    assert!(request
        .to_lowercase()
        .contains("authorization: bearer test-key"));
    assert!(request.contains(r#""model":"test-model""#));
    assert!(request.contains(r#""content":"say hello""#));
}

#[tokio::test]
async fn openai_compatible_reports_client_error_as_failed_result() {
    let (base_url, _request_rx) = spawn_once_json_server(
        "HTTP/1.1 401 Unauthorized",
        r#"{"error":{"code":"invalid_api_key","message":"bad key"}}"#,
        None,
    )
    .await;

    let mut broker = ApiBroker::new();
    broker.register_provider(OpenAiCompatibleProvider::default());

    let mut task = ApiTask::new("openai_compatible", "text.generate");
    task.params.insert("base_url".into(), json!(base_url));
    task.params.insert("no_auth".into(), json!(true));
    task.params.insert("model".into(), json!("test-model"));
    task.inputs.insert("prompt".into(), json!("hello"));

    let result = broker
        .execute(task)
        .await
        .expect("4xx should return result");

    assert_eq!(result.status, ApiStatus::Failed);
    let error = result.error.unwrap();
    assert_eq!(error.code, "invalid_api_key");
    assert_eq!(error.message, "bad key");
}

#[tokio::test]
async fn openai_compatible_image_returns_raw_data() {
    let (base_url, request_rx) = spawn_once_json_server(
        "HTTP/1.1 200 OK",
        r#"{"created":123,"data":[{"url":"https://example.test/image.png","revised_prompt":"cat"}]}"#,
        Some("openai-image-test"),
    )
    .await;

    let mut broker = ApiBroker::new();
    broker.register_provider(OpenAiCompatibleProvider::default());

    let mut task = ApiTask::new("openai_compatible", "image.generate");
    task.params.insert("base_url".into(), json!(base_url));
    task.params.insert("no_auth".into(), json!(true));
    task.params.insert("model".into(), json!("image-model"));
    task.params.insert("size".into(), json!("1024x1024"));
    task.inputs.insert("prompt".into(), json!("a small cat"));

    let result = broker.execute(task).await.expect("image task should run");
    let request = request_rx.await.expect("server should capture request");

    assert_eq!(result.status, ApiStatus::Succeeded);
    assert_eq!(
        result.provider_request_id.as_deref(),
        Some("openai-image-test")
    );
    assert_eq!(
        result.output_json.unwrap()["images"][0]["url"],
        json!("https://example.test/image.png")
    );
    assert!(request.contains("POST /images/generations HTTP/1.1"));
    assert!(request.contains(r#""prompt":"a small cat""#));
}

#[tokio::test]
async fn openai_compatible_image_saves_b64_output_file() {
    let image_b64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII=";
    let body = format!(
        r#"{{"created":123,"data":[{{"b64_json":"{image_b64}","revised_prompt":"saved cat"}}]}}"#
    );
    let (base_url, _request_rx) =
        spawn_once_json_server("HTTP/1.1 200 OK", Box::leak(body.into_boxed_str()), None).await;
    let output_dir = temp_output_dir();

    let mut broker = ApiBroker::new();
    broker.register_provider(OpenAiCompatibleProvider::default());

    let mut task = ApiTask::new("openai_compatible", "image.generate");
    task.id = "save-image-test".to_string();
    task.params.insert("base_url".into(), json!(base_url));
    task.params.insert("no_auth".into(), json!(true));
    task.params.insert("model".into(), json!("image-model"));
    task.params.insert("save_outputs".into(), json!(true));
    task.params.insert(
        "output_dir".into(),
        json!(output_dir.to_string_lossy().to_string()),
    );
    task.inputs.insert("prompt".into(), json!("save a cat"));

    let result = broker.execute(task).await.expect("image task should run");

    assert_eq!(result.status, ApiStatus::Succeeded);
    assert_eq!(result.output_files.len(), 1);
    let output_file = &result.output_files[0];
    assert!(std::path::Path::new(&output_file.path).exists());
    assert_eq!(output_file.mime_type.as_deref(), Some("image/png"));
    assert!(output_file.size_bytes.unwrap_or(0) > 0);
    assert!(output_file.sha256.is_some());

    let output_json = result.output_json.unwrap();
    assert_eq!(
        output_json["images"][0]["local_path"],
        json!(output_file.path)
    );
    assert_eq!(
        output_json["images"][0]["local_mime_type"],
        json!("image/png")
    );

    let _ = fs::remove_dir_all(output_dir);
}

#[tokio::test]
async fn openai_compatible_vision_sends_image_message() {
    let (base_url, request_rx) = spawn_once_json_server(
        "HTTP/1.1 200 OK",
        r#"{"id":"vision_test","choices":[{"message":{"role":"assistant","content":"the image is red"},"finish_reason":"stop"}]}"#,
        Some("openai-vision-test"),
    )
    .await;

    let mut broker = ApiBroker::new();
    broker.register_provider(OpenAiCompatibleProvider::default());

    let messages = json!([
        {
            "role": "user",
            "content": [
                {"type": "text", "text": "describe this"},
                {
                    "type": "image_url",
                    "image_url": {
                        "url": "data:image/png;base64,AAAA",
                        "detail": "low"
                    }
                }
            ]
        }
    ]);

    let mut task = ApiTask::new("openai_compatible", "vision.analyze");
    task.params.insert("base_url".into(), json!(base_url));
    task.params.insert("no_auth".into(), json!(true));
    task.params.insert("model".into(), json!("vision-model"));
    task.params.insert("messages".into(), messages);

    let result = broker.execute(task).await.expect("vision task should run");
    let request = request_rx.await.expect("server should capture request");

    assert_eq!(result.status, ApiStatus::Succeeded);
    assert_eq!(
        result.provider_request_id.as_deref(),
        Some("openai-vision-test")
    );
    assert_eq!(
        result.output_json.unwrap()["text"],
        json!("the image is red")
    );
    assert!(request.contains("POST /chat/completions HTTP/1.1"));
    assert!(request.contains(r#""model":"vision-model""#));
    assert!(request.contains(r#""type":"image_url""#));
    assert!(request.contains(r#""detail":"low""#));
}

#[tokio::test]
async fn openai_compatible_uses_credentials_ref_file() {
    let (base_url, request_rx) = spawn_once_json_server(
        "HTTP/1.1 200 OK",
        r#"{"id":"credential_test","choices":[{"message":{"role":"assistant","content":"credential ok"},"finish_reason":"stop"}]}"#,
        Some("openai-credential-test"),
    )
    .await;
    let credentials_file = write_temp_credentials(&base_url);

    let mut broker = ApiBroker::new();
    broker.register_provider(OpenAiCompatibleProvider::default());

    let mut task = ApiTask::new("openai_compatible", "chat.completions");
    task.credentials_ref = Some("local-openai".to_string());
    task.params.insert(
        "credentials_file".into(),
        json!(credentials_file.to_string_lossy().to_string()),
    );
    task.params
        .insert("model".into(), json!("credential-model"));
    task.inputs
        .insert("prompt".into(), json!("hello credential"));

    let result = broker
        .execute(task)
        .await
        .expect("credential ref task should run");
    let request = request_rx.await.expect("server should capture request");
    let _ = fs::remove_file(credentials_file);

    assert_eq!(result.status, ApiStatus::Succeeded);
    assert_eq!(result.output_json.unwrap()["text"], json!("credential ok"));
    assert!(request
        .to_lowercase()
        .contains("authorization: bearer credential-file-key"));
    assert!(request.contains("POST /chat/completions HTTP/1.1"));
}

async fn spawn_once_json_server(
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

    (format!("http://{addr}"), request_rx)
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

fn write_temp_credentials(base_url: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be valid")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("hgripe-credentials-{nonce}.json"));
    let document = json!({
        "local-openai": {
            "provider": "openai_compatible",
            "base_url": base_url,
            "api_key": "credential-file-key"
        }
    });
    fs::write(&path, serde_json::to_string_pretty(&document).unwrap())
        .expect("credentials file should be written");
    path
}

fn temp_output_dir() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be valid")
        .as_nanos();
    std::env::temp_dir().join(format!("hgripe-output-{nonce}"))
}
