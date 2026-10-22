// SPDX-License-Identifier: CC0-1.0

//! `UpdateApplication`, end to end over HTTP.
//!
//! Every case goes through the real router (restJson1, SigV4 signing name
//! `emr-serverless`), so each one exercises routing, deserialization, the handler, the
//! persisted record and serialization — the same path an SDK client takes.
//!
//! Three rules are under test, each asserted over a SET of inputs rather than over one
//! recorded request:
//!
//! 1. An update applies exactly the members the request NAMES to the stored
//!    application and leaves every member it omits as stored; a configuration block it
//!    names REPLACES the stored one with the model's documented defaults filled in.
//!    Every later read reports the updated record.
//! 2. The operation's documented precondition — "An application has to be in a stopped
//!    or created state in order to be updated" — is judged against the state the
//!    application has SETTLED on, and a state outside it is refused with the modelled
//!    `ValidationException` (HTTP 400) without changing the record. A RUNNING
//!    application is the one state that admits part of an update: the members EMR
//!    Serverless documents as updatable on it are asserted in
//!    `update_application_while_started`, and the rows here name a member OUTSIDE that
//!    set, so they assert the general precondition alone.
//! 3. An application that does not exist is refused with `ResourceNotFoundException`
//!    (HTTP 404).

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::application_lifecycle::{
    call, create_application, delete_application, emulator, get_application, wait_for_transition,
    Answer,
};
use super::update_application_while_started::RUNNING_CARVE_OUT_REFUSAL;

/// The `ApplicationState` values that admit an update, and the ones that do not.
///
/// Written out here rather than read from `crate::state`, so a wrong list in `src/`
/// cannot make this test pass. Source: `UpdateApplication`'s own documentation, "An
/// application has to be in a stopped or created state in order to be updated", over
/// the `ApplicationState` enum (`CREATING`, `CREATED`, `STARTING`, `STARTED`,
/// `STOPPING`, `STOPPED`, `TERMINATED`) from
/// `smithy_query(query="shape",shape="ApplicationState")`.
const UPDATABLE_STATES: &[&str] = &["CREATED", "STOPPED"];

/// An image URI satisfying `ImageUri`'s pattern (an Amazon ECR registry reference).
pub(crate) const IMAGE_URI: &str =
    "000000000000.dkr.ecr.eu-west-1.amazonaws.com/emr-serverless:7.1.0";

/// A `CreateApplication` body carrying a value for every member an update can change,
/// so a row that names one member can assert that the OTHERS did not move.
fn rich_create(token: &str) -> Value {
    json!({
        "clientToken": token,
        "releaseLabel": "emr-7.1.0",
        "type": "Spark",
        "name": "baseline",
        "architecture": "X86_64",
        "tags": { "team": "data" },
        // `enabled: false` here is what makes the replace-with-defaults rule visible: an
        // update naming only `idleTimeoutMinutes` must report the documented default
        // `enabled: true` rather than this stored value.
        "autoStopConfiguration": { "enabled": false, "idleTimeoutMinutes": 30 },
        "autoStartConfiguration": { "enabled": false },
        "maximumCapacity": { "cpu": "8 vCPU", "memory": "32 GB" },
        "initialCapacity": {
            "Driver": {
                "workerCount": 1,
                "workerConfiguration": { "cpu": "1 vCPU", "memory": "4 GB" },
            },
        },
        "networkConfiguration": { "subnetIds": ["subnet-00000000"], "securityGroupIds": ["sg-00000000"] },
        "runtimeConfiguration": [
            { "classification": "spark-defaults", "properties": { "spark.executor.cores": "1" } },
        ],
        "interactiveConfiguration": { "studioEnabled": false, "livyEndpointEnabled": false },
        "schedulerConfiguration": { "queueTimeoutMinutes": 360, "maxConcurrentRuns": 15 },
        "monitoringConfiguration": {
            "s3MonitoringConfiguration": { "logUri": "s3://bucket/logs" },
        },
        "jobLevelCostAllocationConfiguration": { "enabled": false },
        "imageConfiguration": { "imageUri": IMAGE_URI },
        "workerTypeSpecifications": {
            "Driver": { "imageConfiguration": { "imageUri": IMAGE_URI } },
        },
    })
}

pub(crate) async fn update_application(
    router: &MultiServiceRouter,
    id: &str,
    body: Value,
) -> Answer {
    call(router, "PATCH", &format!("/applications/{id}"), Some(body)).await
}

/// Create the baseline application, let the clock settle it, and return its id.
pub(crate) async fn settled_application(router: &MultiServiceRouter, token: &str) -> String {
    let created = create_application(router, rich_create(token)).await;
    assert_eq!(
        created.status, 200,
        "case {token:?}: CreateApplication expected 200, got {} with {:?}",
        created.status, created.body
    );
    let id = created
        .at("/applicationId")
        .as_str()
        .unwrap_or_default()
        .to_string();
    wait_for_transition();
    id
}

/// An update body: the required `clientToken` plus the members the row names.
pub(crate) fn update_body(token: &str, members: &Value) -> Value {
    let mut body = json!({ "clientToken": token });
    if let (Some(target), Some(members)) = (body.as_object_mut(), members.as_object()) {
        for (name, value) in members {
            target.insert(name.clone(), value.clone());
        }
    }
    body
}

/// `Application$updatedAt` as epoch seconds.
fn seconds(application: &Value, member: &str) -> f64 {
    application
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_default()
}

/// Rule 1: an update applies exactly the members it names, leaves the rest as stored,
/// and every later read reports the updated record.
///
/// The rows differ along what the rule generalises over: WHICH member is named, the
/// member's type (string, structure, map, list), the number of members named at once,
/// the size of a map (zero, one and two entries) and the values inside it.
#[tokio::test]
async fn an_update_applies_the_members_it_names_and_leaves_the_rest_as_stored() {
    // (case, the members the update names, the members the answer must report)
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "no member beyond the required client token",
            json!({}),
            json!({}),
        ),
        (
            "the maximum capacity alone",
            json!({ "maximumCapacity": { "cpu": "4 vCPU", "memory": "16 GB" } }),
            json!({ "maximumCapacity": { "cpu": "4 vCPU", "memory": "16 GB" } }),
        ),
        (
            "the maximum capacity with a disk",
            json!({
                "maximumCapacity": { "cpu": "400 vCPU", "memory": "3000 GB", "disk": "20000 GB" },
            }),
            json!({
                "maximumCapacity": { "cpu": "400 vCPU", "memory": "3000 GB", "disk": "20000 GB" },
            }),
        ),
        (
            "the release label alone",
            json!({ "releaseLabel": "emr-6.9.0" }),
            json!({ "releaseLabel": "emr-6.9.0" }),
        ),
        (
            "two members at once: the architecture and the auto-start configuration",
            json!({ "architecture": "ARM64", "autoStartConfiguration": { "enabled": true } }),
            json!({ "architecture": "ARM64", "autoStartConfiguration": { "enabled": true } }),
        ),
        (
            // The block REPLACES the stored one: the member the update leaves out comes
            // back as `AutoStopConfig$enabled`'s documented default (true), not as the
            // `false` the create stored.
            "a configuration block naming one of its members",
            json!({ "autoStopConfiguration": { "idleTimeoutMinutes": 1 } }),
            json!({ "autoStopConfiguration": { "enabled": true, "idleTimeoutMinutes": 1 } }),
        ),
        (
            "the idle timeout at its published maximum",
            json!({ "autoStopConfiguration": { "enabled": true, "idleTimeoutMinutes": 10080 } }),
            json!({ "autoStopConfiguration": { "enabled": true, "idleTimeoutMinutes": 10080 } }),
        ),
        (
            "an initial capacity of one worker type",
            json!({
                "initialCapacity": {
                    "Executor": {
                        "workerCount": 4,
                        "workerConfiguration": { "cpu": "2 vCPU", "memory": "8 GB" },
                    },
                },
            }),
            json!({
                "initialCapacity": {
                    "Executor": {
                        "workerCount": 4,
                        "workerConfiguration": { "cpu": "2 vCPU", "memory": "8 GB" },
                    },
                },
            }),
        ),
        (
            "an initial capacity of two worker types, one with a disk",
            json!({
                "initialCapacity": {
                    "Driver": {
                        "workerCount": 1,
                        "workerConfiguration": { "cpu": "4 vCPU", "memory": "16 GB" },
                    },
                    "Executor": {
                        "workerCount": 100,
                        "workerConfiguration": {
                            "cpu": "8 vCPU",
                            "memory": "32 GB",
                            "disk": "200 GB",
                        },
                    },
                },
            }),
            json!({
                "initialCapacity": {
                    "Driver": {
                        "workerCount": 1,
                        "workerConfiguration": { "cpu": "4 vCPU", "memory": "16 GB" },
                    },
                    "Executor": {
                        "workerCount": 100,
                        "workerConfiguration": {
                            "cpu": "8 vCPU",
                            "memory": "32 GB",
                            "disk": "200 GB",
                        },
                    },
                },
            }),
        ),
        (
            // `InitialCapacityConfigMap` is `@length(min: 0, max: 10)`, so the empty map
            // is a value the model admits and the update stores it as given.
            "an empty initial capacity map",
            json!({ "initialCapacity": {} }),
            json!({ "initialCapacity": {} }),
        ),
        (
            "a runtime configuration of two classifications",
            json!({
                "runtimeConfiguration": [
                    {
                        "classification": "spark-defaults",
                        "properties": { "spark.executor.cores": "4" },
                    },
                    { "classification": "hive-site", "properties": {} },
                ],
            }),
            json!({
                "runtimeConfiguration": [
                    {
                        "classification": "spark-defaults",
                        "properties": { "spark.executor.cores": "4" },
                    },
                    { "classification": "hive-site", "properties": {} },
                ],
            }),
        ),
        (
            "an empty runtime configuration list",
            json!({ "runtimeConfiguration": [] }),
            json!({ "runtimeConfiguration": [] }),
        ),
        (
            "the interactive and scheduler configurations together",
            json!({
                "interactiveConfiguration": { "studioEnabled": true, "livyEndpointEnabled": true },
                "schedulerConfiguration": { "queueTimeoutMinutes": 15, "maxConcurrentRuns": 1 },
            }),
            json!({
                "interactiveConfiguration": { "studioEnabled": true, "livyEndpointEnabled": true },
                "schedulerConfiguration": { "queueTimeoutMinutes": 15, "maxConcurrentRuns": 1 },
            }),
        ),
        (
            "the network configuration, with no security group",
            json!({
                "networkConfiguration": {
                    "subnetIds": ["subnet-0000000000000001", "subnet-0000000000000002"],
                    "securityGroupIds": [],
                },
            }),
            json!({
                "networkConfiguration": {
                    "subnetIds": ["subnet-0000000000000001", "subnet-0000000000000002"],
                    "securityGroupIds": [],
                },
            }),
        ),
        (
            "the worker type specifications",
            json!({
                "workerTypeSpecifications": {
                    "Driver": { "imageConfiguration": { "imageUri": IMAGE_URI } },
                    "Executor": { "imageConfiguration": { "imageUri": IMAGE_URI } },
                },
            }),
            json!({
                "workerTypeSpecifications": {
                    "Driver": { "imageConfiguration": { "imageUri": IMAGE_URI } },
                    "Executor": { "imageConfiguration": { "imageUri": IMAGE_URI } },
                },
            }),
        ),
        (
            // `ImageConfigurationInput$imageUri` documents it: "If you leave this field
            // blank in an update, Amazon EMR will remove the image configuration."
            "an image configuration naming no image, which removes it",
            json!({ "imageConfiguration": {} }),
            json!({}),
        ),
        (
            "the job level cost allocation configuration",
            json!({ "jobLevelCostAllocationConfiguration": { "enabled": true } }),
            json!({ "jobLevelCostAllocationConfiguration": { "enabled": true } }),
        ),
        (
            "the monitoring configuration",
            json!({
                "monitoringConfiguration": {
                    "managedPersistenceMonitoringConfiguration": { "enabled": true },
                },
            }),
            json!({
                "monitoringConfiguration": {
                    "managedPersistenceMonitoringConfiguration": { "enabled": true },
                },
            }),
        ),
    ];

    for (index, (case, members, expected)) in cases.into_iter().enumerate() {
        let router = emulator();
        let id = settled_application(&router, &format!("tok-update-{index}")).await;
        let before = get_application(&router, &id).await.at("/application");

        let answered =
            update_application(&router, &id, update_body(&format!("upd-{index}"), &members)).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: UpdateApplication expected 200, got {} with {:?}",
            answered.status, answered.body
        );

        // Every member the row expects is reported with the value the update asked for.
        for (member, value) in expected.as_object().into_iter().flatten() {
            assert_eq!(
                answered.at(&format!("/application/{member}")),
                *value,
                "case {case:?}: UpdateApplication reports {member} as {value:?}, got {:?}",
                answered.at(&format!("/application/{member}"))
            );
        }
        // The removal case: the member the update cleared is reported by nobody.
        if members.get("imageConfiguration").is_some_and(|given| {
            given.get("imageUri").is_none()
        }) {
            assert_eq!(
                answered.at("/application/imageConfiguration"),
                Value::Null,
                "case {case:?}: an update naming no image removes the image configuration, \
                 got {:?}",
                answered.at("/application/imageConfiguration")
            );
        }

        // ROUND TRIP: the update is persisted, so the reader reports exactly what the
        // update answered.
        let read_back = get_application(&router, &id).await;
        assert_eq!(
            read_back.status, 200,
            "case {case:?}: GetApplication expected 200, got {} with {:?}",
            read_back.status, read_back.body
        );
        assert_eq!(
            read_back.at("/application"),
            answered.at("/application"),
            "case {case:?}: GetApplication must report the updated record, got {:?} against \
             {:?}",
            read_back.at("/application"),
            answered.at("/application")
        );

        // Every member the update did NOT name is exactly as it was stored. `updatedAt`
        // is excluded: the update is what moved it.
        let named: BTreeSet<&String> = members.as_object().into_iter().flatten().map(|(name, _)| name).collect();
        let after = read_back.at("/application");
        for (member, stored) in before.as_object().into_iter().flatten() {
            if named.contains(member) || member == "updatedAt" {
                continue;
            }
            assert_eq!(
                after.get(member).unwrap_or(&Value::Null),
                stored,
                "case {case:?}: the update must not change {member}, expected {stored:?}, \
                 got {:?}",
                after.get(member).unwrap_or(&Value::Null)
            );
        }

        // An update is a change to the application, so `updatedAt` is at or after the
        // instant it was stored at and never before `createdAt`.
        let updated_at = seconds(&after, "updatedAt");
        assert!(
            updated_at >= seconds(&before, "updatedAt")
                && updated_at >= seconds(&after, "createdAt"),
            "case {case:?}: updatedAt must not move backwards — createdAt {}, before {}, \
             after {updated_at}",
            seconds(&after, "createdAt"),
            seconds(&before, "updatedAt")
        );
    }
}

/// Rule 1, continued: successive updates accumulate on the one stored record — the
/// second update does not undo what the first one put there.
#[tokio::test]
async fn successive_updates_accumulate_on_the_stored_application() {
    let router = emulator();
    let id = settled_application(&router, "tok-accumulate").await;

    // Each step names a DIFFERENT member, so after the last one every member named
    // along the way still carries the value its own step gave.
    let steps: Vec<(&str, &str, Value)> = vec![
        (
            "the maximum capacity",
            "maximumCapacity",
            json!({ "cpu": "16 vCPU", "memory": "64 GB" }),
        ),
        ("the release label", "releaseLabel", json!("emr-7.0.0")),
        ("the architecture", "architecture", json!("ARM64")),
        (
            "the interactive configuration",
            "interactiveConfiguration",
            json!({ "sessionEnabled": true }),
        ),
    ];

    let mut applied: Vec<(&str, &str, Value)> = Vec::new();
    for (index, (case, member, value)) in steps.into_iter().enumerate() {
        let mut named = serde_json::Map::new();
        named.insert(member.to_string(), value.clone());
        let answered = update_application(
            &router,
            &id,
            update_body(&format!("acc-{index}"), &Value::Object(named)),
        )
        .await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: UpdateApplication expected 200, got {} with {:?}",
            answered.status, answered.body
        );
        applied.push((case, member, value));

        let read_back = get_application(&router, &id).await;
        for (earlier, earlier_member, earlier_value) in &applied {
            assert_eq!(
                read_back.at(&format!("/application/{earlier_member}")),
                *earlier_value,
                "case {case:?}: {earlier:?} must still be in place, expected \
                 {earlier_value:?}, got {:?}",
                read_back.at(&format!("/application/{earlier_member}"))
            );
        }
    }
}

/// Rule 2: the documented precondition, over every `ApplicationState` an application
/// can be in — and judged against the state it has SETTLED on, so a transition the
/// clock still owes never refuses an update the settled application admits.
///
/// Every row names `autoStartConfiguration`, a member OUTSIDE the set EMR Serverless
/// documents as updatable on a RUNNING application, so each one turns on the general
/// precondition alone; the running-application carve-out has its own suite in
/// `update_application_while_started`.
#[tokio::test]
async fn an_update_is_admitted_only_in_the_states_the_operation_documents() {
    // (case, the lifecycle calls after the create, whether the LAST call is given time
    // to settle, the state the application has settled on when the update arrives). A
    // row that does not settle its last call leaves the application in the transient
    // state that call returned (`CREATING`, `STARTING`, `STOPPING`).
    let cases: Vec<(&str, &[&str], bool, &str)> = vec![
        ("just created, settled", &[], true, "CREATED"),
        ("still CREATING", &[], false, "CREATED"),
        ("started, settled", &["start"], true, "STARTED"),
        ("still STARTING", &["start"], false, "STARTED"),
        ("stopped, settled", &["start", "stop"], true, "STOPPED"),
        ("still STOPPING", &["start", "stop"], false, "STOPPED"),
        ("terminated", &["delete"], true, "TERMINATED"),
    ];

    for (index, (case, actions, settle_last, settled_state)) in cases.into_iter().enumerate() {
        let router = emulator();
        let created = create_application(&router, rich_create(&format!("tok-state-{index}"))).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateApplication expected 200, got {} with {:?}",
            created.status, created.body
        );
        let id = created
            .at("/applicationId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        if !actions.is_empty() {
            wait_for_transition();
        }

        for (step, action) in actions.iter().enumerate() {
            let answer = match *action {
                "delete" => delete_application(&router, &id).await,
                other => {
                    call(&router, "POST", &format!("/applications/{id}/{other}"), None).await
                }
            };
            assert_eq!(
                answer.status, 200,
                "case {case:?}: step {step} ({action}) expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            if step + 1 < actions.len() || settle_last {
                wait_for_transition();
            }
        }
        if actions.is_empty() && settle_last {
            wait_for_transition();
        }

        assert_update_precondition(&router, &id, case, settled_state, index).await;
    }
}

/// One row of rule 2: the update is admitted exactly when the settled state is one the
/// operation documents, and a refused update leaves the record alone.
///
/// The member named is outside the running-application carve-out, so a `STARTED`
/// application refuses it too — with the refusal that states which members it would
/// have admitted rather than the one that states the precondition.
async fn assert_update_precondition(
    router: &MultiServiceRouter,
    id: &str,
    case: &str,
    settled_state: &str,
    index: usize,
) {
    let admitted = UPDATABLE_STATES.contains(&settled_state);
    let before = get_application(router, id).await;
    let answered = update_application(
        router,
        id,
        update_body(
            &format!("state-{index}"),
            &json!({ "autoStartConfiguration": { "enabled": true } }),
        ),
    )
    .await;

    let expected_status = if admitted { 200 } else { 400 };
    assert_eq!(
        answered.status, expected_status,
        "case {case:?}: an update with settled state {settled_state} expected \
         {expected_status}, got {} with {:?}",
        answered.status, answered.body
    );

    if admitted {
        assert_eq!(
            answered.at("/application/autoStartConfiguration"),
            json!({ "enabled": true }),
            "case {case:?}: the admitted update must apply the member it named, got {:?}",
            answered.at("/application/autoStartConfiguration")
        );
        return;
    }

    assert_eq!(
        answered.error_code(),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {:?}",
        answered.body
    );
    let message = answered.body["message"].as_str().unwrap_or_default();
    if settled_state == "STARTED" {
        // A running application refuses this member because it is not one of the
        // members an update may name while it runs, so the refusal enumerates those.
        assert_eq!(
            message, RUNNING_CARVE_OUT_REFUSAL,
            "case {case:?}: a running application must refuse a member outside the \
             carve-out with the enumeration of it, got {message:?}"
        );
    } else {
        // Every other state refuses the whole update: the refusal states the condition
        // it refused on — the state the application is in and the states it admits.
        assert!(
            message.contains(settled_state)
                && UPDATABLE_STATES.iter().all(|state| message.contains(state)),
            "case {case:?}: the refusal must state the state it refused on and the states it \
             admits, got {message:?}"
        );
    }
    // And nothing was written: the member the refused update named is as it was.
    let after = get_application(router, id).await;
    assert_eq!(
        after.at("/application/autoStartConfiguration"),
        before.at("/application/autoStartConfiguration"),
        "case {case:?}: a refused update must change nothing, got {:?} against {:?}",
        after.at("/application/autoStartConfiguration"),
        before.at("/application/autoStartConfiguration")
    );
}

/// Rule 2, continued: a release label the application's engine type is not served at is
/// refused, and the record keeps the label it had. The refusal names the RELEASE LABEL:
/// the engine type the application carries is this service's own canonical value, so it
/// is never the member at fault.
///
/// Source: "EMR Serverless is available with Amazon EMR releases 6.6.0 and later", so
/// `emr-6.6.0` is the published bound and `emr-6.5.0` is one past it.
#[tokio::test]
async fn an_update_refuses_a_release_label_the_applications_engine_is_not_served_at() {
    /// The phrase a refusal of the release label carries, written out here rather than
    /// read from `crate::state` so a wrong constant in `src/` cannot make this pass.
    const UNSUPPORTED_RELEASE_LABEL: &str =
        "Type is not supported for the release label or release label does not exist";

    // (case, the release label the update asks for, the expected status)
    let cases: &[(&str, &str, u16)] = &[
        ("the earliest release that serves the engine", "emr-6.6.0", 200),
        ("a later release", "emr-7.2.0", 200),
        ("a release carrying a build suffix", "emr-6.6.0-20220101", 200),
        ("a label with no readable version, which names no release", "latest", 400),
        ("one release below the earliest", "emr-6.5.0", 400),
        ("a much older release", "emr-5.36.0", 400),
    ];

    for (index, (case, release_label, expected_status)) in cases.iter().enumerate() {
        let router = emulator();
        let id = settled_application(&router, &format!("tok-release-{index}")).await;
        let before = get_application(&router, &id)
            .await
            .at("/application/releaseLabel");

        let answered = update_application(
            &router,
            &id,
            update_body(
                &format!("rel-{index}"),
                &json!({ "releaseLabel": release_label }),
            ),
        )
        .await;
        assert_eq!(
            answered.status, *expected_status,
            "case {case:?}: UpdateApplication({release_label:?}) expected {expected_status}, \
             got {} with {:?}",
            answered.status, answered.body
        );

        let after = get_application(&router, &id)
            .await
            .at("/application/releaseLabel");
        if *expected_status == 200 {
            assert_eq!(
                after,
                json!(release_label),
                "case {case:?}: the accepted update must store the release label, got {after:?}"
            );
        } else {
            assert_eq!(
                answered.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                answered.body
            );
            assert_eq!(
                answered.body["message"].as_str().unwrap_or_default(),
                UNSUPPORTED_RELEASE_LABEL,
                "case {case:?}: the refusal must name the release label, got {:?}",
                answered.body
            );
            assert_eq!(
                after, before,
                "case {case:?}: a refused update must keep the stored release label \
                 {before:?}, got {after:?}"
            );
        }
    }
}

/// Rule 3: an application that does not exist is refused with the fixed not-found
/// phrase for its resource class, whatever id was asked for.
///
/// The rows differ along the identifier's length (1, 12 and `ApplicationId`'s modelled
/// maximum of 64) and characters, because the rule generalises over exactly that.
#[tokio::test]
async fn an_update_of_an_unknown_application_is_refused_as_not_found() {
    let router = emulator();
    for unknown in ["7", "doesnotexist", "0".repeat(64).leak()] {
        let answered = update_application(
            &router,
            unknown,
            update_body(
                &format!("missing-{}", unknown.len()),
                &json!({ "maximumCapacity": { "cpu": "1 vCPU", "memory": "4 GB" } }),
            ),
        )
        .await;
        assert_eq!(
            answered.status, 404,
            "case {unknown:?}: expected 404, got {} with {:?}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.error_code(),
            "ResourceNotFoundException",
            "case {unknown:?}: expected ResourceNotFoundException, got {:?}",
            answered.body
        );
        assert!(
            !answered.body.to_string().contains(unknown),
            "case {unknown:?}: the requested id must not be echoed, got {:?}",
            answered.body
        );
    }
}
