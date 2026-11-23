//! The real client against throwaway servers on 127.0.0.1: requests on the wire, streamed
//! responses, error statuses, the output-cap retry and cancellation on a silent socket.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use base::cancel::CancelToken;
use llm::{ChatRequest, Config, LlmClient, LlmError, NoBlobs, ProviderConfig, ProviderKind, Providers, StreamEvent};
use model::{Message, ModelRef, StopReason};

/// What the server does with one connection.
struct Answer {
    bytes: String,
    /// Keep the connection open and silent afterwards.
    hold: bool,
}

fn send(bytes: impl Into<String>) -> Answer {
    Answer { bytes: bytes.into(), hold: false }
}

fn silent_after(bytes: impl Into<String>) -> Answer {
    Answer { bytes: bytes.into(), hold: true }
}

/// Serves one answer per connection, in order, and hands back each request it read.
fn server(answers: Vec<Answer>) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for answer in answers {
            let Ok((mut stream, _)) = listener.accept() else { return };
            let _ = tx.send(read_request(&mut stream));
            let _ = stream.write_all(answer.bytes.as_bytes());
            let _ = stream.flush();
            if answer.hold {
                std::thread::sleep(Duration::from_secs(10));
            }
        }
    });
    (base, rx)
}

fn read_request(stream: &mut TcpStream) -> String {
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = stream.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..end]).to_lowercase();
            let len = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok());
            match len {
                Some(len) if buf.len() >= end + 4 + len => break,
                None if buf.ends_with(b"0\r\n\r\n") => break,
                _ => {}
            }
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn client(base: &str, kind: ProviderKind) -> Providers {
    Providers::new(Config {
        providers: vec![ProviderConfig {
            id: "p".into(),
            kind,
            base_url: base.into(),
            api_key: Some("test-key".into()),
            models: vec![],
        }],
        default_model: None,
    })
}

fn request() -> ChatRequest {
    ChatRequest { messages: vec![Message::user("hi")], max_tokens: 100, ..Default::default() }
}

fn stream(client: &Providers, cancel: &CancelToken) -> (Result<(), LlmError>, Vec<StreamEvent>) {
    let mut events = Vec::new();
    let result = client.stream(&ModelRef::parse("p/some-model"), &request(), &NoBlobs, cancel, &mut |e| events.push(e));
    (result, events)
}

const SSE_HEAD: &str = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n";

#[test]
fn anthropic_streams_over_http() {
    let body = [
        r#"data: {"type":"message_start","message":{"usage":{"input_tokens":5,"output_tokens":1}}}"#,
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
        r#"data: {"type":"content_block_stop","index":0}"#,
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}"#,
        r#"data: {"type":"message_stop"}"#,
    ]
    .join("\r\n\r\n");
    let (base, requests) = server(vec![send(format!("{SSE_HEAD}{body}\r\n\r\n"))]);
    let (result, events) = stream(&client(&base, ProviderKind::Anthropic), &CancelToken::new());
    result.unwrap();
    assert!(events.contains(&StreamEvent::TextDelta("Hello".into())));
    assert_eq!(events.last(), Some(&StreamEvent::Stop(StopReason::EndTurn)));
    let sent = requests.recv().unwrap();
    assert!(sent.starts_with("POST /v1/messages "), "{sent}");
    let lower = sent.to_lowercase();
    assert!(lower.contains("x-api-key: test-key") && lower.contains("anthropic-version: 2023-06-01"), "{sent}");
    assert!(sent.contains(r#""model":"some-model""#), "{sent}");
}

#[test]
fn error_statuses_become_typed_errors() {
    let json = r#"{"error":{"message":"slow down","type":"rate_limit_error"}}"#;
    let answer = format!(
        "HTTP/1.1 429 Too Many Requests\r\nretry-after: 3\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{json}",
        json.len()
    );
    let (base, _requests) = server(vec![send(answer)]);
    let (result, events) = stream(&client(&base, ProviderKind::OpenAi), &CancelToken::new());
    assert_eq!(
        result,
        Err(LlmError::RateLimited { retry_after: Some(Duration::from_secs(3)), message: "slow down (rate_limit_error)".into() })
    );
    assert!(events.is_empty());

    let answer = "HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
    let (base, _requests) = server(vec![send(answer)]);
    let (result, _) = stream(&client(&base, ProviderKind::Anthropic), &CancelToken::new());
    assert!(matches!(result, Err(LlmError::Auth(_))), "{result:?}");
    // Keys never show up in errors.
    assert!(!format!("{result:?}").contains("test-key"));
}

#[test]
fn openai_retries_once_with_the_other_output_cap_field() {
    let json = r#"{"error":{"message":"Unsupported parameter: 'max_tokens'. Use 'max_completion_tokens' instead.","code":"unsupported_parameter"}}"#;
    let rejected = format!("HTTP/1.1 400 Bad Request\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{json}", json.len());
    let body = [
        r#"data: {"choices":[{"index":0,"delta":{"content":"ok"}}]}"#,
        r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
        "data: [DONE]",
    ]
    .join("\n\n");
    let (base, requests) = server(vec![send(rejected), send(format!("{SSE_HEAD}{body}\n\n"))]);
    let (result, events) = stream(&client(&base, ProviderKind::OpenAi), &CancelToken::new());
    result.unwrap();
    assert_eq!(events, vec![StreamEvent::TextDelta("ok".into()), StreamEvent::Stop(StopReason::EndTurn)]);
    let first = requests.recv().unwrap();
    let second = requests.recv().unwrap();
    assert!(first.starts_with("POST /chat/completions ") && first.contains(r#""max_tokens":100"#), "{first}");
    assert!(first.to_lowercase().contains("authorization: bearer test-key"), "{first}");
    assert!(second.contains(r#""max_completion_tokens":100"#) && !second.contains(r#""max_tokens""#), "{second}");
}

/// Cancels `cancel` after 200 ms and checks the stream gives up well within a second of it.
fn assert_cancels_quickly(client: &Providers) {
    let cancel = CancelToken::new();
    let remote = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        remote.cancel();
    });
    let start = Instant::now();
    let (result, _) = stream(client, &cancel);
    let elapsed = start.elapsed();
    assert_eq!(result, Err(LlmError::Cancelled));
    assert!(elapsed < Duration::from_secs(1), "took {elapsed:?}");
}

#[test]
fn cancel_returns_quickly_while_the_server_says_nothing() {
    let (base, _requests) = server(vec![silent_after("")]);
    assert_cancels_quickly(&client(&base, ProviderKind::Anthropic));
}

#[test]
fn cancel_returns_quickly_when_the_body_goes_silent() {
    let partial = format!("{SSE_HEAD}data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"Hi\"}}}}]}}\n\n");
    let (base, _requests) = server(vec![silent_after(partial)]);
    assert_cancels_quickly(&client(&base, ProviderKind::OpenAi));
}

#[test]
fn a_refused_connection_is_a_network_error() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let (result, _) = stream(&client(&base, ProviderKind::OpenAi), &CancelToken::new());
    assert!(matches!(result, Err(LlmError::Network(_))), "{result:?}");
}
