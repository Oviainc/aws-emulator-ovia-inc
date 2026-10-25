// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the bounds `CreateComputationModel` enforces before it
//! writes, driven over HTTP through the same `MultiServiceRouter` the emulator
//! boots with, so every assertion is about the wire behaviour a real client sees.
//!
//! `computation_model_lifecycle.rs` pins what an ACCEPTED create persists and how
//! it reads back; this suite pins the refusals that stand between a request and
//! that write, and that each of them leaves the store exactly as it was:
//!
//! * `computationModelName` and `computationModelDescription` are accepted
//!   exactly over the length range and character set their own shapes model
//!   (smithy_query query="operation" operation="CreateComputationModel":
//!   `RestrictedName`, len 1..256, pattern `^[a-zA-Z0-9 _\-#$*!@.]+$`;
//!   `RestrictedDescription`, len 1..2048, pattern `^[a-zA-Z0-9 _\-#$*!@]+$`,
//!   which admits no `.`), and a value outside either is refused with the
//!   client-input error this operation models (`InvalidRequestException`, HTTP
//!   400) and no computation model stored;
//! * a `tags` key must satisfy the length bound the model puts on `TagKey`
//!   (smithy_query query="shape" shape="TagMap": key target `TagKey`,
//!   `@length(min: 1, max: 128)`) — the one bound the framework's constraint walk
//!   does not reach, because it validates a map's values only — so a key inside
//!   the bound is persisted on the model and a key outside it is refused with the
//!   client-input error this operation models (`InvalidRequestException`, HTTP
//!   400) and nothing stored;
//! * the computation-model store's entry count is bounded, so a create past that
//!   many models is the quota refusal this operation models
//!   (`LimitExceededException`, HTTP 410) and mints nothing.
//!
//! The route, body and ARN helpers are `computation_model_lifecycle.rs`'s and the
//! tag reader is `resource_tags.rs`'s, imported rather than copied so the suites
//! cannot drift on the wire form; the HTTP harness (`router`,
//! `router_with_engine`, `error_code`) is the one the asset suite already has.
//!
//! COST NOTE: the quota suite below drives [`MAX_COMPUTATION_MODELS`] sequential
//! creates to reach the ceiling, so raising that constant changes what this suite
//! costs.

use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, MAX_COMPUTATION_MODELS, TAG_KEY_MAX_LENGTH, TAG_KEY_MIN_LENGTH,
};

use crate::asset_lifecycle::{error_code, router, router_with_engine};
use crate::computation_model_lifecycle::{
    anomaly_detection, asset_model_property, create, describe, expected_arn, id_of,
};
use crate::resource_tags::list_tags;

/// The published bounds of `computationModelName`'s `RestrictedName` and of
/// `computationModelDescription`'s `RestrictedDescription`, from
/// smithy_query query="operation" operation="CreateComputationModel".
const NAME_MIN_LENGTH: usize = 1;
const NAME_MAX_LENGTH: usize = 256;
const DESCRIPTION_MIN_LENGTH: usize = 1;
const DESCRIPTION_MAX_LENGTH: usize = 2048;

/// A well-formed create body carrying `name`: the two members the operation marks
/// required, with a configuration and data binding the accepted path already
/// pins, so each row below differs only in the member under test.
fn body(name: &str) -> Value {
    json!({
        "computationModelName": name,
        "computationModelConfiguration": anomaly_detection("input", "result"),
        "computationModelDataBinding": {
            "input": asset_model_property(&id_of('1'), &id_of('2')),
            "result": asset_model_property(&id_of('1'), &id_of('3')),
        },
    })
}

/// The same, with `member` set to `value`.
fn body_with(name: &str, member: &str, value: &str) -> Value {
    let mut request = body(name);
    request
        .as_object_mut()
        .expect("an object body")
        .insert(member.to_string(), json!(value));
    request
}

/// How many computation models the shared store holds — the count a refused
/// create must leave exactly as it was, and one the API exposes no reader for
/// while `ListComputationModels` is a stub.
fn held(engine: &Arc<engine::InMemoryEngine>) -> usize {
    service_state(&**engine)
        .expect("the IoT SiteWise store is mounted")
        .lock()
        .computation_models
        .len()
}

/// One row of the name/description table: the member under test, the value the
/// create sends for it, and whether that member's own modelled constraints admit
/// it.
#[derive(Debug)]
struct TextCase {
    label: &'static str,
    member: &'static str,
    value: String,
    accepted: bool,
}

/// The rows: each published length bound of each member at its edge and one
/// character past it, a value of mixed form well inside the range, and the one
/// character that tells the two patterns apart — `.`, which `RestrictedName`
/// admits and `RestrictedDescription` does not.
fn text_cases() -> Vec<TextCase> {
    vec![
        TextCase {
            label: "name at the minimum length",
            member: "computationModelName",
            value: "n".repeat(NAME_MIN_LENGTH),
            accepted: true,
        },
        TextCase {
            label: "name at the maximum length",
            member: "computationModelName",
            value: "n".repeat(NAME_MAX_LENGTH),
            accepted: true,
        },
        TextCase {
            label: "name spanning the punctuation the pattern admits",
            member: "computationModelName",
            value: "Pump 4 #1 $rate *peak !hot @site-a_b.c".to_string(),
            accepted: true,
        },
        TextCase {
            label: "name one character past the maximum length",
            member: "computationModelName",
            value: "n".repeat(NAME_MAX_LENGTH + 1),
            accepted: false,
        },
        TextCase {
            label: "name one character short of the minimum length",
            member: "computationModelName",
            value: String::new(),
            accepted: false,
        },
        TextCase {
            label: "name carrying a character outside the pattern",
            member: "computationModelName",
            value: "pump/4".to_string(),
            accepted: false,
        },
        TextCase {
            label: "description at the minimum length",
            member: "computationModelDescription",
            value: "d".repeat(DESCRIPTION_MIN_LENGTH),
            accepted: true,
        },
        TextCase {
            label: "description at the maximum length",
            member: "computationModelDescription",
            value: "d".repeat(DESCRIPTION_MAX_LENGTH),
            accepted: true,
        },
        TextCase {
            label: "description spanning the punctuation the pattern admits",
            member: "computationModelDescription",
            value: "Detects anomalies #1 $rate *peak !hot @site-a_b".to_string(),
            accepted: true,
        },
        TextCase {
            label: "description one character past the maximum length",
            member: "computationModelDescription",
            value: "d".repeat(DESCRIPTION_MAX_LENGTH + 1),
            accepted: false,
        },
        TextCase {
            label: "description one character short of the minimum length",
            member: "computationModelDescription",
            value: String::new(),
            accepted: false,
        },
        TextCase {
            label: "description carrying a dot, which only the name pattern admits",
            member: "computationModelDescription",
            value: "Detects anomalies in pump 4.".to_string(),
            accepted: false,
        },
    ]
}

#[tokio::test]
async fn a_create_enforces_the_modelled_text_bounds_before_it_writes() {
    let (router, engine) = router_with_engine();

    for (at, case) in text_cases().into_iter().enumerate() {
        let before = held(&engine);
        let request = if case.member == "computationModelName" {
            body(&case.value)
        } else {
            body_with(&format!("text-case-{at}"), case.member, &case.value)
        };

        let (status, answer) = create(&router, request).await;

        if !case.accepted {
            assert_eq!(
                status, 400,
                "case {:?}: a value of {} characters outside the member's own constraints must \
                 be refused, got {status} {answer}",
                case.label,
                case.value.chars().count()
            );
            // The refusal is the boundary validator's, and it answers with the
            // client-input error this service models rather than a synthetic one,
            // so one code is what a client matches on whichever member it got
            // wrong.
            assert_eq!(
                error_code(&answer),
                "InvalidRequestException",
                "case {:?}: unexpected error code in {answer}",
                case.label
            );
            assert_eq!(
                held(&engine),
                before,
                "case {:?}: a refused create stored a computation model",
                case.label
            );
            continue;
        }

        assert_eq!(
            status, 202,
            "case {:?}: a value of {} characters inside the member's own constraints must be \
             accepted, got {status} {answer}",
            case.label,
            case.value.chars().count()
        );
        let id = answer["computationModelId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no computationModelId in {answer}", case.label))
            .to_string();
        assert_eq!(
            held(&engine),
            before + 1,
            "case {:?}: an accepted create stored no computation model",
            case.label
        );

        // The accepted value is the one a read reports: persisted, not echoed
        // from the create's own answer.
        let (status, described) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the created model must read back, got {status} {described}",
            case.label
        );
        assert_eq!(
            described[case.member].as_str(),
            Some(case.value.as_str()),
            "case {:?}: the stored {} reads back as {described}",
            case.label,
            case.member
        );
    }
}

/// One row of the tag-key table: the key a create sends and whether the modelled
/// `TagKey` length bound admits it.
#[derive(Debug)]
struct TagKeyCase {
    label: &'static str,
    key: String,
    accepted: bool,
}

/// The rows: each published bound of `TagKey`'s `@length` at its edge and one
/// character past it, measured in CHARACTERS as `@length` counts them (so a
/// multi-byte key is measured the same way an ASCII one is), plus a key of mixed
/// form well inside the range.
fn tag_key_cases() -> Vec<TagKeyCase> {
    assert_eq!(
        (TAG_KEY_MIN_LENGTH, TAG_KEY_MAX_LENGTH),
        (1, 128),
        "TagKey's modelled @length(min: 1, max: 128) is the bound these rows walk, got \
         ({TAG_KEY_MIN_LENGTH}, {TAG_KEY_MAX_LENGTH})"
    );
    vec![
        TagKeyCase {
            label: "ascii key at the minimum length",
            key: "k".repeat(TAG_KEY_MIN_LENGTH),
            accepted: true,
        },
        TagKeyCase {
            label: "ascii key at the maximum length",
            key: "k".repeat(TAG_KEY_MAX_LENGTH),
            accepted: true,
        },
        TagKeyCase {
            label: "multi-byte key at the maximum length",
            key: "é".repeat(TAG_KEY_MAX_LENGTH),
            accepted: true,
        },
        TagKeyCase {
            label: "key of mixed form well inside the bound",
            key: "cost centre.42/north_line:a".to_string(),
            accepted: true,
        },
        TagKeyCase {
            label: "the empty key, one character short of the minimum",
            key: String::new(),
            accepted: false,
        },
        TagKeyCase {
            label: "ascii key one character past the maximum",
            key: "k".repeat(TAG_KEY_MAX_LENGTH + 1),
            accepted: false,
        },
        TagKeyCase {
            label: "multi-byte key one character past the maximum",
            key: "é".repeat(TAG_KEY_MAX_LENGTH + 1),
            accepted: false,
        },
    ]
}

#[tokio::test]
async fn a_create_enforces_the_modelled_tag_key_bound_before_it_writes() {
    let (router, engine) = router_with_engine();

    for (at, case) in tag_key_cases().into_iter().enumerate() {
        let before = held(&engine);
        let mut request = body(&format!("tag-case-{at}"));
        let key = case.key.clone();
        request
            .as_object_mut()
            .expect("an object body")
            .insert("tags".to_string(), json!({ key: "value" }));

        let (status, answer) = create(&router, request).await;

        if !case.accepted {
            assert_eq!(
                (status, error_code(&answer)),
                (400, "InvalidRequestException"),
                "case {:?}: a key of {} characters must be refused with the client-input error \
                 this operation models, got {status} {answer}",
                case.label,
                case.key.chars().count()
            );
            assert_eq!(
                held(&engine),
                before,
                "case {:?}: a refused create stored a computation model",
                case.label
            );
            continue;
        }

        assert_eq!(
            status, 202,
            "case {:?}: a key of {} characters is inside the bound, so the create must answer \
             202, got {status} {answer}",
            case.label,
            case.key.chars().count()
        );
        let id = answer["computationModelId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no computationModelId in {answer}", case.label))
            .to_string();

        // The accepted key is read back through the tag reader, off the one
        // shared store: the tagged resource is the model this create minted.
        let arn = expected_arn(&id);
        let (status, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: the tags of {arn} must be readable, got {status} {listed}",
            case.label
        );
        assert_eq!(
            listed["tags"][&case.key].as_str(),
            Some("value"),
            "case {:?}: the accepted key must be persisted under {arn}, got {listed}",
            case.label
        );
    }
}

#[tokio::test]
async fn a_create_past_the_store_ceiling_is_the_modelled_quota_refusal() {
    let (router, engine) = router_with_engine();

    // Fill the store right up to its ceiling. Every create inside the bound is
    // accepted, so the refusal below is about the count and nothing else.
    for index in 0..MAX_COMPUTATION_MODELS {
        let (status, answer) = create(&router, body(&format!("held-{index}"))).await;
        assert_eq!(
            status, 202,
            "model {index} of {MAX_COMPUTATION_MODELS}: expected 202, got {status} {answer}"
        );
    }
    assert_eq!(
        held(&engine),
        MAX_COMPUTATION_MODELS,
        "expected the store to hold its full ceiling of computation models"
    );

    let (status, refused) = create(&router, body("one-too-many")).await;
    assert_eq!(
        status, 410,
        "a create past the ceiling: expected 410, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "LimitExceededException",
        "a create past the ceiling: unexpected error code in {refused}"
    );
    assert!(
        refused["computationModelId"].is_null(),
        "a create past the ceiling must mint no model, got {refused}"
    );
    assert_eq!(
        held(&engine),
        MAX_COMPUTATION_MODELS,
        "a refused create changed how many computation models the store holds"
    );
}

#[tokio::test]
async fn each_create_without_a_client_token_mints_a_model_of_its_own() {
    let router = router();

    // The same body, sent several times with no `clientToken`: nothing names the
    // request as a retry, so each call is its own create.
    let mut minted: Vec<(String, String)> = Vec::new();
    for attempt in 0..3 {
        let (status, created) = create(&router, body("repeatable")).await;
        assert_eq!(
            status, 202,
            "attempt {attempt}: expected 202, got {status} {created}"
        );
        let id = created["computationModelId"]
            .as_str()
            .unwrap_or_else(|| panic!("attempt {attempt}: no computationModelId in {created}"))
            .to_string();
        let arn = created["computationModelArn"]
            .as_str()
            .unwrap_or_else(|| panic!("attempt {attempt}: no computationModelArn in {created}"))
            .to_string();
        assert_eq!(
            arn,
            expected_arn(&id),
            "attempt {attempt}: unexpected ARN in {created}"
        );
        minted.push((id, arn));
    }

    let distinct: std::collections::BTreeSet<&(String, String)> = minted.iter().collect();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "expected one model per create with no clientToken, got {minted:?}"
    );

    // And every one of them reads back on its own id.
    for (id, _) in &minted {
        let (status, described) = describe(&router, id).await;
        assert_eq!(
            status, 200,
            "model {id} must read back on its own id, got {status} {described}"
        );
        assert_eq!(
            described["computationModelName"].as_str(),
            Some("repeatable"),
            "model {id} reads back as {described}"
        );
    }
}
