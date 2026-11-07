use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{
        DefaultBodyLimit, Multipart, Query, Request, State,
        multipart::{MultipartError, MultipartRejection},
        rejection::{BytesRejection, JsonRejection},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rtk_engine::block::ContentFormat;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Semaphore;

use crate::compress::{self, CompressOptions, CompressOutput, Mode, Stats};
use crate::config::Config;
use crate::encoding::{self, Charset, DecodeError};
use crate::format::{self, FormatSpec};

#[derive(Clone)]
pub struct AppState {
    config: Arc<Config>,
    permits: Arc<Semaphore>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let permits = Arc::new(Semaphore::new(config.max_concurrency()));
        Self {
            config: Arc::new(config),
            permits,
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }
}

pub fn build_router(state: AppState) -> Router {
    let max_body = state.config.max_body();
    Router::new()
        .route("/", get(docs))
        .route("/health", get(health))
        .route("/api/formats", get(formats))
        .route("/api/filters", get(filters))
        .route("/api/compress", post(compress_text))
        .route("/api/compress/json", post(compress_json))
        .route("/api/compress/file", post(compress_file))
        .layer(DefaultBodyLimit::max(max_body))
        .layer(middleware::from_fn(jsonify_routing_errors))
        .with_state(state)
}

async fn jsonify_routing_errors(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    let status = response.status();
    if status != StatusCode::NOT_FOUND && status != StatusCode::METHOD_NOT_ALLOWED {
        return response;
    }

    let message = if status == StatusCode::METHOD_NOT_ALLOWED {
        "method not allowed"
    } else {
        "not found"
    };
    let mut replacement = (status, Json(json!({ "error": message }))).into_response();
    if let Some(allow) = response.headers().get(header::ALLOW) {
        replacement.headers_mut().insert(header::ALLOW, allow.clone());
    }
    replacement
}

async fn docs() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
        include_str!("docs.md"),
    )
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "service": "rtk-web",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn formats() -> Json<Vec<&'static str>> {
    Json(vec!["term", "json", "web", "matches", "lockfile", "auto"])
}

async fn filters() -> Json<Vec<&'static str>> {
    Json(rtk::filter_api::names().to_vec())
}

#[derive(Debug, Deserialize)]
struct CompressQuery {
    format: Option<String>,
    mode: Option<String>,
    lossless: Option<bool>,
    charset: Option<String>,
    filter: Option<String>,
}

struct Prepared {
    output: CompressOutput,
    stats: Stats,
    format: ContentFormat,
    mode: Mode,
    charset: Charset,
    lossless: bool,
    filter: Option<String>,
}

async fn compress_text(
    State(state): State<AppState>,
    Query(query): Query<CompressQuery>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, ApiError> {
    let body = body.map_err(body_rejection)?;
    if body.len() > state.config.max_body() {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "request body exceeds the configured limit",
        ));
    }

    let charset = parse_charset(query.charset.as_deref())?;
    let format = parse_format(query.format.as_deref())?;
    let mode = parse_mode(query.mode.as_deref())?;
    let lossless = query.lossless.unwrap_or(false);
    let filter = parse_filter(query.filter.as_deref())?;

    let prepared = execute(&state, body.to_vec(), charset, format, mode, lossless, filter).await?;
    Ok(text_response(prepared))
}

async fn compress_json(
    State(state): State<AppState>,
    payload: Result<Json<CompressJsonRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(request) = payload.map_err(json_rejection)?;

    let charset = parse_charset(request.charset.as_deref())?;
    let format = parse_format(request.format.as_deref())?;
    let mode = parse_mode(request.mode.as_deref())?;
    let lossless = request.lossless;
    let filter = parse_filter(request.filter.as_deref())?;

    let bytes = request.text.into_bytes();
    if bytes.len() > state.config.max_body() {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "text exceeds the configured limit",
        ));
    }

    let prepared = execute(&state, bytes, charset, format, mode, lossless, filter).await?;
    Ok(Json(json_response(prepared, None)).into_response())
}

async fn compress_file(
    State(state): State<AppState>,
    Query(query): Query<FileQuery>,
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<Response, ApiError> {
    let mut multipart = multipart.map_err(|err| {
        ApiError::bad_request(format!("invalid multipart body: {}", err.body_text()))
    })?;
    let mut upload: Option<(Option<String>, Vec<u8>)> = None;
    while let Some(field) = multipart.next_field().await.map_err(multipart_rejection)? {
        if field.name() != Some("file") {
            continue;
        }
        let name = field.file_name().map(str::to_string);
        let bytes = field.bytes().await.map_err(multipart_rejection)?;
        upload = Some((name, bytes.to_vec()));
        break;
    }

    let Some((file_name, bytes)) = upload else {
        return Err(ApiError::bad_request(
            "missing `file` field in multipart body",
        ));
    };

    if bytes.len() > state.config.max_body() {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "uploaded file exceeds the configured limit",
        ));
    }
    if encoding::looks_binary(&bytes) {
        return Err(ApiError::bad_request(
            "uploaded file looks binary; only text files are supported",
        ));
    }

    let charset = parse_charset(query.charset.as_deref())?;
    let format = match query.format.as_deref() {
        Some(raw) => parse_format(Some(raw))?,
        None => file_name
            .as_deref()
            .map(format::detect_from_filename)
            .unwrap_or(FormatSpec::Known(ContentFormat::Term)),
    };
    let mode = parse_mode(query.mode.as_deref())?;
    let lossless = query.lossless.unwrap_or(false);
    let filter = parse_filter(query.filter.as_deref())?;

    let prepared = execute(&state, bytes, charset, format, mode, lossless, filter).await?;

    match query.response.as_deref() {
        None | Some("text") => Ok(text_response(prepared)),
        Some("json") => Ok(Json(json_response(prepared, file_name)).into_response()),
        Some(other) => Err(ApiError::bad_request(format!(
            "unsupported response `{other}` (use text or json)"
        ))),
    }
}

#[derive(Debug, Deserialize)]
struct FileQuery {
    format: Option<String>,
    mode: Option<String>,
    lossless: Option<bool>,
    charset: Option<String>,
    response: Option<String>,
    filter: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CompressJsonRequest {
    text: String,
    format: Option<String>,
    mode: Option<String>,
    #[serde(default)]
    lossless: bool,
    charset: Option<String>,
    filter: Option<String>,
}

#[derive(Debug, Serialize)]
struct CompressJsonResponse {
    compressed: String,
    format: String,
    mode: String,
    lossless: bool,
    chain: Vec<String>,
    lossy: bool,
    changed: bool,
    stats: Stats,
    #[serde(skip_serializing_if = "Option::is_none")]
    filename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    filter: Option<String>,
}

fn json_response(prepared: Prepared, filename: Option<String>) -> CompressJsonResponse {
    CompressJsonResponse {
        compressed: prepared.output.text,
        format: prepared.format.as_str().to_string(),
        mode: prepared.mode.as_str().to_string(),
        lossless: prepared.lossless,
        chain: prepared.output.chain,
        lossy: prepared.output.lossy,
        changed: prepared.output.changed,
        stats: prepared.stats,
        filename,
        filter: prepared.filter,
    }
}

async fn execute(
    state: &AppState,
    bytes: Vec<u8>,
    charset: Charset,
    format: FormatSpec,
    mode: Mode,
    lossless: bool,
    filter: Option<String>,
) -> Result<Prepared, ApiError> {
    let permit = state
        .permits
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ApiError::internal("compression queue is closed"))?;
    let timeout = state.config.timeout();

    let handle = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let text = encoding::decode(&bytes, charset).map_err(ApiError::from)?;
        let format = match format {
            FormatSpec::Auto => format::detect(&text),
            FormatSpec::Known(value) => value,
        };
        let actual_mode = if lossless { Mode::Chain } else { mode };
        let output = compress::compress(
            &text,
            CompressOptions {
                format,
                mode: actual_mode,
                lossless,
                filter: filter.clone(),
            },
        );
        let stats = Stats::new(&text, &output.text);
        Ok::<_, ApiError>(Prepared {
            output,
            stats,
            format,
            mode: actual_mode,
            charset,
            lossless,
            filter,
        })
    });

    match tokio::time::timeout(timeout, handle).await {
        Ok(Ok(result)) => result,
        Ok(Err(err)) => Err(ApiError::internal(format!("compression task failed: {err}"))),
        Err(_) => Err(ApiError::new(
            StatusCode::GATEWAY_TIMEOUT,
            "compression timed out",
        )),
    }
}

fn parse_charset(value: Option<&str>) -> Result<Charset, ApiError> {
    match value {
        Some(raw) => Charset::parse(raw).ok_or_else(|| {
            ApiError::bad_request(format!(
                "unsupported charset `{raw}` (use auto, utf-8, utf-16le, utf-16be or gbk)"
            ))
        }),
        None => Ok(Charset::Auto),
    }
}

fn parse_format(value: Option<&str>) -> Result<FormatSpec, ApiError> {
    match value {
        Some(raw) => FormatSpec::parse(raw).ok_or_else(|| {
            ApiError::bad_request(format!(
                "unsupported format `{raw}` (use term, json, web, matches, lockfile or auto)"
            ))
        }),
        None => Ok(FormatSpec::Known(ContentFormat::Term)),
    }
}

fn parse_mode(value: Option<&str>) -> Result<Mode, ApiError> {
    match value {
        Some(raw) => Mode::parse(raw).ok_or_else(|| {
            ApiError::bad_request(format!("unsupported mode `{raw}` (use chain or auto)"))
        }),
        None => Ok(Mode::Chain),
    }
}

fn parse_filter(value: Option<&str>) -> Result<Option<String>, ApiError> {
    match value {
        Some(raw) => {
            let name = raw.trim().to_ascii_lowercase();
            if rtk::filter_api::names().iter().any(|known| *known == name) {
                Ok(Some(name))
            } else {
                Err(ApiError::bad_request(format!(
                    "unsupported filter `{raw}`; see GET /api/filters"
                )))
            }
        }
        None => Ok(None),
    }
}

fn text_response(prepared: Prepared) -> Response {
    let mut response = prepared.output.text.clone().into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    insert_header(headers, "x-rtk-format", prepared.format.as_str());
    insert_header(headers, "x-rtk-mode", prepared.mode.as_str());
    insert_header(headers, "x-rtk-charset", prepared.charset.as_str());
    insert_header(headers, "x-rtk-lossless", prepared.lossless);
    insert_header(headers, "x-rtk-filter", prepared.filter.as_deref().unwrap_or("-"));
    insert_header(headers, "x-rtk-lossy", prepared.output.lossy);
    insert_header(headers, "x-rtk-changed", prepared.output.changed);
    insert_header(
        headers,
        "x-rtk-chain",
        if prepared.output.chain.is_empty() {
            "-".to_string()
        } else {
            prepared.output.chain.join(">")
        },
    );
    insert_header(headers, "x-rtk-bytes-before", prepared.stats.bytes_before);
    insert_header(headers, "x-rtk-bytes-after", prepared.stats.bytes_after);
    insert_header(headers, "x-rtk-saved-percent", prepared.stats.saved_percent);
    insert_header(headers, "x-rtk-tokens-before", prepared.stats.tokens_before);
    insert_header(headers, "x-rtk-tokens-after", prepared.stats.tokens_after);
    response
}

fn insert_header<K>(headers: &mut HeaderMap, name: K, value: impl ToString)
where
    K: axum::http::header::IntoHeaderName,
{
    let value = HeaderValue::from_str(&value.to_string())
        .unwrap_or_else(|_| HeaderValue::from_static("invalid"));
    headers.insert(name, value);
}

fn body_rejection(err: BytesRejection) -> ApiError {
    let status = err.status();
    let message = if status == StatusCode::PAYLOAD_TOO_LARGE {
        "request body exceeds the configured limit".to_string()
    } else {
        format!("failed to read request body: {}", err.body_text())
    };
    ApiError::new(status, message)
}

fn json_rejection(err: JsonRejection) -> ApiError {
    if err.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "request body exceeds the configured limit",
        )
    } else {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("invalid JSON body: {}", err.body_text()),
        )
    }
}

fn multipart_rejection(err: MultipartError) -> ApiError {
    if err.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "uploaded file exceeds the configured limit",
        )
    } else {
        ApiError::bad_request(format!("invalid multipart body: {}", err.body_text()))
    }
}

struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }
}

impl From<DecodeError> for ApiError {
    fn from(err: DecodeError) -> Self {
        Self::bad_request(err.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_format_is_term_and_default_mode_is_chain() {
        assert_eq!(
            parse_format(None).ok().map(|f| matches!(f, FormatSpec::Known(ContentFormat::Term))),
            Some(true)
        );
        assert_eq!(parse_mode(None).ok(), Some(Mode::Chain));
        assert_eq!(parse_charset(None).ok(), Some(Charset::Auto));
    }

    #[test]
    fn bad_parameters_are_rejected() {
        assert!(parse_format(Some("xml")).is_err());
        assert!(parse_mode(Some("turbo")).is_err());
        assert!(parse_charset(Some("latin-1")).is_err());
    }
}