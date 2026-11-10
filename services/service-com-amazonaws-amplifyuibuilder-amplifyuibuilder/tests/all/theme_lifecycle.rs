// SPDX-License-Identifier: CC0-1.0

//! The Amplify UI Builder theme lifecycle, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test is the one the control plane states for a theme: a
//! theme's members are whatever the create stored, and every operation computes its
//! answer from that record — never from the request that is asking.

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The longest `ThemeName` the model admits: `CreateThemeData$name` targets `ThemeName`,
/// modelled `@length(min: 1, max: 255)`
/// (`smithy_query(query="shape", shape="CreateThemeData")`). Spelled out here,
/// independently of the model the crate compiles against, so a wrong bound fails.
const THEME_NAME_MAX_LENGTH: usize = 255;

/// The shortest `ThemeName` that member admits.
const THEME_NAME_MIN_LENGTH: usize = 1;

/// A SigV4 credential scope naming this service: it is what routes a request to the
/// amplifyuibuilder service.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment the cases address, unless a case is about addressing
/// another one.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// A well-formed `Uuid` nothing created, for the not-found rows.
const NEVER_CREATED_ID: &str = "11111111-2222-4333-8444-555555555555";

/// `Theme$id` targets the model's `Uuid` shape. Checked here against the textual form of a
/// version 4 UUID rather than through the crate's own minting code, so an id that is not a
/// UUID cannot make this test pass.
fn is_uuid_v4(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        let ok = match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase(),
        };
        if !ok {
            return false;
        }
    }
    // Version nibble, then the variant nibble the RFC pins to one of 8, 9, a, b.
    bytes[14] == b'4' && matches!(bytes[19], b'8' | b'9' | b'a' | b'b')
}

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
    /// `CreateTheme` and `GetTheme` both bind their theme to `@httpPayload`, so the body
    /// IS the `Theme`.
    fn id(&self) -> String {
        self.body["id"].as_str().unwrap_or_default().to_string()
    }

    /// The error-type discriminator the body carries — `__type`, or the `code` / `Code`
    /// spellings the same JSON error parsers also accept — or the empty string when it
    /// carries none.
    ///
    /// Every refusal of one of this service's own modelled exceptions must carry NONE of
    /// them: AmplifyUIBuilder names no exception on the wire, so a client classifies a
    /// failure by HTTP status (see `component_refusals.rs`).
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

async fn create_in_with_token(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    data: Value,
    token: &str,
) -> Answer {
    let uri = format!("{}?clientToken={}", themes_uri(app_id, environment), token);
    call(router, "POST", &uri, Some(data)).await
}

async fn create_with_token(router: &MultiServiceRouter, data: Value, token: &str) -> Answer {
    create_in_with_token(router, APP_ID, ENVIRONMENT, data, token).await
}

async fn get_in(router: &MultiServiceRouter, app_id: &str, environment: &str, id: &str) -> Answer {
    let uri = format!("{}/{id}", themes_uri(app_id, environment));
    call(router, "GET", &uri, None).await
}

async fn get(router: &MultiServiceRouter, id: &str) -> Answer {
    get_in(router, APP_ID, ENVIRONMENT, id).await
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// True when `value` is a JSON string an ISO-8601 / RFC 3339 date-time parser accepts —
/// the wire form `@timestampFormat("date-time")` requires of `Theme$createdAt` and
/// `$modifiedAt`.
fn is_date_time(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|text| DateTime::from_str(text, Format::DateTime).is_ok())
}

/// The create payloads the round-trip cases drive, differing along what the rule
/// generalises over: how many `values` entries there are, how deeply their
/// `ThemeValue$children` nest, whether the optional `overrides` and `tags` are present at
/// all, and the published length bounds of `name`.
fn create_cases() -> Vec<(&'static str, Value)> {
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
                    {"key": "borderRadius", "value": {"value": "4px"}},
                ],
                "tags": {"env": "test", "team": "studio"},
            }),
        ),
        (
            "values nested three levels deep, plus overrides",
            json!({
                "name": "Nested",
                "values": [
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
                ],
                "overrides": [
                    {
                        "key": "dark",
                        "value": {
                            "children": [
                                {"key": "primaryColor", "value": {"value": "#ffffff"}},
                            ],
                        },
                    },
                ],
            }),
        ),
        (
            "a value carrying neither key nor value, and an empty overrides list",
            json!({"name": "Sparse", "values": [{}], "overrides": []}),
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

/// `CreateTheme` persists what it was given and `GetTheme` reports that record: every
/// member the create carried comes back with the value it was stored with, the id is a
/// fresh UUID, `createdAt` and `modifiedAt` are date-time strings at the same instant, and
/// the two bodies are equal.
#[tokio::test]
async fn a_created_theme_is_read_back_from_what_was_stored() {
    let router = emulator();
    let mut minted: Vec<String> = Vec::new();

    for (case, data) in create_cases() {
        let created = create(&router, data.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateTheme, got {} with {:?}",
            created.status, created.body
        );

        let id = created.id();
        assert!(
            is_uuid_v4(&id),
            "case {case:?}: expected a version 4 UUID id, got {id:?}"
        );
        assert!(
            !minted.contains(&id),
            "case {case:?}: CreateTheme reissued the id {id:?}"
        );
        minted.push(id.clone());

        assert_eq!(
            at(&created.body, "/appId"),
            json!(APP_ID),
            "case {case:?}: the created theme must carry the appId it was created under, got {:?}",
            at(&created.body, "/appId")
        );
        assert_eq!(
            at(&created.body, "/environmentName"),
            json!(ENVIRONMENT),
            "case {case:?}: the created theme must carry the environment it was created \
             under, got {:?}",
            at(&created.body, "/environmentName")
        );
        assert_eq!(
            at(&created.body, "/name"),
            data["name"],
            "case {case:?}: the created theme must carry the name it was created with, got {:?}",
            at(&created.body, "/name")
        );
        assert_eq!(
            at(&created.body, "/values"),
            data["values"],
            "case {case:?}: values must come back exactly as supplied, got {:?}",
            at(&created.body, "/values")
        );
        assert_eq!(
            at(&created.body, "/overrides"),
            data.get("overrides").cloned().unwrap_or(Value::Null),
            "case {case:?}: an absent overrides stays absent and a supplied one comes back \
             as supplied, got {:?}",
            at(&created.body, "/overrides")
        );
        // `Theme$tags` is reported for every theme, so a create that supplied none reports
        // the empty map it was stored with — the same convention `Component$tags` follows.
        assert_eq!(
            at(&created.body, "/tags"),
            data.get("tags").cloned().unwrap_or_else(|| json!({})),
            "case {case:?}: tags must come back as supplied, got {:?}",
            at(&created.body, "/tags")
        );
        assert!(
            is_date_time(&at(&created.body, "/createdAt")),
            "case {case:?}: createdAt must be an ISO-8601 date-time string, got {:?}",
            at(&created.body, "/createdAt")
        );
        assert_eq!(
            at(&created.body, "/modifiedAt"),
            at(&created.body, "/createdAt"),
            "case {case:?}: a theme nothing has updated reports modifiedAt at its createdAt, \
             got {:?}",
            at(&created.body, "/modifiedAt")
        );

        let read = get(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetTheme, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, created.body,
            "case {case:?}: GetTheme must report the record CreateTheme stored, got {:?}",
            read.body
        );

        // A read reports; it changes nothing a later caller sees.
        assert_eq!(
            get(&router, &id).await.body,
            read.body,
            "case {case:?}: reading a theme twice must report the same record"
        );
    }

    // Every theme created above is still readable: a later create never disturbed an
    // earlier record.
    for id in &minted {
        assert_eq!(
            get(&router, id).await.id(),
            *id,
            "theme {id:?} must still be readable after the whole table was created"
        );
    }
}

/// A `name` outside the published `@length(min: 1, max: 255)` of `ThemeName` is refused,
/// and the refusal stores nothing.
#[tokio::test]
async fn a_name_outside_the_published_bounds_is_refused() {
    let router = emulator();

    // One past each published bound.
    for length in [THEME_NAME_MIN_LENGTH - 1, THEME_NAME_MAX_LENGTH + 1] {
        let refused = create(
            &router,
            json!({"name": "n".repeat(length), "values": []}),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case name length {length}: a name outside @length(1, 255) must be refused, got \
             {} with {:?}",
            refused.status, refused.body
        );
        assert!(
            refused.id().is_empty(),
            "case name length {length}: a refused create must mint no theme, got {:?}",
            refused.body
        );
    }

    // Each bound itself is accepted, so the refusal above is about the bound and not about
    // the member.
    for length in [THEME_NAME_MIN_LENGTH, THEME_NAME_MAX_LENGTH] {
        let created = create(
            &router,
            json!({"name": "n".repeat(length), "values": []}),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case name length {length}: a name on the published bound must be accepted, got \
             {} with {:?}",
            created.status, created.body
        );
    }
}

/// `CreateThemeRequest$clientToken` is an `@idempotencyToken`: the same token in the same
/// app environment names the same theme, a different token mints another, and the token is
/// scoped to the app environment it was used in.
#[tokio::test]
async fn the_client_token_makes_create_theme_idempotent() {
    let router = emulator();
    let theme_named = |name: &str| {
        json!({
            "name": name,
            "values": [{"key": "primaryColor", "value": {"value": "#047d95"}}],
        })
    };

    let cases = [
        ("an ascii token", "token-1"),
        ("a uuid-shaped token", "6f1f0f28-4c1f-4df5-9a4e-6f2d5a6a0001"),
        ("a token with punctuation", "retry::1/2"),
    ];

    // A theme name is unique among the live themes of one app environment, so every row
    // and every create within a row that mints another theme carries its own name.
    for (row, (case, token)) in cases.into_iter().enumerate() {
        let data = theme_named(&format!("Tokened-{row}"));
        let first = create_with_token(&router, data.clone(), token).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected HTTP 200 from CreateTheme, got {} with {:?}",
            first.status, first.body
        );
        let retried = create_with_token(&router, data.clone(), token).await;
        assert_eq!(
            retried.body, first.body,
            "case {case:?}: a retry carrying the same clientToken must report the theme that \
             token created, got {:?}",
            retried.body
        );

        // Another token, same payload but its own name: a second theme, not the first one
        // again.
        let other = create_with_token(
            &router,
            theme_named(&format!("Tokened-{row}-other")),
            &format!("{token}-other"),
        )
        .await;
        assert_ne!(
            other.id(),
            first.id(),
            "case {case:?}: a different clientToken must mint another theme, got the id {:?} \
             twice",
            other.id()
        );

        // The same token against another app environment addresses that environment's own
        // theme, which is not the one the token created here.
        let elsewhere = create_in_with_token(&router, APP_ID, "prod", data.clone(), token).await;
        assert_ne!(
            elsewhere.id(),
            first.id(),
            "case {case:?}: a clientToken is scoped to the app environment it was used in, \
             got the id {:?} in both",
            elsewhere.id()
        );
        assert_eq!(
            at(&elsewhere.body, "/environmentName"),
            json!("prod"),
            "case {case:?}: the theme created under another environment must report it, got {:?}",
            at(&elsewhere.body, "/environmentName")
        );
    }
}

/// `GetTheme` answers from the record the addressed (appId, environmentName) holds, and
/// tells the two miss classes apart: an id another app environment owns is a wrong-scope
/// request (HTTP 400) and an id that names no theme anywhere is absent (HTTP 404). Either
/// refusal leaves the store alone.
#[tokio::test]
async fn get_theme_refuses_an_id_that_names_no_theme_in_that_app_environment() {
    let router = emulator();
    let id = create(
        &router,
        json!({"name": "Scoped", "values": [{"key": "a", "value": {"value": "b"}}]}),
    )
    .await
    .id();

    // The rows differ in HOW the lookup misses: an id nobody created (absent), an id of
    // this environment asked for under another app, and the same id under another
    // environment of this app (both wrong-scope).
    let rows: Vec<(&str, &str, &str, &str, u16)> = vec![
        (
            "an id nobody created",
            APP_ID,
            ENVIRONMENT,
            NEVER_CREATED_ID,
            404,
        ),
        ("another app", "d9999zzzz", ENVIRONMENT, &id, 400),
        ("another environment of this app", APP_ID, "prod", &id, 400),
    ];

    for (case, app_id, environment, lookup, expected) in rows {
        let read = get_in(&router, app_id, environment, lookup).await;
        assert_eq!(
            read.status, expected,
            "case {case:?}: expected HTTP {expected} from GetTheme, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error_code(),
            "",
            "case {case:?}: a refusal must name no error type, got {:?}",
            read.body
        );
    }

    // The theme is still there: a failed lookup changed nothing.
    assert_eq!(
        get(&router, &id).await.id(),
        id,
        "a 404 lookup must not touch the store"
    );
}
