// SPDX-License-Identifier: CC0-1.0

//! How AWS Backup REFUSES an audit-framework call, and what a refusal says.
//!
//! The rules under test:
//!
//! * a `FrameworkControls` entry naming a control Backup Audit Manager does not
//!   support is refused 400 `InvalidParameterValueException` before anything is
//!   stored, and the refusal names the unsupported control taken from the
//!   request — while every control the published catalog does name is accepted
//!   and read back;
//! * a framework-scoped refusal names the resource it is about: the read of a
//!   name this account holds no framework under names that name and the account
//!   the call was made in, the create that cannot add a framework names the
//!   account and the name the create asked for, and the mutation refused
//!   because a previous action on the framework is still deploying names the
//!   framework and the account;
//! * a deleted framework is GONE: `DeleteFramework` removes the record, so every
//!   later read of the name is refused exactly as a name that was never created
//!   is, a second delete is refused the same way, and the name is free for a
//!   later create to take;
//! * a delete that arrives while the framework's deployment is still in flight
//!   is refused 409 `ConflictException` and leaves the framework readable.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! status, the `x-amzn-errortype` header and the body a `backup` SDK client
//! parses. The request harness is the shared one in `vault_and_backup_job.rs`
//! and the framework helpers are the ones in `framework.rs`, so there is ONE
//! wire path for this service's suites.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backup_cryocontrollerusermanager::state::SUPPORTED_FRAMEWORK_CONTROL_NAMES;

use crate::framework::{
    create_framework, framework_name_of_length, text, DEPLOYMENT_COMPLETED,
    MAX_FRAMEWORK_NAME_LENGTH, PAST_FRAMEWORK_SETTLE,
};
use crate::vault_and_backup_job::{call_reported, error_code, router, ACCOUNT};

/// `POST /audit/frameworks` with the error shape the answer NAMES to a client
/// beside its status and body.
async fn create_reported(router: &MultiServiceRouter, request: &Value) -> (u16, String, Value) {
    call_reported(
        router,
        "POST",
        "/audit/frameworks",
        Some(&request.to_string()),
    )
    .await
}

/// `GET /audit/frameworks/{FrameworkName}`, reported the same way.
async fn describe_reported(router: &MultiServiceRouter, name: &str) -> (u16, String, Value) {
    call_reported(router, "GET", &format!("/audit/frameworks/{name}"), None).await
}

/// `DELETE /audit/frameworks/{FrameworkName}` — `DeleteFramework`'s HTTP
/// binding, reported the same way.
///
/// `pub(crate)` so a sibling suite in this test binary drives the same one wire
/// path rather than growing a second copy of it.
pub(crate) async fn delete_framework(
    router: &MultiServiceRouter,
    name: &str,
) -> (u16, String, Value) {
    call_reported(router, "DELETE", &format!("/audit/frameworks/{name}"), None).await
}

/// How AWS words "the framework asked about is not one this account holds": the
/// fixed text around the name the call addressed and the account it was made in.
fn not_found_message(name: &str) -> String {
    format!("Framework {name} not found in account {ACCOUNT}.")
}

/// How AWS words "this account already holds a framework the one being created
/// would duplicate".
fn already_exists_message(name: &str) -> String {
    format!(
        "A framework with these controls and configurations already exists in this account \
         {ACCOUNT}. To create new framework {name}, change the number or scope of the controls."
    )
}

/// How AWS words "a control the request names is not one Backup Audit Manager
/// supports".
fn unsupported_control_message(control_name: &str) -> String {
    format!("You can't create the framework control {control_name} because it is unsupported.")
}

/// How AWS words "a previous action on this framework has not finished".
fn concurrent_update_message(name: &str) -> String {
    format!(
        "Operation failed because of a concurrent update for the framework {name} in account \
         {ACCOUNT}. Try again later."
    )
}

/// One `FrameworkControls` entry naming `control`.
fn control(control_name: &str) -> Value {
    json!({"ControlName": control_name})
}

/// The rows the "a control the service does not support is refused" table
/// walks. They differ in the SPELLING of the unsupported value (a name no
/// control carries, the lower-case form of a supported one, a supported one with
/// a suffix, the AWS Config rule id spelling, a one-character name) and in WHERE
/// in the list the unsupported entry sits — which is what the rule generalises
/// over.
///
/// The third element is the control the refusal must name: the unsupported value
/// taken verbatim from the request.
fn unsupported_control_cases() -> Vec<(&'static str, Value, &'static str)> {
    vec![
        (
            "a name no Backup control carries, as the only control",
            json!([control("NOT_A_REAL_BACKUP_CONTROL")]),
            "NOT_A_REAL_BACKUP_CONTROL",
        ),
        (
            "the lower-case spelling of a supported control",
            json!([control("backup_recovery_point_encrypted")]),
            "backup_recovery_point_encrypted",
        ),
        (
            "a supported control name carrying a suffix",
            json!([control("BACKUP_RECOVERY_POINT_ENCRYPTED_V2")]),
            "BACKUP_RECOVERY_POINT_ENCRYPTED_V2",
        ),
        (
            "the Config rule id spelling rather than the control name",
            json!([control("backup-recovery-point-encrypted")]),
            "backup-recovery-point-encrypted",
        ),
        (
            "a one-character control name",
            json!([control("X")]),
            "X",
        ),
        (
            "an unsupported control after a supported one",
            json!([
                control("BACKUP_RECOVERY_POINT_ENCRYPTED"),
                control("BACKUP_PLAN_FREQUENCY_CHECK"),
            ]),
            "BACKUP_PLAN_FREQUENCY_CHECK",
        ),
        (
            "an unsupported control last of three, carrying parameters and a scope",
            json!([
                control("BACKUP_RECOVERY_POINT_ENCRYPTED"),
                control("BACKUP_LAST_RECOVERY_POINT_CREATED"),
                {
                    "ControlName": "BACKUP_RESOURCES_PROTECTED_BY_TAG",
                    "ControlInputParameters": [
                        {"ParameterName": "requiredRetentionDays", "ParameterValue": "35"},
                    ],
                    "ControlScope": {"ComplianceResourceTypes": ["EBS"]},
                },
            ]),
            "BACKUP_RESOURCES_PROTECTED_BY_TAG",
        ),
    ]
}

/// A create naming a control the service does not support is refused with the
/// modelled exception for a bad parameter value, the refusal names the control
/// the request carried, and nothing is stored under the name the create asked
/// for.
#[tokio::test]
async fn an_unsupported_control_name_is_refused_and_stores_nothing() {
    for (case, controls, named) in unsupported_control_cases() {
        // One router per row: the row's account holds nothing another row stored.
        let router = router();
        let name = "ProbedFramework";
        let (status, errortype, refused) = create_reported(
            &router,
            &json!({"FrameworkName": name, "FrameworkControls": controls}),
        )
        .await;
        assert_eq!(
            (status, errortype.as_str(), error_code(&refused).as_str()),
            (400, "InvalidParameterValueException", "InvalidParameterValueException"),
            "case {case:?}: expected 400 InvalidParameterValueException, got {status} \
             {errortype} {refused:?}"
        );
        assert_eq!(
            text(&refused, "Message"),
            unsupported_control_message(named),
            "case {case:?}: the refusal must name the unsupported control, got {refused:?}"
        );

        let (status, errortype, missing) = describe_reported(&router, name).await;
        assert_eq!(
            (status, errortype.as_str(), text(&missing, "Message").as_str()),
            (
                400,
                "ResourceNotFoundException",
                not_found_message(name).as_str()
            ),
            "case {case:?}: a refused create must store nothing, got {status} {missing:?}"
        );
    }
}

/// Every control the published Backup Audit Manager catalog names is accepted
/// and reported back by a read of the framework — the refusal above is a
/// catalog check, not a blanket refusal.
#[tokio::test]
async fn every_supported_control_name_is_accepted_and_read_back() {
    let router = router();
    for (index, supported) in SUPPORTED_FRAMEWORK_CONTROL_NAMES.iter().enumerate() {
        let name = format!("Catalog_{index}");
        let (status, created) = create_framework(
            &router,
            &json!({"FrameworkName": &name, "FrameworkControls": [control(supported)]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "control {supported:?}: expected CreateFramework 200, got {status} {created:?}"
        );

        let (status, _errortype, read) = describe_reported(&router, &name).await;
        assert_eq!(
            (status, read.get("FrameworkControls")),
            (200, Some(&json!([control(supported)]))),
            "control {supported:?}: expected the stored control back, got {status} {read:?}"
        );
    }
}

/// A read of a name this account holds no framework under is refused with the
/// sentence that names the framework asked about and the account the call was
/// made in.
#[tokio::test]
async fn a_read_of_an_unheld_name_names_the_framework_and_the_account() {
    let router = router();
    let (status, created) = create_framework(
        &router,
        &json!({
            "FrameworkName": "HeldByTheAccount",
            "FrameworkControls": [control("BACKUP_RECOVERY_POINT_ENCRYPTED")],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateFramework 200, got {status} {created:?}"
    );

    // The rows differ in the shape of the absent name: one never created, the
    // case-variant of a held one, the shortest and the longest the model admits.
    let absent = [
        "NeverCreated".to_string(),
        "heldbytheaccount".to_string(),
        framework_name_of_length(1),
        framework_name_of_length(MAX_FRAMEWORK_NAME_LENGTH),
    ];
    for name in &absent {
        let (status, errortype, missing) = describe_reported(&router, name).await;
        assert_eq!(
            (status, errortype.as_str(), error_code(&missing).as_str()),
            (400, "ResourceNotFoundException", "ResourceNotFoundException"),
            "name {name:?}: expected 400 ResourceNotFoundException, got {status} \
             {errortype} {missing:?}"
        );
        assert_eq!(
            text(&missing, "Message"),
            not_found_message(name),
            "name {name:?}: the refusal must name the framework and the account, got {missing:?}"
        );
    }
}

/// A create this account already holds the framework for is refused with AWS's
/// two sentences: the conflict is attributed to the controls and configuration,
/// and the remediation names the framework the create asked for.
#[tokio::test]
async fn a_create_of_a_held_framework_states_the_controls_and_configuration_conflict() {
    // The rows differ in the name (a plain one, one at the longest length the
    // model admits) and in how many controls the framework carries.
    let cases: Vec<(&str, String, Value)> = vec![
        (
            "one control",
            "DuplicatedFramework".to_string(),
            json!([control("BACKUP_RECOVERY_POINT_ENCRYPTED")]),
        ),
        (
            "three controls",
            "ManyControlFramework".to_string(),
            json!([
                control("BACKUP_RECOVERY_POINT_MINIMUM_RETENTION_CHECK"),
                control("BACKUP_RECOVERY_POINT_MANUAL_DELETION_DISABLED"),
                control("BACKUP_RESOURCES_PROTECTED_BY_BACKUP_VAULT_LOCK"),
            ]),
        ),
        (
            "a name at the longest length the model admits",
            framework_name_of_length(MAX_FRAMEWORK_NAME_LENGTH),
            json!([control("BACKUP_LAST_RECOVERY_POINT_CREATED")]),
        ),
    ];

    for (case, name, controls) in cases {
        let router = router();
        let request = json!({"FrameworkName": &name, "FrameworkControls": controls});
        let (status, created) = create_framework(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first CreateFramework 200, got {status} {created:?}"
        );

        let (status, errortype, refused) = create_reported(&router, &request).await;
        assert_eq!(
            (status, errortype.as_str(), error_code(&refused).as_str()),
            (400, "AlreadyExistsException", "AlreadyExistsException"),
            "case {case:?}: expected 400 AlreadyExistsException, got {status} \
             {errortype} {refused:?}"
        );
        assert_eq!(
            text(&refused, "Message"),
            already_exists_message(&name),
            "case {case:?}: the refusal must state AWS's conflict sentences, got {refused:?}"
        );

        // The refused create changed nothing about the framework it collided with.
        let (status, _errortype, read) = describe_reported(&router, &name).await;
        assert_eq!(
            (status, read.get("FrameworkArn")),
            (200, created.get("FrameworkArn")),
            "case {case:?}: a refused create must change nothing, got {status} {read:?}"
        );
    }
}

/// A delete removes the framework: the read of its name is then refused exactly
/// as a name that was never created is, a second delete is refused the same
/// way, and a later create may take the name again.
#[tokio::test]
async fn a_deleted_framework_is_unreadable_and_its_name_is_free_again() {
    // The rows differ in how many controls the deleted framework carried and in
    // whether it had a description, so the delete is not keyed to one shape of
    // framework.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "one control, no description",
            "DeletedFramework",
            json!({
                "FrameworkName": "DeletedFramework",
                "FrameworkControls": [control("BACKUP_RECOVERY_POINT_ENCRYPTED")],
            }),
        ),
        (
            "two controls and a description",
            "DescribedFramework",
            json!({
                "FrameworkName": "DescribedFramework",
                "FrameworkDescription": "deleted while this suite runs",
                "FrameworkControls": [
                    control("BACKUP_RESOURCES_PROTECTED_BY_BACKUP_PLAN"),
                    control("BACKUP_PLAN_MIN_FREQUENCY_AND_MIN_RETENTION_CHECK"),
                ],
            }),
        ),
    ];

    for (case, name, request) in cases {
        let router = router();
        let (status, created) = create_framework(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateFramework 200, got {status} {created:?}"
        );

        // The deployment the create owes has to have settled before a delete is
        // allowed to run, which this suite's conflict case pins.
        std::thread::sleep(PAST_FRAMEWORK_SETTLE);
        let (status, _errortype, deployed) = describe_reported(&router, name).await;
        assert_eq!(
            (status, text(&deployed, "DeploymentStatus").as_str()),
            (200, DEPLOYMENT_COMPLETED),
            "case {case:?}: expected a settled deployment before the delete, got {deployed:?}"
        );

        let (status, _errortype, deleted) = delete_framework(&router, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteFramework 200, got {status} {deleted:?}"
        );

        // The read of a deleted name is the refusal a never-created name gets.
        let (status, errortype, missing) = describe_reported(&router, name).await;
        assert_eq!(
            (
                status,
                errortype.as_str(),
                text(&missing, "Message").as_str()
            ),
            (
                400,
                "ResourceNotFoundException",
                not_found_message(name).as_str()
            ),
            "case {case:?}: a deleted framework must be unreadable, got {status} {missing:?}"
        );

        // So is a second delete of the same name.
        let (status, errortype, again) = delete_framework(&router, name).await;
        assert_eq!(
            (status, errortype.as_str(), text(&again, "Message").as_str()),
            (
                400,
                "ResourceNotFoundException",
                not_found_message(name).as_str()
            ),
            "case {case:?}: a second delete must be refused, got {status} {again:?}"
        );

        // The name is free again, and the framework created under it is the new
        // one rather than the deleted one.
        let (status, recreated) = create_framework(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the name must be free after the delete, got {status} {recreated:?}"
        );
        assert_ne!(
            recreated.get("FrameworkArn"),
            created.get("FrameworkArn"),
            "case {case:?}: the re-created framework is a new resource, got {recreated:?}"
        );
        let (status, _errortype, read) = describe_reported(&router, name).await;
        assert_eq!(
            (status, read.get("FrameworkArn")),
            (200, recreated.get("FrameworkArn")),
            "case {case:?}: the re-created framework reads back, got {status} {read:?}"
        );
    }
}

/// A delete that arrives while the framework's deployment is still in flight is
/// refused with the sentence naming the framework and the account, and leaves
/// the framework readable; once the clock has settled the deployment the same
/// delete succeeds.
#[tokio::test]
async fn a_delete_during_the_deployment_is_refused_and_keeps_the_framework() {
    let router = router();
    let name = "DeployingFramework";
    let request = json!({
        "FrameworkName": name,
        "FrameworkControls": [control("BACKUP_RECOVERY_POINT_ENCRYPTED")],
    });
    let (status, created) = create_framework(&router, &request).await;
    assert_eq!(
        status, 200,
        "setup: expected CreateFramework 200, got {status} {created:?}"
    );

    let (status, errortype, refused) = delete_framework(&router, name).await;
    assert_eq!(
        (status, errortype.as_str(), error_code(&refused).as_str()),
        (409, "ConflictException", "ConflictException"),
        "expected 409 ConflictException while the deployment runs, got {status} \
         {errortype} {refused:?}"
    );
    assert_eq!(
        text(&refused, "Message"),
        concurrent_update_message(name),
        "the refusal must name the framework and the account, got {refused:?}"
    );

    // The refused delete left the framework exactly as it was.
    let (status, _errortype, read) = describe_reported(&router, name).await;
    assert_eq!(
        (status, read.get("FrameworkArn")),
        (200, created.get("FrameworkArn")),
        "a refused delete must keep the framework, got {status} {read:?}"
    );

    std::thread::sleep(PAST_FRAMEWORK_SETTLE);
    let (status, _errortype, deleted) = delete_framework(&router, name).await;
    assert_eq!(
        status, 200,
        "expected the delete to succeed once the deployment settled, got {status} {deleted:?}"
    );
    let (status, errortype, missing) = describe_reported(&router, name).await;
    assert_eq!(
        (status, errortype.as_str(), text(&missing, "Message").as_str()),
        (
            400,
            "ResourceNotFoundException",
            not_found_message(name).as_str()
        ),
        "the deleted framework must be unreadable, got {status} {missing:?}"
    );
}
