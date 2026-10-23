// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's `StopNotebookInstance`, read back through
//! `DescribeNotebookInstance`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets. The harness is this crate's
//! shared one, from `tests/all/inference_deployment_lifecycle.rs`.
//!
//! The tests assert the RULE over a TABLE of inputs rather than one recorded call: a
//! stop is accepted from every status where an ML compute instance exists to terminate
//! and moves the instance to `Stopping`, with `Stopped` owed by the clock; it is refused
//! from every other status and for a name the store does not hold; it preserves every
//! stored member (SageMaker AI preserves the ML storage volume); and it touches only the
//! instance it names.

use std::time::Duration;

use serde_json::{json, Value};

// The harness this crate's suites share, rather than another copy of it.
use super::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};
// The notebook-instance request shape is stated once, in the suite that created it.
use super::notebook_instance_lifecycle::{create_body, describe, PAST_IN_SERVICE_DUE};

/// Long enough to pass `state::NOTEBOOK_INSTANCE_STOPPED_AFTER` (300 ms).
const PAST_STOPPED_DUE: Duration = Duration::from_millis(600);

/// `StopNotebookInstance` for `name`.
async fn stop(emulator: &Emulator, name: &str) -> (u16, Value) {
    emulator
        .call("StopNotebookInstance", &json!({ "NotebookInstanceName": name }))
        .await
}

fn status_of(described: &Value) -> String {
    str_field(described, "NotebookInstanceStatus")
}

fn number(described: &Value, member: &str) -> f64 {
    described
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("{member} must be a stored timestamp, got {described:?}"))
}

/// RULE: a stop is accepted while an ML compute instance exists to terminate — the
/// instance is serving or still coming up — moves the instance to `Stopping` at once, and
/// the clock owes the move to `Stopped`; whatever the instance stored stays stored,
/// because the ML storage volume is preserved across a stop.
#[tokio::test]
async fn stopping_a_running_notebook_instance_moves_it_through_stopping_to_stopped() {
    // Rows differ along what the rule generalises over: the status the instance is in
    // when the stop lands (still provisioning vs. serving), the signed region, and how
    // much configuration the instance carries.
    let cases: Vec<(&str, bool, &str, Value)> = vec![
        ("stop-while-pending", false, "us-east-1", json!({})),
        ("stop-while-in-service", true, "us-east-1", json!({})),
        (
            "stop-a-configured-instance",
            true,
            "eu-west-1",
            json!({
                "SubnetId": "subnet-0abc1234",
                "SecurityGroupIds": ["sg-00000001"],
                "DirectInternetAccess": "Disabled",
                "VolumeSizeInGB": 120,
                "RootAccess": "Disabled",
                "LifecycleConfigName": "install-packages",
            }),
        ),
        (
            "stop-in-another-region",
            false,
            "ap-southeast-2",
            json!({ "VolumeSizeInGB": 16384 }),
        ),
    ];

    for (case, wait_for_in_service, region, extra) in cases {
        let emulator = Emulator::new();
        let (status, created) = emulator
            .call_in(region, "CreateNotebookInstance", &create_body(case, &extra))
            .await;
        assert_eq!(status, 200, "case {case}: CreateNotebookInstance {created}");
        if wait_for_in_service {
            tokio::time::sleep(PAST_IN_SERVICE_DUE).await;
        }

        let before = describe(&emulator, case).await;
        // A row that waited out the create's span must be serving; one that did not is
        // normally still `Pending`, but the rule holds from either status, so the
        // precondition is read rather than demanded of the clock.
        let came_up = status_of(&before) == "InService";
        if wait_for_in_service {
            assert!(
                came_up,
                "case {case}: expected InService after waiting out the create's span, got \
                 {before:?}"
            );
        } else {
            assert!(
                came_up || status_of(&before) == "Pending",
                "case {case}: expected Pending or InService before the stop, got {before:?}"
            );
        }

        let (status, stopped) = emulator
            .call_in(
                region,
                "StopNotebookInstance",
                &json!({ "NotebookInstanceName": case }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 from StopNotebookInstance, got {status} \
             {stopped}"
        );
        assert!(
            stopped.is_null() || stopped.as_object().is_some_and(|members| members.is_empty()),
            "case {case}: StopNotebookInstance reports no members, got {stopped:?}"
        );

        // The instance is `Stopping` until the span the clock owes has passed — read
        // rather than demanded of the clock, since a loaded box can let the span elapse
        // between two round trips. Whichever of the two it reports, the stop is in flight
        // or already settled, never still serving.
        let stopping = describe(&emulator, case).await;
        assert!(
            ["Stopping", "Stopped"].contains(&status_of(&stopping).as_str()),
            "case {case}: expected Stopping or Stopped straight after the stop, got \
             {stopping:?}"
        );
        // A read changes nothing: the same read answers identically.
        let again = describe(&emulator, case).await;
        if status_of(&stopping) == "Stopping" {
            assert_eq!(
                again, stopping,
                "case {case}: a second read of a stopping instance must answer identically"
            );
        }

        tokio::time::sleep(PAST_STOPPED_DUE).await;

        let settled = describe(&emulator, case).await;
        assert_eq!(
            status_of(&settled),
            "Stopped",
            "case {case}: expected Stopped once the clock owes it, got {settled:?}"
        );
        assert!(
            number(&settled, "LastModifiedTime") >= number(&stopping, "LastModifiedTime"),
            "case {case}: LastModifiedTime must move with the status, got {} then {}",
            number(&stopping, "LastModifiedTime"),
            number(&settled, "LastModifiedTime")
        );
        assert!(
            number(&settled, "LastModifiedTime") > number(&before, "LastModifiedTime"),
            "case {case}: LastModifiedTime must move past where the stop found it, got {} \
             then {}",
            number(&before, "LastModifiedTime"),
            number(&settled, "LastModifiedTime")
        );
        assert_eq!(
            settled.get("CreationTime"),
            before.get("CreationTime"),
            "case {case}: CreationTime must not move on a stop, got {settled:?}"
        );
        let settled_again = describe(&emulator, case).await;
        assert_eq!(
            settled_again, settled,
            "case {case}: a stopped instance must read back identically"
        );

        // The Jupyter endpoint and the network interface exist only for an instance that
        // actually came up, so one stopped while it was still provisioning reports
        // neither, before or after the stop settles.
        for member in ["Url", "NetworkInterfaceId"] {
            assert_eq!(
                settled.get(member).is_some(),
                came_up,
                "case {case}: {member} is reported only for an instance that came up, got \
                 {settled:?}"
            );
        }

        // The storage volume and the rest of the configuration are preserved, so every
        // member the create carried still reads back.
        assert_eq!(
            str_field(&settled, "NotebookInstanceArn"),
            str_field(&created, "NotebookInstanceArn"),
            "case {case}: the ARN must survive the stop, got {settled:?}"
        );
        for (sent, reported) in [
            ("InstanceType", "InstanceType"),
            ("RoleArn", "RoleArn"),
            ("SubnetId", "SubnetId"),
            ("SecurityGroupIds", "SecurityGroups"),
            ("DirectInternetAccess", "DirectInternetAccess"),
            ("VolumeSizeInGB", "VolumeSizeInGB"),
            ("RootAccess", "RootAccess"),
            ("LifecycleConfigName", "NotebookInstanceLifecycleConfigName"),
        ] {
            let request = create_body(case, &extra);
            if let Some(expected) = request.get(sent) {
                assert_eq!(
                    settled.get(reported),
                    Some(expected),
                    "case {case}: {reported} must survive the stop, got {settled:?}"
                );
            }
        }
    }
}

/// RULE: from a status with no ML compute instance to terminate, the stop is refused
/// with a 400 `ValidationException` that names the status it found and the instance it
/// refused, and the stored status stands.
#[tokio::test]
async fn stopping_an_instance_that_is_not_running_is_refused_and_changes_nothing() {
    // Rows: every status this slice can place an instance in besides the two stoppable
    // ones — `Stopping` (a stop already in flight) and `Stopped` (already terminated).
    let cases = [("second-stop-while-stopping", false), ("second-stop-while-stopped", true)];

    for (case, wait_out_the_stop) in cases {
        let emulator = Emulator::new();
        emulator
            .ok("CreateNotebookInstance", &create_body(case, &json!({})))
            .await;
        tokio::time::sleep(PAST_IN_SERVICE_DUE).await;
        let (status, body) = stop(&emulator, case).await;
        assert_eq!(status, 200, "case {case}: the first stop {status} {body}");
        if wait_out_the_stop {
            tokio::time::sleep(PAST_STOPPED_DUE).await;
        }

        let before = describe(&emulator, case).await;
        // Read the status rather than demanding it of the clock: without the wait it is
        // `Stopping`, unless a loaded box let the owed span elapse between round trips.
        let found = status_of(&before);
        if wait_out_the_stop {
            assert_eq!(
                found, "Stopped",
                "case {case}: expected Stopped once the stop's span passed, got {before:?}"
            );
        } else {
            assert!(
                ["Stopping", "Stopped"].contains(&found.as_str()),
                "case {case}: expected Stopping or Stopped before the second stop, got \
                 {before:?}"
            );
        }

        let (status, parsed) = stop(&emulator, case).await;
        assert_eq!(
            status, 400,
            "case {case}: a stop with nothing to terminate must be refused, got {status} \
             {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: error code, got {parsed:?}"
        );
        let refusal = message(&parsed);
        assert!(
            refusal.contains(&found)
                && refusal.contains(&format!("{ACCOUNT}:notebook-instance/{case}")),
            "case {case}: the refusal must name the status it found ({found}) and the \
             instance, got {refusal:?}"
        );

        let after = describe(&emulator, case).await;
        assert_eq!(
            after, before,
            "case {case}: a refused stop must leave the stored instance untouched"
        );
    }
}

/// RULE: a stop naming a `NotebookInstanceName` the store does not hold is refused with
/// a 400 `ValidationException` naming the ARN it could not find, in the region the
/// caller signed for.
#[tokio::test]
async fn stopping_an_absent_notebook_instance_is_refused_with_a_400_validation_exception() {
    // Rows: name shapes nothing created, across signed regions, plus a name that exists
    // in another emulator's store but not in this one.
    let longest = format!("{}b", "a".repeat(62));
    let names = ["absent", "a", "never-created-notebook", longest.as_str()];
    for region in ["us-east-1", "eu-central-1"] {
        for name in names {
            let emulator = Emulator::new();
            let case = format!("region={region} name={name}");
            let (status, parsed) = emulator
                .call_in(
                    region,
                    "StopNotebookInstance",
                    &json!({ "NotebookInstanceName": name }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400 for an absent instance, got {status} \
                 {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                "ValidationException",
                "case {case}: error code, got {parsed:?}"
            );
            assert!(
                message(&parsed).contains(&format!(
                    "arn:aws:sagemaker:{region}:{ACCOUNT}:notebook-instance/{name}"
                )),
                "case {case}: the refusal must name the instance it could not find, got \
                 {parsed:?}"
            );
        }
    }

    // An unsigned stop names no region, so the ARN it reports carries the emulator's
    // default one.
    let emulator = Emulator::new();
    let (status, parsed) = emulator
        .call_unsigned(
            "StopNotebookInstance",
            &json!({ "NotebookInstanceName": "absent-unsigned" }),
        )
        .await;
    assert_eq!(status, 400, "case unsigned: expected HTTP 400, got {status} {parsed}");
    assert!(
        message(&parsed).contains(&format!(
            "arn:aws:sagemaker:us-east-1:{ACCOUNT}:notebook-instance/absent-unsigned"
        )),
        "case unsigned: the refusal must fall back to the default region, got {parsed:?}"
    );
}

/// RULE: a stop resolves the one instance its `NotebookInstanceName` names, so every
/// other stored instance keeps the status it had.
#[tokio::test]
async fn a_stop_moves_only_the_instance_it_names() {
    let emulator = Emulator::new();
    // Rows: instances whose names share prefixes, so a name resolved by anything looser
    // than equality would catch the wrong one.
    let names = ["nb", "nb-one", "nb-one-extra", "other"];
    for name in names {
        emulator
            .ok("CreateNotebookInstance", &create_body(name, &json!({})))
            .await;
    }
    tokio::time::sleep(PAST_IN_SERVICE_DUE).await;

    let (status, body) = stop(&emulator, "nb-one").await;
    assert_eq!(status, 200, "case stop-nb-one: {status} {body}");

    for name in names {
        let described = describe(&emulator, name).await;
        let found = status_of(&described);
        if name == "nb-one" {
            assert!(
                ["Stopping", "Stopped"].contains(&found.as_str()),
                "case {name}: the stopped instance must be Stopping or Stopped, got \
                 {described:?}"
            );
        } else {
            assert_eq!(
                found, "InService",
                "case {name}: an instance the stop did not name must stay InService, got \
                 {described:?}"
            );
        }
    }

    // Each of the others can still be stopped in its own right: the refusal above is
    // about the status, not about one stop per emulator.
    for name in ["nb", "nb-one-extra", "other"] {
        let (status, body) = stop(&emulator, name).await;
        assert_eq!(status, 200, "case stop-{name}: {status} {body}");
        let found = status_of(&describe(&emulator, name).await);
        assert!(
            ["Stopping", "Stopped"].contains(&found.as_str()),
            "case stop-{name}: expected Stopping or Stopped after its own stop, got {found}"
        );
    }
}

/// RULE: a `NotebookInstanceName` outside the member's modelled constraint is refused at
/// the boundary with HTTP 400 before the handler runs; a name ON the published bound is
/// resolved normally.
#[tokio::test]
async fn a_stop_outside_the_modelled_constraints_is_refused() {
    let emulator = Emulator::new();

    // `NotebookInstanceName` is `len 0..63` with pattern `^[a-zA-Z0-9](-*[a-zA-Z0-9])*$`
    // (smithy_query operation=StopNotebookInstance), so these sit one past each bound.
    let too_long = "a".repeat(64);
    let cases: Vec<(&str, Value)> = vec![
        ("name-past-63", json!({ "NotebookInstanceName": too_long })),
        (
            "name-with-an-illegal-character",
            json!({ "NotebookInstanceName": "not a notebook name" }),
        ),
        ("name-starting-with-a-hyphen", json!({ "NotebookInstanceName": "-leading" })),
        ("no-name-at-all", json!({})),
    ];
    for (case, body) in cases {
        let (status, parsed) = emulator.call("StopNotebookInstance", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: StopNotebookInstance must be refused, got {status} {parsed}"
        );
    }

    // ON the upper bound: a 63-character name is accepted and stops.
    let longest = format!("{}b", "a".repeat(62));
    emulator
        .ok("CreateNotebookInstance", &create_body(&longest, &json!({})))
        .await;
    tokio::time::sleep(PAST_IN_SERVICE_DUE).await;
    let (status, body) = stop(&emulator, &longest).await;
    assert_eq!(status, 200, "case name-on-63: {status} {body}");
    let found = status_of(&describe(&emulator, &longest).await);
    assert!(
        ["Stopping", "Stopped"].contains(&found.as_str()),
        "case name-on-63: expected Stopping or Stopped after the stop, got {found}"
    );
}
