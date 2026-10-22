// SPDX-License-Identifier: CC0-1.0

//! The dictionary lifecycle, end to end over the wire: `CreateDictionary` and
//! `GetDictionary`.
//!
//! Every case here drives the real request path — the router picks the service from the
//! SigV4 credential scope, the framework deserializes and validates the body, the
//! handler runs, and the response is serialized as `restJson1` — so what these tests
//! assert is what a client sees.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! case:
//!
//! * a dictionary is MINTED and PERSISTED: the create response is built from the stored
//!   record, and `GetDictionary` returns that same record member for member, for every
//!   language the model lists and whether or not the request carried entries or tags;
//! * a dictionary accepted with 202 starts in the transient `CREATING` status and
//!   reaches `AVAILABLE` on the clock, and reading it never moves it anywhere else;
//! * an id no live dictionary has is `ResourceNotFoundException` (HTTP 404), never a 501
//!   and never a 200;
//! * a `language` outside the modelled enum, and an `entries` payload past the modelled
//!   maximum, are refused as HTTP 400 `ValidationException`;
//! * the dictionary store is BOUNDED: past the quota `CreateDictionary` answers the
//!   modelled `ServiceQuotaExceededException` (HTTP 402).

use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    MAX_DICTIONARIES, TRANSITION_SPAN,
};
use service_com_amazonaws_elementalinference_elementalinference::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The region every request below is signed for. The service reads it out of the SigV4
/// credential scope, so the ARNs it mints must carry exactly this region.
const REGION: &str = "eu-west-1";

/// This service's SigV4 signing name (`OPERATIONS.md`, "Wire protocol"): the router
/// resolves the claiming service from it.
const SIGNING_NAME: &str = "elemental-inference";

/// Every value the `DictionaryLanguage` enum admits, from
/// `smithy_query(query="shape",shape="DictionaryLanguage")`: `eng`, `fra`, `ita`,
/// `deu`, `spa`, `por`.
const LANGUAGES: &[&str] = &["eng", "fra", "ita", "deu", "spa", "por"];

/// The modelled maximum length of `DictionaryEntriesPayload`
/// (`smithy_query(query="shape",shape="DictionaryEntriesPayload")`: `length max 40960`).
const MAX_ENTRIES_LEN: usize = 40960;

/// Long enough for a clock-owed dictionary transition to have fallen due.
fn past_the_transition_span() {
    // A plain blocking sleep: the test's runtime has nothing else to run, and the
    // service never sleeps for a transition — it stores the due instant.
    std::thread::sleep(TRANSITION_SPAN + std::time::Duration::from_millis(250));
}

/// A router serving this service alone over one shared engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These requests carry an unsigned placeholder Authorization header, which is
    // there only so the router can resolve the signing name.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request and return its status and parsed body (`Value::Null` when empty).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |body| serde_json::to_vec(&body).unwrap());
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .unwrap();
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|e| {
            panic!(
                "{method} {uri}: body is not JSON ({e}): {}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, parsed)
}

/// Whether `id` satisfies the modelled `DictionaryId` constraints: `^[a-zA-Z0-9]+$`,
/// between 1 and 19 characters.
fn is_dictionary_id(id: &str) -> bool {
    (1..=19).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric())
}

/// The ARN form the model gives a dictionary: `dictionary/<id>` under this service's
/// namespace, in the request's region and the emulator's account (the modelled
/// `resourceArn` pattern admits `:(feed|dictionary)/[a-zA-Z0-9]{1,19}`).
fn expected_arn(id: &str) -> String {
    format!(
        "arn:aws:{SIGNING_NAME}:{REGION}:{}:dictionary/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// One row of the create table.
struct DictionaryCase {
    label: String,
    body: Value,
}

/// A row per `DictionaryLanguage` value, differing also in whether `entries` is absent,
/// empty, short or at the modelled maximum, and in how many tags the request carries.
fn create_cases() -> Vec<DictionaryCase> {
    let entries_for = |index: usize, language: &str| -> Option<String> {
        match index % 4 {
            // No payload at all: `entries` is an optional member.
            0 => None,
            // A payload that is present but empty (the modelled length has no minimum).
            1 => Some(String::new()),
            2 => Some(format!("{language}-brand-name\n{language}-technical-term\n")),
            // Exactly the modelled maximum, so the longest admissible payload round
            // trips too.
            _ => Some("w".repeat(MAX_ENTRIES_LEN)),
        }
    };
    LANGUAGES
        .iter()
        .enumerate()
        .map(|(index, language)| {
            let entries = entries_for(index, language);
            let tags: Value = match index % 3 {
                0 => json!({}),
                1 => json!({"team": "subtitles"}),
                _ => json!({"team": "subtitles", "env": "test", "owner": "qa"}),
            };
            let mut body = json!({
                "name": format!("dict{index}-{language}"),
                "language": language,
            });
            let object = body.as_object_mut().unwrap();
            if let Some(entries) = entries.clone() {
                object.insert("entries".to_string(), json!(entries));
            }
            if !tags.as_object().unwrap().is_empty() {
                object.insert("tags".to_string(), tags.clone());
            }
            DictionaryCase {
                label: format!(
                    "language={language}, entries={}, tags={}",
                    entries.map_or("absent".to_string(), |e| format!("{} chars", e.len())),
                    tags.as_object().unwrap().len()
                ),
                body,
            }
        })
        .collect()
}

/// CreateDictionary mints and persists a dictionary, and GetDictionary returns that
/// stored record member for member.
#[tokio::test]
async fn create_dictionary_persists_a_dictionary_get_dictionary_reports_member_for_member() {
    let router = router();
    let mut minted: Vec<String> = Vec::new();

    for case in create_cases() {
        let label = &case.label;
        let submitted = case.body.clone();
        let (status, created) = call(&router, "POST", "/v1/dictionary", Some(case.body)).await;
        assert_eq!(
            status, 202,
            "case {label:?}: CreateDictionary answers 202, got {status} with {created}"
        );

        let id = created["id"].as_str().unwrap_or_default().to_string();
        assert!(
            is_dictionary_id(&id),
            "case {label:?}: the minted id satisfies the modelled DictionaryId, got {:?}",
            created["id"]
        );
        assert!(
            !minted.contains(&id),
            "case {label:?}: every minted id is distinct, got {id:?} twice"
        );
        minted.push(id.clone());

        assert_eq!(
            created["arn"],
            json!(expected_arn(&id)),
            "case {label:?}: the dictionary ARN names the minted id, got {:?}",
            created["arn"]
        );
        assert_eq!(
            created["name"], submitted["name"],
            "case {label:?}: the response repeats the requested name, got {:?}",
            created["name"]
        );
        assert_eq!(
            created["language"], submitted["language"],
            "case {label:?}: the response repeats the requested language, got {:?}",
            created["language"]
        );
        assert_eq!(
            created["status"],
            json!("CREATING"),
            "case {label:?}: a dictionary accepted with 202 starts in CREATING, got {:?}",
            created["status"]
        );
        assert_eq!(
            created["references"],
            json!([]),
            "case {label:?}: nothing references a just-created dictionary, got {:?}",
            created["references"]
        );
        match submitted.get("tags") {
            Some(tags) => assert_eq!(
                &created["tags"], tags,
                "case {label:?}: the response carries the requested tags, got {:?}",
                created["tags"]
            ),
            None => assert!(
                created["tags"].is_null(),
                "case {label:?}: a dictionary created without tags reports none, got {:?}",
                created["tags"]
            ),
        }

        // The round trip: everything but the status comes back from storage
        // unchanged.
        let (status, fetched) = call(&router, "GET", &format!("/v1/dictionary/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetDictionary on the created id answers 200, got {status} with {fetched}"
        );
        let strip_status = |body: &Value| {
            let mut body = body.clone();
            body.as_object_mut().unwrap().remove("status");
            body
        };
        assert_eq!(
            strip_status(&fetched),
            strip_status(&created),
            "case {label:?}: GetDictionary reports the stored dictionary, got {fetched}"
        );
    }

    assert_eq!(
        minted.len(),
        LANGUAGES.len(),
        "every language row minted a dictionary, got {minted:?}"
    );
}

/// A dictionary accepted in CREATING reaches AVAILABLE on the clock, and a caller
/// reading it straight away still sees the transient status. Reading it never moves it
/// anywhere else: once settled it stays AVAILABLE.
#[tokio::test]
async fn a_created_dictionary_settles_from_creating_to_available() {
    let router = router();
    for language in LANGUAGES {
        let (_, created) = call(
            &router,
            "POST",
            "/v1/dictionary",
            Some(json!({"name": format!("settling{language}"), "language": language})),
        )
        .await;
        let id = created["id"].as_str().unwrap().to_string();
        let label = format!("language={language}, id={id}");

        let (_, immediately) = call(&router, "GET", &format!("/v1/dictionary/{id}"), None).await;
        assert_eq!(
            immediately["status"],
            json!("CREATING"),
            "case {label:?}: a dictionary read straight after creation is still CREATING, got {:?}",
            immediately["status"]
        );

        past_the_transition_span();
        let (_, settled) = call(&router, "GET", &format!("/v1/dictionary/{id}"), None).await;
        assert_eq!(
            settled["status"],
            json!("AVAILABLE"),
            "case {label:?}: once creation is due the dictionary is AVAILABLE, got {:?}",
            settled["status"]
        );
        let (_, again) = call(&router, "GET", &format!("/v1/dictionary/{id}"), None).await;
        assert_eq!(
            again["status"],
            json!("AVAILABLE"),
            "case {label:?}: reading a settled dictionary leaves it AVAILABLE, got {:?}",
            again["status"]
        );
    }
}

/// An id no live dictionary has is a 404 `ResourceNotFoundException` — never a 501 and
/// never a 200.
#[tokio::test]
async fn an_unknown_dictionary_id_is_a_404() {
    let router = router();
    // Create one dictionary, so the 404 is about the id and not about an empty store.
    let (_, created) = call(
        &router,
        "POST",
        "/v1/dictionary",
        Some(json!({"name": "theonlydictionary", "language": "eng"})),
    )
    .await;
    let live_id = created["id"].as_str().unwrap().to_string();

    // Every id below satisfies `^[a-zA-Z0-9]+$` within 1..19 characters, so the
    // boundary admits it and the handler is what refuses it: the shortest admissible
    // id, the longest, and one in between.
    for id in ["a", "0000000000000000000", "Unknown123"] {
        assert_ne!(
            id, live_id,
            "the unknown ids must not be the live dictionary's"
        );
        let label = format!("GET /v1/dictionary/{id}");
        let (status, error) = call(&router, "GET", &format!("/v1/dictionary/{id}"), None).await;
        assert_eq!(
            status, 404,
            "case {label:?}: an unknown dictionary id is a 404, got {status} with {error}"
        );
        assert_eq!(
            error["__type"],
            json!("ResourceNotFoundException"),
            "case {label:?}: that 404 is a ResourceNotFoundException, got {error}"
        );
    }

    // And the live dictionary is untouched by all of that.
    let (status, fetched) = call(&router, "GET", &format!("/v1/dictionary/{live_id}"), None).await;
    assert_eq!(
        status, 200,
        "the live dictionary is still there, got {status} with {fetched}"
    );
}

/// A request outside the modelled constraints is refused as HTTP 400
/// `ValidationException` and persists nothing: a `language` the enum does not list, an
/// `entries` payload one character past the modelled maximum, and a name the
/// `ResourceName` pattern rejects.
#[tokio::test]
async fn a_request_outside_the_modelled_constraints_is_refused() {
    let router = router();
    let oversized = "w".repeat(MAX_ENTRIES_LEN + 1);
    let cases = [
        (
            "language not in the enum",
            json!({"name": "badlanguage", "language": "eng-us"}),
        ),
        (
            "language in the wrong case",
            json!({"name": "badlanguage", "language": "ENG"}),
        ),
        (
            "entries one past the modelled maximum",
            json!({"name": "bigentries", "language": "eng", "entries": oversized}),
        ),
        (
            "name the ResourceName pattern rejects",
            json!({"name": "-leading-dash", "language": "eng"}),
        ),
    ];

    for (label, body) in cases {
        let (status, error) = call(&router, "POST", "/v1/dictionary", Some(body)).await;
        assert_eq!(
            status, 400,
            "case {label:?}: the request is refused with 400, got {status} with {error}"
        );
        assert_eq!(
            error["__type"],
            json!("ValidationException"),
            "case {label:?}: that 400 is a ValidationException, got {error}"
        );
    }

    // The entries payload AT the modelled maximum is admissible, so the bound itself
    // is not what was refused above.
    let (status, created) = call(
        &router,
        "POST",
        "/v1/dictionary",
        Some(json!({
            "name": "atthemaximum",
            "language": "eng",
            "entries": "w".repeat(MAX_ENTRIES_LEN),
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "an entries payload at the modelled maximum is accepted, got {status} with {created}"
    );
}

/// The dictionary store is bounded: past the quota CreateDictionary refuses with the
/// modelled `ServiceQuotaExceededException` (HTTP 402) instead of growing state
/// further, and the dictionaries already stored keep working.
#[tokio::test]
async fn create_dictionary_refuses_past_the_dictionary_quota() {
    let router = router();
    let mut first_id = String::new();
    for index in 0..MAX_DICTIONARIES {
        let (status, created) = call(
            &router,
            "POST",
            "/v1/dictionary",
            Some(json!({"name": format!("quota{index}"), "language": "eng"})),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {index}: a create within the quota answers 202, got {status} with {created}"
        );
        if index == 0 {
            first_id = created["id"].as_str().unwrap().to_string();
        }
    }

    let (status, error) = call(
        &router,
        "POST",
        "/v1/dictionary",
        Some(json!({"name": "onetoomany", "language": "eng"})),
    )
    .await;
    assert_eq!(
        status, 402,
        "the create past the quota is refused with 402, got {status} with {error}"
    );
    assert_eq!(
        error["__type"],
        json!("ServiceQuotaExceededException"),
        "that 402 is a ServiceQuotaExceededException, got {error}"
    );

    let (status, fetched) = call(&router, "GET", &format!("/v1/dictionary/{first_id}"), None).await;
    assert_eq!(
        status, 200,
        "the dictionaries already stored keep working, got {status} with {fetched}"
    );
}
