// SPDX-License-Identifier: CC0-1.0

//! How this service puts an ERROR on the wire, over the real awsJson1_0 envelope.
//!
//! Three rules, each asserted over a SET of inputs rather than one recorded request:
//!
//! 1. **Status is the fault, not `@httpError`.** `@httpError` is an HTTP-BINDING trait
//!    and the awsJson family binds nothing to the transport
//!    (`smithy_query(query="service_info")` -> `protocols: ["awsJson1_0"]`), so a
//!    modelled error is served with the status of its `@error` kind: a client fault is
//!    HTTP 400 whatever 4xx its shape declares, and a server fault keeps its 5xx.
//! 2. **A refusal carries the MODELLED `Message` member.** A request that breaks a rule
//!    the model declares is refused with the service's own `ValidationException` shape
//!    — `smithy_query(query="shape",shape="ValidationException")`: `errorKind client`,
//!    `httpError 400`, one member `Message` (target `HealthLakeString`, length
//!    0..10000) — whose `Message` names the member that failed and the constraint it
//!    failed, under that exact key, and carries the SAME sentence under the
//!    framework's lowercase `message` beside it, so no client path reads an empty
//!    refusal and the two can never state different reasons.
//! 3. **A not-found refusal names what it could not find.** Every
//!    `ResourceNotFoundException` the data store surface raises carries
//!    [`DATASTORE_NOT_FOUND_MESSAGE`] and nothing built from the request, however the
//!    request addressed the data store (`DatastoreId` or `ResourceARN`); a profile's
//!    carries the sentence that names the profile.
//!
//! Every call below goes through [`MultiServiceRouter::dispatch`][d] by way of the
//! shared harness in `fhir_datastore_lifecycle` — the same entry point `core/router`
//! serves — so routing, deserialization, the modelled constraints and the error envelope
//! are all exercised, not bypassed.
//!
//! [d]: smithy_server_runtime::MultiServiceRouter::dispatch

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::scaffolded::wire_http_status;
use service_com_amazonaws_healthlake_healthlake::state::{
    profile_not_found_message, DATASTORE_NOT_FOUND_MESSAGE,
};

use crate::fhir_datastore_lifecycle::{error_code, HealthLake, PAST_TRANSITION};

/// This service's Smithy model, as it ships beside the crate under test: the SOURCE of
/// the `@httpError` catalog the status table below is built from, so no status is
/// hand-copied here.
const MODEL_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-healthlake-healthlake.json"
);

/// The create request every row below starts from: the one `@required` member, at the
/// only value the modelled `FHIRVersion` enum admits.
fn a_data_store() -> Value {
    json!({ "DatastoreTypeVersion": "R4" })
}

/// The modelled `Message` member of an error body, under that exact key.
///
/// `None` when the body carries no such member — which is the defect this suite pins:
/// a client deserialising the modelled shape must find the sentence here.
fn modelled_message(payload: &Value) -> Option<&str> {
    payload.get("Message").and_then(Value::as_str)
}

/// Every error shape this service's own namespace declares, as
/// `(shape name, @httpError, errorKind)`.
///
/// Read out of the shipped model at test time, so the catalog's source is the model
/// itself rather than a list written here that could drift from it. Restricted to
/// `com.amazonaws.healthlake#` — the namespace the service shape lives in
/// (`smithy_query(query="list_operations")`) — because the shipped model also carries
/// shapes from `com.amazon.awshealthlakedatatransformationfrontendservice`, which this
/// service does not bind.
fn modelled_error_shapes() -> Vec<(String, u16, String)> {
    let model = std::fs::read_to_string(MODEL_PATH).expect("this crate ships its Smithy model");
    let model: serde_json::Value = serde_json::from_str(&model).expect("the Smithy model is JSON");
    let shapes = model
        .get("shapes")
        .and_then(Value::as_object)
        .expect("a Smithy model has shapes")
        .clone();
    let mut found: Vec<(String, u16, String)> = shapes
        .iter()
        .filter(|(id, _)| id.starts_with("com.amazonaws.healthlake#"))
        .filter_map(|(id, shape)| {
            let traits = shape.get("traits")?;
            let kind = traits.get("smithy.api#error")?.as_str()?.to_string();
            let status = traits
                .get("smithy.api#httpError")
                .and_then(Value::as_u64)
                // Smithy defaults a declared error with no `@httpError` to 400 for a
                // client fault and 500 for a server one.
                .unwrap_or(if kind == "server" { 500 } else { 400 }) as u16;
            Some((
                id.rsplit('#').next().unwrap_or_default().to_string(),
                status,
                kind,
            ))
        })
        .collect();
    found.sort();
    found
}

/// RULE 1, over the WHOLE catalog: a modelled client fault goes on the wire as HTTP
/// 400 whatever 4xx its `@httpError` names; a server fault keeps its 5xx.
///
/// The rows come from the model (every error shape it declares), and the two extra rows
/// pin the classification edge either side of the 4xx/5xx boundary — a status the model
/// happens not to annotate today must still classify by its class.
#[test]
fn a_modelled_errors_wire_status_is_its_fault_not_its_http_error() {
    let catalog = modelled_error_shapes();
    assert!(
        catalog.len() >= 6,
        "the model must declare the error shapes this rule is about, got {catalog:?}"
    );
    // At least one shape the issue did not cite must be in the catalog, so the rule is
    // asserted beyond the recorded cases.
    assert!(
        catalog
            .iter()
            .any(|(shape, _, _)| shape == "AccessDeniedException"),
        "the catalog must cover shapes beyond the cited ones, got {catalog:?}"
    );

    for (shape, modelled, kind) in &catalog {
        let expected = if kind == "server" { *modelled } else { 400 };
        let actual = wire_http_status(*modelled);
        assert_eq!(
            actual, expected,
            "case {shape:?} (@error {kind}, @httpError {modelled}): expected wire status \
             {expected}, got {actual}"
        );
    }

    // Each published bound of the client/server split, and one past it.
    for (case, modelled, expected) in [
        ("the last client status", 499_u16, 400_u16),
        ("the first server status", 500, 500),
        ("one past the first server status", 501, 501),
    ] {
        let actual = wire_http_status(modelled);
        assert_eq!(
            actual, expected,
            "case {case:?} (@httpError {modelled}): expected wire status {expected}, got {actual}"
        );
    }
}

/// RULE 1 end to end: every modelled error a request can actually reach is answered
/// with 400 and its own `__type`, whatever 4xx its shape declares.
#[tokio::test]
async fn a_reachable_client_error_is_answered_with_400_and_its_own_type() {
    let service = HealthLake::new();
    let (creating, arn) = service.create(&a_data_store()).await;

    // (case, operation, request, expected __type, the shape's own modelled @httpError)
    let rows: Vec<(&str, &str, Value, &str, u16)> = vec![
        (
            "a constraint the model declares",
            "DescribeFHIRDatastore",
            json!({ "DatastoreId": "" }),
            "ValidationException",
            400,
        ),
        (
            "a data store id nothing was minted under",
            "DescribeFHIRDatastore",
            json!({ "DatastoreId": "00000000000000000000000000000000" }),
            "ResourceNotFoundException",
            404,
        ),
        (
            "an ARN naming no data store",
            "ListTagsForResource",
            json!({
                "ResourceARN":
                    "arn:aws:healthlake:us-east-1:000000000000:datastore/fhir/\
                     00000000000000000000000000000000"
            }),
            "ResourceNotFoundException",
            404,
        ),
        (
            "a data store still in transition",
            "DeleteFHIRDatastore",
            json!({ "DatastoreId": &creating }),
            "ConflictException",
            409,
        ),
        (
            "a merge past the resource's tag ceiling",
            "TagResource",
            json!({
                "ResourceARN": &arn,
                "Tags": (0..=service_com_amazonaws_healthlake_healthlake::state::MAX_TAGS_PER_DATASTORE)
                    .map(|index| json!({ "Key": format!("key{index}"), "Value": "v" }))
                    .collect::<Vec<Value>>(),
            }),
            "ValidationException",
            400,
        ),
    ];

    for (case, operation, request, expected_type, modelled) in rows {
        let (status, payload) = service.call(operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?} on {operation} (the shape's @httpError is {modelled}): expected \
             HTTP 400, got {status}: {payload}"
        );
        assert_eq!(
            error_code(&payload),
            expected_type,
            "case {case:?} on {operation}: expected __type {expected_type}, got {payload}"
        );
        let message = modelled_message(&payload);
        assert!(
            message.is_some_and(|text| !text.is_empty()),
            "case {case:?} on {operation}: expected a populated modelled Message member, \
             got {payload}"
        );
    }
}

/// One row of the constraint table below: a request that breaks exactly ONE rule the
/// model declares, and what the refusal's modelled `Message` must say about it.
struct ConstraintCase {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The operation the request names.
    operation: &'static str,
    /// The request body, satisfying every modelled rule but one.
    request: Value,
    /// The violation path the `Message` must name — the lowerCamel spelling of the
    /// Smithy member name, as the framework's own violation paths are built.
    member_path: &'static str,
    /// The requirement clause the `Message` must state.
    requirement: &'static str,
    /// A prefix of the rejected value the clause must echo, or `None` where the
    /// constraint class echoes no value at all.
    echoed: Option<String>,
}

/// RULE 2: a request that breaks a rule the model declares is refused with the
/// MODELLED `ValidationException`, whose `Message` names the member that failed and
/// the constraint it failed.
///
/// The rows differ along the CONSTRAINT CLASS the model can declare — an enum value
/// set, a `@pattern`, each end of a `@length`, a `@required` member absent, and a
/// constraint on an element nested inside a list — across several operations and
/// members, so the rule is not keyed to the recorded request's shape. Each expected
/// member path is the lowerCamel spelling of the Smithy member name, as the framework's
/// own violation paths are built.
///
/// The last column is the other half of the rule: the clause ECHOES the rejected value
/// for every class where the value is the diagnostic, and does NOT for an enum, where
/// the permitted set is, nor for a `@pattern`, where the pattern is.
#[tokio::test]
async fn a_constraint_refusal_names_the_member_and_the_constraint_in_the_modelled_message() {
    let service = HealthLake::new();

    // A `DatastoreName` one character past the modelled `@length` max of 256, and a
    // `DatastoreId` one past its max of 32.
    let over_long_name = "n".repeat(257);
    let over_long_id = "0".repeat(33);
    // A tag key one character past `TagKey`'s modelled `@length` max of 128.
    let over_long_tag_key = "k".repeat(129);
    // A prefix of the over-long name, short enough to survive the framework's 256-character
    // elision of an echoed value.
    let elided_name_prefix = "n".repeat(200);

    // Each row breaks exactly one rule the model declares; `echoed` is a PREFIX of the
    // rejected value, because the framework elides an echoed value at 256 characters so
    // a request cannot amplify itself into an arbitrarily long response.
    let rows: Vec<ConstraintCase> = vec![
        ConstraintCase {
            case: "an enum value outside the declared set",
            operation: "CreateFHIRDatastore",
            request: json!({ "DatastoreTypeVersion": "R5" }),
            member_path: "datastoreTypeVersion",
            requirement: "Member must satisfy enum value set: [R4]",
            echoed: None,
        },
        ConstraintCase {
            case: "a required member left out",
            operation: "CreateFHIRDatastore",
            request: json!({}),
            member_path: "datastoreTypeVersion",
            requirement: "Member must not be null",
            echoed: None,
        },
        ConstraintCase {
            case: "an ARN shorter than the declared minimum",
            operation: "TagResource",
            request: json!({ "ResourceARN": "", "Tags": [] }),
            member_path: "resourceARN",
            requirement: "Member must have length greater than or equal to 1",
            echoed: Some(String::new()),
        },
        ConstraintCase {
            case: "a data store id longer than the declared maximum",
            operation: "DeleteFHIRDatastore",
            request: json!({ "DatastoreId": &over_long_id }),
            member_path: "datastoreId",
            requirement: "Member must have length less than or equal to 32",
            echoed: Some(over_long_id.clone()),
        },
        ConstraintCase {
            case: "a name longer than the declared maximum",
            operation: "CreateFHIRDatastore",
            request: json!({ "DatastoreTypeVersion": "R4", "DatastoreName": &over_long_name }),
            member_path: "datastoreName",
            requirement: "Member must have length less than or equal to 256",
            echoed: Some(elided_name_prefix),
        },
        ConstraintCase {
            case: "a client token outside its declared pattern",
            operation: "CreateFHIRDatastore",
            request: json!({ "DatastoreTypeVersion": "R4", "ClientToken": "not a token" }),
            member_path: "clientToken",
            requirement: "Member must satisfy regular expression pattern",
            echoed: None,
        },
        ConstraintCase {
            case: "a constraint on an element nested inside a list",
            operation: "CreateFHIRDatastore",
            request: json!({
                "DatastoreTypeVersion": "R4",
                "Tags": [{ "Key": &over_long_tag_key, "Value": "v" }],
            }),
            member_path: "tags.1.member.key",
            requirement: "Member must have length less than or equal to 128",
            echoed: Some(over_long_tag_key.clone()),
        },
        ConstraintCase {
            case: "a required member of a nested element left out",
            operation: "TagResource",
            request: json!({
                "ResourceARN":
                    "arn:aws:healthlake:us-east-1:000000000000:datastore/fhir/\
                     00000000000000000000000000000000",
                "Tags": [{ "Key": "env" }],
            }),
            member_path: "tags.1.member.value",
            requirement: "Member must not be null",
            echoed: None,
        },
    ];

    for ConstraintCase {
        case,
        operation,
        request,
        member_path,
        requirement,
        echoed,
    } in rows
    {
        let (status, payload) = service.call(operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?} on {operation}: expected HTTP 400, got {status}: {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?} on {operation}: expected __type ValidationException, got {payload}"
        );
        let Some(message) = modelled_message(&payload) else {
            panic!(
                "case {case:?} on {operation}: expected the modelled Message member, got {payload}"
            );
        };
        assert!(
            message.contains(&format!("at '{member_path}'")),
            "case {case:?} on {operation}: Message must name the member {member_path:?}, \
             got {message:?}"
        );
        assert!(
            message.contains(requirement),
            "case {case:?} on {operation}: Message must state {requirement:?}, got {message:?}"
        );
        assert!(
            message.contains("validation error"),
            "case {case:?} on {operation}: Message must carry the validation envelope, \
             got {message:?}"
        );
        match &echoed {
            Some(value) => assert!(
                message.contains(&format!("Value '{value}")),
                "case {case:?} on {operation}: Message must echo the rejected value \
                 (prefix {value:?}), got {message:?}"
            ),
            None => assert!(
                !message.contains("Value '"),
                "case {case:?} on {operation}: Message must echo no rejected value, \
                 got {message:?}"
            ),
        }
        // The modelled shape spells the member `Message`; the framework's envelope
        // spells it lowercase. Both reach the wire carrying the SAME sentence, so a
        // client reading either one reads the same reason.
        assert_eq!(
            payload.get("message").and_then(Value::as_str),
            Some(message),
            "case {case:?} on {operation}: both message members must carry one sentence, \
             got {payload}"
        );
    }
}

/// RULE 3: every not-found refusal of the data store surface states the service's one
/// fixed sentence and carries nothing built from the request; a profile's names the
/// profile it could not find.
///
/// The rows span how a request can address a data store the service does not hold:
/// by `DatastoreId` (an id nothing was minted under, a one-character id, an id one
/// character off a live one) and by `ResourceARN` (an unknown id, another region,
/// another account, another partition, and a `dataTransformationProfile` ARN — a
/// different resource this slice never holds), across every operation that resolves a
/// data store. A data store that existed and has since reached its terminal status is
/// asserted separately, below: it is still READABLE for its retention, so only the
/// operation that acts on it refuses.
#[tokio::test]
async fn a_not_found_refusal_states_one_fixed_sentence_that_quotes_nothing_from_the_request() {
    let service = HealthLake::new();
    let (live, live_arn) = service.create(&a_data_store()).await;

    let unknown_id = "00000000000000000000000000000000".to_string();
    let id_rows: Vec<(&str, String)> = vec![
        ("an id nothing was minted under", unknown_id.clone()),
        ("a single-character id", "a".to_string()),
        ("an id one character off a live one", {
            let mut id = live.clone();
            let flipped = if id.ends_with('0') { '1' } else { '0' };
            id.pop();
            id.push(flipped);
            id
        }),
    ];
    for (case, id) in &id_rows {
        for operation in ["DescribeFHIRDatastore", "DeleteFHIRDatastore"] {
            let (status, payload) = service
                .call(operation, &json!({ "DatastoreId": id }))
                .await;
            assert_not_found(
                case,
                operation,
                status,
                &payload,
                id,
                DATASTORE_NOT_FOUND_MESSAGE,
            );
        }
    }

    // Each row carries the fixed sentence of the RESOURCE its ARN names: the
    // modelled `AmazonResourceName` admits a data store ARN and a data
    // transformation profile ARN, and this slice holds neither of the records
    // these rows address.
    let arn_rows: Vec<(&str, String, String)> = vec![
        (
            "an ARN whose id nothing was minted under",
            format!(
                "arn:aws:healthlake:us-east-1:000000000000:datastore/fhir/{unknown_id}"
            ),
            DATASTORE_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "an ARN of a live id with its last character changed",
            {
                let mut arn = live_arn.clone();
                let flipped = if arn.ends_with('0') { '1' } else { '0' };
                arn.pop();
                arn.push(flipped);
                arn
            },
            DATASTORE_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "a dataTransformationProfile ARN no profile was minted under",
            "arn:aws:healthlake:us-east-1:000000000000:dataTransformationProfile/\
             0123456789abcdef0123456789abcdef"
                .to_string(),
            profile_not_found_message("0123456789abcdef0123456789abcdef"),
        ),
        (
            "a live id in another region",
            format!("arn:aws:healthlake:eu-west-1:000000000000:datastore/fhir/{live}"),
            DATASTORE_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "a live id in another account",
            format!("arn:aws:healthlake:us-east-1:210987654321:datastore/fhir/{live}"),
            DATASTORE_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "a live id in another partition",
            format!("arn:aws-cn:healthlake:us-east-1:000000000000:datastore/fhir/{live}"),
            DATASTORE_NOT_FOUND_MESSAGE.to_string(),
        ),
    ];
    for (case, arn, expected_message) in &arn_rows {
        for (operation, request) in [
            ("ListTagsForResource", json!({ "ResourceARN": arn })),
            (
                "TagResource",
                json!({ "ResourceARN": arn, "Tags": [{ "Key": "env", "Value": "test" }] }),
            ),
        ] {
            let (status, payload) = service.call(operation, &request).await;
            assert_not_found(case, operation, status, &payload, arn, expected_message);
        }
    }

    // The live data store still resolves through both addressing forms, so the refusal
    // is about the resource and not about the surface.
    let described = service
        .ok("DescribeFHIRDatastore", &json!({ "DatastoreId": &live }))
        .await;
    assert_eq!(
        described["DatastoreProperties"]["DatastoreId"], live,
        "the live data store must still describe, got {described}"
    );
    let listed = service
        .ok("ListTagsForResource", &json!({ "ResourceARN": &live_arn }))
        .await;
    assert!(
        listed["Tags"].is_array(),
        "the live data store must still report its tags, got {listed}"
    );

    // A data store that existed and has reached its terminal status is still READABLE
    // for its retention, so Describe answers it; the operation that would ACT on it
    // refuses with the same one fixed sentence as an id nothing was ever minted under.
    let (gone, _) = service.create(&a_data_store()).await;
    tokio::time::sleep(PAST_TRANSITION).await;
    service
        .ok("DeleteFHIRDatastore", &json!({ "DatastoreId": &gone }))
        .await;
    tokio::time::sleep(PAST_TRANSITION).await;
    let (status, payload) = service
        .call("DeleteFHIRDatastore", &json!({ "DatastoreId": &gone }))
        .await;
    assert_not_found(
        "a data store already at its terminal status",
        "DeleteFHIRDatastore",
        status,
        &payload,
        &gone,
        DATASTORE_NOT_FOUND_MESSAGE,
    );
}

/// One not-found refusal: HTTP 400, `ResourceNotFoundException`, the addressed
/// resource's fixed sentence in the modelled `Message`, and no trace of the identifier
/// the caller sent.
fn assert_not_found(
    case: &str,
    operation: &str,
    status: u16,
    payload: &Value,
    addressed: &str,
    expected_message: &str,
) {
    assert_eq!(
        status, 400,
        "case {case:?} on {operation}: expected HTTP 400, got {status}: {payload}"
    );
    assert_eq!(
        error_code(payload),
        "ResourceNotFoundException",
        "case {case:?} on {operation}: expected __type ResourceNotFoundException, got {payload}"
    );
    let actual = modelled_message(payload);
    assert_eq!(
        actual,
        Some(expected_message),
        "case {case:?} on {operation}: expected Message {expected_message:?}, \
         got {actual:?}"
    );
    // Nothing from the request may appear in the body beyond what the resource's own
    // sentence names — the data store texts name nothing, a profile's names the
    // profile. Checked for an identifier long enough to be distinctive: a one- or
    // two-character id can occur inside the fixed sentence by coincidence, which would
    // be a false failure rather than a leak.
    if addressed.len() >= 8 && !expected_message.contains(addressed) {
        assert!(
            !payload.to_string().contains(addressed),
            "case {case:?} on {operation}: the refusal must quote nothing from the request, \
             but the body carries {addressed:?}: {payload}"
        );
    }
}
