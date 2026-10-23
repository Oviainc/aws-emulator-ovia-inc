// SPDX-License-Identifier: CC0-1.0

//! `DeprecateActivityType`, end to end over this service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: SimpleWorkflowService.<OperationName>`),
//! dispatched through the same `MultiServiceRouter` the emulator mounts in production.
//! The harness is the one `workflow_execution_lifecycle` already builds, and the stored
//! record is read back through `DescribeActivityType`.
//!
//! The rule every case here asserts: `DeprecateActivityType` moves the ONE stored
//! activity-type record its `(domain, name, version)` triple addresses from
//! `REGISTERED` to `DEPRECATED` and stamps that record's `deprecationDate` with the
//! instant of THIS call, leaving everything else the record stores — and every other
//! stored record, of either kind, in any domain — exactly as it was; a record already
//! `DEPRECATED` is HTTP 400 `TypeDeprecatedFault` with nothing written, so the stamp a
//! record carries is the instant of its FIRST deprecation.
//!
//! `tests/all/activity_type_lifecycle.rs` drives the deprecate/undeprecate/delete cycle
//! over the shape of the identity; the tables here vary the dimensions that cycle does
//! not: which of several domains holds an identical identity, whether a workflow type of
//! the same `(name, version)` exists beside the activity type, how far apart in time the
//! registration and the deprecation are, and how many times the refusal is repeated.
//! No row is privileged.

use serde_json::{json, Value};
use service_com_amazonaws_swf_simpleworkflowservice::ops::types::REGISTRATION_STATUS_VALUES;

use crate::workflow_execution_lifecycle::{error_code, register_domain_body, string_at, Emulator};

/// The modelled maximum length of `ActivityType$name`
/// (`smithy_query(query="shape",shape="com.amazonaws.swf#ActivityType")`: `name` targets
/// `Name`, len 1..256).
const MAX_TYPE_NAME_LEN: usize = 256;

/// The modelled maximum length of `ActivityType$version` (same query: `version` targets
/// `Version`, len 1..64).
const MAX_TYPE_VERSION_LEN: usize = 64;

/// How long a case waits between registering a type and deprecating it, so the two
/// instants are separated by far more than the clock's resolution and the stamp a
/// deprecation writes is distinguishable from the `creationDate` registration wrote.
const CLOCK_SEPARATION: std::time::Duration = std::time::Duration::from_millis(10);

/// Slack allowed when comparing a wire timestamp (epoch seconds, serialized as a JSON
/// number) against an instant this test read from the same clock: the serialization is
/// a rounding, not an identity.
const TIMESTAMP_SLACK_SECONDS: f64 = 0.001;

/// One activity-type identity the rule is asserted over. Rows differ in the form and
/// length of `(name, version)`, including both modelled bounds, and two rows share a
/// name at different versions so "the one record addressed" has a near neighbour.
struct Identity {
    case: &'static str,
    name: String,
    version: String,
}

fn identities() -> Vec<Identity> {
    vec![
        Identity {
            case: "plain identity",
            name: "ship-order".to_string(),
            version: "1.0".to_string(),
        },
        Identity {
            // The same name at another version is another record entirely.
            case: "same name, other version",
            name: "ship-order".to_string(),
            version: "2.0".to_string(),
        },
        Identity {
            case: "modelled minimum lengths",
            name: "a".to_string(),
            version: "1".to_string(),
        },
        Identity {
            case: "modelled maximum lengths",
            name: "n".repeat(MAX_TYPE_NAME_LEN),
            version: "v".repeat(MAX_TYPE_VERSION_LEN),
        },
        Identity {
            // Punctuation Amazon SWF's name constraint allows: it refuses only `:`, `/`,
            // `|`, control characters and the literal `arn` (the `ValidationException`
            // RegisterActivityType answers for those).
            case: "symbols and punctuation",
            name: "pack_items-v2.final!step".to_string(),
            version: "2026-01-01+build.7".to_string(),
        },
        Identity {
            case: "non-ASCII identity",
            name: "活動タイプ".to_string(),
            version: "ß-1".to_string(),
        },
    ]
}

/// The request body every type-scoped operation of this family shares.
fn type_request(domain: &str, name: &str, version: &str) -> Value {
    json!({ "domain": domain, "activityType": { "name": name, "version": version } })
}

/// How Amazon SWF names one activity type in a fault message, built from the requested
/// identity alone.
fn type_reference(name: &str, version: &str) -> String {
    format!("ActivityType=[name={name}, version={version}]")
}

/// This clock, as the wire timestamps are reported: epoch seconds.
fn epoch_seconds_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the Unix epoch")
        .as_secs_f64()
}

async fn register_domain(emulator: &Emulator, case: &str, domain: &str) {
    emulator
        .ok(case, "RegisterDomain", &register_domain_body(domain, "7"))
        .await;
}

async fn register_activity_type(emulator: &Emulator, case: &str, domain: &str, id: &Identity) {
    emulator
        .ok(
            case,
            "RegisterActivityType",
            &json!({ "domain": domain, "name": id.name, "version": id.version }),
        )
        .await;
}

async fn describe_activity_type(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    id: &Identity,
) -> Value {
    emulator
        .ok(
            case,
            "DescribeActivityType",
            &type_request(domain, &id.name, &id.version),
        )
        .await
}

/// Deprecate one record, requiring the modelled empty success.
async fn deprecate_activity_type(emulator: &Emulator, case: &str, domain: &str, id: &Identity) {
    let response = emulator
        .ok(
            case,
            "DeprecateActivityType",
            &type_request(domain, &id.name, &id.version),
        )
        .await;
    assert_eq!(
        response,
        json!({}),
        "case {case:?}: DeprecateActivityType's modelled output shape is Unit, \
         got {response:?}"
    );
}

/// The modelled `RegistrationStatus` the record reports, checked against the model's own
/// value set so no case passes on a status the model does not define.
fn modelled_status(case: &str, described: &Value) -> String {
    let status = string_at(described, "/typeInfo/status");
    assert!(
        REGISTRATION_STATUS_VALUES.contains(&status.as_str()),
        "case {case:?}: status must be one of the modelled RegistrationStatus values \
         {REGISTRATION_STATUS_VALUES:?}, got {status:?}"
    );
    status
}

fn seconds_at(case: &str, described: &Value, pointer: &str) -> f64 {
    described
        .pointer(pointer)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a timestamp at {pointer} of {described:?}"))
}

/// Assert `described` reports a record that has not been deprecated.
fn assert_registered(case: &str, described: &Value) {
    let status = modelled_status(case, described);
    assert_eq!(
        status, "REGISTERED",
        "case {case:?}: a type no deprecation addressed reports REGISTERED, \
         got {status:?} in {described:?}"
    );
    assert_eq!(
        described.pointer("/typeInfo/deprecationDate"),
        None,
        "case {case:?}: deprecationDate is reported only while the type is DEPRECATED, \
         got {described:?}"
    );
}

/// Assert `described` reports a deprecated record and return its `deprecationDate`.
fn assert_deprecated(case: &str, described: &Value) -> f64 {
    let status = modelled_status(case, described);
    assert_eq!(
        status, "DEPRECATED",
        "case {case:?}: a deprecated type reports DEPRECATED, got {status:?} in \
         {described:?}"
    );
    seconds_at(case, described, "/typeInfo/deprecationDate")
}

/// The record with its status and stamp dropped: everything the deprecation must leave
/// exactly as registration stored it.
fn without_deprecation_members(described: &Value) -> Value {
    let mut copy = described.clone();
    let type_info = copy
        .pointer_mut("/typeInfo")
        .and_then(Value::as_object_mut)
        .expect("DescribeActivityType reports a typeInfo structure");
    type_info.remove("status");
    type_info.remove("deprecationDate");
    copy
}

#[tokio::test]
async fn deprecation_stamps_the_instant_of_the_call_not_of_the_registration() {
    let emulator = Emulator::new();
    let domain = "stamping";
    register_domain(&emulator, "seed", domain).await;

    for id in identities() {
        register_activity_type(&emulator, id.case, domain, &id).await;
        let registered = describe_activity_type(&emulator, id.case, domain, &id).await;
        assert_registered(id.case, &registered);
        let creation = seconds_at(id.case, &registered, "/typeInfo/creationDate");

        // Separate the two instants by far more than the clock's resolution, so a stamp
        // taken at registration time could not pass for one taken now.
        std::thread::sleep(CLOCK_SEPARATION);
        let before = epoch_seconds_now();
        deprecate_activity_type(&emulator, id.case, domain, &id).await;
        let after = epoch_seconds_now();

        let deprecated = describe_activity_type(&emulator, id.case, domain, &id).await;
        let stamp = assert_deprecated(id.case, &deprecated);
        assert!(
            stamp >= before - TIMESTAMP_SLACK_SECONDS && stamp <= after + TIMESTAMP_SLACK_SECONDS,
            "case {:?}: deprecationDate must be the instant of the call, inside \
             [{before}, {after}], got {stamp}",
            id.case
        );
        assert!(
            stamp > creation,
            "case {:?}: deprecationDate {stamp} must be strictly after the creationDate \
             {creation} recorded at least {CLOCK_SEPARATION:?} earlier",
            id.case
        );
        assert_eq!(
            seconds_at(id.case, &deprecated, "/typeInfo/creationDate"),
            creation,
            "case {:?}: a deprecation must not restamp creationDate, got {deprecated:?}",
            id.case
        );
        assert_eq!(
            without_deprecation_members(&deprecated),
            without_deprecation_members(&registered),
            "case {:?}: the deprecation moves the status and writes the stamp, and \
             changes nothing else the record stores, got {deprecated:?}",
            id.case
        );
    }
}

#[tokio::test]
async fn an_already_deprecated_record_is_refused_and_keeps_its_first_stamp() {
    let emulator = Emulator::new();
    let domain = "refusals";
    register_domain(&emulator, "seed", domain).await;
    let ids = identities();
    for id in &ids {
        register_activity_type(&emulator, id.case, domain, id).await;
    }

    for id in &ids {
        deprecate_activity_type(&emulator, id.case, domain, id).await;
        let first = describe_activity_type(&emulator, id.case, domain, id).await;
        let stamp = assert_deprecated(id.case, &first);

        // The refusal is the record's own state, so it repeats and writes nothing: the
        // stamp stays the one the FIRST deprecation gave it, however many calls follow.
        for round in 1..=3 {
            let case = format!("{} refusal #{round}", id.case);
            std::thread::sleep(CLOCK_SEPARATION);
            let (status, body) = emulator
                .call(
                    "DeprecateActivityType",
                    &type_request(domain, &id.name, &id.version),
                )
                .await;
            let code = error_code(&body);
            assert_eq!(
                (status, code.as_str()),
                (400, "TypeDeprecatedFault"),
                "case {case:?}: an already-deprecated record is refused with \
                 400 TypeDeprecatedFault, got {status} {code} with {body:?}"
            );
            let message = body
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {body:?}"));
            assert_eq!(
                message,
                type_reference(&id.name, &id.version),
                "case {case:?}: the fault names the requested type, got {message:?}"
            );

            let after = describe_activity_type(&emulator, &case, domain, id).await;
            assert_eq!(
                after, first,
                "case {case:?}: a refused deprecation changes nothing a later caller \
                 sees — stamp {stamp} included — got {after:?}"
            );
        }
    }
}

#[tokio::test]
async fn deprecation_moves_only_the_one_triple_it_addresses() {
    // The same identities are registered in several domains, so every row has a twin
    // under another `(domain, ..)` scope and a neighbour under the same one.
    let emulator = Emulator::new();
    let domains = ["alpha", "beta", "gamma"];
    let ids = identities();
    for domain in domains {
        register_domain(&emulator, "seed", domain).await;
        for id in &ids {
            register_activity_type(&emulator, id.case, domain, id).await;
        }
    }

    let mut deprecated: Vec<(usize, usize)> = Vec::new();
    for (domain_index, domain) in domains.iter().enumerate() {
        for (id_index, id) in ids.iter().enumerate() {
            let case = format!("{domain}/{}", id.case);
            deprecate_activity_type(&emulator, &case, domain, id).await;
            deprecated.push((domain_index, id_index));

            // Every stored record of every domain now reads back exactly the status its
            // OWN deprecation history gives it.
            for (other_domain_index, other_domain) in domains.iter().enumerate() {
                for (other_id_index, other_id) in ids.iter().enumerate() {
                    let other_case =
                        format!("{case} -> {other_domain}/{}", other_id.case);
                    let described =
                        describe_activity_type(&emulator, &other_case, other_domain, other_id)
                            .await;
                    if deprecated.contains(&(other_domain_index, other_id_index)) {
                        assert_deprecated(&other_case, &described);
                    } else {
                        assert_registered(&other_case, &described);
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn deprecating_an_activity_type_touches_neither_its_domain_nor_a_same_named_workflow_type() {
    // `(name, version)` identifies an activity type only within its own kind: a workflow
    // type registered under the very same pair is a different record, and so is the
    // domain that holds them both.
    let emulator = Emulator::new();
    let domain = "two-kinds";
    register_domain(&emulator, "seed", domain).await;
    let ids = identities();

    for id in &ids {
        register_activity_type(&emulator, id.case, domain, id).await;
        emulator
            .ok(
                id.case,
                "RegisterWorkflowType",
                &json!({ "domain": domain, "name": id.name, "version": id.version }),
            )
            .await;
    }

    for id in &ids {
        let workflow_request =
            json!({ "domain": domain, "workflowType": { "name": id.name, "version": id.version } });
        let workflow_before = emulator
            .ok(id.case, "DescribeWorkflowType", &workflow_request)
            .await;
        assert_registered(id.case, &workflow_before);

        deprecate_activity_type(&emulator, id.case, domain, id).await;
        let activity = describe_activity_type(&emulator, id.case, domain, id).await;
        let stamp = assert_deprecated(id.case, &activity);

        let workflow_after = emulator
            .ok(id.case, "DescribeWorkflowType", &workflow_request)
            .await;
        assert_eq!(
            workflow_after, workflow_before,
            "case {:?}: deprecating the activity type must leave the workflow type of \
             the same (name, version) untouched, got {workflow_after:?}",
            id.case
        );

        let domain_record = emulator
            .ok(id.case, "DescribeDomain", &json!({ "name": domain }))
            .await;
        let domain_status = string_at(&domain_record, "/domainInfo/status");
        assert_eq!(
            domain_status, "REGISTERED",
            "case {:?}: deprecating a type must not move its domain's own status, \
             got {domain_status:?} in {domain_record:?}",
            id.case
        );

        // And the workflow type's own deprecation is still available, and leaves the
        // activity type's stamp where it is.
        std::thread::sleep(CLOCK_SEPARATION);
        emulator
            .ok(id.case, "DeprecateWorkflowType", &workflow_request)
            .await;
        let activity_again = describe_activity_type(&emulator, id.case, domain, id).await;
        assert_eq!(
            assert_deprecated(id.case, &activity_again),
            stamp,
            "case {:?}: deprecating the workflow type must not restamp the activity \
             type, got {activity_again:?}",
            id.case
        );
    }
}
