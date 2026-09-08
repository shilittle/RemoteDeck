use axum::{
    extract::Request,
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use include_dir::{Dir, include_dir};

// Production builds fail if the real frontend has not been built; no placeholder UI.
static WEB: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../web/dist");

pub(crate) async fn serve(request: Request) -> Response {
    let path = request.uri().path().trim_start_matches('/');
    if request.method() != axum::http::Method::GET && request.method() != axum::http::Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    if path.starts_with("api/")
        || path.starts_with("internal/")
        || path.contains('\\')
        || path.split('/').any(|p| p == ".." || p == ".")
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let file = if path.is_empty() {
        WEB.get_file("index.html")
    } else {
        WEB.get_file(path)
    };
    let Some(file) = file else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mime = mime_guess::from_path(file.path())
        .first_or_octet_stream()
        .to_string();
    let mut response = if request.method() == axum::http::Method::HEAD {
        Vec::<u8>::new().into_response()
    } else {
        file.contents().to_vec().into_response()
    };
    if let Ok(mime) = HeaderValue::from_str(&mime) {
        response.headers_mut().insert("content-type", mime);
    }
    response
}
