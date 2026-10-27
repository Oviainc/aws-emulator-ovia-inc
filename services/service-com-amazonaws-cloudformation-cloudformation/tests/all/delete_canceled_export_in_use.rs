// SPDX-License-Identifier: CC0-1.0

//! `DeleteStack` on a stack whose exported output value another live stack still imports,
//! end to end over the awsQuery wire.
//!
//! RULE under test: **`DeleteStack` is ACCEPTED (200) whatever the stack publishes — the
//! model declares no error for a cross-stack reference — and a deletion that would remove
//! an exported output value another live stack still imports is carried out as far as the
//! attempt and then CANCELLED: the stack reports `DELETE_IN_PROGRESS` ("User Initiated"),
//! then goes back to the status it held with `StackStatusReason` `Delete canceled. Cannot
//! delete export <export> as it is in use by <importing stack>.`, with nothing of it
//! disposed of; once no live stack imports it the identical call deletes the stack.**
//!
//! Why it matters: a client reads which reference blocked a teardown off the STACK, by
//! polling the delete it issued. A synchronous refusal of the call gives it an exception
//! instead, so the cancellation reason — the documented way to learn which export and which
//! importer stand in the way — is never produced, and automation that treats `DeleteStack`
//! as always-accepted breaks.
//!
//! The rows differ along what the rule generalises over: WHICH of several exports of the
//! stack is the imported one (the first, the last), where the importing stack resolved it
//! (an output value, a resource property), how MANY live stacks import it, and the status
//! the exporting stack held when the call arrived (created, updated) — which is the status
//! the cancelled deletion puts back. A stack nothing imports is the control row: its
//! deletion is not cancelled and completes.

use std::time::Instant;

use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, member, router, wait_for_the_transition, xml_text, xml_texts};
use crate::stack_lifecycle::{deploy, describe_stack, expected_status};

/// The reason a cancelled deletion leaves on the stack, as AWS words it: the fixed text
/// around the export and the stack that still imports it. Written out here rather than read
/// off the emulator's own formatter, so the assertion pins the wording.
fn canceled_reason(export_name: &str, importing_stack_name: &str) -> String {
    format!("Delete canceled. Cannot delete export {export_name} as it is in use by {importing_stack_name}.")
}

/// A stack template publishing one export per entry of `exports`, over a resource no
/// service has to provision.
fn exporter_template(exports: &[(&str, &str)]) -> String {
    let outputs: Vec<String> = exports
        .iter()
        .enumerate()
        .map(|(index, (name, value))| {
            format!(r#""O{index}":{{"Value":"{value}","Export":{{"Name":"{name}"}}}}"#)
        })
        .collect();
    format!(
        r#"{{"Resources":{{"Handle":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}},
            "Outputs":{{{}}}}}"#,
        outputs.join(",")
    )
}

/// Where an importing stack resolved its `Fn::ImportValue`.
#[derive(Clone, Copy, Debug)]
enum ImportedIn {
    /// An output value of the importing stack.
    AnOutput,
    /// A property of a resource the importing stack really creates (an `AWS::SSM::Parameter`
    /// in AWS Systems Manager).
    AResourceProperty,
}

fn importer_template(export_name: &str, where_imported: ImportedIn) -> String {
    match where_imported {
        ImportedIn::AnOutput => format!(
            r#"{{"Resources":{{"Handle":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}},
                "Outputs":{{"Echoed":{{"Value":{{"Fn::ImportValue":"{export_name}"}}}}}}}}"#
        ),
        ImportedIn::AResourceProperty => format!(
            r#"{{"Resources":{{"Param":{{"Type":"AWS::SSM::Parameter","Properties":
                {{"Type":"String","Value":{{"Fn::ImportValue":"{export_name}"}}}}}}}}}}"#
        ),
    }
}

/// One row: the exporting stack and the live stacks importing one of its exports.
struct Row {
    case: &'static str,
    /// The exports the exporting stack publishes, in template order.
    exports: Vec<(&'static str, &'static str)>,
    /// The export the importing stacks hold, which the cancellation reason must name.
    held: &'static str,
    /// The importing stacks, each with where its template resolved the import.
    importers: Vec<(&'static str, ImportedIn)>,
    /// Whether the exporting stack is UPDATED before the delete: the status a cancelled
    /// deletion puts back is the one the stack held, whichever operation left it there.
    update_first: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the FIRST of three exports is the one imported, by an output value",
            exports: vec![
                ("alpha-export", "alpha-value"),
                ("beta-export", "beta-value"),
                ("gamma-export", "gamma-value"),
            ],
            held: "alpha-export",
            importers: vec![("first-held-importer", ImportedIn::AnOutput)],
            update_first: false,
        },
        Row {
            case: "the LAST of three exports is the one imported, by a resource property",
            exports: vec![
                ("alpha-export", "alpha-value"),
                ("beta-export", "beta-value"),
                ("gamma-export", "gamma-value"),
            ],
            held: "gamma-export",
            importers: vec![("last-held-importer", ImportedIn::AResourceProperty)],
            update_first: false,
        },
        Row {
            case: "one export, imported by TWO live stacks",
            exports: vec![("sole-export", "sole-value")],
            held: "sole-export",
            importers: vec![
                ("one-importer", ImportedIn::AnOutput),
                ("another-importer", ImportedIn::AResourceProperty),
            ],
            update_first: false,
        },
        Row {
            case: "the exporting stack was last UPDATED, so that is the status put back",
            exports: vec![("updated-export", "updated-value")],
            held: "updated-export",
            importers: vec![("updated-held-importer", ImportedIn::AnOutput)],
            update_first: true,
        },
    ]
}

/// The `(StackStatus, StackStatusReason)` one stack reports, read through the suite-wide
/// `DescribeStacks` helper.
async fn stack_status(router: &MultiServiceRouter, case: &str, name: &str) -> (String, String) {
    let (code, body) = describe_stack(router, name).await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStacks({name}) must answer 200, got {code} {body}"
    );
    (
        xml_text(&body, "StackStatus").unwrap_or_default(),
        xml_text(&body, "StackStatusReason").unwrap_or_default(),
    )
}

/// The whole event stream of one stack, newest first.
async fn events(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (code, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", name)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStackEvents({name}) must answer 200, got {code} {body}"
    );
    body
}

/// The export names `ListExports` reports.
async fn exported_names(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (code, body) = call(router, "ListExports", &[]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: ListExports must answer 200, got {code} {body}"
    );
    xml_texts(&body, "Name")
}

#[tokio::test]
async fn a_delete_blocked_by_a_live_importer_is_accepted_and_then_canceled() {
    for row in rows() {
        let router = router();
        let exporter = "held-exporter";
        deploy(&router, exporter, &exporter_template(&row.exports)).await;

        for (importer, where_imported) in &row.importers {
            deploy(
                &router,
                importer,
                &importer_template(row.held, *where_imported),
            )
            .await;
        }

        // The status the cancelled deletion has to put back is whatever the stack holds
        // when the call arrives.
        let held_status = if row.update_first {
            // The same export NAMES, carrying changed values: the update is a real change,
            // and the reference the importing stack holds is still held.
            let revalued: Vec<(&str, String)> = row
                .exports
                .iter()
                .map(|(name, value)| (*name, format!("{value}-updated")))
                .collect();
            let revalued: Vec<(&str, &str)> = revalued
                .iter()
                .map(|(name, value)| (*name, value.as_str()))
                .collect();
            let (code, body) = call(
                &router,
                "UpdateStack",
                &[
                    member("StackName", exporter),
                    member("TemplateBody", &exporter_template(&revalued)),
                ],
            )
            .await;
            assert_eq!(
                code, 200,
                "case {:?}: UpdateStack must succeed, got {code} {body}",
                row.case
            );
            wait_for_the_transition().await;
            status::UPDATE_COMPLETE
        } else {
            status::CREATE_COMPLETE
        };
        let (before, _) = stack_status(&router, row.case, exporter).await;
        assert_eq!(
            before, held_status,
            "case {:?}: expected the exporter to be {held_status} before the delete, got \
             {before:?}",
            row.case
        );

        // The CALL is accepted: no error is modelled for a cross-stack reference.
        let started = Instant::now();
        let (code, body) = call(
            &router,
            "DeleteStack",
            &[member("StackName", exporter)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {:?}: DeleteStack must be accepted, got {code} {body}",
            row.case
        );

        // The deletion was STARTED: a caller reading straight after the call sees the
        // attempt (or, having lost the race to the clock, the status it was put back to).
        let (reading, _) = stack_status(&router, row.case, exporter).await;
        let expected_now = expected_status(started, status::DELETE_IN_PROGRESS, held_status);
        assert_eq!(
            reading, expected_now,
            "case {:?}: expected {expected_now} right after the accepted delete, got \
             {reading:?}",
            row.case
        );

        wait_for_the_transition().await;

        // Cancelled: the stack is back where it was, saying which export and which importer
        // stood in the way.
        let (settled, reason) = stack_status(&router, row.case, exporter).await;
        assert_eq!(
            settled, held_status,
            "case {:?}: expected the cancelled delete to put the stack back to \
             {held_status}, got {settled:?}",
            row.case
        );
        let accepted: Vec<String> = row
            .importers
            .iter()
            .map(|(importer, _)| canceled_reason(row.held, importer))
            .collect();
        assert!(
            accepted.contains(&reason),
            "case {:?}: expected one of {accepted:?} as the StackStatusReason, got {reason:?}",
            row.case
        );

        // The attempt is on the event stream, with the reason CloudFormation opens a stack
        // operation with, and the cancellation is recorded after it.
        let stream = events(&router, row.case, exporter).await;
        assert!(
            stream.contains(status::DELETE_IN_PROGRESS),
            "case {:?}: expected a {} event for the attempt, got {stream}",
            row.case,
            status::DELETE_IN_PROGRESS
        );
        assert!(
            stream.contains("User Initiated"),
            "case {:?}: expected the attempt's event to be User Initiated, got {stream}",
            row.case
        );
        assert!(
            stream.contains("Delete canceled."),
            "case {:?}: expected the cancellation on the event stream, got {stream}",
            row.case
        );

        // NOTHING was destroyed: every export the stack publishes is still published, and
        // the resource it created is still reported.
        let published = exported_names(&router, row.case).await;
        for (name, _) in &row.exports {
            assert!(
                published.contains(&name.to_string()),
                "case {:?}: expected {name:?} to survive the cancelled delete, got \
                 {published:?}",
                row.case
            );
        }
        let (code, resources) = call(
            &router,
            "DescribeStackResources",
            &[member("StackName", exporter)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {:?}: DescribeStackResources must answer 200, got {code} {resources}",
            row.case
        );
        assert!(
            xml_texts(&resources, "LogicalResourceId").contains(&"Handle".to_string()),
            "case {:?}: expected the exporter's resource to be intact, got {resources}",
            row.case
        );

        // Once the last importer is gone the identical call deletes the stack.
        for (importer, _) in &row.importers {
            let (code, body) = call(
                &router,
                "DeleteStack",
                &[member("StackName", importer)],
            )
            .await;
            assert_eq!(
                code, 200,
                "case {:?}: deleting the importer {importer} must succeed, got {code} {body}",
                row.case
            );
        }
        wait_for_the_transition().await;

        let (code, body) = call(
            &router,
            "DeleteStack",
            &[member("StackName", exporter)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {:?}: the identical delete must be accepted, got {code} {body}",
            row.case
        );
        wait_for_the_transition().await;

        let published = exported_names(&router, row.case).await;
        for (name, _) in &row.exports {
            assert!(
                !published.contains(&name.to_string()),
                "case {:?}: expected {name:?} to be gone with its stack, got {published:?}",
                row.case
            );
        }
    }
}

/// A stack is never LEFT in an `*_IN_PROGRESS` status: a cancelled deletion puts back the
/// status the stack SETTLES to, so one taken while another operation is still in flight
/// goes to that operation's terminal status rather than to the status of an operation that
/// had not concluded.
#[tokio::test]
async fn a_canceled_delete_never_puts_back_an_in_progress_status() {
    let case = "the delete arrives while an update of the exporter is still in flight";
    let router = router();
    let exports = [("in-flight-export", "in-flight-value")];
    deploy(&router, "in-flight-exporter", &exporter_template(&exports)).await;
    deploy(
        &router,
        "in-flight-importer",
        &importer_template("in-flight-export", ImportedIn::AnOutput),
    )
    .await;

    // No wait between the two calls: the update is still in flight when the delete arrives
    // (and if it loses the race to the clock, it has settled to the same status).
    let revalued = [("in-flight-export", "in-flight-value-updated")];
    let (code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "in-flight-exporter"),
            member("TemplateBody", &exporter_template(&revalued)),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: UpdateStack must be accepted, got {code} {body}"
    );
    let (code, body) = call(
        &router,
        "DeleteStack",
        &[member("StackName", "in-flight-exporter")],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: DeleteStack must be accepted, got {code} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;

    let (settled, reason) = stack_status(&router, case, "in-flight-exporter").await;
    assert!(
        !settled.ends_with("_IN_PROGRESS"),
        "case {case:?}: expected a settled status, got {settled:?}"
    );
    assert_eq!(
        settled,
        status::UPDATE_COMPLETE,
        "case {case:?}: expected the update's terminal status to be put back, got {settled:?}"
    );
    assert_eq!(
        reason,
        canceled_reason("in-flight-export", "in-flight-importer"),
        "case {case:?}: expected the cancellation reason, got {reason:?}"
    );
}

#[tokio::test]
async fn a_delete_nothing_imports_from_is_not_canceled() {
    let case = "no live stack imports what the exporter publishes";
    let router = router();
    let exports = [("free-export", "free-value")];
    deploy(&router, "free-exporter", &exporter_template(&exports)).await;

    // A stack importing ANOTHER name is no importer of this export, so it cancels nothing.
    deploy(
        &router,
        "unrelated-importer",
        &importer_template("some-other-export", ImportedIn::AnOutput),
    )
    .await;

    let (code, body) = call(
        &router,
        "DeleteStack",
        &[member("StackName", "free-exporter")],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: DeleteStack must be accepted, got {code} {body}"
    );
    wait_for_the_transition().await;

    // Addressed by STACK ID, since a completed deletion leaves the live set.
    let (code, listed) = call(&router, "ListStacks", &[]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: ListStacks must answer 200, got {code} {listed}"
    );
    let stack_id = xml_texts(&listed, "StackId")
        .into_iter()
        .find(|id| id.contains("/free-exporter/"))
        .unwrap_or_default();
    let (settled, reason) = stack_status(&router, case, &stack_id).await;
    assert_eq!(
        settled,
        status::DELETE_COMPLETE,
        "case {case:?}: expected an uncancelled delete to complete, got {settled:?}"
    );
    assert!(
        !reason.contains("Delete canceled."),
        "case {case:?}: expected no cancellation reason, got {reason:?}"
    );
    let published = exported_names(&router, case).await;
    assert!(
        !published.contains(&"free-export".to_string()),
        "case {case:?}: expected the export to go with its stack, got {published:?}"
    );
}
