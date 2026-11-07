use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use http_body_util::BodyExt;
use rtk_web::{AppState, Config, build_router};
use std::time::Duration;
use tower::ServiceExt;

fn app(max_body: usize) -> axum::Router {
    let config = Config::new(
        "127.0.0.1:0".parse().expect("addr"),
        max_body,
        4,
        Duration::from_secs(5),
    );
    build_router(AppState::new(config))
}

async fn send(app: axum::Router, request: Request<Body>) -> Response {
    app.oneshot(request).await.expect("request runs")
}

async fn text_of(response: Response) -> String {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body collects")
        .to_bytes();
    String::from_utf8(bytes.to_vec()).expect("utf-8 body")
}

fn post(path: &str, body: impl Into<Body>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .body(body.into())
        .expect("request builds")
}

#[tokio::test]
async fn health_reports_ok() {
    let response = send(
        app(1024),
        Request::builder()
            .uri("/health")
            .body(Body::empty())
            .expect("request builds"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = text_of(response).await;
    assert!(body.contains("\"status\":\"ok\""));
}

#[tokio::test]
async fn compress_folds_repetition_and_reports_stats() {
    let input = "warning: unused\nwarning: unused\nwarning: unused\nwarning: unused\n";
    let response = send(app(1024), post("/api/compress", input)).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("x-rtk-chain")
            .and_then(|value| value.to_str().ok()),
        Some("ansi>progress>dedup>base64-elide>cap")
    );
    assert_eq!(
        response
            .headers()
            .get("x-rtk-format")
            .and_then(|value| value.to_str().ok()),
        Some("term")
    );
    let bytes_before = response
        .headers()
        .get("x-rtk-bytes-before")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .expect("bytes before");
    assert_eq!(bytes_before, input.len());

    let body = text_of(response).await;
    assert_eq!(body, "warning: unused\n[x4]\n");
}

#[tokio::test]
async fn lossless_mode_keeps_long_output_intact() {
    let input: String = (0..1200).map(|i| format!("line {i}\n")).collect();
    let response = send(app(1024 * 1024), post("/api/compress?lossless=true", input.clone())).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("x-rtk-lossy")
            .and_then(|value| value.to_str().ok()),
        Some("false")
    );
    assert_eq!(text_of(response).await, input);
}

#[tokio::test]
async fn utf16le_bodies_are_decoded() {
    let mut bytes = vec![0xFF, 0xFE];
    for unit in "错误 error".encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    let response = send(app(1024), post("/api/compress", bytes)).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("x-rtk-charset")
            .and_then(|value| value.to_str().ok()),
        Some("auto")
    );
    assert_eq!(text_of(response).await, "错误 error");
}

#[tokio::test]
async fn bad_mode_is_rejected() {
    let response = send(app(1024), post("/api/compress?mode=turbo", "hello")).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(text_of(response).await.contains("unsupported mode"));
}

#[tokio::test]
async fn oversized_bodies_are_rejected() {
    let response = send(app(16), post("/api/compress", "x".repeat(1024))).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(text_of(response).await.contains("\"error\""));
}

#[tokio::test]
async fn oversized_json_bodies_keep_413() {
    let request = Request::builder()
        .method("POST")
        .uri("/api/compress/json")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(format!("{{\"text\":\"{}\"}}", "x".repeat(1024))))
        .expect("request builds");

    let response = send(app(16), request).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(text_of(response).await.contains("\"error\""));
}

#[tokio::test]
async fn oversized_uploads_keep_413() {
    let response = send(
        app(16),
        multipart_request("/api/compress/file", "big.log", &"x".repeat(1024)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(text_of(response).await.contains("\"error\""));
}

#[tokio::test]
async fn unknown_routes_and_methods_return_json() {
    let not_found = send(
        app(1024),
        Request::builder()
            .uri("/nope")
            .body(Body::empty())
            .expect("request builds"),
    )
    .await;
    assert_eq!(not_found.status(), StatusCode::NOT_FOUND);
    assert!(text_of(not_found).await.contains("\"error\""));

    let not_allowed = send(
        app(1024),
        Request::builder()
            .method("GET")
            .uri("/api/compress")
            .body(Body::empty())
            .expect("request builds"),
    )
    .await;
    assert_eq!(not_allowed.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert!(text_of(not_allowed).await.contains("\"error\""));
}

#[tokio::test]
async fn json_endpoint_returns_structured_stats() {
    let request = Request::builder()
        .method("POST")
        .uri("/api/compress/json")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"text":"a\na\na\na\n","mode":"chain","format":"term"}"#,
        ))
        .expect("request builds");

    let response = send(app(1024), request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = text_of(response).await;
    assert!(body.contains("\"compressed\":\"a\\n[x4]\\n\""));
    assert!(body.contains("\"bytes_before\":8"));
    assert!(body.contains("\"lossy\":true"));
}

#[tokio::test]
async fn auto_format_detects_json() {
    let input = r#"[{"id":1,"name":"a"},{"id":2,"name":"b"}]"#;
    let response = send(
        app(1024),
        post("/api/compress?format=auto&mode=chain", input),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("x-rtk-format")
            .and_then(|value| value.to_str().ok()),
        Some("json")
    );
}
fn header_of<'a>(response: &'a Response, name: &str) -> Option<&'a str> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
}

fn multipart_request(path: &str, filename: &str, content: &str) -> Request<Body> {
    const BOUNDARY: &str = "rtk-test-boundary";
    let body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n{content}\r\n--{BOUNDARY}--\r\n"
    );
    Request::builder()
        .method("POST")
        .uri(path)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
        .expect("request builds")
}

#[tokio::test]
async fn uploaded_log_file_is_compressed() {
    let input = "warn: unused\nwarn: unused\nwarn: unused\nwarn: unused\n";
    let response = send(
        app(1024),
        multipart_request("/api/compress/file", "build.log", input),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header_of(&response, "x-rtk-format"), Some("term"));
    assert_eq!(text_of(response).await, "warn: unused\n[x4]\n");
}

#[tokio::test]
async fn uploaded_json_file_selects_json_format() {
    let response = send(
        app(1024),
        multipart_request(
            "/api/compress/file",
            "data.json",
            r#"[{"id":1},{"id":2}]"#,
        ),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header_of(&response, "x-rtk-format"), Some("json"));
}

#[tokio::test]
async fn uploaded_file_can_return_json() {
    let response = send(
        app(1024),
        multipart_request(
            "/api/compress/file?response=json",
            "build.log",
            "a\na\na\na\n",
        ),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = text_of(response).await;
    assert!(body.contains("\"filename\":\"build.log\""));
    assert!(body.contains("\"compressed\":\"a\\n[x4]\\n\""));
}

#[tokio::test]
async fn multipart_without_file_is_rejected() {
    const BOUNDARY: &str = "rtk-empty";
    let body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"other\"\r\n\r\nx\r\n--{BOUNDARY}--\r\n"
    );
    let request = Request::builder()
        .method("POST")
        .uri("/api/compress/file")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
        .expect("request builds");

    let response = send(app(1024), request).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(text_of(response).await.contains("missing `file`"));
}

#[tokio::test]
async fn binary_upload_is_rejected() {
    let response = send(
        app(1024),
        multipart_request("/api/compress/file", "blob.bin", "\0\0\0"),
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(text_of(response).await.contains("binary"));
}
#[tokio::test]
async fn named_filter_compresses_ls_output() {
    let raw = "total 8\n\
               drwxr-xr-x  2 root root 4096 Nov  7 10:00 src\n\
               -rw-r--r--  1 root root  341 Nov  7 09:00 hello.go\n";
    let response = send(app(4096), post("/api/compress?filter=ls", raw)).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header_of(&response, "x-rtk-filter"), Some("ls"));
    let body = text_of(response).await;
    assert!(body.contains("755  src/"), "{body}");
    assert!(!body.contains("total"), "{body}");
}

#[tokio::test]
async fn unknown_filter_is_rejected() {
    let response = send(app(1024), post("/api/compress?filter=nope", "x")).await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(text_of(response).await.contains("unsupported filter"));
}

#[tokio::test]
async fn filters_endpoint_lists_registered_names() {
    let response = send(
        app(1024),
        Request::builder()
            .uri("/api/filters")
            .body(Body::empty())
            .expect("request builds"),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = text_of(response).await;
    assert!(body.contains("\"ls\""), "{body}");
    assert!(body.contains("\"cargo-test\""), "{body}");
}