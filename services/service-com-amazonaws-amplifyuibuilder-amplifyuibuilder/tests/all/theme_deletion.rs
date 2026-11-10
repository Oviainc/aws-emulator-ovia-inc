// SPDX-License-Identifier: CC0-1.0

//! `DeleteTheme`, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rule under test: `DeleteTheme` removes the one theme the addressed
//! (appId, environmentName, id) holds and answers an empty 2xx, after which that theme is
//! absent for every later reader, while every other stored theme is untouched; an id that
//! names no theme in the addressed app environment is refused with the modelled
//! `ResourceNotFoundException` at HTTP 404 and removes nothing.
//!
//! What a refusal SAYS — the fixed literal of each failure class, and that it interpolates
//! nothing from the request — is the subject of `resource_error_messages.rs`, which runs
//! that rule over every resource kind including this one's delete; the cases here assert
//! the status and the state.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request to the
/// amplifyuibuilder service.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment the cases address, unless a case is about addressing
/// another one.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// A well-formed `Uuid` nothing created, for the not-found rows. `DeleteThemeRequest$id`
/// targets the model's `Uuid` shape, so a miss has to be spelled as one.
const NEVER_CREATED_ID: &str = "11111111-2222-4333-8444-555555555555";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The theme id the answer carries, or the empty string. `CreateTheme` and `GetTheme`
    /// both bind their theme to `@httpPayload`, so the body IS the `Theme`.
    fn id(&self) -> String {
        self.body["id"].as_str().unwrap_or_default().to_string()
    }

    /// The error-type discriminator the body carries — `__type`, or the `code` / `Code`
    /// spellings the same JSON error parsers also accept — or the empty string when it
    /// carries none. AmplifyUIBuilder names no exception on the wire, so a client
    /// classifies a failure by HTTP status.
    fn error_code(&self) -> &str {
        ["__type", "code", "Code"]
            .iter()
            .find_map(|member| self.body[*member].as_str())
            .unwrap_or_default()
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

async fn create_in(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    data: Value,
) -> Answer {
    call(router, "POST", &themes_uri(app_id, environment), Some(data)).await
}

async fn create(router: &MultiServiceRouter, data: Value) -> Answer {
    create_in(router, APP_ID, ENVIRONMENT, data).await
}

async fn create_with_token(router: &MultiServiceRouter, data: Value, token: &str) -> Answer {
    let uri = format!("{}?clientToken={}", themes_uri(APP_ID, ENVIRONMENT), token);
    call(router, "POST", &uri, Some(data)).await
}

async fn get_in(router: &MultiServiceRouter, app_id: &str, environment: &str, id: &str) -> Answer {
    let uri = format!("{}/{id}", themes_uri(app_id, environment));
    call(router, "GET", &uri, None).await
}

async fn get(router: &MultiServiceRouter, id: &str) -> Answer {
    get_in(router, APP_ID, ENVIRONMENT, id).await
}

async fn delete_in(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    id: &str,
) -> Answer {
    let uri = format!("{}/{id}", themes_uri(app_id, environment));
    call(router, "DELETE", &uri, None).await
}

async fn delete(router: &MultiServiceRouter, id: &str) -> Answer {
    delete_in(router, APP_ID, ENVIRONMENT, id).await
}

/// The theme payloads the deletion cases drive, differing along what the rule generalises
/// over: how many `values` entries a theme carries, how deeply its `ThemeValue$children`
/// nest and whether the optional `overrides` and `tags` are present at all. The rule is
/// indifferent to all of it — any stored theme is deletable.
fn theme_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "only the required members, with an empty values list",
            json!({"name": "Bare", "values": []}),
        ),
        (
            "one flat value",
            json!({
                "name": "Flat",
                "values": [{"key": "tokens.colors.brand", "value": {"value": "#1f1f1f"}}],
            }),
        ),
        (
            "several flat values and tags",
            json!({
                "name": "Studio",
                "values": [
                    {"key": "primaryColor", "value": {"value": "#047d95"}},
                    {"key": "secondaryColor", "value": {"value": "#ff6600"}},
                ],
                "tags": {"env": "test", "team": "studio"},
            }),
        ),
        (
            "values nested two levels deep, plus overrides",
            json!({
                "name": "Nested",
                "values": [
                    {
                        "key": "tokens",
                        "value": {
                            "children": [
                                {"key": "space", "value": {"value": "8px"}},
                            ],
                        },
                    },
                ],
                "overrides": [
                    {"key": "dark", "value": {"value": "#ffffff"}},
                ],
            }),
        ),
    ]
}

/// A stored theme is removed by `DeleteTheme`, whatever it carries: the delete answers an
/// empty HTTP 200, the theme is then absent for `GetTheme` (404) and a second delete of
/// the same id is the same 404 — one record, deleted once.
#[tokio::test]
async fn delete_theme_removes_the_theme_it_names() {
    let router = emulator();

    for (case, data) in theme_cases() {
        let created = create(&router, data.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateTheme, got {} with {:?}",
            created.status, created.body
        );
        let id = created.id();
        assert_eq!(
            get(&router, &id).await.status,
            200,
            "case {case:?}: the created theme {id:?} must be readable before the delete"
        );

        let deleted = delete(&router, &id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected HTTP 200 from DeleteTheme, got {} with {:?}",
            deleted.status, deleted.body
        );
        // `DeleteThemeResponse` is `Unit` (`smithy_query(query="operation",
        // operation="DeleteTheme")` reports the output shape `Unit`): the answer is the
        // status alone, with no body.
        assert_eq!(
            deleted.body,
            Value::Null,
            "case {case:?}: DeleteTheme answers Unit, so it must carry no body, got {:?}",
            deleted.body
        );

        let read = get(&router, &id).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a deleted theme must be absent for GetTheme, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error_code(),
            "",
            "case {case:?}: a refusal must name no error type, got {:?}",
            read.body
        );

        let again = delete(&router, &id).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: deleting an already deleted theme must be refused with HTTP 404, \
             got {} with {:?}",
            again.status, again.body
        );
    }
}

/// `DeleteTheme` removes exactly the theme its id names: whichever member of a stored set
/// is deleted — the first, a middle one or the last — every other theme in that app
/// environment is still readable, unchanged.
#[tokio::test]
async fn delete_theme_leaves_every_other_theme_alone() {
    // The rows differ in how many themes the environment holds and which position in
    // creation order the delete addresses.
    let rows: Vec<(&str, usize, usize)> = vec![
        ("the only theme", 1, 0),
        ("the first of three", 3, 0),
        ("the middle of three", 3, 1),
        ("the last of three", 3, 2),
        ("one of five", 5, 3),
    ];

    for (case, count, target) in rows {
        let router = emulator();
        let mut created: Vec<(String, Value)> = Vec::new();
        for index in 0..count {
            let answer = create(
                &router,
                json!({
                    "name": format!("Theme{index}"),
                    "values": [{"key": "primaryColor", "value": {"value": "#047d95"}}],
                }),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200 from CreateTheme, got {} with {:?}",
                answer.status, answer.body
            );
            created.push((answer.id(), answer.body.clone()));
        }

        let (target_id, _) = created[target].clone();
        let deleted = delete(&router, &target_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected HTTP 200 from DeleteTheme, got {} with {:?}",
            deleted.status, deleted.body
        );

        for (index, (id, body)) in created.iter().enumerate() {
            let read = get(&router, id).await;
            if index == target {
                assert_eq!(
                    read.status, 404,
                    "case {case:?}: the deleted theme {id:?} must answer HTTP 404, got {} \
                     with {:?}",
                    read.status, read.body
                );
            } else {
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: the surviving theme {id:?} must still answer HTTP 200, \
                     got {} with {:?}",
                    read.status, read.body
                );
                assert_eq!(
                    &read.body, body,
                    "case {case:?}: the surviving theme {id:?} must report the record it was \
                     created with, got {:?}",
                    read.body
                );
            }
        }
    }
}

/// A theme is addressed by (appId, environmentName, id), so an id that resolves to no
/// theme in the addressed app environment removes nothing and is refused the way its class
/// calls for: HTTP 400 for an id another app environment owns, HTTP 404 for an id that
/// names no theme anywhere.
#[tokio::test]
async fn delete_theme_refuses_an_id_that_names_no_theme_in_that_app_environment() {
    let router = emulator();
    let data = json!({"name": "Scoped", "values": [{"key": "a", "value": {"value": "b"}}]});
    let here = create(&router, data.clone()).await.id();
    let elsewhere = create_in(&router, "d9999zzzz", ENVIRONMENT, data.clone())
        .await
        .id();
    let other_environment = create_in(&router, APP_ID, "prod", data.clone()).await.id();

    // The rows differ in HOW the lookup misses, and so in which class it falls in.
    let rows: Vec<(&str, &str, &str, &str, u16)> = vec![
        (
            "an id nobody created",
            APP_ID,
            ENVIRONMENT,
            NEVER_CREATED_ID,
            404,
        ),
        ("an id another app owns", APP_ID, ENVIRONMENT, &elsewhere, 400),
        (
            "an id another environment of this app owns",
            APP_ID,
            ENVIRONMENT,
            &other_environment,
            400,
        ),
        (
            "this id asked for under another app",
            "d9999zzzz",
            ENVIRONMENT,
            &here,
            400,
        ),
        (
            "this id asked for under another environment",
            APP_ID,
            "prod",
            &here,
            400,
        ),
    ];

    for (case, app_id, environment, lookup, expected) in rows {
        let refused = delete_in(&router, app_id, environment, lookup).await;
        assert_eq!(
            refused.status, expected,
            "case {case:?}: expected HTTP {expected} from DeleteTheme, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "",
            "case {case:?}: a refusal must name no error type, got {:?}",
            refused.body
        );
    }

    // Nothing the refusals named was removed: all three themes are still readable where
    // they were created.
    for (case, app_id, environment, id) in [
        ("this app environment", APP_ID, ENVIRONMENT, &here),
        ("another app", "d9999zzzz", ENVIRONMENT, &elsewhere),
        ("another environment", APP_ID, "prod", &other_environment),
    ] {
        let read = get_in(&router, app_id, environment, id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: a refused delete must leave the theme {id:?} in place, got {} \
             with {:?}",
            read.status, read.body
        );
    }
}

/// Deleting a theme releases the `clientToken` that created it: the record is gone, so a
/// create carrying that same token has no theme to report and mints a new one instead of
/// resurrecting the deleted id.
#[tokio::test]
async fn deleting_a_theme_releases_the_client_token_that_created_it() {
    let router = emulator();
    let data = json!({"name": "Tokened", "values": [{"key": "a", "value": {"value": "b"}}]});

    let cases = [
        ("an ascii token", "token-1"),
        ("a uuid-shaped token", "6f1f0f28-4c1f-4df5-9a4e-6f2d5a6a0001"),
        ("a token with punctuation", "retry::1/2"),
    ];

    for (case, token) in cases {
        let first = create_with_token(&router, data.clone(), token).await;
        let first_id = first.id();
        assert_eq!(
            delete(&router, &first_id).await.status,
            200,
            "case {case:?}: expected HTTP 200 from DeleteTheme for {first_id:?}"
        );

        let retried = create_with_token(&router, data.clone(), token).await;
        assert_eq!(
            retried.status, 200,
            "case {case:?}: expected HTTP 200 from CreateTheme, got {} with {:?}",
            retried.status, retried.body
        );
        assert_ne!(
            retried.id(),
            first_id,
            "case {case:?}: a create after the delete must mint a new theme, got the deleted \
             id {:?} again",
            retried.id()
        );
        assert_eq!(
            get(&router, &first_id).await.status,
            404,
            "case {case:?}: the deleted theme {first_id:?} must stay absent"
        );

        // The theme the retry minted is a live record of its own, and deletable too.
        assert_eq!(
            delete(&router, &retried.id()).await.status,
            200,
            "case {case:?}: the theme the retry minted must be deletable"
        );
    }
}
