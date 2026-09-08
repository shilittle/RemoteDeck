//! Real router/state tests. No browser bridge or mocked business outcomes.
use crate::{
    api::AppContext,
    auth::{Auth, random_token},
    transport::{EventHub, Server, router},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

async fn setup() -> (tempfile::TempDir, Server, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let hub = EventHub::default();
    let context = AppContext::open(dir.path().to_path_buf(), hub.clone()).unwrap();
    let auth = Auth::new(43127, random_token(), None);
    let launch = auth.launch_url().unwrap();
    let server = Server::new(context, auth, hub);
    let response = router(server.clone())
        .oneshot(
            Request::post("/api/v1/auth/exchange")
                .header("host", &server.auth.authority)
                .header("origin", &server.auth.origin)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"ticket":launch.split("#ticket=").nth(1).unwrap()}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let body = decode(response).await;
    let csrf = body["csrfToken"].as_str().unwrap().to_owned();
    (dir, server, cookie, csrf)
}
async fn decode(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
fn post(
    server: &Server,
    cookie: &str,
    csrf: &str,
    path: &str,
    args: Value,
    key: &str,
) -> Request<Body> {
    Request::post(path)
        .header("host", &server.auth.authority)
        .header("origin", &server.auth.origin)
        .header("cookie", cookie)
        .header("x-remotedeck-csrf", csrf)
        .header("idempotency-key", key)
        .header("content-type", "application/json")
        .body(Body::from(args.to_string()))
        .unwrap()
}

#[tokio::test]
async fn browser_boundary_rejects_unauthorized_host_origin_csrf_and_secret_fields() {
    let (_dir, server, cookie, csrf) = setup().await;
    let app = router(server.clone());
    let key = Uuid::new_v4().to_string();
    let response = app
        .clone()
        .oneshot(post(
            &server,
            &cookie,
            &csrf,
            "/api/v1/generate_key",
            json!({"request":{"passphrase":"must-never-enter-an-operation-record"}}),
            &key,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(server.operations.list().is_empty());
    for (header, value, status) in [
        ("cookie", "", StatusCode::UNAUTHORIZED),
        ("host", "evil.example:43127", StatusCode::FORBIDDEN),
        ("origin", "http://localhost:43127", StatusCode::FORBIDDEN),
        ("x-remotedeck-csrf", "wrong", StatusCode::FORBIDDEN),
    ] {
        let mut request = post(
            &server,
            &cookie,
            &csrf,
            "/api/v1/bootstrap",
            json!({}),
            &key,
        );
        request.headers_mut().insert(
            axum::http::HeaderName::from_bytes(header.as_bytes()).unwrap(),
            value.parse().unwrap(),
        );
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            status,
            "{header}"
        );
    }
    let response = app
        .clone()
        .oneshot(post(
            &server,
            &cookie,
            &csrf,
            "/api/v1/start_terminal",
            json!({"hostId":"unknown","rows":30,"cols":80,"password":"must-not-be-accepted"}),
            &key,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = app
        .clone()
        .oneshot(post(
            &server,
            &cookie,
            &csrf,
            "/api/v1/not_registered",
            json!({}),
            &key,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    let mut internal = post(&server, &cookie, &csrf, "/internal/launch", json!({}), &key);
    internal
        .headers_mut()
        .insert("authorization", "Bearer invalid".parse().unwrap());
    assert_eq!(
        app.oneshot(internal).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn simultaneous_identical_creates_commit_once_and_bootstrap_has_monotonic_revision() {
    let (_dir, server, cookie, csrf) = setup().await;
    let app = router(server.clone());
    let draft = json!({"draft":{"alias":"fixture","hostname":"192.0.2.55","port":22,"username":"tester","monitorEnabled":false}});
    let key = Uuid::new_v4().to_string();
    let first = app.clone().oneshot(post(
        &server,
        &cookie,
        &csrf,
        "/api/v1/save_host",
        draft.clone(),
        &key,
    ));
    let second = app.clone().oneshot(post(
        &server,
        &cookie,
        &csrf,
        "/api/v1/save_host",
        draft,
        &key,
    ));
    let (first, second) = tokio::join!(first, second);
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(decode(first).await, decode(second).await);
    let snapshot = app
        .clone()
        .oneshot(post(
            &server,
            &cookie,
            &csrf,
            "/api/v1/bootstrap",
            json!({}),
            &Uuid::new_v4().to_string(),
        ))
        .await
        .unwrap();
    let snapshot = decode(snapshot).await;
    assert_eq!(snapshot["hosts"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["configRevision"], 1);
    let changed = app
        .oneshot(post(
            &server,
            &cookie,
            &csrf,
            "/api/v1/save_host",
            json!({"draft":{"alias":"different"}}),
            &key,
        ))
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn accepted_long_operation_outlives_http_request_and_is_visible_to_reopened_ui() {
    let (_dir, server, cookie, csrf) = setup().await;
    let app = router(server.clone());
    let key = Uuid::new_v4().to_string();
    let response = app
        .clone()
        .oneshot(post(
            &server,
            &cookie,
            &csrf,
            "/api/v1/test_connection",
            json!({"hostId":"missing-host"}),
            &key,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body = decode(response).await;
    let id = body["operationId"].as_str().unwrap();
    let completed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let state = server.operations.get(id).unwrap();
            if state["state"] == "failed" {
                break state;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(completed["error"]["code"], "not_found");
    let repeated = app
        .oneshot(post(
            &server,
            &cookie,
            &csrf,
            "/api/v1/test_connection",
            json!({"hostId":"missing-host"}),
            &key,
        ))
        .await
        .unwrap();
    assert_eq!(decode(repeated).await["operationId"], id);
    assert_eq!(server.operations.list().len(), 1);
}

#[tokio::test]
async fn shutdown_closes_admission_before_any_new_mutation() {
    let (_dir, server, cookie, csrf) = setup().await;
    server.request_stop();
    let response = router(server.clone()).oneshot(post(&server, &cookie, &csrf, "/api/v1/save_host", json!({"draft":{"alias":"forbidden","hostname":"192.0.2.55","port":22,"username":"tester"}}), &Uuid::new_v4().to_string())).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(server.context.state.repository.snapshot().hosts.is_empty());
    assert!(server.admission.cancel_and_wait().await);
}
