// SPDX-License-Identifier: CC0-1.0

//! `UpdateTheme`, end to end over HTTP.
//!
//! The rule under test: `UpdateTheme` merges the `updatedTheme` patch into the stored
//! theme the request names — the `@required` `values` replaces what was stored, an
//! optional member the request carries replaces what was stored and one it leaves out
//! keeps it, the members that identify the record never move, `modifiedAt` moves to the
//! instant of the call — and `GetTheme` then reports exactly that record. An id that names
//! no theme in the addressed app environment is refused (HTTP 404) and nothing is created:
//! an update is not an upsert.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization.

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The published length bounds of `UpdateThemeData$name`, which targets `ThemeName`,
/// modelled `@length(min: 1, max: 255)`
/// (`smithy_query(query="shape", shape="UpdateThemeData")`). Spelled out here,
/// independently of the model the crate compiles against, so a wrong bound fails.
const THEME_NAME_MAX_LENGTH: usize = 255;
const THEME_NAME_MIN_LENGTH: usize = 1;

/// A SigV4 credential scope naming this service: it is what routes a request to the
/// amplifyuibuilder service.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// A well-formed `Uuid` nothing created.
const NEVER_CREATED_ID: &str = "11111111-2222-4333-8444-555555555555";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    fn id(&self) -> String {
        self.body["id"].as_str().unwrap_or_default().to_string()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "amplifyuibuilder.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_AMPLIFYUIBUILDER);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

fn themes_uri(app_id: &str, environment: &str) -> String {
    format!("/app/{app_id}/environment/{environment}/themes")
}

async fn create(router: &MultiServiceRouter, data: Value) -> Answer {
    call(
        router,
        "POST",
        &themes_uri(APP_ID, ENVIRONMENT),
        Some(data),
    )
    .await
}

async fn update_in(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    id: &str,
    patch: Value,
) -> Answer {
    let uri = format!("{}/{id}", themes_uri(app_id, environment));
    call(router, "PATCH", &uri, Some(patch)).await
}

async fn update(router: &MultiServiceRouter, id: &str, patch: Value) -> Answer {
    update_in(router, APP_ID, ENVIRONMENT, id, patch).await
}

async fn get(router: &MultiServiceRouter, id: &str) -> Answer {
    let uri = format!("{}/{id}", themes_uri(APP_ID, ENVIRONMENT));
    call(router, "GET", &uri, None).await
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// The instant a `@timestampFormat("date-time")` member carries, in seconds.
fn instant(value: &Value) -> f64 {
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("expected a date-time string, got {value:?}"));
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|error| panic!("{text:?} is not an ISO-8601 date-time ({error})"))
        .as_secs_f64()
}

/// The theme every merge case starts from: it carries each optional member, so a patch
/// that leaves one out is visibly keeping a stored value rather than an absence.
fn stored_theme() -> Value {
    json!({
        "name": "Base",
        "values": [
            {"key": "primaryColor", "value": {"value": "#047d95"}},
            {"key": "borderRadius", "value": {"value": "4px"}},
        ],
        "overrides": [{"key": "dark", "value": {"value": "#ffffff"}}],
        "tags": {"env": "test"},
    })
}

/// The patches the merge cases drive, with what each one must leave behind. The rows
/// differ along what the rule generalises over: which optional members the patch carries,
/// how many `values` entries it supplies, how deeply they nest, and the published length
/// bounds of `name`.
#[allow(clippy::type_complexity)]
fn merge_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "only the required values, replacing both entries with one",
            json!({"values": [{"key": "primaryColor", "value": {"value": "#111111"}}]}),
        ),
        (
            "values emptied",
            json!({"values": []}),
        ),
        (
            "a new name, values unchanged in content",
            json!({
                "name": "Renamed",
                "values": [
                    {"key": "primaryColor", "value": {"value": "#047d95"}},
                    {"key": "borderRadius", "value": {"value": "4px"}},
                ],
            }),
        ),
        (
            "overrides replaced",
            json!({
                "values": [{"key": "primaryColor", "value": {"value": "#222222"}}],
                "overrides": [
                    {"key": "light", "value": {"value": "#000000"}},
                    {"key": "dark", "value": {"value": "#eeeeee"}},
                ],
            }),
        ),
        (
            "overrides emptied",
            json!({
                "values": [{"key": "primaryColor", "value": {"value": "#333333"}}],
                "overrides": [],
            }),
        ),
        (
            "values nested three levels deep",
            json!({
                "values": [
                    {
                        "key": "tokens",
                        "value": {
                            "children": [
                                {
                                    "key": "colors",
                                    "value": {
                                        "children": [
                                            {"key": "brand", "value": {"value": "#e0f5f7"}},
                                        ],
                                    },
                                },
                            ],
                        },
                    },
                ],
            }),
        ),
        (
            "the id the httpLabel already names, restated in the payload",
            json!({"values": [{"key": "a", "value": {"value": "b"}}], "name": "Restated"}),
        ),
        (
            "the longest name the model admits",
            json!({
                "name": "n".repeat(THEME_NAME_MAX_LENGTH),
                "values": [{"key": "a", "value": {"value": "b"}}],
            }),
        ),
        (
            "the shortest name the model admits",
            json!({
                "name": "n".repeat(THEME_NAME_MIN_LENGTH),
                "values": [{"key": "a", "value": {"value": "b"}}],
            }),
        ),
    ]
}

/// A patch replaces the members it carries and keeps the ones it leaves out, the record's
/// identity never moves, and `GetTheme` reports the merged record.
#[tokio::test]
async fn update_theme_merges_the_patch_into_the_stored_theme() {
    for (case, patch) in merge_cases() {
        let router = emulator();
        let before = create(&router, stored_theme()).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: expected HTTP 200 from CreateTheme, got {} with {:?}",
            before.status, before.body
        );
        let id = before.id();

        // The row that restates the id in the payload states the one the label names.
        let mut patch = patch;
        if case.starts_with("the id the httpLabel") {
            patch["id"] = json!(id);
        }

        let updated = update(&router, &id, patch.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateTheme, got {} with {:?}",
            updated.status, updated.body
        );

        // `values` is @required on `UpdateThemeData`, so every patch carries it and it
        // replaces what was stored.
        assert_eq!(
            at(&updated.body, "/values"),
            patch["values"],
            "case {case:?}: values must come back exactly as the patch supplied, got {:?}",
            at(&updated.body, "/values")
        );
        // `name` and `overrides` are optional: carried replaces, absent keeps.
        let expected_name = patch
            .get("name")
            .cloned()
            .unwrap_or_else(|| at(&before.body, "/name"));
        assert_eq!(
            at(&updated.body, "/name"),
            expected_name,
            "case {case:?}: expected the name {expected_name:?}, got {:?}",
            at(&updated.body, "/name")
        );
        let expected_overrides = patch
            .get("overrides")
            .cloned()
            .unwrap_or_else(|| at(&before.body, "/overrides"));
        assert_eq!(
            at(&updated.body, "/overrides"),
            expected_overrides,
            "case {case:?}: expected the overrides {expected_overrides:?}, got {:?}",
            at(&updated.body, "/overrides")
        );

        // The members that identify the record are not `UpdateThemeData` members at all,
        // so an update never moves them — `tags` among them.
        for member in ["/id", "/appId", "/environmentName", "/createdAt", "/tags"] {
            assert_eq!(
                at(&updated.body, member),
                at(&before.body, member),
                "case {case:?}: an update must not move {member}, got {:?}",
                at(&updated.body, member)
            );
        }

        // `modifiedAt` reports when the theme last changed, so it is at or after the
        // instant the theme was created.
        assert!(
            instant(&at(&updated.body, "/modifiedAt")) >= instant(&at(&before.body, "/createdAt")),
            "case {case:?}: modifiedAt must be at or after createdAt, got {:?} against {:?}",
            at(&updated.body, "/modifiedAt"),
            at(&before.body, "/createdAt")
        );

        let read = get(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetTheme, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, updated.body,
            "case {case:?}: GetTheme must report the record UpdateTheme stored, got {:?}",
            read.body
        );

        // A read reports; reading twice reports the same record.
        assert_eq!(
            get(&router, &id).await.body,
            read.body,
            "case {case:?}: reading the updated theme twice must report the same record"
        );
    }
}

/// `UpdateThemeData$id` names the theme being updated, so a payload naming another theme
/// is a contradictory request (HTTP 400) and changes nothing.
#[tokio::test]
async fn a_payload_naming_another_theme_is_refused() {
    let router = emulator();
    let target = create(&router, stored_theme()).await;
    let other = create(&router, stored_theme()).await;

    let rows = [
        ("another live theme's id", other.id()),
        ("an id nobody created", NEVER_CREATED_ID.to_string()),
    ];

    for (case, named) in rows {
        let refused = update(
            &router,
            &target.id(),
            json!({"id": named, "values": [{"key": "x", "value": {"value": "y"}}]}),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 from UpdateTheme, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            get(&router, &target.id()).await.body,
            target.body,
            "case {case:?}: a refused update must leave the theme as it was"
        );
    }
}

/// An id `UpdateTheme` cannot resolve is refused the way its class calls for — HTTP 400
/// for an id another app environment owns, HTTP 404 for one that names no theme anywhere —
/// and nothing is created: an update is not an upsert.
#[tokio::test]
async fn update_theme_refuses_an_id_that_names_no_theme_in_that_app_environment() {
    let router = emulator();
    let here = create(&router, stored_theme()).await.id();
    let patch = json!({"values": [{"key": "x", "value": {"value": "y"}}], "name": "Ghost"});

    // The rows differ in HOW the lookup misses, and so in which class it falls in.
    let rows: Vec<(&str, &str, &str, &str, u16)> = vec![
        (
            "an id nobody created",
            APP_ID,
            ENVIRONMENT,
            NEVER_CREATED_ID,
            404,
        ),
        (
            "this id under another app",
            "d9999zzzz",
            ENVIRONMENT,
            &here,
            400,
        ),
        (
            "this id under another environment",
            APP_ID,
            "prod",
            &here,
            400,
        ),
    ];

    for (case, app_id, environment, id, expected) in rows {
        let refused = update_in(&router, app_id, environment, id, patch.clone()).await;
        assert_eq!(
            refused.status, expected,
            "case {case:?}: expected HTTP {expected} from UpdateTheme, got {} with {:?}",
            refused.status, refused.body
        );
        // Nothing was created where the update missed: the read misses the same way.
        let read = call(
            &router,
            "GET",
            &format!("{}/{id}", themes_uri(app_id, environment)),
            None,
        )
        .await;
        assert_eq!(
            read.status, expected,
            "case {case:?}: a refused update must create nothing and the read must agree, got \
             {} with {:?}",
            read.status, read.body
        );
    }
}

/// A `name` outside the published `@length(min: 1, max: 255)` of `ThemeName` is refused,
/// and the refusal leaves the stored theme as it was.
#[tokio::test]
async fn a_patched_name_outside_the_published_bounds_is_refused() {
    let router = emulator();
    let before = create(&router, stored_theme()).await;
    let id = before.id();

    // One past each published bound.
    for length in [THEME_NAME_MIN_LENGTH - 1, THEME_NAME_MAX_LENGTH + 1] {
        let refused = update(
            &router,
            &id,
            json!({"name": "n".repeat(length), "values": []}),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case name length {length}: a name outside @length(1, 255) must be refused, got \
             {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            get(&router, &id).await.body,
            before.body,
            "case name length {length}: a refused update must change nothing"
        );
    }

    // Each bound itself is accepted, so the refusal above is about the bound.
    for length in [THEME_NAME_MIN_LENGTH, THEME_NAME_MAX_LENGTH] {
        let accepted = update(
            &router,
            &id,
            json!({"name": "n".repeat(length), "values": []}),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case name length {length}: a name on the published bound must be accepted, got \
             {} with {:?}",
            accepted.status, accepted.body
        );
    }
}
