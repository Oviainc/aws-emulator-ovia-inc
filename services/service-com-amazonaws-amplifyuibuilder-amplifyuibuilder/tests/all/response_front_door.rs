// SPDX-License-Identifier: CC0-1.0

//! The two response headers AWS's front door puts on every AmplifyUIBuilder response,
//! end to end over HTTP.
//!
//! The rule under test: every response this service serves that carries a representation
//! declares the JSON media type WITH its charset parameter
//! (`application/json; charset=utf-8`, not the bare media type the restJson1 codec
//! writes) and carries an `ETag` validator derived from the response body — the same two
//! headers on every operation, because they come from the layer AWS fronts the service
//! with rather than from any operation's model (no operation of this service binds an
//! `@httpHeader` output member:
//! `smithy_query(query="operation", operation="GetComponent,GetTheme,UpdateTheme")`).
//! A response with no representation (an operation whose output is `Unit`) has no entity
//! to tag, so it carries no validator.
//!
//! The rows below span the kinds of response the framework renders — an `@httpPayload`
//! structure on a read and on a write, a document output, an empty (`Unit`) output, a
//! modelled refusal, a refusal the framework makes before any handler runs, a required
//! list output and a request that names no operation at all — because the rule is a
//! property of this service's response path, not of one operation. Every case goes
//! through `MultiServiceRouter::dispatch`, so each exercises routing, the handler and
//! serialization exactly as a served request does.
//!
//! The validator's VALUE is not asserted against any particular string: what a client
//! relies on is the contract of an entity tag, so the cases assert that instead — a
//! non-empty tag, the same tag for the same representation, and a different tag for a
//! different one.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The media type every AmplifyUIBuilder response that carries a JSON representation
/// declares. Recorded behaviour of the real service, which its model states nowhere.
const JSON_CONTENT_TYPE: &str = "application/json; charset=utf-8";

/// A SigV4 credential scope naming this service: it is what routes a request to the
/// amplifyuibuilder service.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment the cases address.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// A well-formed `Uuid` nothing created, for the not-found row.
const NEVER_CREATED_ID: &str = "11111111-2222-4333-8444-555555555555";

/// The longest `ThemeName` the model admits (`@length(min: 1, max: 255)` on `ThemeName`),
/// so one more character is a request the framework refuses before any handler runs.
const THEME_NAME_MAX_LENGTH: usize = 255;

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the status, the two headers under test, and the body
/// bytes the validator must be a function of.
struct Answer {
    status: u16,
    content_type: Option<String>,
    etag: Option<String>,
    body: Vec<u8>,
}

impl Answer {
    /// The answer's body as JSON, or `Value::Null` when it carries none.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "the answer is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The resource id an `@httpPayload` answer carries, or the empty string.
    fn id(&self) -> String {
        self.json()["id"].as_str().unwrap_or_default().to_string()
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
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    let content_type = header("content-type");
    let etag = header("etag");
    Answer {
        status,
        content_type,
        etag,
        body: response.into_body().to_vec(),
    }
}

fn components_uri() -> String {
    format!("/app/{APP_ID}/environment/{ENVIRONMENT}/components")
}

fn themes_uri() -> String {
    format!("/app/{APP_ID}/environment/{ENVIRONMENT}/themes")
}

/// The six members `CreateComponentData` requires, as a starting point for a case that is
/// about something other than those values.
fn component_data(name: &str) -> Value {
    json!({
        "name": name,
        "componentType": "View",
        "properties": {},
        "variants": [],
        "overrides": {},
        "bindingProperties": {},
    })
}

/// A `CreateThemeData` with one value.
fn theme_data(name: &str) -> Value {
    json!({"name": name, "values": [{"key": "primaryColor", "value": {"value": "#047d95"}}]})
}

/// One request the table drives, with what the rule expects of its answer.
struct Case {
    /// What this row is about, named in every assertion it drives.
    about: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// The status the row expects, so a row that stopped exercising what it is about
    /// (a refusal that became a success, say) fails instead of passing vacuously.
    status: u16,
    /// Whether the answer carries a representation — a non-empty body — and so must
    /// carry a validator for it.
    carries_a_representation: bool,
}

/// Assert the front-door rule on one answer.
fn assert_front_door(case: &str, answer: &Answer) {
    if answer.body.is_empty() {
        // Nothing to tag, and nothing to declare a media type for; a media type declared
        // anyway must still state its charset.
        assert_eq!(
            answer.etag, None,
            "case {case:?}: a response with no representation carries no ETag, got {:?}",
            answer.etag
        );
        if let Some(declared) = answer.content_type.as_deref() {
            assert_eq!(
                declared, JSON_CONTENT_TYPE,
                "case {case:?}: expected the declared media type {JSON_CONTENT_TYPE:?}, got \
                 {declared:?}"
            );
        }
        return;
    }
    assert_eq!(
        answer.content_type.as_deref(),
        Some(JSON_CONTENT_TYPE),
        "case {case:?}: expected content-type {JSON_CONTENT_TYPE:?}, got {:?}",
        answer.content_type
    );
    assert!(
        answer.etag.as_deref().is_some_and(|tag| !tag.is_empty()),
        "case {case:?}: expected a non-empty ETag over the {} byte body, got {:?}",
        answer.body.len(),
        answer.etag
    );
}

/// Every kind of response this service serves declares the charset-qualified media type
/// and carries an `ETag` over its body, and two answers carry the same validator exactly
/// when they carry the same representation.
#[tokio::test]
async fn every_response_carries_the_front_doors_media_type_and_validator() {
    let router = emulator();

    // Seeded so the read, update and delete rows address records that exist. The seeds
    // themselves are checked by the first rows of the table.
    let component = call(
        &router,
        "POST",
        &components_uri(),
        Some(component_data("Hero")),
    )
    .await;
    assert_eq!(
        component.status, 200,
        "seeding a component must succeed, got {} with {:?}",
        component.status,
        component.json()
    );
    let component_id = component.id();
    let doomed = call(
        &router,
        "POST",
        &components_uri(),
        Some(component_data("Doomed")),
    )
    .await
    .id();
    let theme = call(&router, "POST", &themes_uri(), Some(theme_data("Studio"))).await;
    assert_eq!(
        theme.status, 200,
        "seeding a theme must succeed, got {} with {:?}",
        theme.status,
        theme.json()
    );
    let theme_id = theme.id();

    let cases = vec![
        Case {
            about: "CreateComponent: an @httpPayload structure on a write",
            method: "POST",
            uri: components_uri(),
            body: Some(component_data("Card")),
            status: 200,
            carries_a_representation: true,
        },
        Case {
            about: "GetComponent: an @httpPayload structure on a read",
            method: "GET",
            uri: format!("{}/{component_id}", components_uri()),
            body: None,
            status: 200,
            carries_a_representation: true,
        },
        Case {
            about: "UpdateComponent: an @httpPayload structure on a patch",
            method: "PATCH",
            uri: format!("{}/{component_id}", components_uri()),
            body: Some(json!({"name": "HeroV2"})),
            status: 200,
            carries_a_representation: true,
        },
        Case {
            about: "ListComponents: a document output",
            method: "GET",
            uri: components_uri(),
            body: None,
            status: 200,
            carries_a_representation: true,
        },
        Case {
            about: "ExportComponents: a document output on another route prefix",
            method: "GET",
            uri: format!("/export/app/{APP_ID}/environment/{ENVIRONMENT}/components"),
            body: None,
            status: 200,
            carries_a_representation: true,
        },
        Case {
            about: "CreateTheme: an @httpPayload structure of another resource",
            method: "POST",
            uri: themes_uri(),
            body: Some(theme_data("Bare")),
            status: 200,
            carries_a_representation: true,
        },
        Case {
            about: "GetTheme: an @httpPayload structure on a read",
            method: "GET",
            uri: format!("{}/{theme_id}", themes_uri()),
            body: None,
            status: 200,
            carries_a_representation: true,
        },
        Case {
            about: "UpdateTheme: an @httpPayload structure on a patch",
            method: "PATCH",
            uri: format!("{}/{theme_id}", themes_uri()),
            body: Some(json!({
                "name": "Restyled",
                "values": [{"key": "primaryColor", "value": {"value": "#000000"}}],
            })),
            status: 200,
            carries_a_representation: true,
        },
        Case {
            about: "GetMetadata: a required map output",
            method: "GET",
            uri: format!("/app/{APP_ID}/environment/{ENVIRONMENT}/metadata"),
            body: None,
            status: 200,
            carries_a_representation: true,
        },
        Case {
            about: "PutMetadataFlag: a Unit output on a write",
            method: "PUT",
            uri: format!("/app/{APP_ID}/environment/{ENVIRONMENT}/metadata/features/isRelationshipSupported"),
            body: Some(json!({"newValue": "true"})),
            status: 200,
            carries_a_representation: false,
        },
        Case {
            about: "DeleteComponent: a Unit output",
            method: "DELETE",
            uri: format!("{}/{doomed}", components_uri()),
            body: None,
            status: 200,
            carries_a_representation: false,
        },
        Case {
            about: "GetComponent: the modelled 404 refusal",
            method: "GET",
            uri: format!("{}/{NEVER_CREATED_ID}", components_uri()),
            body: None,
            status: 404,
            carries_a_representation: true,
        },
        Case {
            about: "CreateTheme: a refusal the framework makes before any handler runs",
            method: "POST",
            uri: themes_uri(),
            body: Some(json!({
                "name": "n".repeat(THEME_NAME_MAX_LENGTH + 1),
                "values": [],
            })),
            status: 400,
            carries_a_representation: true,
        },
        Case {
            about: "ListThemes: a required list output",
            method: "GET",
            uri: themes_uri(),
            body: None,
            status: 200,
            carries_a_representation: true,
        },
        Case {
            about: "a request this service claims that names no operation",
            method: "GET",
            uri: "/app/not-an-operation".to_string(),
            body: None,
            status: 501,
            carries_a_representation: true,
        },
    ];

    // (the case, the body it answered, the validator it answered) for the validator
    // invariants below.
    let mut tagged: Vec<(&'static str, Vec<u8>, String)> = Vec::new();

    for case in cases {
        let answer = call(&router, case.method, &case.uri, case.body.clone()).await;
        assert_eq!(
            answer.status, case.status,
            "case {:?}: expected HTTP {}, got {} with {:?}",
            case.about,
            case.status,
            answer.status,
            answer.json()
        );
        assert_eq!(
            !answer.body.is_empty(),
            case.carries_a_representation,
            "case {:?}: expected a representation={}, got {} bytes",
            case.about,
            case.carries_a_representation,
            answer.body.len()
        );
        assert_front_door(case.about, &answer);

        if let Some(tag) = answer.etag.clone() {
            for (other, body, other_tag) in &tagged {
                if *body == answer.body {
                    assert_eq!(
                        &tag, other_tag,
                        "cases {:?} and {other:?} answered the same representation, so they \
                         must answer the same validator, got {tag:?} and {other_tag:?}",
                        case.about
                    );
                } else {
                    assert_ne!(
                        &tag, other_tag,
                        "cases {:?} and {other:?} answered different representations, so they \
                         must answer different validators, got {tag:?} twice",
                        case.about
                    );
                }
            }
            tagged.push((case.about, answer.body.clone(), tag));
        }
    }

    assert!(
        tagged.len() >= 2,
        "the table must exercise at least two validated responses, got {}",
        tagged.len()
    );
}

/// The validator identifies the representation: re-reading an unchanged resource answers
/// the same tag, two resources answer different tags, and a resource that was updated
/// answers a new one.
#[tokio::test]
async fn the_validator_tracks_the_representation_it_identifies() {
    let router = emulator();

    // Two components and a theme, so the rows differ in resource, in name and in which
    // operation served them.
    let first = call(
        &router,
        "POST",
        &components_uri(),
        Some(component_data("First")),
    )
    .await
    .id();
    let second = call(
        &router,
        "POST",
        &components_uri(),
        Some(component_data("Second")),
    )
    .await
    .id();
    let theme = call(&router, "POST", &themes_uri(), Some(theme_data("Themed")))
        .await
        .id();

    let reads: [(&str, String); 3] = [
        ("GetComponent on the first component", format!("{}/{first}", components_uri())),
        ("GetComponent on the second component", format!("{}/{second}", components_uri())),
        ("GetTheme", format!("{}/{theme}", themes_uri())),
    ];

    let mut tags: Vec<(&str, String)> = Vec::new();
    for (case, uri) in &reads {
        let answer = call(&router, "GET", uri, None).await;
        let tag = answer
            .etag
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: expected an ETag, got none"));

        // A read reports; reading twice answers the same representation and so the same
        // validator.
        let again = call(&router, "GET", uri, None).await;
        assert_eq!(
            again.etag.as_deref(),
            Some(tag.as_str()),
            "case {case:?}: re-reading an unchanged resource must answer the same validator, \
             got {:?} then {:?}",
            tag,
            again.etag
        );

        for (other, other_tag) in &tags {
            assert_ne!(
                &tag, other_tag,
                "cases {case:?} and {other:?} are different resources, so their validators \
                 must differ, got {tag:?} twice"
            );
        }
        tags.push((case, tag));
    }

    // A write that changes the representation changes the validator with it.
    let before = tags
        .first()
        .map(|(_, tag)| tag.clone())
        .expect("the first read answered a validator");
    let updated = call(
        &router,
        "PATCH",
        &format!("{}/{first}", components_uri()),
        Some(json!({"name": "FirstRenamed"})),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "UpdateComponent must succeed, got {} with {:?}",
        updated.status,
        updated.json()
    );
    let after = call(&router, "GET", &format!("{}/{first}", components_uri()), None)
        .await
        .etag;
    assert_ne!(
        after.as_deref(),
        Some(before.as_str()),
        "case \"an updated component\": a changed representation must answer a new validator, \
         got {before:?} twice"
    );
    assert!(
        after.as_deref().is_some_and(|tag| !tag.is_empty()),
        "case \"an updated component\": expected a non-empty validator, got {after:?}"
    );
}
