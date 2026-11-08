// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the web-portal control-plane slice: `CreatePortal`,
//! `GetPortal`, `ListPortals`, `DeletePortal`, `CreateUserSettings`, `GetUserSettings`,
//! `AssociateUserSettings` and `DeleteUserSettings`.
//!
//! Every case drives the real `MultiServiceRouter` over a single shared engine, so the
//! assertions are about what a client observes on the wire, not about internal state.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
// The service's own capacity and page-size bounds, read from the crate rather than
// restated, so a test can never disagree with the handler about them.
use service_com_amazonaws_workspacesweb_awserminecontrolplaneservice::state::{
    MAX_LIST_PAGE_SIZE, MAX_PORTALS, MAX_USER_SETTINGS,
};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header in the shape every signed AWS SDK request carries.
/// Its credential scope is what routes the request to this service and what the handlers
/// read the ARN region out of; the signature itself is never checked here because the
/// router is built with request authentication off.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/workspaces-web/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
    )
}

/// The emulator under test: one shared engine, this one service, authentication off.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_workspacesweb_awserminecontrolplaneservice::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Percent-encode a value for use inside a URI path, leaving `/` as a path separator —
/// what an AWS SDK does for a greedy `@httpLabel`.
fn encode_path(value: &str) -> String {
    encode(value, true)
}

/// Percent-encode a value for use as a query-string value, where `/` is encoded too.
fn encode_query(value: &str) -> String {
    encode(value, false)
}

fn encode(value: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~');
        if unreserved || (keep_slash && byte == b'/') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// One call against the emulator: the HTTP status and the parsed JSON body (`Value::Null`
/// when the body is empty).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_in(router, "us-east-1", method, path_and_query, body).await
}

async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|b| b.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://workspaces-web.{region}.amazonaws.com{path_and_query}"
        ))
        .header("authorization", authorization(region))
        .header("content-type", "application/json")
        .header("x-amz-date", "20260101T000000Z")
        .body(Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            Value::String(String::from_utf8_lossy(&bytes).to_string())
        })
    };
    (status, parsed)
}

/// The wire error code of an error response, as a client reads it.
fn error_code(status: u16, body: &Value) -> String {
    let code = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("")
        .to_string();
    format!("{status} {code}")
}

/// Create a portal with the given request body, asserting it succeeded.
async fn create_portal(router: &MultiServiceRouter, body: Value) -> (String, String) {
    let (status, out) = call(router, "POST", "/portals", Some(body.clone())).await;
    assert_eq!(
        status, 200,
        "case {body}: CreatePortal must answer 200, got {status} {out}"
    );
    (
        out["portalArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {body}: CreatePortal must report a portalArn: {out}"))
            .to_string(),
        out["portalEndpoint"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {body}: CreatePortal must report a portalEndpoint: {out}")
            })
            .to_string(),
    )
}

async fn get_portal(router: &MultiServiceRouter, portal_arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/portals/{}", encode_path(portal_arn)),
        None,
    )
    .await
}

async fn list_portals(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("/portals{query}"), None).await
}

/// Create a user-settings resource with the given request body, asserting it succeeded.
async fn create_user_settings(router: &MultiServiceRouter, body: Value) -> String {
    let (status, out) = call(router, "POST", "/userSettings", Some(body.clone())).await;
    assert_eq!(
        status, 200,
        "case {body}: CreateUserSettings must answer 200, got {status} {out}"
    );
    out["userSettingsArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {body}: must report a userSettingsArn: {out}"))
        .to_string()
}

async fn get_user_settings(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/userSettings/{}", encode_path(arn)),
        None,
    )
    .await
}

async fn associate_user_settings(
    router: &MultiServiceRouter,
    portal_arn: &str,
    user_settings_arn: &str,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!(
            "/portals/{}/userSettings?userSettingsArn={}",
            encode_path(portal_arn),
            encode_query(user_settings_arn)
        ),
        None,
    )
    .await
}

/// A request body that satisfies every required `CreateUserSettings` member, so a case
/// can vary one member at a time.
fn minimal_user_settings() -> Value {
    json!({
        "copyAllowed": "Enabled",
        "pasteAllowed": "Enabled",
        "downloadAllowed": "Enabled",
        "uploadAllowed": "Enabled",
        "printAllowed": "Enabled"
    })
}

/// A well-formed ARN of `kind` that names a resource no call ever created: the modelled
/// `ARN` pattern is satisfied, so the request reaches the handler and the answer is about
/// the store rather than about the shape of the ARN.
fn absent_arn(kind: &str) -> String {
    format!("arn:aws:workspaces-web:us-east-1:000000000000:{kind}/00000000-0000-4000-8000-000000000000")
}

/// A smoke case that pins the whole happy path of the slice in order, so a failure here
/// localises before the table-driven cases below run.
#[tokio::test]
async fn the_portal_slice_round_trips_end_to_end() {
    let router = emulator();

    let (portal_arn, portal_endpoint) = create_portal(
        &router,
        json!({
            "displayName": "demo-portal",
            "authenticationType": "Standard",
            "instanceType": "standard.regular",
            "maxConcurrentSessions": 10,
            "clientToken": "tok-1"
        }),
    )
    .await;

    let (status, out) = get_portal(&router, &portal_arn).await;
    assert_eq!(status, 200, "GetPortal must answer 200, got {status} {out}");
    let portal = &out["portal"];
    assert_eq!(
        portal["portalArn"].as_str(),
        Some(portal_arn.as_str()),
        "GetPortal must report the stored ARN, got {portal}"
    );
    assert_eq!(
        portal["portalEndpoint"].as_str(),
        Some(portal_endpoint.as_str()),
        "GetPortal must report the endpoint create minted, got {portal}"
    );
    assert_eq!(
        portal["displayName"].as_str(),
        Some("demo-portal"),
        "GetPortal must report the stored displayName, got {portal}"
    );
    assert_eq!(
        portal["maxConcurrentSessions"].as_i64(),
        Some(10),
        "GetPortal must report the stored maxConcurrentSessions, got {portal}"
    );
    assert_eq!(
        portal["portalStatus"].as_str(),
        Some("Incomplete"),
        "a portal with no network and no user settings is Incomplete, got {portal}"
    );
    assert!(
        portal.get("creationDate").is_some_and(|d| !d.is_null()),
        "GetPortal must report a creationDate, got {portal}"
    );

    let (status, out) = call(
        &router,
        "POST",
        "/userSettings",
        Some(json!({
            "copyAllowed": "Enabled",
            "pasteAllowed": "Enabled",
            "downloadAllowed": "Disabled",
            "uploadAllowed": "Disabled",
            "printAllowed": "Enabled",
            "idleDisconnectTimeoutInMinutes": 15
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateUserSettings must answer 200, got {status} {out}"
    );
    let user_settings_arn = out["userSettingsArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserSettings must report a userSettingsArn: {out}"))
        .to_string();

    let (status, out) = call(
        &router,
        "PUT",
        &format!(
            "/portals/{}/userSettings?userSettingsArn={}",
            encode_path(&portal_arn),
            encode_query(&user_settings_arn)
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "AssociateUserSettings must answer 200, got {status} {out}"
    );
    assert_eq!(
        (
            out["portalArn"].as_str(),
            out["userSettingsArn"].as_str()
        ),
        (Some(portal_arn.as_str()), Some(user_settings_arn.as_str())),
        "AssociateUserSettings must echo the stored pair, got {out}"
    );

    let (_, out) = get_portal(&router, &portal_arn).await;
    assert_eq!(
        out["portal"]["userSettingsArn"].as_str(),
        Some(user_settings_arn.as_str()),
        "GetPortal must report the association, got {out}"
    );

    let (status, out) = list_portals(&router, "").await;
    assert_eq!(status, 200, "ListPortals must answer 200, got {status} {out}");
    let arns: Vec<&str> = out["portals"]
        .as_array()
        .expect("ListPortals must report a portals list")
        .iter()
        .filter_map(|p| p["portalArn"].as_str())
        .collect();
    assert_eq!(
        arns,
        vec![portal_arn.as_str()],
        "ListPortals must report exactly the one stored portal, got {out}"
    );

    let (status, out) = call(
        &router,
        "DELETE",
        &format!("/portals/{}", encode_path(&portal_arn)),
        None,
    )
    .await;
    assert_eq!(status, 200, "DeletePortal must answer 200, got {status} {out}");

    let (status, out) = get_portal(&router, &portal_arn).await;
    assert_eq!(
        error_code(status, &out),
        "404 ResourceNotFoundException",
        "GetPortal after DeletePortal must be a 404 ResourceNotFoundException, got {out}"
    );

    let (_, out) = list_portals(&router, "").await;
    let arns: BTreeSet<&str> = out["portals"]
        .as_array()
        .expect("ListPortals must report a portals list")
        .iter()
        .filter_map(|p| p["portalArn"].as_str())
        .collect();
    assert!(
        !arns.contains(portal_arn.as_str()),
        "ListPortals must not report a deleted portal, got {out}"
    );
}

/// The portal id inside a `portal/<uuid>` ARN.
fn portal_id(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or("")
}

/// The members `CreatePortal` stores and `GetPortal` must report back unchanged.
/// `authenticationType` is the only one the model documents a default for.
const PORTAL_ECHOED_MEMBERS: &[&str] = &[
    "additionalEncryptionContext",
    "authenticationType",
    "customerManagedKey",
    "displayName",
    "instanceType",
    "maxConcurrentSessions",
    "portalCustomDomain",
];

/// RULE: every member of a stored portal is reported from the store - identical to what
/// create persisted, absent when create persisted nothing, and `Standard` for the one
/// member (`authenticationType`) whose model documents that default. The recorded
/// request is one row of this table.
#[tokio::test]
async fn every_portal_member_is_reported_from_the_store() {
    let long_name = "n".repeat(64);
    let cases: Vec<(&str, Value)> = vec![
        ("shortest display name", json!({"displayName": "p"})),
        ("display name at the 64-character maximum", json!({"displayName": long_name})),
        ("no optional member at all", json!({})),
        (
            "IAM Identity Center, largest instance, session cap at the maximum",
            json!({
                "displayName": "idc",
                "authenticationType": "IAM_Identity_Center",
                "instanceType": "standard.xlarge",
                "maxConcurrentSessions": 5000
            }),
        ),
        (
            "large instance, session cap at the minimum",
            json!({
                "displayName": "one-session",
                "authenticationType": "Standard",
                "instanceType": "standard.large",
                "maxConcurrentSessions": 1
            }),
        ),
        (
            "customer managed key with a two-entry encryption context",
            json!({
                "displayName": "encrypted",
                "instanceType": "standard.regular",
                "customerManagedKey":
                    "arn:aws:kms:us-east-1:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab",
                "additionalEncryptionContext": {"team": "web", "env": "test"}
            }),
        ),
        (
            "custom domain and tags",
            json!({
                "displayName": "branded",
                "portalCustomDomain": "portal.example.com",
                "tags": [{"Key": "owner", "Value": "team"}, {"Key": "tier", "Value": "gold"}]
            }),
        ),
    ];

    for (case, body) in cases {
        let router = emulator();
        let (portal_arn, portal_endpoint) = create_portal(&router, body.clone()).await;

        let id = portal_id(&portal_arn).to_string();
        assert_eq!(
            portal_arn,
            format!("arn:aws:workspaces-web:us-east-1:000000000000:portal/{id}"),
            "case {case:?}: the ARN must be the canonical workspaces-web portal ARN, \
             got {portal_arn:?}"
        );
        assert!(
            id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
            "case {case:?}: the ARN resource id must be a 36-character UUID, got {id:?}"
        );
        assert_eq!(
            portal_endpoint,
            format!("{id}.workspaces-web.us-east-1.amazonaws.com"),
            "case {case:?}: the endpoint must be built from the portal's own id and \
             region, got {portal_endpoint:?}"
        );

        let (status, out) = get_portal(&router, &portal_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPortal must answer 200, got {status} {out}"
        );
        let portal = &out["portal"];

        for member in PORTAL_ECHOED_MEMBERS {
            let expected = match (*member, body.get(member)) {
                // The model's own doc for `CreatePortal$authenticationType`: "Defaults
                // to Standard."
                ("authenticationType", None) => Some(json!("Standard")),
                (_, other) => other.cloned(),
            };
            assert_eq!(
                portal.get(member).cloned().filter(|v| !v.is_null()),
                expected,
                "case {case:?}: GetPortal must report {member} from the store, got \
                 {actual:?}",
                actual = portal.get(member)
            );
        }

        assert_eq!(
            portal["portalArn"].as_str(),
            Some(portal_arn.as_str()),
            "case {case:?}: GetPortal must report the stored ARN, got {portal}"
        );
        assert_eq!(
            portal["portalEndpoint"].as_str(),
            Some(portal_endpoint.as_str()),
            "case {case:?}: GetPortal must report the stored endpoint, got {portal}"
        );
        assert_eq!(
            portal["portalStatus"].as_str(),
            Some("Incomplete"),
            "case {case:?}: a portal missing its network and user settings is \
             Incomplete, got {portal}"
        );
        // The model declares exactly one value for each of these two enums
        // (`smithy_query shape=RendererType,BrowserType`).
        assert_eq!(
            (
                portal["rendererType"].as_str(),
                portal["browserType"].as_str()
            ),
            (Some("AppStream"), Some("Chrome")),
            "case {case:?}: the renderer and browser types are the model's single \
             values, got {portal}"
        );
        for absent in [
            "browserSettingsArn",
            "dataProtectionSettingsArn",
            "ipAccessSettingsArn",
            "networkSettingsArn",
            "sessionLoggerArn",
            "trustStoreArn",
            "userAccessLoggingSettingsArn",
            "userSettingsArn",
        ] {
            assert!(
                portal.get(absent).is_none_or(Value::is_null),
                "case {case:?}: {absent} must stay absent until it is associated, got \
                 {portal}"
            );
        }
    }
}

/// RULE: the ARN and endpoint a create mints carry the region of the credential scope
/// the caller signed the request with.
#[tokio::test]
async fn minted_arns_carry_the_requests_own_region() {
    for region in ["us-east-1", "eu-west-2", "ap-southeast-1"] {
        let router = emulator();
        let (status, out) = call_in(
            &router,
            region,
            "POST",
            "/portals",
            Some(json!({"displayName": "regional"})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: CreatePortal must answer 200, got {status} {out}"
        );
        let arn = out["portalArn"].as_str().unwrap_or_default().to_string();
        let endpoint = out["portalEndpoint"].as_str().unwrap_or_default();
        let id = portal_id(&arn);
        assert_eq!(
            arn,
            format!("arn:aws:workspaces-web:{region}:000000000000:portal/{id}"),
            "case {region:?}: the ARN must carry the request's region, got {arn:?}"
        );
        assert_eq!(
            endpoint,
            format!("{id}.workspaces-web.{region}.amazonaws.com"),
            "case {region:?}: the endpoint must carry the request's region, got \
             {endpoint:?}"
        );
    }
}

/// RULE: a value outside a modelled constraint - an enum value set, a `@length` bound or
/// a `@range` bound - is refused with HTTP 400 `ValidationException` and nothing is
/// stored. Each range is exercised on both published bounds and one step past each.
#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused() {
    let too_long = "n".repeat(65);
    let refused: Vec<(&str, &str, Value)> = vec![
        ("empty displayName", "/portals", json!({"displayName": ""})),
        (
            "displayName one past the 64-character maximum",
            "/portals",
            json!({"displayName": too_long}),
        ),
        (
            "maxConcurrentSessions one below the minimum",
            "/portals",
            json!({"displayName": "p", "maxConcurrentSessions": 0}),
        ),
        (
            "maxConcurrentSessions one past the maximum",
            "/portals",
            json!({"displayName": "p", "maxConcurrentSessions": 5001}),
        ),
        (
            "authenticationType outside the value set",
            "/portals",
            json!({"displayName": "p", "authenticationType": "Nope"}),
        ),
        (
            "instanceType outside the value set",
            "/portals",
            json!({"displayName": "p", "instanceType": "standard.huge"}),
        ),
        (
            "copyAllowed outside the EnabledType value set",
            "/userSettings",
            json!({
                "copyAllowed": "Maybe", "pasteAllowed": "Enabled",
                "downloadAllowed": "Enabled", "uploadAllowed": "Enabled",
                "printAllowed": "Enabled"
            }),
        ),
        (
            "disconnectTimeoutInMinutes one below the minimum",
            "/userSettings",
            json!({
                "copyAllowed": "Enabled", "pasteAllowed": "Enabled",
                "downloadAllowed": "Enabled", "uploadAllowed": "Enabled",
                "printAllowed": "Enabled", "disconnectTimeoutInMinutes": 0
            }),
        ),
        (
            "disconnectTimeoutInMinutes one past the maximum",
            "/userSettings",
            json!({
                "copyAllowed": "Enabled", "pasteAllowed": "Enabled",
                "downloadAllowed": "Enabled", "uploadAllowed": "Enabled",
                "printAllowed": "Enabled", "disconnectTimeoutInMinutes": 601
            }),
        ),
        (
            "idleDisconnectTimeoutInMinutes one past the maximum",
            "/userSettings",
            json!({
                "copyAllowed": "Enabled", "pasteAllowed": "Enabled",
                "downloadAllowed": "Enabled", "uploadAllowed": "Enabled",
                "printAllowed": "Enabled", "idleDisconnectTimeoutInMinutes": 61
            }),
        ),
    ];

    for (case, path, body) in refused {
        let router = emulator();
        let (status, out) = call(&router, "POST", path, Some(body)).await;
        assert_eq!(
            error_code(status, &out),
            "400 ValidationException",
            "case {case:?}: must be refused with 400 ValidationException, got {out}"
        );
        let (_, listed) = list_portals(&router, "").await;
        assert_eq!(
            listed["portals"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a refused request must store nothing, got {listed}"
        );
    }

    // The published bounds themselves are accepted, so the refusals above are about the
    // bound and not about the member being present at all.
    let accepted: Vec<(&str, &str, Value)> = vec![
        (
            "maxConcurrentSessions at the minimum",
            "/portals",
            json!({"displayName": "p", "maxConcurrentSessions": 1}),
        ),
        (
            "maxConcurrentSessions at the maximum",
            "/portals",
            json!({"displayName": "p", "maxConcurrentSessions": 5000}),
        ),
        (
            "disconnect timeouts at their minimums",
            "/userSettings",
            json!({
                "copyAllowed": "Enabled", "pasteAllowed": "Enabled",
                "downloadAllowed": "Enabled", "uploadAllowed": "Enabled",
                "printAllowed": "Enabled",
                "disconnectTimeoutInMinutes": 1, "idleDisconnectTimeoutInMinutes": 0
            }),
        ),
        (
            "disconnect timeouts at their maximums",
            "/userSettings",
            json!({
                "copyAllowed": "Disabled", "pasteAllowed": "Disabled",
                "downloadAllowed": "Disabled", "uploadAllowed": "Disabled",
                "printAllowed": "Disabled",
                "disconnectTimeoutInMinutes": 600, "idleDisconnectTimeoutInMinutes": 60
            }),
        ),
    ];
    for (case, path, body) in accepted {
        let router = emulator();
        let (status, out) = call(&router, "POST", path, Some(body)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a value on the published bound must be accepted, got \
             {status} {out}"
        );
    }
}

/// RULE: a repeated `@idempotencyToken` returns the resource the first call created; a
/// different token, or none at all, creates a new one.
#[tokio::test]
async fn a_replayed_client_token_never_mints_a_second_resource() {
    let cases: &[(&str, &str)] = &[
        ("short token", "t"),
        ("uuid-shaped token", "3f2504e0-4f89-11d3-9a0c-0305e82c3301"),
        ("token with punctuation", "client/token:1 retry"),
    ];
    for (case, token) in cases {
        let router = emulator();
        let body = json!({"displayName": "idempotent", "clientToken": token});
        let (first_arn, first_endpoint) = create_portal(&router, body.clone()).await;
        let (second_arn, second_endpoint) = create_portal(&router, body.clone()).await;
        assert_eq!(
            (second_arn.as_str(), second_endpoint.as_str()),
            (first_arn.as_str(), first_endpoint.as_str()),
            "case {case:?}: a replayed clientToken must return the first portal"
        );

        let (_, listed) = list_portals(&router, "").await;
        assert_eq!(
            listed["portals"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: a replayed clientToken must leave exactly one portal, got \
             {listed}"
        );

        // A different token is a different request, so it mints a second portal.
        let (other_arn, _) = create_portal(
            &router,
            json!({"displayName": "idempotent", "clientToken": format!("{token}-other")}),
        )
        .await;
        assert_ne!(
            other_arn, first_arn,
            "case {case:?}: a different clientToken must mint a new portal"
        );

        // With no token there is nothing to replay, so every call mints one.
        let (a, _) = create_portal(&router, json!({"displayName": "untokened"})).await;
        let (b, _) = create_portal(&router, json!({"displayName": "untokened"})).await;
        assert_ne!(
            a, b,
            "case {case:?}: without a clientToken every CreatePortal mints a new portal"
        );

        let (_, listed) = list_portals(&router, "").await;
        assert_eq!(
            listed["portals"].as_array().map(Vec::len),
            Some(4),
            "case {case:?}: two tokens plus two untokened creates make four portals, \
             got {listed}"
        );
    }

    // The same rule holds for the other create in this slice.
    let router = emulator();
    let body = json!({
        "copyAllowed": "Enabled", "pasteAllowed": "Enabled", "downloadAllowed": "Enabled",
        "uploadAllowed": "Enabled", "printAllowed": "Enabled", "clientToken": "us-1"
    });
    let first = create_user_settings(&router, body.clone()).await;
    let second = create_user_settings(&router, body).await;
    assert_eq!(
        first, second,
        "case \"CreateUserSettings replay\": a replayed clientToken must return the \
         first user-settings ARN"
    );
}

/// RULE: a list reports every stored resource exactly once in one deterministic total
/// order, cut into pages of at most `maxResults`, with a `nextToken` emitted only while
/// more remain and resuming exactly after the previous page's last item.
#[tokio::test]
async fn list_portals_pages_the_whole_collection_exactly_once() {
    let router = emulator();
    let total = 7;
    let mut created = BTreeSet::new();
    for i in 0..total {
        let (arn, _) = create_portal(&router, json!({"displayName": format!("p-{i}")})).await;
        created.insert(arn);
    }

    let (_, unpaged) = list_portals(&router, "").await;
    let full_order: Vec<String> = unpaged["portals"]
        .as_array()
        .expect("ListPortals must report a portals list")
        .iter()
        .filter_map(|p| p["portalArn"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        full_order.iter().cloned().collect::<BTreeSet<_>>(),
        created,
        "case \"unpaged\": one page must report every stored portal, got {unpaged}"
    );
    assert!(
        unpaged.get("nextToken").is_none_or(Value::is_null),
        "case \"unpaged\": a page that reported everything must emit no nextToken, got \
         {unpaged}"
    );

    for page_size in [1usize, 2, 3, total, total + 1] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                None => format!("?maxResults={page_size}"),
                Some(t) => format!("?maxResults={page_size}&nextToken={}", encode_query(t)),
            };
            let (status, out) = list_portals(&router, &query).await;
            assert_eq!(
                status, 200,
                "case page_size={page_size}: ListPortals must answer 200, got {status} \
                 {out}"
            );
            let page: Vec<String> = out["portals"]
                .as_array()
                .expect("ListPortals must report a portals list")
                .iter()
                .filter_map(|p| p["portalArn"].as_str().map(str::to_string))
                .collect();
            assert!(
                page.len() <= page_size,
                "case page_size={page_size}: a page must hold at most maxResults items, \
                 got {len}",
                len = page.len()
            );
            seen.extend(page.iter().cloned());
            token = out
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            assert_eq!(
                token.is_some(),
                seen.len() < total,
                "case page_size={page_size}: a nextToken must be emitted exactly while \
                 portals remain, got {out}"
            );
            pages += 1;
            assert!(
                pages <= total + 2,
                "case page_size={page_size}: paging must terminate, got {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, full_order,
            "case page_size={page_size}: the pages must reproduce the unpaged order with \
             no repeat and no gap"
        );
    }
}

/// RULE: a `nextToken` this service did not issue is not a position in the page order, so
/// it is refused with HTTP 400 `ValidationException`.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let router = emulator();
    create_portal(&router, json!({"displayName": "p"})).await;
    let cases: &[(&str, &str)] = &[
        ("not hexadecimal", "not-a-token"),
        ("hex of text with no separator", "6162630a"),
        ("hex of a non-numeric position", "7a7a7a23617272"),
        ("odd-length hex", "abc"),
    ];
    for (case, token) in cases {
        let (status, out) = list_portals(&router, &format!("?nextToken={}", encode_query(token))).await;
        assert_eq!(
            error_code(status, &out),
            "400 ValidationException",
            "case {case:?}: an unissued nextToken must be refused with 400 \
             ValidationException, got {out}"
        );
    }
}

/// The members `CreateUserSettings` stores and `GetUserSettings` must report back.
const USER_SETTINGS_ECHOED_MEMBERS: &[&str] = &[
    "additionalEncryptionContext",
    "cookieSynchronizationConfiguration",
    "copyAllowed",
    "customerManagedKey",
    "deepLinkAllowed",
    "disconnectTimeoutInMinutes",
    "downloadAllowed",
    "idleDisconnectTimeoutInMinutes",
    "pasteAllowed",
    "printAllowed",
    "toolbarConfiguration",
    "uploadAllowed",
    "webAuthnAllowed",
];

/// RULE: every member of a stored user-settings resource is reported from the store,
/// absent when create persisted nothing.
#[tokio::test]
async fn every_user_settings_member_is_reported_from_the_store() {
    let cases: Vec<(&str, Value)> = vec![
        ("all five permissions enabled", minimal_user_settings()),
        (
            "all five permissions disabled",
            json!({
                "copyAllowed": "Disabled", "pasteAllowed": "Disabled",
                "downloadAllowed": "Disabled", "uploadAllowed": "Disabled",
                "printAllowed": "Disabled"
            }),
        ),
        (
            "mixed permissions with both timeouts",
            json!({
                "copyAllowed": "Enabled", "pasteAllowed": "Disabled",
                "downloadAllowed": "Enabled", "uploadAllowed": "Disabled",
                "printAllowed": "Enabled",
                "disconnectTimeoutInMinutes": 42,
                "idleDisconnectTimeoutInMinutes": 15
            }),
        ),
        (
            "deep links, WebAuthn, toolbar and cookie synchronization",
            json!({
                "copyAllowed": "Enabled", "pasteAllowed": "Enabled",
                "downloadAllowed": "Disabled", "uploadAllowed": "Disabled",
                "printAllowed": "Enabled",
                "deepLinkAllowed": "Enabled",
                "webAuthnAllowed": "Disabled",
                // Values from the model's own enums: smithy_query
                // shape=ToolbarType,VisualMode,ToolbarItem,MaxDisplayResolution.
                "toolbarConfiguration": {
                    "toolbarType": "Docked",
                    "visualMode": "Dark",
                    "hiddenToolbarItems": ["Webcam", "Microphone"],
                    "maxDisplayResolution": "size1920X1080"
                },
                "cookieSynchronizationConfiguration": {
                    "allowlist": [{"domain": "example.com", "name": "sid", "path": "/"}],
                    "blocklist": [{"domain": "ads.example.com"}]
                },
                "customerManagedKey":
                    "arn:aws:kms:us-east-1:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab",
                "additionalEncryptionContext": {"team": "web"}
            }),
        ),
    ];

    for (case, body) in cases {
        let router = emulator();
        let arn = create_user_settings(&router, body.clone()).await;
        assert!(
            arn.starts_with("arn:aws:workspaces-web:us-east-1:000000000000:userSettings/"),
            "case {case:?}: the minted ARN must name a userSettings resource, got {arn:?}"
        );

        let (status, out) = get_user_settings(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetUserSettings must answer 200, got {status} {out}"
        );
        let settings = &out["userSettings"];
        assert_eq!(
            settings["userSettingsArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: GetUserSettings must report the stored ARN, got {settings}"
        );
        for member in USER_SETTINGS_ECHOED_MEMBERS {
            assert_eq!(
                settings.get(member).cloned().filter(|v| !v.is_null()),
                body.get(member).cloned(),
                "case {case:?}: GetUserSettings must report {member} from the store, got \
                 {actual:?}",
                actual = settings.get(member)
            );
        }
        assert_eq!(
            settings["associatedPortalArns"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a fresh user-settings resource is associated with no portal, \
             got {settings}"
        );
    }
}

/// RULE: an association is stored on both resources, so each reports the other; and a
/// resource another still references cannot be deleted.
#[tokio::test]
async fn an_association_is_stored_on_both_resources() {
    let router = emulator();
    let (portal_a, _) = create_portal(&router, json!({"displayName": "a"})).await;
    let (portal_b, _) = create_portal(&router, json!({"displayName": "b"})).await;
    let settings_one = create_user_settings(&router, minimal_user_settings()).await;
    let settings_two = create_user_settings(&router, minimal_user_settings()).await;

    // One user-settings resource serves many portals.
    for portal in [&portal_a, &portal_b] {
        let (status, out) = associate_user_settings(&router, portal, &settings_one).await;
        assert_eq!(
            status, 200,
            "case {portal:?}: AssociateUserSettings must answer 200, got {status} {out}"
        );
        let (_, got) = get_portal(&router, portal).await;
        assert_eq!(
            got["portal"]["userSettingsArn"].as_str(),
            Some(settings_one.as_str()),
            "case {portal:?}: the portal must report the association, got {got}"
        );
    }
    let (_, got) = get_user_settings(&router, &settings_one).await;
    let associated: BTreeSet<&str> = got["userSettings"]["associatedPortalArns"]
        .as_array()
        .expect("associatedPortalArns must be reported")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(
        associated,
        BTreeSet::from([portal_a.as_str(), portal_b.as_str()]),
        "case \"both portals\": the user settings must report both portals, got {got}"
    );

    // Re-associating the same pair is idempotent, not a second entry.
    associate_user_settings(&router, &portal_a, &settings_one).await;
    let (_, got) = get_user_settings(&router, &settings_one).await;
    assert_eq!(
        got["userSettings"]["associatedPortalArns"]
            .as_array()
            .map(Vec::len),
        Some(2),
        "case \"repeat association\": re-associating the same pair must not duplicate \
         the back-reference, got {got}"
    );

    // A resource a portal still references cannot be deleted.
    let (status, out) = call(
        &router,
        "DELETE",
        &format!("/userSettings/{}", encode_path(&settings_one)),
        None,
    )
    .await;
    assert_eq!(
        error_code(status, &out),
        "409 ConflictException",
        "case \"still associated\": DeleteUserSettings must be refused with 409 \
         ConflictException, got {out}"
    );

    // A portal holds at most one user-settings resource and the binding is not replaced
    // in place, so associating a different one is a conflict.
    let (status, out) = associate_user_settings(&router, &portal_a, &settings_two).await;
    assert_eq!(
        error_code(status, &out),
        "409 ConflictException",
        "case \"second resource\": associating a different resource onto an \
         already-associated portal must be refused with 409 ConflictException, got {out}"
    );
    let (_, got) = get_portal(&router, &portal_a).await;
    assert_eq!(
        got["portal"]["userSettingsArn"].as_str(),
        Some(settings_one.as_str()),
        "case \"second resource\": the refused association must leave the portal's \
         binding as it was, got {got}"
    );
    let (_, got) = get_user_settings(&router, &settings_two).await;
    assert_eq!(
        got["userSettings"]["associatedPortalArns"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "case \"second resource\": the refused association must record nothing on the \
         other resource, got {got}"
    );

    // Deleting the portals releases both resources for deletion.
    for portal in [&portal_a, &portal_b] {
        let (status, _) = call(
            &router,
            "DELETE",
            &format!("/portals/{}", encode_path(portal)),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {portal:?}: DeletePortal must answer 200");
    }
    for settings in [&settings_one, &settings_two] {
        let (status, out) = call(
            &router,
            "DELETE",
            &format!("/userSettings/{}", encode_path(settings)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {settings:?}: once no portal references it, DeleteUserSettings must \
             answer 200, got {out}"
        );
        let (status, out) = get_user_settings(&router, settings).await;
        assert_eq!(
            error_code(status, &out),
            "404 ResourceNotFoundException",
            "case {settings:?}: a deleted resource must be a 404, got {out}"
        );
    }
}

/// RULE: an operation that names a resource this service holds no record of answers HTTP
/// 404 `ResourceNotFoundException`, naming the absent resource.
#[tokio::test]
async fn an_absent_resource_is_reported_as_not_found() {
    let router = emulator();
    let (portal_arn, _) = create_portal(&router, json!({"displayName": "present"})).await;
    let settings_arn = create_user_settings(&router, minimal_user_settings()).await;
    let missing_portal = absent_arn("portal");
    let missing_settings = absent_arn("userSettings");

    let cases: Vec<(&str, String, String, &str)> = vec![
        (
            "GetPortal on an absent portal",
            "GET".into(),
            format!("/portals/{}", encode_path(&missing_portal)),
            &missing_portal,
        ),
        (
            "GetUserSettings on an absent resource",
            "GET".into(),
            format!("/userSettings/{}", encode_path(&missing_settings)),
            &missing_settings,
        ),
        (
            "AssociateUserSettings with an absent portal",
            "PUT".into(),
            format!(
                "/portals/{}/userSettings?userSettingsArn={}",
                encode_path(&missing_portal),
                encode_query(&settings_arn)
            ),
            &missing_portal,
        ),
        (
            "AssociateUserSettings with absent user settings",
            "PUT".into(),
            format!(
                "/portals/{}/userSettings?userSettingsArn={}",
                encode_path(&portal_arn),
                encode_query(&missing_settings)
            ),
            &missing_settings,
        ),
    ];

    for (case, method, path, resource) in cases {
        let (status, out) = call(&router, &method, &path, None).await;
        assert_eq!(
            error_code(status, &out),
            "404 ResourceNotFoundException",
            "case {case:?}: must be a 404 ResourceNotFoundException, got {out}"
        );
        assert_eq!(
            out["resourceId"].as_str(),
            Some(resource),
            "case {case:?}: the error must name the absent resource, got {out}"
        );
    }

    // A refused association left neither record changed.
    let (_, got) = get_portal(&router, &portal_arn).await;
    assert!(
        got["portal"].get("userSettingsArn").is_none_or(Value::is_null),
        "case \"refused association\": a 404 must leave the portal unassociated, got \
         {got}"
    );
}

/// RULE: delete is idempotent wherever the model declares no `ResourceNotFoundException`
/// for it, so deleting a resource that is already gone succeeds.
#[tokio::test]
async fn delete_is_idempotent_where_the_model_declares_no_not_found() {
    let router = emulator();
    let (portal_arn, _) = create_portal(&router, json!({"displayName": "p"})).await;
    let settings_arn = create_user_settings(&router, minimal_user_settings()).await;

    let cases: Vec<(&str, String)> = vec![
        (
            "DeletePortal",
            format!("/portals/{}", encode_path(&portal_arn)),
        ),
        (
            "DeleteUserSettings",
            format!("/userSettings/{}", encode_path(&settings_arn)),
        ),
        (
            "DeletePortal on an ARN never created",
            format!("/portals/{}", encode_path(&absent_arn("portal"))),
        ),
    ];
    for (case, path) in cases {
        for attempt in 1..=2 {
            let (status, out) = call(&router, "DELETE", &path, None).await;
            assert_eq!(
                status, 200,
                "case {case:?} attempt {attempt}: delete must answer 200, got {status} \
                 {out}"
            );
        }
    }
}

/// RULE: a request whose modelled URI pattern ends in a literal segment after a greedy
/// `@httpLabel` reaches THAT operation, not the one whose pattern stops at the label.
///
/// One row per operation whose modelled `@http` URI pattern puts literal segments after
/// a greedy `@httpLabel` — the whole class this model declares — with the methods,
/// literal segments and required query parameters taken from the model
/// (`smithy_query(query="digest")` plus `query="operation"` for the query bindings).
/// Every one of those operations except `AssociateUserSettings` is still a scaffold, so
/// reaching it is observable as a 501 naming it; `AssociateUserSettings` is implemented,
/// so reaching it is observable as its own 404 for the absent resource. Reaching the
/// portal-level operation instead would answer `GetPortal`'s portal, run `DeletePortal`
/// or `UpdatePortal`, which is what this pins against.
#[tokio::test]
async fn an_association_sub_resource_reaches_its_own_operation() {
    let router = emulator();
    let (portal_arn, _) = create_portal(&router, json!({"displayName": "routed"})).await;
    let trust_store_arn = absent_arn("trustStore");
    // `CertificateThumbprint`: len 64, pattern ^[A-Fa-f0-9]{64}$.
    let thumbprint = "a".repeat(64);

    // (method, path, expected operation, expected status).
    let portal_sub = |sub: &str, query: String| {
        format!(
            "/portals/{}/{sub}{query}",
            encode_path(&portal_arn)
        )
    };
    let settings_query = |param: &str, kind: &str| {
        format!("?{param}={}", encode_query(&absent_arn(kind)))
    };
    let cases: Vec<(&str, String, &str, u16)> = vec![
        (
            "PUT",
            portal_sub(
                "browserSettings",
                settings_query("browserSettingsArn", "browserSettings"),
            ),
            "AssociateBrowserSettings",
            501,
        ),
        (
            "PUT",
            portal_sub(
                "dataProtectionSettings",
                settings_query("dataProtectionSettingsArn", "dataProtectionSettings"),
            ),
            "AssociateDataProtectionSettings",
            501,
        ),
        (
            "PUT",
            portal_sub(
                "ipAccessSettings",
                settings_query("ipAccessSettingsArn", "ipAccessSettings"),
            ),
            "AssociateIpAccessSettings",
            501,
        ),
        (
            "PUT",
            portal_sub(
                "networkSettings",
                settings_query("networkSettingsArn", "networkSettings"),
            ),
            "AssociateNetworkSettings",
            501,
        ),
        (
            "PUT",
            portal_sub(
                "sessionLogger",
                settings_query("sessionLoggerArn", "sessionLogger"),
            ),
            "AssociateSessionLogger",
            501,
        ),
        (
            "PUT",
            portal_sub("trustStores", settings_query("trustStoreArn", "trustStore")),
            "AssociateTrustStore",
            501,
        ),
        (
            "PUT",
            portal_sub(
                "userAccessLoggingSettings",
                settings_query(
                    "userAccessLoggingSettingsArn",
                    "userAccessLoggingSettings",
                ),
            ),
            "AssociateUserAccessLoggingSettings",
            501,
        ),
        (
            // Implemented: reaching it is observable as its own 404 for the absent
            // user-settings resource, not as a 501.
            "PUT",
            portal_sub(
                "userSettings",
                settings_query("userSettingsArn", "userSettings"),
            ),
            "AssociateUserSettings",
            404,
        ),
        (
            "DELETE",
            portal_sub("browserSettings", String::new()),
            "DisassociateBrowserSettings",
            501,
        ),
        (
            "DELETE",
            portal_sub("dataProtectionSettings", String::new()),
            "DisassociateDataProtectionSettings",
            501,
        ),
        (
            "DELETE",
            portal_sub("ipAccessSettings", String::new()),
            "DisassociateIpAccessSettings",
            501,
        ),
        (
            "DELETE",
            portal_sub("networkSettings", String::new()),
            "DisassociateNetworkSettings",
            501,
        ),
        (
            "DELETE",
            portal_sub("sessionLogger", String::new()),
            "DisassociateSessionLogger",
            501,
        ),
        (
            "DELETE",
            portal_sub("trustStores", String::new()),
            "DisassociateTrustStore",
            501,
        ),
        (
            "DELETE",
            portal_sub("userAccessLoggingSettings", String::new()),
            "DisassociateUserAccessLoggingSettings",
            501,
        ),
        (
            "DELETE",
            portal_sub("userSettings", String::new()),
            "DisassociateUserSettings",
            501,
        ),
        (
            "GET",
            portal_sub("identityProviders", String::new()),
            "ListIdentityProviders",
            501,
        ),
        (
            "GET",
            format!(
                "/trustStores/{}/certificate?thumbprint={thumbprint}",
                encode_path(&trust_store_arn)
            ),
            "GetTrustStoreCertificate",
            501,
        ),
        (
            "GET",
            format!(
                "/trustStores/{}/certificates",
                encode_path(&trust_store_arn)
            ),
            "ListTrustStoreCertificates",
            501,
        ),
    ];

    for (method, path, operation, expected_status) in cases {
        let (status, out) = call(&router, method, &path, None).await;
        assert_eq!(
            status, expected_status,
            "case {method} {operation}: must reach {operation}, got {status} {out}"
        );
        if expected_status == 501 {
            assert_eq!(
                out["__type"].as_str(),
                Some("NotImplemented"),
                "case {method} {operation}: a scaffolded operation answers \
                 NotImplemented, got {out}"
            );
            assert!(
                out["message"]
                    .as_str()
                    .is_some_and(|m| m.contains(operation)),
                "case {method} {operation}: the 501 must name {operation}, got {out}"
            );
        } else {
            assert_eq!(
                out["__type"].as_str(),
                Some("ResourceNotFoundException"),
                "case {method} {operation}: must answer its own not-found, got {out}"
            );
        }
    }

    // A query value may itself contain `://` or `/`; the route must still be named from
    // the PATH alone. A target that differs only in a trailing slash is adapted the same
    // way, so the label never keeps a stray segment.
    for (case, path) in [
        (
            "query value containing a URL",
            format!(
                "/portals/{}/userSettings?trace={}",
                encode_path(&portal_arn),
                encode_query("http://example.com/a/b")
            ),
        ),
        (
            "trailing slash",
            format!("/portals/{}/userSettings/", encode_path(&portal_arn)),
        ),
    ] {
        let (status, out) = call(&router, "DELETE", &path, None).await;
        assert_eq!(
            (status, out["__type"].as_str()),
            (501, Some("NotImplemented")),
            "case {case:?}: the route must be named from the path segments alone, got \
             {status} {out}"
        );
        assert!(
            out["message"]
                .as_str()
                .is_some_and(|m| m.contains("DisassociateUserSettings")),
            "case {case:?}: the 501 must name DisassociateUserSettings, got {out}"
        );
    }

    // None of the above ran a portal-level operation against the portal.
    let (status, out) = get_portal(&router, &portal_arn).await;
    assert_eq!(
        status, 200,
        "case \"portal survives\": the portal must still exist, got {status} {out}"
    );
    assert_eq!(
        out["portal"]["displayName"].as_str(),
        Some("routed"),
        "case \"portal survives\": the portal must be unchanged, got {out}"
    );
}

/// RULE: `maxResults` is a maximum, not a demand - a page holds at most the service's
/// page-size maximum, which is also the page size when the request names none, and a
/// `nextToken` carries the rest.
#[tokio::test]
async fn a_page_never_exceeds_the_services_page_size_maximum() {
    let router = emulator();
    let total = MAX_LIST_PAGE_SIZE + 5;
    for i in 0..total {
        create_portal(&router, json!({"displayName": format!("p-{i}")})).await;
    }

    let cases: &[(&str, String)] = &[
        ("no maxResults at all", String::new()),
        (
            "maxResults at the maximum",
            format!("?maxResults={MAX_LIST_PAGE_SIZE}"),
        ),
        (
            "maxResults one past the maximum",
            format!("?maxResults={}", MAX_LIST_PAGE_SIZE + 1),
        ),
        ("maxResults far past the maximum", "?maxResults=100000".into()),
    ];
    for (case, query) in cases {
        let (status, out) = list_portals(&router, query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListPortals must answer 200, got {status} {out}"
        );
        assert_eq!(
            out["portals"].as_array().map(Vec::len),
            Some(MAX_LIST_PAGE_SIZE),
            "case {case:?}: the page must hold the service maximum, got {out}"
        );
        assert!(
            out.get("nextToken").is_some_and(|t| t.is_string()),
            "case {case:?}: the rest of the collection must be reachable by nextToken, \
             got {out}"
        );
    }

    // The clamp loses nothing: the remainder is still reachable, exactly once.
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let query = match &token {
            None => String::new(),
            Some(t) => format!("?nextToken={}", encode_query(t)),
        };
        let (_, out) = list_portals(&router, &query).await;
        seen.extend(
            out["portals"]
                .as_array()
                .expect("ListPortals must report a portals list")
                .iter()
                .filter_map(|p| p["portalArn"].as_str().map(str::to_string)),
        );
        token = out
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        seen.iter().collect::<BTreeSet<_>>().len(),
        total,
        "case \"clamped paging\": every portal must still be reachable exactly once, \
         saw {len} entries",
        len = seen.len()
    );
}

/// RULE: a create is refused with HTTP 402 `ServiceQuotaExceededException` once this
/// emulator holds as many of that resource as it stores, and a retry of a resource that
/// already exists still succeeds at the bound.
#[tokio::test]
async fn a_create_is_refused_at_the_capacity_bound() {
    let cases: Vec<(&str, &str, usize, Value, &str)> = vec![
        (
            "portals",
            "/portals",
            MAX_PORTALS,
            json!({"displayName": "filler"}),
            "portalArn",
        ),
        (
            "user settings",
            "/userSettings",
            MAX_USER_SETTINGS,
            minimal_user_settings(),
            "userSettingsArn",
        ),
    ];

    for (case, path, limit, body, arn_member) in cases {
        let router = emulator();

        // One of the records is created with a client token, so the replay path can be
        // exercised once the store is full.
        let mut tokened = body.clone();
        tokened["clientToken"] = json!("retry-me");
        let (status, out) = call(&router, "POST", path, Some(tokened.clone())).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first create must answer 200, got {status} {out}"
        );
        let replayed_arn = out[arn_member].as_str().unwrap_or_default().to_string();

        for i in 1..limit {
            let (status, out) = call(&router, "POST", path, Some(body.clone())).await;
            assert_eq!(
                status, 200,
                "case {case:?}: create {i} must answer 200 below the bound, got {status} \
                 {out}"
            );
        }

        let (status, out) = call(&router, "POST", path, Some(body.clone())).await;
        assert_eq!(
            error_code(status, &out),
            "402 ServiceQuotaExceededException",
            "case {case:?}: a create past the bound must be refused with 402 \
             ServiceQuotaExceededException, got {out}"
        );

        // A retry of something that already exists is not a new resource, so the bound
        // does not refuse it.
        let (status, out) = call(&router, "POST", path, Some(tokened)).await;
        assert_eq!(
            (status, out[arn_member].as_str()),
            (200, Some(replayed_arn.as_str())),
            "case {case:?}: a replayed clientToken must still answer at the bound, got \
             {status} {out}"
        );
    }
}

/// RULE: a client token's record is released with the resource it names, so the token is
/// no longer a replay once that resource is deleted.
#[tokio::test]
async fn a_client_tokens_record_is_released_with_its_resource() {
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "portal",
            "/portals",
            json!({"displayName": "released", "clientToken": "tok"}),
            "portalArn",
        ),
        (
            "user settings",
            "/userSettings",
            json!({
                "copyAllowed": "Enabled", "pasteAllowed": "Enabled",
                "downloadAllowed": "Enabled", "uploadAllowed": "Enabled",
                "printAllowed": "Enabled", "clientToken": "tok"
            }),
            "userSettingsArn",
        ),
    ];

    for (case, path, body, arn_member) in cases {
        let router = emulator();
        let (_, out) = call(&router, "POST", path, Some(body.clone())).await;
        let first = out[arn_member].as_str().unwrap_or_default().to_string();

        let (status, out) = call(
            &router,
            "DELETE",
            &format!("{path}/{}", encode_path(&first)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: delete must answer 200, got {status} {out}"
        );

        let (status, out) = call(&router, "POST", path, Some(body)).await;
        let second = out[arn_member].as_str().unwrap_or_default().to_string();
        assert_eq!(
            status, 200,
            "case {case:?}: the create must answer 200, got {status} {out}"
        );
        assert_ne!(
            second, first,
            "case {case:?}: once the resource is gone its client token is no longer a \
             replay, so a fresh resource is minted"
        );
    }
}

/// RULE: a branding configuration the create call carries is STORED, with each image's
/// MIME type and file extension derived from the image itself and its timestamp from the
/// upload instant, and reported back by the read; an image of no type the model's
/// `MimeType` declares is refused rather than accepted and dropped.
#[tokio::test]
async fn a_branding_configuration_is_stored_and_reported_back() {
    // Base64 of each format's own signature: the PNG signature (RFC 2083 3.1), the JPEG
    // start-of-image marker, and the Windows icon header.
    let png_blob = "iVBORw0KGgo=";
    let jpeg_blob = "/9j/";
    let icon_blob = "AAABAA==";
    let strings = json!({
        // Locales from the model's own `Locale` enum.
        "en-US": {"browserTabTitle": "Portal", "welcomeText": "Welcome"},
        "fr-FR": {"browserTabTitle": "Portail", "welcomeText": "Bienvenue"}
    });

    // (case, logo, favicon, wallpaper, expected logo/favicon/wallpaper metadata)
    /// The `(mimeType, fileExtension)` a stored image must report.
    type Expected = (&'static str, &'static str);
    /// One row of the branding table.
    type BrandingCase = (
        &'static str,
        Value,
        Value,
        Option<Value>,
        Expected,
        Expected,
        Option<Expected>,
    );
    let cases: Vec<BrandingCase> = vec![
        (
            "inline PNG logo, inline icon favicon, S3 JPEG wallpaper",
            json!({"blob": png_blob}),
            json!({"blob": icon_blob}),
            Some(json!({"s3Uri": "s3://branding-bucket/wallpaper.jpg"})),
            ("image/png", "png"),
            ("image/x-icon", "ico"),
            Some(("image/jpeg", "jpg")),
        ),
        (
            "inline JPEG logo and favicon, no wallpaper",
            json!({"blob": jpeg_blob}),
            json!({"blob": jpeg_blob}),
            None,
            ("image/jpeg", "jpg"),
            ("image/jpeg", "jpg"),
            None,
        ),
        (
            "S3 keys, extensions in upper case",
            json!({"s3Uri": "s3://branding-bucket/Logo.PNG"}),
            json!({"s3Uri": "s3://branding.bucket.example/nested/key/Favicon.ICO"}),
            Some(json!({"s3Uri": "s3://branding-bucket/Paper.JPEG"})),
            ("image/png", "png"),
            ("image/x-icon", "ico"),
            Some(("image/jpeg", "jpg")),
        ),
    ];

    for (case, logo, favicon, wallpaper, want_logo, want_favicon, want_wallpaper) in cases {
        let router = emulator();
        let mut branding = json!({
            "logo": logo,
            "favicon": favicon,
            "localizedStrings": strings,
            "colorTheme": "Dark",
            "termsOfService": "# Terms\nBe nice."
        });
        if let Some(wallpaper) = wallpaper {
            branding["wallpaper"] = wallpaper;
        }
        let mut body = minimal_user_settings();
        body["brandingConfigurationInput"] = branding.clone();

        let arn = create_user_settings(&router, body).await;
        let (status, out) = get_user_settings(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetUserSettings must answer 200, got {status} {out}"
        );
        let stored = &out["userSettings"]["brandingConfiguration"];

        for (member, (mime, extension)) in [("logo", want_logo), ("favicon", want_favicon)] {
            assert_eq!(
                (
                    stored[member]["mimeType"].as_str(),
                    stored[member]["fileExtension"].as_str()
                ),
                (Some(mime), Some(extension)),
                "case {case:?}: {member} metadata must come from the image itself, got \
                 {stored}"
            );
            assert!(
                stored[member]
                    .get("lastUploadTimestamp")
                    .is_some_and(|t| !t.is_null()),
                "case {case:?}: {member} must carry its upload instant, got {stored}"
            );
        }
        match want_wallpaper {
            None => assert!(
                stored.get("wallpaper").is_none_or(Value::is_null),
                "case {case:?}: no wallpaper was uploaded, so none is reported, got \
                 {stored}"
            ),
            Some((mime, extension)) => assert_eq!(
                (
                    stored["wallpaper"]["mimeType"].as_str(),
                    stored["wallpaper"]["fileExtension"].as_str()
                ),
                (Some(mime), Some(extension)),
                "case {case:?}: wallpaper metadata must come from the image itself, got \
                 {stored}"
            ),
        }
        assert_eq!(
            (
                stored["colorTheme"].clone(),
                stored["localizedStrings"].clone(),
                stored["termsOfService"].clone()
            ),
            (
                branding["colorTheme"].clone(),
                branding["localizedStrings"].clone(),
                branding["termsOfService"].clone()
            ),
            "case {case:?}: the stored branding must report what was persisted, got \
             {stored}"
        );
    }

    // No branding uploaded, none reported.
    let router = emulator();
    let arn = create_user_settings(&router, minimal_user_settings()).await;
    let (_, out) = get_user_settings(&router, &arn).await;
    assert!(
        out["userSettings"]
            .get("brandingConfiguration")
            .is_none_or(Value::is_null),
        "case \"no branding\": nothing was uploaded, so nothing is reported, got {out}"
    );

    // An image of no accepted type is refused, and nothing is stored.
    let refused: &[(&str, Value)] = &[
        ("logo blob that is not an image", json!({"blob": "Zm9v"})),
        (
            "logo S3 key with an unsupported extension",
            json!({"s3Uri": "s3://branding-bucket/logo.gif"}),
        ),
        (
            "logo S3 key with no extension at all",
            json!({"s3Uri": "s3://branding-bucket/logo"}),
        ),
        (
            "logo S3 key with no extension under a dotted bucket",
            json!({"s3Uri": "s3://branding.bucket.example/logo"}),
        ),
    ];
    for (case, logo) in refused {
        let router = emulator();
        let mut body = minimal_user_settings();
        body["brandingConfigurationInput"] = json!({
            "logo": logo,
            "favicon": {"blob": png_blob},
            "localizedStrings": strings,
            "colorTheme": "Light"
        });
        let (status, out) = call(&router, "POST", "/userSettings", Some(body)).await;
        assert_eq!(
            error_code(status, &out),
            "400 ValidationException",
            "case {case:?}: must be refused with 400 ValidationException, got {out}"
        );
    }
}

/// The audit target the state-changing handlers record under.
const AUDIT_TARGET: &str = "emulator::audit";

/// One captured tracing event: its fields, flattened to strings.
type Record = std::collections::BTreeMap<String, String>;

/// Collects every field of an event into a [`Record`].
#[derive(Default)]
struct RecordFields(Record);

impl tracing::field::Visit for RecordFields {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_string(), format!("{value:?}"));
    }
}

/// A `tracing` subscriber that keeps every event recorded under [`AUDIT_TARGET`], so a
/// test can assert on the audit trail a request produced.
///
/// Implemented against `tracing`'s own `Subscriber` trait — the crate this emulator
/// records with — rather than pulling a second tracing crate into the harness. Spans are
/// not part of the audit contract, so the span half of the trait is inert.
struct CaptureAudit {
    records: Arc<std::sync::Mutex<Vec<Record>>>,
}

impl tracing::Subscriber for CaptureAudit {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == AUDIT_TARGET
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        if event.metadata().target() != AUDIT_TARGET {
            return;
        }
        let mut fields = RecordFields::default();
        event.record(&mut fields);
        self.records
            .lock()
            .expect("the capture mutex must not be poisoned")
            .push(fields.0);
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

/// The process-wide audit sink, installing the capturing subscriber the first time it is
/// asked for.
///
/// Global rather than thread-local because `tracing` caches callsite interest for the
/// whole process: a thread-local subscriber installed after another suite had already hit
/// the audit callsite would see nothing. Records are therefore filtered by the ARNs the
/// asserting suite created.
fn audit_sink() -> Arc<std::sync::Mutex<Vec<Record>>> {
    static SINK: std::sync::OnceLock<Arc<std::sync::Mutex<Vec<Record>>>> =
        std::sync::OnceLock::new();
    Arc::clone(SINK.get_or_init(|| {
        let records = Arc::new(std::sync::Mutex::new(Vec::new()));
        // A subscriber another harness already installed would leave the sink empty, so
        // assert the install rather than letting the assertions below fail for an
        // unrelated reason.
        tracing::subscriber::set_global_default(CaptureAudit {
            records: Arc::clone(&records),
        })
        .expect("this suite installs the only tracing subscriber in the test binary");
        // `tracing` caches each callsite's INTEREST the first time it is hit, and with no
        // subscriber installed yet that cached answer is "never enabled" — so an audit
        // callsite some other case of this binary reached before this install would stay
        // silent, and which records arrive would depend on the order the cases happened to
        // run in. Rebuilding the cache re-asks this subscriber about every callsite.
        tracing::callsite::rebuild_interest_cache();
        records
    }))
}

/// RULE: every state-changing operation emits one structured audit record naming the
/// operation, the access key id the caller presented, the region of its credential
/// scope, the resource it changed and how the call ended.
#[tokio::test]
async fn every_state_changing_operation_emits_an_audit_record() {
    let sink = audit_sink();
    // Where this case's own records start: every case of this binary shares the sink, so
    // anything already in it was emitted by another one.
    let before = sink
        .lock()
        .expect("the capture mutex must not be poisoned")
        .len();

    let router = emulator();
    let (portal_arn, _) = create_portal(&router, json!({"displayName": "audited"})).await;
    let settings_arn = create_user_settings(&router, minimal_user_settings()).await;
    associate_user_settings(&router, &portal_arn, &settings_arn).await;
    call(
        &router,
        "DELETE",
        &format!("/portals/{}", encode_path(&portal_arn)),
        None,
    )
    .await;
    call(
        &router,
        "DELETE",
        &format!("/userSettings/{}", encode_path(&settings_arn)),
        None,
    )
    .await;

    // Only this case's own records: the ones it appended, for its own resources — other
    // cases share the process-wide sink.
    let captured: Vec<Record> = sink
        .lock()
        .expect("the capture mutex must not be poisoned")
        .iter()
        .skip(before)
        .filter(|record| {
            record
                .get("resource")
                .is_some_and(|arn| arn == &portal_arn || arn == &settings_arn)
        })
        .cloned()
        .collect();

    let cases: &[(&str, &str)] = &[
        ("CreatePortal", "created"),
        ("CreateUserSettings", "created"),
        ("AssociateUserSettings", "associated"),
        ("DeletePortal", "deleted"),
        ("DeleteUserSettings", "deleted"),
    ];
    assert_eq!(
        captured.len(),
        cases.len(),
        "each state-changing call must emit exactly one audit record, got {captured:?}"
    );
    for ((operation, outcome), record) in cases.iter().zip(&captured) {
        assert_eq!(
            record.get("operation").map(String::as_str),
            Some(*operation),
            "case {operation:?}: the record must name the operation, got {record:?}"
        );
        assert!(
            record
                .get("outcome")
                .is_some_and(|got| got.starts_with(outcome)),
            "case {operation:?}: the record must state the outcome {outcome:?}, got \
             {record:?}"
        );
        // The access key id the caller presented is a public identifier, so the record
        // can attribute the call; no secret is recorded.
        assert_eq!(
            record.get("akid").map(String::as_str),
            Some("AKIDEXAMPLE"),
            "case {operation:?}: the record must name the presented access key id, got \
             {record:?}"
        );
        assert_eq!(
            record.get("region").map(String::as_str),
            Some("us-east-1"),
            "case {operation:?}: the record must name the credential scope's region, got \
             {record:?}"
        );
        assert!(
            record
                .get("resource")
                .is_some_and(|arn| arn.starts_with("arn:aws:workspaces-web:")),
            "case {operation:?}: the record must name the resource it changed, got \
             {record:?}"
        );
    }
}
