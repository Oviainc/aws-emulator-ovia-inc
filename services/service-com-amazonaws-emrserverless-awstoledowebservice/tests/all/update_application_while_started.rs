// SPDX-License-Identifier: CC0-1.0

//! `UpdateApplication` on a RUNNING application, end to end over HTTP.
//!
//! Every case goes through the real router (restJson1, SigV4 signing name
//! `emr-serverless`), so each one exercises routing, deserialization, the handler, the
//! persisted record and serialization — the same path an SDK client takes.
//!
//! ONE rule is under test, asserted over a SET of inputs rather than over one recorded
//! request: a state precondition on an update is judged per member path the request
//! NAMES, not per request. EMR Serverless documents a subset of configuration members
//! as updatable while an application is `STARTED`, so an update of a running
//! application that names only paths inside that subset is applied (HTTP 200, the
//! application stays `STARTED`, the member is persisted), and one that names ANY path
//! outside it is refused with the modelled `ValidationException` (HTTP 400) enumerating
//! the subset, with nothing written.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::application_lifecycle::{
    emulator, get_application, start_application, wait_for_transition,
};
use super::update_application::{settled_application, update_application, update_body, IMAGE_URI};

/// The refusal a running application answers an update naming a member outside the
/// carve-out with, written out here rather than read from `crate::state` so a wrong
/// constant in `src/` cannot make this test pass.
///
/// Source: the `ValidationException` EMR Serverless answers such an update with, which
/// enumerates the set it admits.
pub(crate) const RUNNING_CARVE_OUT_REFUSAL: &str =
    "Only the following configurations can be updated when an application is in STARTED \
     state: schedulerConfiguration.maxConcurrentRuns, extendedSupportConfiguration, \
     imageConfiguration, workerTypeSpecifications.imageConfiguration, maximumCapacity.";

/// Create the baseline application, start it, and let the start settle, so the update
/// under test arrives at an application that has settled on `STARTED`.
async fn running_application(router: &MultiServiceRouter, token: &str) -> String {
    let id = settled_application(router, token).await;
    let started = start_application(router, &id).await;
    assert_eq!(
        started.status, 200,
        "case {token:?}: StartApplication expected 200, got {} with {:?}",
        started.status, started.body
    );
    wait_for_transition();
    let state = get_application(router, &id).await.at("/application/state");
    assert_eq!(
        state,
        json!("STARTED"),
        "case {token:?}: the application must have settled on STARTED, got {state:?}"
    );
    id
}

/// The whole stored application, as `GetApplication` reports it.
async fn stored_application(router: &MultiServiceRouter, id: &str) -> Value {
    get_application(router, id).await.at("/application")
}

/// An accepted update of a running application: the paths the carve-out names.
///
/// The rows differ along what the rule generalises over — WHICH path is named, the
/// member's type (structure, map, nested number), the number of paths named at once,
/// the size of the map named (one and two worker types), the values inside it, and the
/// published bounds of a nested range — because the rule turns on the member path
/// alone.
#[tokio::test]
async fn a_running_application_admits_an_update_of_the_members_the_service_documents() {
    // (case, the members the update names, the members the answer must report)
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "no configuration member beyond the required client token",
            json!({}),
            json!({}),
        ),
        (
            "the maximum capacity alone",
            json!({ "maximumCapacity": { "cpu": "8 vCPU", "memory": "16 GB" } }),
            json!({ "maximumCapacity": { "cpu": "8 vCPU", "memory": "16 GB" } }),
        ),
        (
            "a maximum capacity carrying a disk, an order of magnitude larger",
            json!({
                "maximumCapacity": { "cpu": "400 vCPU", "memory": "3000 GB", "disk": "20000 GB" },
            }),
            json!({
                "maximumCapacity": { "cpu": "400 vCPU", "memory": "3000 GB", "disk": "20000 GB" },
            }),
        ),
        (
            "the image configuration for all worker types",
            json!({ "imageConfiguration": { "imageUri": IMAGE_URI } }),
            json!({ "imageConfiguration": { "imageUri": IMAGE_URI } }),
        ),
        (
            // `ImageConfigurationInput$imageUri` documents it: "If you leave this field
            // blank in an update, Amazon EMR will remove the image configuration."
            "an image configuration naming no image, which removes it",
            json!({ "imageConfiguration": {} }),
            json!({}),
        ),
        (
            "the image configuration of one worker type",
            json!({
                "workerTypeSpecifications": {
                    "Executor": { "imageConfiguration": { "imageUri": IMAGE_URI } },
                },
            }),
            json!({
                "workerTypeSpecifications": {
                    "Executor": { "imageConfiguration": { "imageUri": IMAGE_URI } },
                },
            }),
        ),
        (
            "the image configuration of two worker types",
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
            // `SchedulerConfiguration$maxConcurrentRuns`: "The valid range is 1 to
            // 1000." The block's OTHER member is outside the carve-out, so the stored
            // `queueTimeoutMinutes` (360, from the create) stays exactly where it was.
            "the maximum concurrent runs at the bottom of its published range",
            json!({ "schedulerConfiguration": { "maxConcurrentRuns": 1 } }),
            json!({ "schedulerConfiguration": { "maxConcurrentRuns": 1, "queueTimeoutMinutes": 360 } }),
        ),
        (
            "the maximum concurrent runs at the top of its published range",
            json!({ "schedulerConfiguration": { "maxConcurrentRuns": 1000 } }),
            json!({
                "schedulerConfiguration": { "maxConcurrentRuns": 1000, "queueTimeoutMinutes": 360 },
            }),
        ),
        (
            // The block is named with NEITHER sub-member: the one path of it the
            // carve-out admits is not named, so nothing inside it moves and the stored
            // block is reported unchanged (`maxConcurrentRuns` 15, `queueTimeoutMinutes`
            // 360, from the create).
            "the scheduler configuration naming neither of its members",
            json!({ "schedulerConfiguration": {} }),
            json!({
                "schedulerConfiguration": { "maxConcurrentRuns": 15, "queueTimeoutMinutes": 360 },
            }),
        ),
        (
            "every path of the carve-out at once",
            json!({
                "maximumCapacity": { "cpu": "2 vCPU", "memory": "8 GB" },
                "imageConfiguration": { "imageUri": IMAGE_URI },
                "workerTypeSpecifications": {
                    "Driver": { "imageConfiguration": { "imageUri": IMAGE_URI } },
                },
                "schedulerConfiguration": { "maxConcurrentRuns": 42 },
            }),
            json!({
                "maximumCapacity": { "cpu": "2 vCPU", "memory": "8 GB" },
                "imageConfiguration": { "imageUri": IMAGE_URI },
                "workerTypeSpecifications": {
                    "Driver": { "imageConfiguration": { "imageUri": IMAGE_URI } },
                },
                "schedulerConfiguration": { "maxConcurrentRuns": 42, "queueTimeoutMinutes": 360 },
            }),
        ),
    ];

    for (index, (case, members, expected)) in cases.into_iter().enumerate() {
        let router = emulator();
        let id = running_application(&router, &format!("tok-running-{index}")).await;

        let answered =
            update_application(&router, &id, update_body(&format!("run-{index}"), &members)).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: UpdateApplication on a STARTED application expected 200, got {} \
             with {:?}",
            answered.status, answered.body
        );

        for (member, value) in expected.as_object().into_iter().flatten() {
            assert_eq!(
                answered.at(&format!("/application/{member}")),
                *value,
                "case {case:?}: the accepted update must report {member} as {value:?}, got {:?}",
                answered.at(&format!("/application/{member}"))
            );
        }
        if members
            .get("imageConfiguration")
            .is_some_and(|given| given.get("imageUri").is_none())
        {
            assert_eq!(
                answered.at("/application/imageConfiguration"),
                Value::Null,
                "case {case:?}: an update naming no image removes the image configuration, \
                 got {:?}",
                answered.at("/application/imageConfiguration")
            );
        }

        // The application KEEPS running across the update: resizing a running
        // application is what the carve-out exists for, so it does not stop it.
        assert_eq!(
            answered.at("/application/state"),
            json!("STARTED"),
            "case {case:?}: the application must stay STARTED across the update, got {:?}",
            answered.at("/application/state")
        );

        // ROUND TRIP: the update is persisted, so the reader reports exactly what the
        // update answered.
        let read_back = stored_application(&router, &id).await;
        assert_eq!(
            read_back,
            answered.at("/application"),
            "case {case:?}: GetApplication must report the updated record, got {read_back:?} \
             against {:?}",
            answered.at("/application")
        );
    }
}

/// The same rule, from the other side: every configuration member of
/// `UpdateApplicationRequest` that the carve-out does NOT name is refused while the
/// application is running, and the refused update writes nothing.
///
/// The catalog of members is
/// `smithy_query(query="operation",operation="UpdateApplication")`'s input, less
/// `applicationId` and `clientToken` (not configuration) and less the paths the
/// carve-out admits; the last row mixes an admitted path with a refused one, because
/// the rule is about every path the request names rather than about any one of them.
#[tokio::test]
async fn a_running_application_refuses_an_update_of_every_other_member() {
    // (case, the members the update names)
    let cases: Vec<(&str, Value)> = vec![
        ("the release label", json!({ "releaseLabel": "emr-7.2.0" })),
        ("the architecture", json!({ "architecture": "ARM64" })),
        (
            "the initial capacity",
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
            "the auto-start configuration",
            json!({ "autoStartConfiguration": { "enabled": true } }),
        ),
        (
            "the auto-stop configuration",
            json!({ "autoStopConfiguration": { "enabled": true, "idleTimeoutMinutes": 30 } }),
        ),
        (
            "the network configuration",
            json!({
                "networkConfiguration": {
                    "subnetIds": ["subnet-0000000000000001"],
                    "securityGroupIds": ["sg-0000000000000001"],
                },
            }),
        ),
        (
            "the runtime configuration",
            json!({
                "runtimeConfiguration": [
                    { "classification": "spark-defaults", "properties": {} },
                ],
            }),
        ),
        (
            "the interactive configuration",
            json!({ "interactiveConfiguration": { "studioEnabled": true } }),
        ),
        (
            "the monitoring configuration",
            json!({
                "monitoringConfiguration": {
                    "managedPersistenceMonitoringConfiguration": { "enabled": true },
                },
            }),
        ),
        (
            "the disk encryption configuration",
            json!({
                "diskEncryptionConfiguration": {
                    "encryptionKeyArn":
                        "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-0000-0000-000000000001",
                },
            }),
        ),
        (
            "the identity center configuration",
            json!({
                "identityCenterConfiguration": {
                    "identityCenterInstanceArn":
                        "arn:aws:sso:::instance/ssoins-0000000000000001",
                },
            }),
        ),
        (
            "the job level cost allocation configuration",
            json!({ "jobLevelCostAllocationConfiguration": { "enabled": true } }),
        ),
        (
            // The block is in the carve-out only through `maxConcurrentRuns`, so its
            // other member is refused even though the block itself can be named.
            "the queue timeout inside the scheduler configuration",
            json!({ "schedulerConfiguration": { "queueTimeoutMinutes": 15 } }),
        ),
        (
            "the queue timeout alongside the maximum concurrent runs",
            json!({
                "schedulerConfiguration": { "queueTimeoutMinutes": 15, "maxConcurrentRuns": 7 },
            }),
        ),
        (
            "an admitted member alongside a refused one",
            json!({
                "maximumCapacity": { "cpu": "8 vCPU", "memory": "16 GB" },
                "releaseLabel": "emr-7.2.0",
            }),
        ),
    ];

    for (index, (case, members)) in cases.into_iter().enumerate() {
        let router = emulator();
        let id = running_application(&router, &format!("tok-refused-{index}")).await;
        let before = stored_application(&router, &id).await;

        let answered = update_application(
            &router,
            &id,
            update_body(&format!("ref-{index}"), &members),
        )
        .await;
        assert_eq!(
            answered.status, 400,
            "case {case:?}: UpdateApplication on a STARTED application expected 400, got {} \
             with {:?}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answered.body
        );
        // The refusal enumerates the set it admits — the rule itself, so it names
        // nothing the request carried.
        assert_eq!(
            answered.body["message"].as_str().unwrap_or_default(),
            RUNNING_CARVE_OUT_REFUSAL,
            "case {case:?}: the refusal must enumerate the members a STARTED application \
             admits, got {:?}",
            answered.body
        );

        // A refused update writes NOTHING: not the member outside the carve-out, not
        // the admitted member a mixed request named alongside it, and not `updatedAt` —
        // the refusal is answered before the record is stamped, so the WHOLE record is
        // the one the start left behind.
        let after = stored_application(&router, &id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must change nothing, got {after:?} against \
             {before:?}"
        );
        assert_eq!(
            after.get("state").cloned().unwrap_or(Value::Null),
            json!("STARTED"),
            "case {case:?}: a refused update must leave the application STARTED, got {:?}",
            after.get("state")
        );
    }
}
