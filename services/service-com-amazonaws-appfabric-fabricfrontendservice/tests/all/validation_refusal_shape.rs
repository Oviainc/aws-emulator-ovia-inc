// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SHAPE of every `ValidationException` this service
//! answers, driven over the wire (restJson1) through the same router the emulator
//! boots.
//!
//! The RULE under test: a refusal that names the modelled `ValidationException`
//! carries every member the model requires of it — `message` AND `reason` — whichever
//! layer of the service found the fault, so a client can deserialize the modelled
//! shape and branch on `reason` for any refused call. A refusal that faults a NAMED
//! member of the request also names it in `fieldList`.
//!
//! The table below spans the fault classes a request can carry rather than one
//! recorded call: each constraint class the model declares (`@range` past either
//! published bound, `@length` past either, `@pattern`, an absent `@required` member,
//! a nested member inside a list), both the places a value is bound (a path label, a
//! query parameter, a body member), a request the codec cannot read at all, and the
//! refusals this service's own handlers raise — over several operations, because the
//! members are a property of the exception shape and not of one operation.

use serde_json::{json, Value};

use super::app_authorization_and_ingestion_dependency::bundle_with_ingestion;
use super::app_bundle_ingestion_lifecycle::{arn_of, encode, uuid_of, Answer, Emulator};
use super::ingestion_enablement::start_ingestion;
use super::pagination_token_lifetime::message;
use super::resource_tags::tag_list;

/// Every value the model's `ValidationExceptionReason` declares — the set a refusal's
/// `reason` is drawn from, from `smithy_query(query="shape",
/// shape="ValidationExceptionReason")`.
const MODELLED_REASONS: &[&str] = &[
    "unknownOperation",
    "cannotParse",
    "fieldValidationFailed",
    "other",
];

/// The modelled reason for a request whose NAMED member the service read and does not
/// accept.
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The modelled reason for a request the service could not read at all.
const CANNOT_PARSE: &str = "cannotParse";

/// The modelled reason for a refusal that faults no member of the request.
const OTHER: &str = "other";

/// The longest `nextToken` the model allows, from `String2048`'s
/// `@length(min: 1, max: 2048)`.
const MAX_NEXT_TOKEN: usize = 2048;

/// The longest tag key the model allows, from `TagKey`'s `@length(min: 1, max: 128)`.
const MAX_TAG_KEY: usize = 128;

/// One refused call: how it is made, and what its refusal must state.
struct Refusal {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// The `reason` the refusal must carry.
    reason: &'static str,
    /// The member `fieldList` must name, or `None` where the refusal faults no
    /// member of the request (the model leaves `fieldList` optional).
    ///
    /// Matched as a PREFIX of the name: a fault inside a collection names the
    /// dotted path from the request member down to the value that failed, and the
    /// member it descends from is what the row is about.
    field: Option<&'static str>,
}

/// The `reason` of a refusal, or `""`.
pub(crate) fn reason(answer: &Answer) -> String {
    answer
        .body
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `name`/`message` pairs of a refusal's `fieldList`, in the order it states
/// them.
pub(crate) fn field_list(answer: &Answer) -> Vec<(String, String)> {
    answer
        .body
        .get("fieldList")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    let member = |name: &str| {
                        entry
                            .get(name)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    (member("name"), member("message"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Assert one refusal states the modelled shape: the status and code the model gives
/// `ValidationException`, a non-empty `message`, a `reason` from the modelled set, and
/// a `fieldList` naming the member at fault where there is one.
fn assert_modelled_refusal(refused: &Refusal, answer: &Answer) {
    let case = refused.case;
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {}",
        answer.body
    );
    assert!(
        !message(answer).is_empty(),
        "case {case:?}: the refusal must state a message, got {}",
        answer.body
    );
    let stated = reason(answer);
    assert!(
        MODELLED_REASONS.contains(&stated.as_str()),
        "case {case:?}: the reason must be one of {MODELLED_REASONS:?}, got {stated:?} in {}",
        answer.body
    );
    assert_eq!(
        stated, refused.reason,
        "case {case:?}: expected reason {:?}, got {stated:?} in {}",
        refused.reason, answer.body
    );
    let fields = field_list(answer);
    match refused.field {
        Some(member) => {
            assert!(
                fields.iter().any(|(name, _)| name.starts_with(member)),
                "case {case:?}: expected a fieldList entry naming {member:?}, got {fields:?}"
            );
            assert!(
                fields.iter().all(|(_, stated)| !stated.is_empty()),
                "case {case:?}: every fieldList entry states what is wrong, got {fields:?}"
            );
        }
        None => assert!(
            fields.is_empty(),
            "case {case:?}: a refusal that faults no member names none, got {fields:?}"
        ),
    }
}

/// Every refused call of the table, over a bundle and an ingestion the fixture
/// created.
fn refusals(bundle: &str, ingestion: &str) -> Vec<Refusal> {
    let bundle_path = encode(bundle);
    let ingestion_path = encode(ingestion);
    vec![
        // `@range` on `maxResults` (`MaxResults`, `@range(min: 1, max: 100)`), past
        // each published bound, bound as a query parameter.
        Refusal {
            case: "maxResults past the published maximum",
            method: "GET",
            uri: "/appbundles?maxResults=101".to_string(),
            body: None,
            reason: FIELD_VALIDATION_FAILED,
            field: Some("maxResults"),
        },
        Refusal {
            case: "maxResults below the published minimum",
            method: "GET",
            uri: "/appbundles?maxResults=0".to_string(),
            body: None,
            reason: FIELD_VALIDATION_FAILED,
            field: Some("maxResults"),
        },
        Refusal {
            case: "maxResults past the published maximum on a nested listing",
            method: "GET",
            uri: format!("/appbundles/{bundle_path}/ingestions?maxResults=1000"),
            body: None,
            reason: FIELD_VALIDATION_FAILED,
            field: Some("maxResults"),
        },
        // `@length` on `nextToken` (`String2048`), past each published bound.
        Refusal {
            case: "nextToken below the published minimum length",
            method: "GET",
            uri: "/appbundles?nextToken=".to_string(),
            body: None,
            reason: FIELD_VALIDATION_FAILED,
            field: Some("nextToken"),
        },
        Refusal {
            case: "nextToken past the published maximum length",
            method: "GET",
            uri: format!("/appbundles?nextToken={}", "a".repeat(MAX_NEXT_TOKEN + 1)),
            body: None,
            reason: FIELD_VALIDATION_FAILED,
            field: Some("nextToken"),
        },
        // `@pattern` on `Identifier`, bound as a path label.
        Refusal {
            case: "an app bundle identifier outside the modelled pattern",
            method: "GET",
            uri: "/appbundles/neither-an-arn-nor-a-uuid".to_string(),
            body: None,
            reason: FIELD_VALIDATION_FAILED,
            field: Some("appBundleIdentifier"),
        },
        Refusal {
            case: "an ingestion identifier outside the modelled pattern",
            method: "GET",
            uri: format!("/appbundles/{bundle_path}/ingestions/not-an-identifier"),
            body: None,
            reason: FIELD_VALIDATION_FAILED,
            field: Some("ingestionIdentifier"),
        },
        // `@pattern` on `Arn`, bound as a path label.
        Refusal {
            case: "a tag write naming a resource outside the modelled ARN pattern",
            method: "POST",
            uri: "/tags/not-an-arn".to_string(),
            body: Some(json!({ "tags": tag_list(&[("owner", "platform")]) })),
            reason: FIELD_VALIDATION_FAILED,
            field: Some("resourceArn"),
        },
        // `@pattern` on body members: the idempotency token (`UUID`) and the key
        // identifier (`Identifier`).
        Refusal {
            case: "a client token outside the modelled UUID pattern",
            method: "POST",
            uri: "/appbundles".to_string(),
            body: Some(json!({ "clientToken": "not-a-uuid" })),
            reason: FIELD_VALIDATION_FAILED,
            field: Some("clientToken"),
        },
        Refusal {
            case: "a customer managed key identifier outside the modelled pattern",
            method: "POST",
            uri: "/appbundles".to_string(),
            body: Some(json!({ "customerManagedKeyIdentifier": "neither-an-arn-nor-a-uuid" })),
            reason: FIELD_VALIDATION_FAILED,
            field: Some("customerManagedKeyIdentifier"),
        },
        // An absent `@required` member, which is found while READING the request
        // rather than while walking the value it built.
        Refusal {
            case: "an absent required body member",
            method: "POST",
            uri: "/useraccess/start".to_string(),
            body: Some(json!({ "email": "user@example.com" })),
            reason: FIELD_VALIDATION_FAILED,
            field: Some("appBundleIdentifier"),
        },
        // A value inside a LIST member: the fault is a member of a tag, not a member
        // of the request's own shape, so the entry names the position it sits at.
        Refusal {
            case: "a tag key past the published maximum length",
            method: "POST",
            uri: format!("/tags/{}", encode(bundle)),
            body: Some(json!({
                "tags": tag_list(&[(&"k".repeat(MAX_TAG_KEY + 1), "platform")])
            })),
            reason: FIELD_VALIDATION_FAILED,
            field: Some("tags"),
        },
        // Requests the codec cannot READ: a member carrying a value of the wrong JSON
        // type, and a body that is not the operation's input object at all.
        Refusal {
            case: "a body member of the wrong JSON type",
            method: "POST",
            uri: "/appbundles".to_string(),
            body: Some(json!({ "clientToken": 7 })),
            reason: CANNOT_PARSE,
            field: None,
        },
        Refusal {
            case: "a list member sent as a string",
            method: "POST",
            uri: "/appbundles".to_string(),
            body: Some(json!({ "tags": "owner=platform" })),
            reason: CANNOT_PARSE,
            field: None,
        },
        // The refusals this service's own HANDLERS raise, which the same client reads
        // out of the same members: a `nextToken` this listing never issued faults the
        // member, and a stored resource whose state forbids the call faults none.
        Refusal {
            case: "a nextToken this listing never issued",
            method: "GET",
            uri: "/appbundles?nextToken=not-a-token-this-service-issued".to_string(),
            body: None,
            reason: FIELD_VALIDATION_FAILED,
            field: Some("nextToken"),
        },
        Refusal {
            case: "deleting an ingestion the stored state forbids deleting",
            method: "DELETE",
            uri: format!("/appbundles/{bundle_path}/ingestions/{ingestion_path}"),
            body: None,
            reason: OTHER,
            field: None,
        },
    ]
}

/// RULE: every refusal this service answers with the modelled `ValidationException`
/// carries the members the model requires of that shape — `message` and `reason`,
/// `reason` drawn from the modelled `ValidationExceptionReason` set — and names the
/// member at fault in `fieldList` when the fault is a member of the request; so a
/// client reads any refused call of any operation out of the one modelled shape.
///
/// The table spans each constraint class the model declares and each place a value is
/// bound, a request the codec cannot read, and the refusals the handlers raise
/// themselves, over several operations.
#[tokio::test]
async fn every_validation_refusal_states_the_members_the_model_requires() {
    let emulator = Emulator::new();
    let (bundle, ingestion) =
        bundle_with_ingestion(&emulator, "JIRA", "tenant-refusal-shape", "set-up").await;
    // The ingestion is enabled so the last row's delete is refused by the state of
    // the STORED resource rather than by anything in the request.
    let started = start_ingestion(&emulator, &bundle, &ingestion).await;
    assert_eq!(
        started.status, 200,
        "the fixture ingestion must start, got {} with body {}",
        started.status, started.body
    );

    for refused in refusals(&bundle, &ingestion) {
        let answer = emulator
            .call(refused.method, &refused.uri, refused.body.clone())
            .await;
        assert_modelled_refusal(&refused, &answer);
    }
}

/// RULE: a request that violates no modelled constraint is served, so stating a
/// refusal in the modelled shape refuses nothing a caller is entitled to: each
/// published bound itself, the identifier forms the model allows and a well-formed
/// body all answer the operation's success status.
#[tokio::test]
async fn a_request_inside_every_modelled_constraint_is_still_served() {
    let emulator = Emulator::new();
    let bundle = arn_of(
        &emulator.create_app_bundle(json!({})).await.body,
        "appBundle",
    );

    // (case, method, uri, body, expected status)
    let served: Vec<(&str, &str, String, Option<Value>, u16)> = vec![
        (
            "maxResults at the published minimum",
            "GET",
            "/appbundles?maxResults=1".to_string(),
            None,
            200,
        ),
        (
            "maxResults at the published maximum",
            "GET",
            "/appbundles?maxResults=100".to_string(),
            None,
            200,
        ),
        (
            "an app bundle read by its ARN",
            "GET",
            format!("/appbundles/{}", encode(&bundle)),
            None,
            200,
        ),
        (
            // The other form the `Identifier` pattern allows, so the alternation is
            // served on both of its branches.
            "an app bundle read by its UUID",
            "GET",
            format!("/appbundles/{}", encode(&uuid_of(&bundle))),
            None,
            200,
        ),
        (
            "a tag write with a key of the maximum allowed length",
            "POST",
            format!("/tags/{}", encode(&bundle)),
            Some(json!({ "tags": tag_list(&[(&"k".repeat(MAX_TAG_KEY), "platform")]) })),
            200,
        ),
    ];

    for (case, method, uri, body, expected) in served {
        let answer = emulator.call(method, &uri, body).await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with body {}",
            answer.status, answer.body
        );
    }
}
