// SPDX-License-Identifier: CC0-1.0

//! What holds ACROSS successive `UpdateTheme` calls, end to end over HTTP.
//!
//! `theme_update.rs` pins the per-call merge rule (which members one patch replaces and
//! which it keeps). This suite pins the two rules that only show up over a SEQUENCE of
//! calls, and so cannot be seen in a single-patch case:
//!
//! 1. Updates accumulate on the ONE stored record: each call replaces the members it
//!    carries, the members it leaves out keep whatever the PREVIOUS update left (not what
//!    the create stored), the record's identity never moves, and `modifiedAt` never goes
//!    backwards.
//! 2. Applying the same `updatedTheme` twice leaves the same record — the update is
//!    idempotent in every member it writes, with or without a `clientToken`, and
//!    `modifiedAt` is the only member a repeat can move.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes — and reads the record back through `GetTheme`.

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The longest `ThemeName` the model admits: `UpdateThemeData$name` targets `ThemeName`,
/// modelled `@length(min: 1, max: 255)`
/// (`smithy_query(query="shape", shape="UpdateThemeData")`). Spelled out here,
/// independently of the model the crate compiles against, so a wrong bound fails.
const THEME_NAME_MAX_LENGTH: usize = 255;

/// The shortest `ThemeName` that member admits.
const THEME_NAME_MIN_LENGTH: usize = 1;

/// A SigV4 credential scope naming this service: it is what routes a request to the
/// amplifyuibuilder service.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment the cases address.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

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
    /// The theme id the answer carries, or the empty string.
    ///
    /// `CreateTheme`, `UpdateTheme` and `GetTheme` all bind their theme to
    /// `@httpPayload`, so the body IS the `Theme`.
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

async fn update(router: &MultiServiceRouter, id: &str, patch: Value) -> Answer {
    let uri = format!("{}/{id}", themes_uri(APP_ID, ENVIRONMENT));
    call(router, "PATCH", &uri, Some(patch)).await
}

async fn update_with_token(
    router: &MultiServiceRouter,
    id: &str,
    patch: Value,
    token: &str,
) -> Answer {
    let uri = format!(
        "{}/{id}?clientToken={token}",
        themes_uri(APP_ID, ENVIRONMENT)
    );
    call(router, "PATCH", &uri, Some(patch)).await
}

async fn get(router: &MultiServiceRouter, id: &str) -> Answer {
    let uri = format!("{}/{id}", themes_uri(APP_ID, ENVIRONMENT));
    call(router, "GET", &uri, None).await
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// The epoch seconds a `@timestampFormat("date-time")` member carries, so two of them can
/// be ordered. Panics when the member is not a date-time string, which is itself a defect.
fn epoch_seconds(value: &Value, what: &str) -> f64 {
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("{what} must be an ISO-8601 date-time string, got {value:?}"));
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|error| panic!("{what} must parse as a date-time ({error}): {text:?}"))
        .as_secs_f64()
}

/// The theme the sequences start from, under the name the case gives it: it carries every
/// optional member, so a patch that leaves one out is visibly keeping a stored value rather
/// than an absence.
///
/// A theme name is unique among the live themes of one app environment, so a suite that
/// creates one theme per row names each row's theme for itself.
fn base_theme_named(name: &str) -> Value {
    let mut data = base_theme();
    data["name"] = json!(name);
    data
}

/// The theme the sequences start from, under the one name a case that creates a single
/// theme uses.
fn base_theme() -> Value {
    json!({
        "name": "Base",
        "values": [{"key": "primaryColor", "value": {"value": "#047d95"}}],
        "overrides": [{"key": "dark", "value": {"value": "#000000"}}],
        "tags": {"env": "test", "team": "studio"},
    })
}

/// The patches the sequence is driven with, in order. They differ along what the rule
/// generalises over: how many `values` entries a patch carries, how deeply they nest,
/// which optional members it carries at all, and the published length bounds of `name`.
fn patch_sequence() -> Vec<(&'static str, Value)> {
    vec![
        (
            "one flat value, nothing else carried",
            json!({"values": [{"key": "a", "value": {"value": "b"}}]}),
        ),
        ("the values list emptied", json!({"values": []})),
        (
            "several flat values",
            json!({"values": [
                {"key": "primaryColor", "value": {"value": "#ff6600"}},
                {"key": "secondaryColor", "value": {"value": "#1f1f1f"}},
                {"key": "borderRadius", "value": {"value": "8px"}},
            ]}),
        ),
        (
            "values nested three levels deep",
            json!({"values": [
                {
                    "key": "tokens",
                    "value": {
                        "children": [
                            {
                                "key": "colors",
                                "value": {
                                    "children": [
                                        {
                                            "key": "brand",
                                            "value": {
                                                "children": [
                                                    {"key": "10", "value": {"value": "#e0f5f7"}},
                                                    {"key": "80", "value": {"value": "#047d95"}},
                                                ],
                                            },
                                        },
                                    ],
                                },
                            },
                            {"key": "space", "value": {"value": "8px"}},
                        ],
                    },
                },
            ]}),
        ),
        (
            "a value carrying neither key nor value",
            json!({"values": [{}]}),
        ),
        (
            "a new name beside the values",
            json!({
                "name": "Renamed",
                "values": [{"key": "a", "value": {"value": "b"}}],
            }),
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
        (
            "a replacement overrides list",
            json!({
                "values": [{"key": "a", "value": {"value": "b"}}],
                "overrides": [{"key": "light", "value": {"value": "#ffffff"}}],
            }),
        ),
        (
            "the overrides list emptied",
            json!({
                "values": [{"key": "a", "value": {"value": "b"}}],
                "overrides": [],
            }),
        ),
        (
            "nested overrides",
            json!({
                "values": [],
                "overrides": [
                    {
                        "key": "dark",
                        "value": {"children": [{"key": "primaryColor", "value": {"value": "#fff"}}]},
                    },
                ],
            }),
        ),
    ]
}

/// Updates accumulate on the one record: a member a patch leaves out keeps whatever the
/// PREVIOUS update left it, the identity members never move, `modifiedAt` never goes
/// backwards, and `GetTheme` reports the record the latest update wrote.
#[tokio::test]
async fn successive_updates_accumulate_on_one_record() {
    let router = emulator();
    let created = create(&router, base_theme()).await;
    assert_eq!(
        created.status, 200,
        "expected HTTP 200 from CreateTheme, got {} with {:?}",
        created.status, created.body
    );
    let id = created.id();

    // What each member must read as, carried forward from one call to the next.
    let mut expected_name = base_theme()["name"].clone();
    let mut expected_overrides = base_theme()["overrides"].clone();
    let mut previous_modified = epoch_seconds(&at(&created.body, "/modifiedAt"), "modifiedAt");

    for (case, patch) in patch_sequence() {
        let updated = update(&router, &id, patch.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateTheme, got {} with {:?}",
            updated.status, updated.body
        );
        if let Some(name) = patch.get("name") {
            expected_name = name.clone();
        }
        if let Some(overrides) = patch.get("overrides") {
            expected_overrides = overrides.clone();
        }

        // `UpdateThemeData$values` is `@required`, so each call replaces the list.
        assert_eq!(
            at(&updated.body, "/values"),
            patch["values"],
            "case {case:?}: values must be the list the latest update carried, got {:?}",
            at(&updated.body, "/values")
        );
        // `name` and `overrides` are optional: a call that leaves one out keeps what the
        // last call that CARRIED it left, not what the create stored.
        assert_eq!(
            at(&updated.body, "/name"),
            expected_name,
            "case {case:?}: expected the name {expected_name:?} the last update carrying it \
             set, got {:?}",
            at(&updated.body, "/name")
        );
        assert_eq!(
            at(&updated.body, "/overrides"),
            expected_overrides,
            "case {case:?}: expected the overrides {expected_overrides:?} the last update \
             carrying them set, got {:?}",
            at(&updated.body, "/overrides")
        );

        // The members `UpdateThemeData` does not carry at all never move, however many
        // updates have been applied.
        for (member, expected) in [
            ("/appId", json!(APP_ID)),
            ("/environmentName", json!(ENVIRONMENT)),
            ("/id", json!(id.clone())),
            ("/tags", base_theme()["tags"].clone()),
            ("/createdAt", at(&created.body, "/createdAt")),
        ] {
            assert_eq!(
                at(&updated.body, member),
                expected,
                "case {case:?}: {member} must stay {expected:?} across an update, got {:?}",
                at(&updated.body, member)
            );
        }

        // `modifiedAt` reports when the theme last changed, so it moves forward only.
        let modified = epoch_seconds(&at(&updated.body, "/modifiedAt"), "modifiedAt");
        assert!(
            modified >= previous_modified,
            "case {case:?}: modifiedAt ({modified}) must not go backwards from \
             {previous_modified}"
        );
        previous_modified = modified;

        // One record, not a new one per update, and the answer came from it.
        let read = get(&router, &id).await;
        assert_eq!(
            read.body, updated.body,
            "case {case:?}: GetTheme must report the record the latest update wrote, got {:?}",
            read.body
        );
        assert_eq!(
            get(&router, &id).await.body,
            read.body,
            "case {case:?}: reading an updated theme twice must report the same record"
        );
    }
}

/// Applying the same `updatedTheme` twice leaves the same record: the update is idempotent
/// in every member it writes, with or without a `clientToken`, and `modifiedAt` is the only
/// member a repeat can move.
#[tokio::test]
async fn repeating_an_update_leaves_the_same_record() {
    let router = emulator();

    let token_cases = [
        ("no clientToken", None),
        ("an ascii clientToken", Some("token-1")),
        (
            "a uuid-shaped clientToken",
            Some("6f1f0f28-4c1f-4df5-9a4e-6f2d5a6a0001"),
        ),
        ("a clientToken with punctuation", Some("retry::1/2")),
    ];

    for (row, (case, token)) in token_cases.into_iter().enumerate() {
        let id = create(&router, base_theme_named(&format!("Base-{row}")))
            .await
            .id();
        let patch = json!({
            "name": format!("Repeated-{row}"),
            "values": [{"key": "primaryColor", "value": {"value": "#ff6600"}}],
            "overrides": [],
        });

        let first = match token {
            None => update(&router, &id, patch.clone()).await,
            Some(token) => update_with_token(&router, &id, patch.clone(), token).await,
        };
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateTheme, got {} with {:?}",
            first.status, first.body
        );
        let second = match token {
            None => update(&router, &id, patch.clone()).await,
            Some(token) => update_with_token(&router, &id, patch.clone(), token).await,
        };
        assert_eq!(
            second.status, 200,
            "case {case:?}: expected HTTP 200 from the repeated UpdateTheme, got {} with {:?}",
            second.status, second.body
        );

        for member in [
            "/appId",
            "/environmentName",
            "/id",
            "/name",
            "/values",
            "/overrides",
            "/tags",
            "/createdAt",
        ] {
            assert_eq!(
                at(&second.body, member),
                at(&first.body, member),
                "case {case:?}: repeating the same update must leave {member} at {:?}, got {:?}",
                at(&first.body, member),
                at(&second.body, member)
            );
        }
        let first_modified = epoch_seconds(&at(&first.body, "/modifiedAt"), "modifiedAt");
        let second_modified = epoch_seconds(&at(&second.body, "/modifiedAt"), "modifiedAt");
        assert!(
            second_modified >= first_modified,
            "case {case:?}: modifiedAt ({second_modified}) must not go backwards from \
             {first_modified}"
        );

        // The repeat wrote the same record, so the reader reports it too.
        assert_eq!(
            get(&router, &id).await.body,
            second.body,
            "case {case:?}: GetTheme must report the record the repeated update left"
        );
    }
}
