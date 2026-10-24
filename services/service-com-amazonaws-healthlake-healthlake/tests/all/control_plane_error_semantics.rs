// SPDX-License-Identifier: CC0-1.0

//! How this service states its control-plane refusals, and what a mutation on a
//! data store that is not `ACTIVE` answers.
//!
//! Three rules, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! 1. a data-store not-found answer is ONE fixed service text that quotes
//!    neither the requested `DatastoreId` nor the requested ARN;
//! 2. a refusal for a value outside an enumerated type's value set names the
//!    member and the permitted set and does NOT echo the rejected value, while
//!    every other constraint class keeps the AWS framing that quotes the value;
//! 3. `UpdateFHIRDatastore` applies its members to the stored record only while
//!    that record is `ACTIVE`; any other status is refused with
//!    `AccessDeniedException` and nothing is written. Its wire STATUS is 400, not the
//!    `@httpError(403)` the shape declares: see `tests/all/error_envelopes.rs` for
//!    that rule, which holds for every modelled client error of this service.
//!
//! Every call goes through [`MultiServiceRouter::dispatch`] by way of the shared
//! harness in `fhir_datastore_lifecycle`, so routing, deserialization, the
//! modelled constraints and the error envelope are all exercised.

use serde_json::{json, Value};

use crate::fhir_datastore_lifecycle::{error_code, HealthLake, PAST_TRANSITION};

/// The text this service answers a data store it holds no record of with, as
/// `service_com_amazonaws_healthlake_healthlake::state` spells it. Read from the
/// crate rather than retyped, so the test and the service cannot drift.
const NOT_FOUND: &str = service_com_amazonaws_healthlake_healthlake::state::DATASTORE_NOT_FOUND_MESSAGE;
/// The fixed sentence this service refuses a data transformation profile no record
/// holds with.
const PROFILE_NOT_FOUND: &str =
    service_com_amazonaws_healthlake_healthlake::state::PROFILE_NOT_FOUND_MESSAGE;

/// The text this service refuses a mutation on a data store that is not
/// `ACTIVE` with.
const NOT_ACTIVE: &str =
    service_com_amazonaws_healthlake_healthlake::state::DATASTORE_NOT_ACTIVE_MESSAGE;

/// The `message` of an error envelope, under either casing a protocol uses for
/// it (`Message` on the modelled shapes, `message` on a framework refusal).
fn error_message(payload: &Value) -> String {
    for member in ["Message", "message"] {
        if let Some(text) = payload.get(member).and_then(Value::as_str) {
            return text.to_string();
        }
    }
    panic!("an error body must carry a message, got {payload}")
}

/// The enum clause this service states for `path` and `allowed`: the member and
/// the permitted set, with no echo of the rejected value.
fn enum_envelope(path: &str, allowed: &[&str]) -> String {
    format!(
        "1 validation error detected: Value at '{path}' failed to satisfy constraint: \
         Member must satisfy enum value set: [{}]",
        allowed.join(", ")
    )
}

/// Assert that `payload` is this service's not-found answer for `request_value`:
/// `expected`, the fixed text of the resource that was addressed, and no echo of what
/// was asked for.
fn assert_not_found(
    case: &str,
    status: u16,
    payload: &Value,
    request_value: &str,
    expected: &str,
) {
    assert_eq!(
        status, 400,
        "case {case:?}: a resource no record holds must answer 400, got {status}: {payload}"
    );
    assert_eq!(
        error_code(payload),
        "ResourceNotFoundException",
        "case {case:?}: must answer ResourceNotFoundException, got {payload}"
    );
    let message = error_message(payload);
    assert_eq!(
        message, expected,
        "case {case:?}: the not-found text is fixed, expected {expected:?}, got {message:?}"
    );
    // The fixed sentence is short and shares letters with any input, so the
    // echo check only applies to a value long enough that its presence would be
    // unambiguous.
    if request_value.len() >= 8 {
        assert!(
            !message.contains(request_value),
            "case {case:?}: the not-found text must not quote {request_value:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn the_not_found_text_is_fixed_and_quotes_no_requested_identifier() {
    let service = HealthLake::new();
    // A real data store exists, so every 404 below is about the identifier and
    // not about an empty service.
    let (real, real_arn) = service
        .create(&json!({ "DatastoreTypeVersion": "R4", "DatastoreName": "real" }))
        .await;
    tokio::time::sleep(PAST_TRANSITION).await;

    // Rows span the forms an unknown `DatastoreId` can take inside the member's
    // modelled 1..32 length bound and `^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$` pattern:
    // the shortest possible id, hex ids of the minted width, a unicode id, a
    // punctuation id, the real id truncated, and the real id with one character
    // provably changed.
    let one_off = {
        let mut id = real.clone();
        let flipped = if id.ends_with('0') { '1' } else { '0' };
        id.pop();
        id.push(flipped);
        id
    };
    assert_ne!(
        one_off, real,
        "the one-character-off row must differ from the minted id"
    );
    let truncated = real[..31].to_string();
    let id_cases: Vec<String> = vec![
        "a".to_string(),
        "00000000000000000000000000000000".to_string(),
        "deadbeefdeadbeefdeadbeefdeadbeef".to_string(),
        "хранилище.данных".to_string(),
        "store:one/two=three+four%five@x".to_string(),
        truncated,
        one_off,
    ];

    // Every operation that resolves a data store BY ID states the same refusal.
    for id in &id_cases {
        for operation in [
            "DescribeFHIRDatastore",
            "DeleteFHIRDatastore",
            "UpdateFHIRDatastore",
        ] {
            let (status, payload) = service
                .call(operation, &json!({ "DatastoreId": id }))
                .await;
            assert_not_found(
                &format!("{operation} with id {id:?}"),
                status,
                &payload,
                id,
                NOT_FOUND,
            );
        }
    }

    // And every operation that resolves one BY ARN. Rows span the ARN forms the
    // modelled `AmazonResourceName` pattern admits, each paired with the fixed
    // sentence of the resource its form names: a data transformation profile no
    // profile was minted under, an unknown data store, and the real data store in
    // another region, in another account, and in another partition.
    let parts: Vec<&str> = real_arn.split(':').collect();
    assert_eq!(parts.len(), 6, "a data store ARN has six fields: {real_arn}");
    let arn_cases: Vec<(String, &str)> = vec![
        (
            format!(
                "arn:aws:healthlake:us-east-1:000000000000:dataTransformationProfile/{}",
                "deadbeefdeadbeefdeadbeefdeadbeef"
            ),
            PROFILE_NOT_FOUND,
        ),
        (
            "arn:aws:healthlake:us-east-1:000000000000:datastore/fhir/00000000000000000000000000000000"
                .to_string(),
            NOT_FOUND,
        ),
        (
            format!(
                "arn:aws:healthlake:eu-west-1:{}:datastore/fhir/{real}",
                parts[4]
            ),
            NOT_FOUND,
        ),
        (
            format!("arn:aws:healthlake:{}:210987654321:datastore/fhir/{real}", parts[3]),
            NOT_FOUND,
        ),
        (
            format!(
                "arn:aws-us-gov:healthlake:{}:{}:datastore/fhir/{real}",
                parts[3], parts[4]
            ),
            NOT_FOUND,
        ),
    ];
    for (arn, expected) in &arn_cases {
        for operation in ["ListTagsForResource", "TagResource"] {
            let mut request = json!({ "ResourceARN": arn });
            if operation == "TagResource" {
                request["Tags"] = json!([{ "Key": "env", "Value": "test" }]);
            }
            let (status, payload) = service.call(operation, &request).await;
            assert_not_found(
                &format!("{operation} with arn {arn:?}"),
                status,
                &payload,
                arn,
                expected,
            );
        }
    }

    // The real resource still resolves by both keys, so the refusals above are
    // about identity and not about a broken lookup.
    let described = service
        .ok("DescribeFHIRDatastore", &json!({ "DatastoreId": &real }))
        .await;
    assert_eq!(
        described["DatastoreProperties"]["DatastoreName"], "real",
        "the existing data store must still describe, got {described}"
    );
    service
        .ok("ListTagsForResource", &json!({ "ResourceARN": &real_arn }))
        .await;
}

#[tokio::test]
async fn an_enum_refusal_names_the_member_and_the_set_but_not_the_rejected_value() {
    let service = HealthLake::new();

    // `FHIRVersion` admits exactly `R4` (smithy_query(query="shape",
    // shape="FHIRVersion") -> values ["R4"]). The rows differ in the FORM of the
    // rejected name — another release, wrong case, a longer name, a numeric one,
    // whitespace, empty, unicode — because the rule generalises over the value.
    let expected = enum_envelope("datastoreTypeVersion", &["R4"]);
    for case in ["R5", "r4", "DSTU2", "4", " R4", "", "Ⓡ4"] {
        let (status, payload) = service
            .call(
                "CreateFHIRDatastore",
                &json!({ "DatastoreTypeVersion": case, "DatastoreName": "rejected" }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a value outside the enum must answer 400, got {status}: {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: must answer ValidationException, got {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains(&format!("'{case}'")),
            "case {case:?}: the enum clause must not quote the rejected value, got {message:?}"
        );
    }

    // The same rule on an enum member the recorded case never touched:
    // `DatastoreFilter.DatastoreStatus` targets `DatastoreStatus`, whose value
    // set is smithy_query(query="shape", shape="DatastoreStatus") -> ["CREATING",
    // "ACTIVE", "DELETING", "DELETED", "CREATE_FAILED", "UPDATING",
    // "UPDATE_FAILED"].
    let statuses = [
        "CREATING",
        "ACTIVE",
        "DELETING",
        "DELETED",
        "CREATE_FAILED",
        "UPDATING",
        "UPDATE_FAILED",
    ];
    for case in ["NOT_A_STATUS", "active", "", "DELETED "] {
        let (status, payload) = service
            .call(
                "ListFHIRDatastores",
                &json!({ "Filter": { "DatastoreStatus": case } }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a status outside the enum must answer 400, got {status}: {payload}"
        );
        let message = error_message(&payload);
        let clause = format!(
            "Member must satisfy enum value set: [{}]",
            statuses.join(", ")
        );
        assert!(
            message.starts_with("1 validation error detected: Value at '")
                && message.contains("datastoreStatus")
                && message.ends_with(&clause),
            "case {case:?}: expected the member-and-set clause {clause:?} with no value, got {message:?}"
        );
        assert!(
            !message.contains(&format!("'{case}'")),
            "case {case:?}: the enum clause must not quote the rejected value, got {message:?}"
        );
    }

    // A positive control, and the boundary of the rule: `R4` is accepted, and a
    // constraint class that is NOT an enum keeps the AWS framing that quotes the
    // (length-capped) value — here `DatastoreId`'s modelled 1..32 maximum.
    let (id, _) = service
        .create(&json!({ "DatastoreTypeVersion": "R4", "DatastoreName": "accepted" }))
        .await;
    let too_long = "f".repeat(40);
    let (status, payload) = service
        .call("DescribeFHIRDatastore", &json!({ "DatastoreId": &too_long }))
        .await;
    assert_eq!(
        status, 400,
        "an over-long DatastoreId must answer 400, got {status}: {payload}"
    );
    let message = error_message(&payload);
    assert_eq!(
        message,
        format!(
            "1 validation error detected: Value '{too_long}' at 'datastoreId' failed to satisfy \
             constraint: Member must have length less than or equal to 32"
        ),
        "a length violation keeps the framing that quotes the value, got {message:?}"
    );

    // None of the refused creates minted anything: the accepted one is the only
    // data store the service holds.
    let listed = service.ok("ListFHIRDatastores", &json!({})).await;
    let properties = listed["DatastorePropertiesList"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        properties.len(),
        1,
        "only the accepted create may have minted a data store, got {listed}"
    );
    assert_eq!(
        properties[0]["DatastoreId"], id,
        "the one data store must be the accepted one, got {listed}"
    );
}

/// What one `UpdateFHIRDatastore` carries, and the members it is expected to
/// leave alone. The rule generalises over WHICH members a request supplies, so
/// the rows differ along that and along the form of the values.
fn update_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "rename, ascii",
            json!({ "DatastoreName": "renamed-store" }),
        ),
        (
            "rename, unicode with spaces",
            json!({ "DatastoreName": "клиника записи 7" }),
        ),
        (
            "rename at the modelled 256-character maximum",
            json!({ "DatastoreName": "n".repeat(256) }),
        ),
        (
            "analytics and NLP configuration, no rename",
            json!({
                "AnalyticsConfiguration": { "Status": "ENABLED" },
                "NlpConfiguration": { "Status": "DISABLED" },
            }),
        ),
        (
            "identity provider configuration and a rename together",
            json!({
                "DatastoreName": "smart.on:fhir/store",
                "IdentityProviderConfiguration": {
                    "AuthorizationStrategy": "SMART_ON_FHIR",
                    "FineGrainedAuthorizationEnabled": true,
                    "Metadata": "{\"issuer\":\"https://example.test\"}",
                },
            }),
        ),
        (
            "no optional member at all: nothing changes",
            json!({}),
        ),
    ]
}

#[tokio::test]
async fn update_applies_its_members_to_the_active_record_and_leaves_the_rest() {
    let service = HealthLake::new();
    let created = json!({
        "DatastoreTypeVersion": "R4",
        "DatastoreName": "original",
        "SseConfiguration": {
            "KmsEncryptionConfig": { "CmkType": "AWS_OWNED_KMS_KEY" },
        },
        "Tags": [{ "Key": "env", "Value": "test" }],
    });
    let (id, arn) = service.create(&created).await;
    tokio::time::sleep(PAST_TRANSITION).await;

    // The mutable members, as the running expectation of what the record holds.
    let mut expected = json!({ "DatastoreName": "original" });
    for (case, members) in update_cases() {
        let mut request = json!({ "DatastoreId": &id });
        for (member, value) in members.as_object().expect("a row is an object") {
            request[member] = value.clone();
            expected[member] = value.clone();
        }

        let updated = service.ok("UpdateFHIRDatastore", &request).await;
        let properties = &updated["DatastoreProperties"];
        for (member, value) in expected.as_object().expect("the expectation is an object") {
            assert_eq!(
                &properties[member], value,
                "case {case:?}: Update must report {member} as {value}, got {properties}"
            );
        }
        // What the update did not carry is untouched — including the members
        // only `CreateFHIRDatastore` can set.
        assert_eq!(
            properties["DatastoreStatus"], "ACTIVE",
            "case {case:?}: an applied update leaves the data store ACTIVE, got {properties}"
        );
        assert_eq!(
            properties["DatastoreId"], id,
            "case {case:?}: the identity of the record must not change, got {properties}"
        );
        assert_eq!(
            properties["DatastoreArn"], arn,
            "case {case:?}: the ARN of the record must not change, got {properties}"
        );
        assert_eq!(
            properties["DatastoreTypeVersion"], "R4",
            "case {case:?}: Update cannot change the FHIR version, got {properties}"
        );
        assert_eq!(
            properties["SseConfiguration"], created["SseConfiguration"],
            "case {case:?}: the create-time SseConfiguration must survive, got {properties}"
        );

        // The write is in the STORE, not just in the answer: the reader reports
        // exactly what the update returned.
        let described = service
            .ok("DescribeFHIRDatastore", &json!({ "DatastoreId": &id }))
            .await;
        assert_eq!(
            described["DatastoreProperties"], *properties,
            "case {case:?}: Describe must report what Update wrote, got {described}"
        );
        // And an update is not a tag operation.
        let listed = service
            .ok("ListTagsForResource", &json!({ "ResourceARN": &arn }))
            .await;
        assert_eq!(
            listed["Tags"],
            created["Tags"],
            "case {case:?}: the tags must survive an update, got {listed}"
        );
    }
}

#[tokio::test]
async fn update_refuses_a_data_store_that_is_not_active_with_403_and_writes_nothing() {
    let service = HealthLake::new();

    // One row per non-ACTIVE status a data store reaches in this slice, each
    // reached through the real lifecycle rather than by poking the state.
    for (case, settle_before_delete, settle_after_delete) in [
        ("CREATING", false, false),
        ("DELETING", true, false),
        ("DELETED", true, true),
    ] {
        let (id, _) = service
            .create(&json!({ "DatastoreTypeVersion": "R4", "DatastoreName": "gated" }))
            .await;
        if settle_before_delete {
            tokio::time::sleep(PAST_TRANSITION).await;
            service
                .ok("DeleteFHIRDatastore", &json!({ "DatastoreId": &id }))
                .await;
        }
        if settle_after_delete {
            tokio::time::sleep(PAST_TRANSITION).await;
        }

        let described = service
            .ok("DescribeFHIRDatastore", &json!({ "DatastoreId": &id }))
            .await;
        assert_eq!(
            described["DatastoreProperties"]["DatastoreStatus"], case,
            "case {case:?}: the row must put the data store in that status, got {described}"
        );

        let (status, payload) = service
            .call(
                "UpdateFHIRDatastore",
                &json!({ "DatastoreId": &id, "DatastoreName": "renamed-after-the-fact" }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a mutation on a non-ACTIVE data store must answer 400, got {status}: {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "AccessDeniedException",
            "case {case:?}: must answer AccessDeniedException, got {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, NOT_ACTIVE,
            "case {case:?}: expected {NOT_ACTIVE:?}, got {message:?}"
        );

        // The refusal wrote nothing: the name is still the one the create set,
        // and the lifecycle the record was already on is undisturbed — the
        // transition the clock owed still lands, and a terminal record stays
        // terminal.
        tokio::time::sleep(PAST_TRANSITION).await;
        let after = service
            .ok("DescribeFHIRDatastore", &json!({ "DatastoreId": &id }))
            .await;
        assert_eq!(
            after["DatastoreProperties"]["DatastoreName"], "gated",
            "case {case:?}: a refused update must not rename the data store, got {after}"
        );
        let settled = if case == "CREATING" { "ACTIVE" } else { "DELETED" };
        assert_eq!(
            after["DatastoreProperties"]["DatastoreStatus"], settled,
            "case {case:?}: a refused update must leave the lifecycle alone, expected {settled}, got {after}"
        );
    }
}
