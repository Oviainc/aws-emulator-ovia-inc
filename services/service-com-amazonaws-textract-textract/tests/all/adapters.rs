// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of Textract's adapter control plane over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: Textract.<Operation>`).
//!
//! Each test asserts a RULE over a TABLE of inputs rather than one recorded request:
//!
//! * `CreateAdapter` persists the adapter the request describes under a minted
//!   `AdapterId`, and `GetAdapter` reports exactly that stored configuration back —
//!   `AutoUpdate` defaulting to `DISABLED` when the request named none;
//! * an `AdapterId` is this service's own, unique per created adapter, and a `GetAdapter`
//!   of an id no create minted is refused with the modelled `ResourceNotFoundException`;
//! * one `ClientRequestToken` names one adapter: replayed with the same parameters it
//!   answers with that adapter, and with different parameters it is refused with the
//!   modelled `ValidationException` — the code Amazon Textract answers a `CreateAdapter`
//!   token conflict with;
//! * an adapter can only be trained on the feature type the service supports for
//!   adapters, so every other modelled `FeatureType` is refused;
//! * `GetAdapter` is a read: repeating it, and creating other adapters, never changes
//!   what it answers.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_textract_textract::ops::types::{
    AUTO_UPDATE_VALUES, FEATURE_TYPE_VALUES,
};
use service_com_amazonaws_textract_textract::state;
use service_com_amazonaws_textract_textract::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The emulator, with this one service mounted on a shared engine.
fn harness() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Textract.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice::<Value>(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// The wire error code (`__type`), without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn string_member(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `CreateAdapter`, asserting only that it succeeded, and the `AdapterId` it minted.
async fn create_adapter(router: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let (status, body) = call(router, "CreateAdapter", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateAdapter, got {status} with {body}"
    );
    let adapter_id = string_member(&body, "AdapterId");
    // `AdapterId`'s modelled length is 12..1011 (smithy_query shape=AdapterId).
    assert!(
        (12..=1011).contains(&adapter_id.len()),
        "case {case:?}: AdapterId must be 12..1011 characters, got {adapter_id:?} from {body}"
    );
    adapter_id
}

/// One row of the create/read-back table.
struct CreateCase {
    name: &'static str,
    request: Value,
    /// What `GetAdapter` must report back, member by member.
    expect_name: String,
    expect_description: Option<String>,
    expect_auto_update: &'static str,
    expect_feature_types: Vec<&'static str>,
    expect_tags: Value,
}

/// A name of `length` characters, inside `AdapterName`'s `^[a-zA-Z0-9-_]+$` pattern.
fn name_of_length(length: usize) -> String {
    "a".repeat(length)
}

/// A description of `length` characters, inside `AdapterDescription`'s pattern.
fn description_of_length(length: usize) -> String {
    "d".repeat(length)
}

#[tokio::test]
async fn create_adapter_persists_what_get_adapter_reports_back() {
    let router = harness();

    // The rows differ along what the rule generalises over: the name's length and
    // character set, whether a description is given, which `AutoUpdate` is asked for
    // (including none at all), and how many tags are attached. The modelled bounds come
    // from smithy_query: AdapterName len 1..128, AdapterDescription len 1..256,
    // TagMap len 0..200, AutoUpdate one of AUTO_UPDATE_VALUES.
    let cases = vec![
        CreateCase {
            name: "only the required members",
            request: json!({ "AdapterName": "minimal-adapter", "FeatureTypes": ["QUERIES"] }),
            expect_name: "minimal-adapter".to_string(),
            expect_description: None,
            // AWS, on CreateAdapter: "By default, AutoUpdate is set to DISABLED."
            expect_auto_update: "DISABLED",
            expect_feature_types: vec!["QUERIES"],
            expect_tags: json!({}),
        },
        CreateCase {
            name: "name at the modelled minimum length",
            request: json!({
                "AdapterName": name_of_length(1),
                "FeatureTypes": ["QUERIES"],
            }),
            expect_name: name_of_length(1),
            expect_description: None,
            expect_auto_update: "DISABLED",
            expect_feature_types: vec!["QUERIES"],
            expect_tags: json!({}),
        },
        CreateCase {
            name: "name at the modelled maximum length",
            request: json!({
                "AdapterName": name_of_length(128),
                "FeatureTypes": ["QUERIES"],
            }),
            expect_name: name_of_length(128),
            expect_description: None,
            expect_auto_update: "DISABLED",
            expect_feature_types: vec!["QUERIES"],
            expect_tags: json!({}),
        },
        CreateCase {
            name: "name using the pattern's - and _ characters",
            request: json!({
                "AdapterName": "Claims_Adapter-02",
                "FeatureTypes": ["QUERIES"],
                "AutoUpdate": "ENABLED",
            }),
            expect_name: "Claims_Adapter-02".to_string(),
            expect_description: None,
            expect_auto_update: "ENABLED",
            expect_feature_types: vec!["QUERIES"],
            expect_tags: json!({}),
        },
        CreateCase {
            name: "description with punctuation the pattern allows",
            request: json!({
                "AdapterName": "described-adapter",
                "FeatureTypes": ["QUERIES"],
                "Description": "Trained on claim forms (2024): rev. 3!",
                "AutoUpdate": "DISABLED",
            }),
            expect_name: "described-adapter".to_string(),
            expect_description: Some("Trained on claim forms (2024): rev. 3!".to_string()),
            expect_auto_update: "DISABLED",
            expect_feature_types: vec!["QUERIES"],
            expect_tags: json!({}),
        },
        CreateCase {
            name: "description at the modelled maximum length",
            request: json!({
                "AdapterName": "long-description-adapter",
                "FeatureTypes": ["QUERIES"],
                "Description": description_of_length(256),
            }),
            expect_name: "long-description-adapter".to_string(),
            expect_description: Some(description_of_length(256)),
            expect_auto_update: "DISABLED",
            expect_feature_types: vec!["QUERIES"],
            expect_tags: json!({}),
        },
        CreateCase {
            name: "an empty tag map",
            request: json!({
                "AdapterName": "untagged-adapter",
                "FeatureTypes": ["QUERIES"],
                "Tags": {},
            }),
            expect_name: "untagged-adapter".to_string(),
            expect_description: None,
            expect_auto_update: "DISABLED",
            expect_feature_types: vec!["QUERIES"],
            expect_tags: json!({}),
        },
        CreateCase {
            name: "three tags",
            request: json!({
                "AdapterName": "tagged-adapter",
                "FeatureTypes": ["QUERIES"],
                "Tags": { "team": "claims", "env": "test", "owner": "finance-ops" },
            }),
            expect_name: "tagged-adapter".to_string(),
            expect_description: None,
            expect_auto_update: "DISABLED",
            expect_feature_types: vec!["QUERIES"],
            expect_tags: json!({ "team": "claims", "env": "test", "owner": "finance-ops" }),
        },
        CreateCase {
            name: "the feature type listed twice",
            request: json!({
                "AdapterName": "repeated-feature-adapter",
                "FeatureTypes": ["QUERIES", "QUERIES"],
            }),
            expect_name: "repeated-feature-adapter".to_string(),
            expect_description: None,
            expect_auto_update: "DISABLED",
            expect_feature_types: vec!["QUERIES", "QUERIES"],
            expect_tags: json!({}),
        },
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for case in &cases {
        let adapter_id = create_adapter(&router, case.name, &case.request).await;
        assert!(
            minted.insert(adapter_id.clone()),
            "case {:?}: every created adapter needs its own AdapterId, {adapter_id:?} repeated",
            case.name
        );

        let (status, body) = call(
            &router,
            "GetAdapter",
            &json!({ "AdapterId": adapter_id.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: GetAdapter of {adapter_id:?} must succeed, got {status} with {body}",
            case.name
        );
        assert_eq!(
            string_member(&body, "AdapterId"),
            adapter_id,
            "case {:?}: GetAdapter must report the id it was asked for, got {body}",
            case.name
        );
        assert_eq!(
            string_member(&body, "AdapterName"),
            case.expect_name,
            "case {:?}: expected AdapterName {:?}, got {body}",
            case.name,
            case.expect_name
        );
        match &case.expect_description {
            Some(expected) => assert_eq!(
                &string_member(&body, "Description"),
                expected,
                "case {:?}: expected Description {expected:?}, got {body}",
                case.name
            ),
            None => assert!(
                body.get("Description").is_none(),
                "case {:?}: an adapter created without a Description must report none, got {body}",
                case.name
            ),
        }
        assert_eq!(
            string_member(&body, "AutoUpdate"),
            case.expect_auto_update,
            "case {:?}: expected AutoUpdate {:?}, got {body}",
            case.name,
            case.expect_auto_update
        );
        assert_eq!(
            body.get("FeatureTypes").cloned().unwrap_or(Value::Null),
            json!(case.expect_feature_types),
            "case {:?}: expected FeatureTypes {:?}, got {body}",
            case.name,
            case.expect_feature_types
        );
        assert_eq!(
            body.get("Tags").cloned().unwrap_or(Value::Null),
            case.expect_tags,
            "case {:?}: expected Tags {}, got {body}",
            case.name,
            case.expect_tags
        );
        let creation_time = body
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        assert!(
            creation_time > 0.0,
            "case {:?}: CreationTime must be the instant the adapter was created, got {body}",
            case.name
        );
    }

    // Every AutoUpdate value the model declares was exercised by the table above, so a
    // new one in the model shows up here rather than silently going untested.
    let exercised: BTreeSet<&str> = cases
        .iter()
        .map(|case| case.expect_auto_update)
        .collect::<BTreeSet<&str>>();
    let modelled: BTreeSet<&str> = AUTO_UPDATE_VALUES.iter().copied().collect();
    assert_eq!(
        exercised, modelled,
        "the table must cover every modelled AutoUpdate value, missing {:?}",
        modelled.difference(&exercised).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn every_minted_adapter_id_is_distinct_and_reads_back_its_own_adapter() {
    let router = harness();

    // One rule, many inputs: ids are this service's own, so a batch of creates under the
    // SAME adapter name must still mint one id per adapter, each reading back its own
    // description. Bounded at 64 so the test stays cheap.
    let mut ids: BTreeSet<String> = BTreeSet::new();
    for index in 0..64u32 {
        let description = format!("adapter number {index}");
        let case = format!("create #{index}");
        let adapter_id = create_adapter(
            &router,
            &case,
            &json!({
                "AdapterName": "same-name-every-time",
                "FeatureTypes": ["QUERIES"],
                "Description": description.clone(),
            }),
        )
        .await;
        assert!(
            ids.insert(adapter_id.clone()),
            "case {case:?}: AdapterId {adapter_id:?} was already minted for another adapter"
        );

        let (status, body) = call(&router, "GetAdapter", &json!({ "AdapterId": adapter_id })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetAdapter must succeed, got {status} with {body}"
        );
        assert_eq!(
            string_member(&body, "Description"),
            description,
            "case {case:?}: expected Description {description:?}, got {body}"
        );
    }
}

/// One row of the `ClientRequestToken` table: a second create carrying the SAME token.
struct TokenCase {
    name: &'static str,
    /// The members the second call changes, merged over the first call's body.
    changes: Value,
    /// Whether the second call must answer with the first call's adapter.
    idempotent: bool,
}

/// `base` with every member of `changes` applied; a `null` in `changes` DROPS the member.
fn merged(base: &Value, changes: &Value) -> Value {
    let mut out: Map<String, Value> = base.as_object().cloned().unwrap_or_default();
    for (key, value) in changes.as_object().cloned().unwrap_or_default() {
        if value.is_null() {
            out.remove(&key);
        } else {
            out.insert(key, value);
        }
    }
    Value::Object(out)
}

#[tokio::test]
async fn one_client_request_token_names_one_adapter() {
    let router = harness();

    // Rows differ along WHICH member the replay changes — the rule is that any member
    // shaping the adapter makes the token a mismatch, while a list repetition that
    // denotes the same feature set does not.
    let cases = [
        TokenCase {
            name: "identical replay",
            changes: json!({}),
            idempotent: true,
        },
        TokenCase {
            name: "the feature type repeated in the list",
            changes: json!({ "FeatureTypes": ["QUERIES", "QUERIES"] }),
            idempotent: true,
        },
        TokenCase {
            name: "a different adapter name",
            changes: json!({ "AdapterName": "some-other-name" }),
            idempotent: false,
        },
        TokenCase {
            name: "a different description",
            changes: json!({ "Description": "a different description" }),
            idempotent: false,
        },
        TokenCase {
            name: "the description dropped",
            changes: json!({ "Description": null }),
            idempotent: false,
        },
        TokenCase {
            name: "a different auto update",
            changes: json!({ "AutoUpdate": "DISABLED" }),
            idempotent: false,
        },
        TokenCase {
            name: "a different tag value",
            changes: json!({ "Tags": { "team": "billing" } }),
            idempotent: false,
        },
        TokenCase {
            name: "the tags dropped",
            changes: json!({ "Tags": null }),
            idempotent: false,
        },
        TokenCase {
            name: "an extra tag",
            changes: json!({ "Tags": { "team": "claims", "env": "test" } }),
            idempotent: false,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        // A fresh token per row, so rows do not interfere.
        let token = format!("token-{index}");
        let first = json!({
            "AdapterName": "token-adapter",
            "FeatureTypes": ["QUERIES"],
            "Description": "the first description",
            "AutoUpdate": "ENABLED",
            "Tags": { "team": "claims" },
            "ClientRequestToken": token,
        });
        let first_id = create_adapter(&router, case.name, &first).await;

        let (status, body) = call(&router, "CreateAdapter", &merged(&first, &case.changes)).await;
        if case.idempotent {
            assert_eq!(
                status, 200,
                "case {:?}: a replay of the same request must succeed, got {status} with {body}",
                case.name
            );
            assert_eq!(
                string_member(&body, "AdapterId"),
                first_id,
                "case {:?}: expected the first adapter {first_id:?}, got {body}",
                case.name
            );
        } else {
            assert_eq!(
                status, 400,
                "case {:?}: a reused token with other parameters must be refused, got {status} \
                 with {body}",
                case.name
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {:?}: expected ValidationException, got {body}",
                case.name
            );
            // The sentence AWS states this conflict with, and it quotes nothing of the
            // request back — not the token, and not the adapter it already names.
            assert_eq!(
                body.get("Message").and_then(Value::as_str),
                Some("ClientRequestToken already sent with different parameters."),
                "case {:?}: expected the token-conflict wording, got {body}",
                case.name
            );
            assert!(
                !body.to_string().contains(token.as_str()),
                "case {:?}: a refusal must not quote the token back, got {body}",
                case.name
            );
            assert!(
                body.get("AdapterId").is_none(),
                "case {:?}: a refusal must mint no adapter, got {body}",
                case.name
            );
        }
    }
}

#[tokio::test]
async fn adapters_created_without_one_token_are_separate_adapters() {
    let router = harness();
    let request = json!({ "AdapterName": "repeatable", "FeatureTypes": ["QUERIES"] });

    // No token at all: each call is its own adapter.
    let untokened_first = create_adapter(&router, "no token, first call", &request).await;
    let untokened_second = create_adapter(&router, "no token, second call", &request).await;
    assert_ne!(
        untokened_first, untokened_second,
        "two creates with no ClientRequestToken must create two adapters, both answered \
         {untokened_first:?}"
    );

    // Two DIFFERENT tokens over identical parameters: still two adapters.
    let mut tokened: Vec<String> = Vec::new();
    for token in ["token-a", "token-b"] {
        let body = merged(&request, &json!({ "ClientRequestToken": token }));
        tokened.push(create_adapter(&router, token, &body).await);
    }
    assert_ne!(
        tokened[0], tokened[1],
        "two creates under different tokens must create two adapters, both answered {:?}",
        tokened[0]
    );
    assert!(
        !tokened.contains(&untokened_first) && !tokened.contains(&untokened_second),
        "a tokened create must not answer with an adapter created without a token: {tokened:?} \
         vs {untokened_first:?}/{untokened_second:?}"
    );
}

#[tokio::test]
async fn an_adapter_can_only_be_trained_on_the_supported_feature_type() {
    let router = harness();

    // `FEATURE_TYPE_VALUES` is the FeatureType enum from the Smithy model; AWS, on
    // `CreateAdapterRequest$FeatureTypes`: "Currrenly, supported feature types are:
    // QUERIES". So every other modelled value, and a list mixing one in, is refused —
    // including values this table does not name individually.
    let mut cases: Vec<(String, Value)> = vec![
        ("an empty FeatureTypes list".to_string(), json!([])),
        (
            "the supported type mixed with another".to_string(),
            json!(["QUERIES", "TABLES"]),
        ),
    ];
    for feature_type in FEATURE_TYPE_VALUES
        .iter()
        .filter(|value| **value != state::ADAPTER_FEATURE_TYPE)
    {
        cases.push((format!("only {feature_type}"), json!([feature_type])));
    }
    assert!(
        cases.len() > 2,
        "the model must declare feature types beyond {}",
        state::ADAPTER_FEATURE_TYPE
    );

    for (name, feature_types) in &cases {
        let (status, body) = call(
            &router,
            "CreateAdapter",
            &json!({ "AdapterName": "unsupported-feature", "FeatureTypes": feature_types }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterException",
            "case {name:?}: expected InvalidParameterException, got {body}"
        );
        assert!(
            body.get("AdapterId").is_none(),
            "case {name:?}: a refused create must mint no adapter, got {body}"
        );
    }
}

#[tokio::test]
async fn an_auto_update_outside_the_modelled_enum_is_refused() {
    let router = harness();

    // `AUTO_UPDATE_VALUES` is the AutoUpdate enum from the Smithy model: a value outside
    // it is refused with HTTP 400 and mints nothing. Which 400 code answers depends on
    // whether the boundary validator or the handler refuses it, so the row asserts the
    // status and that no adapter was created, not one of the two codes.
    let cases = ["MAYBE", "enabled", "", "ENABLED "];
    for value in cases {
        assert!(
            !AUTO_UPDATE_VALUES.contains(&value),
            "case {value:?}: the table must name values the model does not declare"
        );
        let (status, body) = call(
            &router,
            "CreateAdapter",
            &json!({
                "AdapterName": "bad-auto-update",
                "FeatureTypes": ["QUERIES"],
                "AutoUpdate": value,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {value:?}: expected HTTP 400, got {status} with {body}"
        );
        assert!(
            !error_code(&body).is_empty(),
            "case {value:?}: a refusal must carry a wire error code, got {body}"
        );
        assert!(
            body.get("AdapterId").is_none(),
            "case {value:?}: a refused create must mint no adapter, got {body}"
        );
    }
}

#[tokio::test]
async fn get_adapter_refuses_an_id_no_create_minted() {
    let router = harness();
    let known = create_adapter(
        &router,
        "the one real adapter",
        &json!({ "AdapterName": "real-adapter", "FeatureTypes": ["QUERIES"] }),
    )
    .await;

    // Rows span the modelled AdapterId length (12..1011) and an id that differs from a
    // real one by a single character.
    let mut flipped: String = known.clone();
    flipped.pop();
    flipped.push(if known.ends_with('0') { '1' } else { '0' });
    let cases = [
        ("an id at the modelled minimum length", "a".repeat(12)),
        ("an id at the modelled maximum length", "b".repeat(1011)),
        ("a plausible 32-character id", "c".repeat(32)),
        ("a real id with its last character changed", flipped),
    ];

    for (name, adapter_id) in &cases {
        assert_ne!(
            adapter_id, &known,
            "case {name:?}: the table must not name the adapter that exists"
        );
        let (status, body) = call(&router, "GetAdapter", &json!({ "AdapterId": adapter_id })).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn get_adapter_does_not_change_what_a_later_caller_sees() {
    let router = harness();
    let request = json!({
        "AdapterName": "stable-adapter",
        "FeatureTypes": ["QUERIES"],
        "Description": "read me twice",
        "AutoUpdate": "ENABLED",
        "Tags": { "team": "claims" },
    });
    let adapter_id = create_adapter(&router, "the adapter under test", &request).await;
    let read = json!({ "AdapterId": adapter_id.clone() });

    let (first_status, first) = call(&router, "GetAdapter", &read).await;
    assert_eq!(
        first_status, 200,
        "the first read must succeed, got {first_status} with {first}"
    );

    // Reading again, and creating OTHER adapters in between, must not move anything.
    for index in 0..3u32 {
        let case = format!("after {index} intervening creates");
        let (status, repeat) = call(&router, "GetAdapter", &read).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the repeated read must succeed, got {status} with {repeat}"
        );
        assert_eq!(
            repeat, first,
            "case {case:?}: a read must answer identically, got {repeat} after {first}"
        );
        create_adapter(
            &router,
            &case,
            &json!({
                "AdapterName": format!("other-adapter-{index}"),
                "FeatureTypes": ["QUERIES"],
            }),
        )
        .await;
    }
}
