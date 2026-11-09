// SPDX-License-Identifier: CC0-1.0

//! `DeleteStack` on a stack whose exported output value another live stack still imports,
//! end to end over the awsQuery wire.
//!
//! RULE under test: **an exported output value another live stack still imports is PINNED,
//! so a `DeleteStack` the service accepts (200) for the stack that publishes it reports
//! `DELETE_IN_PROGRESS` ("User Initiated") and is then CANCELLED: the stack goes back to
//! the healthy status it held, carrying
//! `Delete canceled. Cannot delete export <export> as it is in use by <importing stack>.`
//! as its `StackStatusReason`, with nothing of the stack disposed of; once no live stack
//! imports it the identical call deletes the stack and reaches `DELETE_COMPLETE`.**
//!
//! Why it matters: a terminal `DELETE_FAILED` conventionally means the stack's resources are
//! stranded and the delete should be retried — with `RetainResources`, or forced — so
//! automation that follows that convention would drive the delete through and destroy a
//! resource another live stack still imports. A cancelled delete says the opposite: the
//! stack is healthy and the dependency, not the stack, is what has to change.
//!
//! The rows differ along what the rule generalises over: WHICH of several exports of the
//! stack is the imported one (the first, the last), where the importing stack resolved it
//! (an output value, a resource property), how MANY live stacks import it, and the status
//! the exporting stack held when the call arrived (created, updated) — which is the status
//! it is restored to. A stack nothing imports is the control row: its deletion is not
//! blocked and completes.

use std::time::Instant;

use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, member, router, wait_for_the_transition, xml_text, xml_texts};
use crate::stack_lifecycle::{deploy, describe_stack, expected_status};

/// The reason a cancelled deletion carries, as AWS words it: the fixed text around the
/// export and the stack that still imports it, ending with its period. Written out here
/// rather than read off the emulator's own formatter, so the assertion pins the wording.
fn in_use_reason(export_name: &str, importing_stack_name: &str) -> String {
    format!(
        "Delete canceled. Cannot delete export {export_name} as it is in use by \
         {importing_stack_name}."
    )
}

/// A stack template publishing one export per entry of `exports`, over a resource no
/// service has to provision.
fn exporter_template(exports: &[(&str, &str)]) -> String {
    exporter_template_over(exports, 1)
}

/// The same template over `resources` resources, for an update that has to be a real change
/// to the stack without touching what it publishes.
fn exporter_template_over(exports: &[(&str, &str)], resources: usize) -> String {
    let outputs: Vec<String> = exports
        .iter()
        .enumerate()
        .map(|(index, (name, value))| {
            format!(r#""O{index}":{{"Value":"{value}","Export":{{"Name":"{name}"}}}}"#)
        })
        .collect();
    let declared: Vec<String> = (0..resources)
        .map(|index| {
            format!(r#""Handle{index}":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}"#)
        })
        .collect();
    format!(
        r#"{{"Resources":{{{}}},"Outputs":{{{}}}}}"#,
        declared.join(","),
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
    /// The export the importing stacks hold, which the failure reason must name.
    held: &'static str,
    /// The importing stacks, each with where its template resolved the import.
    importers: Vec<(&'static str, ImportedIn)>,
    /// Whether the exporting stack is UPDATED before the delete: the deletion is cancelled
    /// the same way whichever operation left the stack where it was, and the status it is
    /// restored to is that operation's.
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
            case: "the exporting stack was last UPDATED, and the delete is cancelled alike",
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
async fn a_delete_blocked_by_a_live_importer_is_canceled_and_the_stack_stays_healthy() {
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

        // Whatever healthy status the stack holds when the call arrives, the cancelled
        // deletion puts it back into that one.
        let held_status = if row.update_first {
            // The same exports, over one resource MORE: a real change to the stack that
            // withdraws nothing the importing stack holds. (An update that CHANGED or
            // dropped the imported export would be cancelled in its own right — see
            // `update_stack_export_in_use.rs` — so the row that pins the DELETE being
            // cancelled is driven by an update the export pin does not touch.)
            let (code, body) = call(
                &router,
                "UpdateStack",
                &[
                    member("StackName", exporter),
                    member("TemplateBody", &exporter_template_over(&row.exports, 2)),
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
        // attempt (or, having lost the race to the clock, the status it was restored to).
        let (reading, _) = stack_status(&router, row.case, exporter).await;
        let expected_now = expected_status(started, status::DELETE_IN_PROGRESS, held_status);
        assert_eq!(
            reading, expected_now,
            "case {:?}: expected {expected_now} right after the accepted delete, got \
             {reading:?}",
            row.case
        );

        wait_for_the_transition().await;

        // CANCELLED: the stack is back in the healthy status it held, saying which export
        // and which importer stood in the way.
        let (settled, reason) = stack_status(&router, row.case, exporter).await;
        assert_eq!(
            settled, held_status,
            "case {:?}: expected the cancelled delete to restore {held_status}, got \
             {settled:?}",
            row.case
        );
        let accepted: Vec<String> = row
            .importers
            .iter()
            .map(|(importer, _)| in_use_reason(row.held, importer))
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
            stream.contains("Delete canceled") && stream.contains("as it is in use by"),
            "case {:?}: expected the cancelled deletion and its reason on the event stream, \
             got {stream}",
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
            xml_texts(&resources, "LogicalResourceId").contains(&"Handle0".to_string()),
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

        // Addressed by STACK ID, since a completed deletion leaves the live set: the retried
        // delete reached the other terminal status a waiter stops on.
        let (code, listed) = call(&router, "ListStacks", &[]).await;
        assert_eq!(
            code, 200,
            "case {:?}: ListStacks must answer 200, got {code} {listed}",
            row.case
        );
        let stack_id = xml_texts(&listed, "StackId")
            .into_iter()
            .find(|id| id.contains(&format!("/{exporter}/")))
            .unwrap_or_default();
        let (settled, _) = stack_status(&router, row.case, &stack_id).await;
        assert_eq!(
            settled,
            status::DELETE_COMPLETE,
            "case {:?}: expected the retried delete to complete, got {settled:?}",
            row.case
        );
    }
}

/// A stack a cancelled deletion left behind is never LEFT in an `*_IN_PROGRESS` status: the
/// cancellation restores the healthy status the stack held.
///
/// A delete that arrives while an UPDATE of that stack is still in flight never gets as far
/// as being cancelled — CloudFormation runs one operation at a time against a stack, so it is
/// refused and the update goes on running (`delete_stack_single_writer.rs`) — and the delete
/// that stack takes once the update has concluded is the one this case follows.
#[tokio::test]
async fn a_canceled_delete_never_leaves_an_in_progress_status() {
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

    // No wait between the two calls: the update is still in flight when the delete arrives.
    // The update keeps publishing the export the importer holds — one that withdrew it would
    // be cancelled in its own right (`update_stack_export_in_use.rs`) — and changes the stack
    // by declaring one resource more.
    let in_flight = [("in-flight-export", "in-flight-value")];
    let (code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "in-flight-exporter"),
            member("TemplateBody", &exporter_template_over(&in_flight, 2)),
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
    // Refused while that update runs; a delete that lost the race to the clock met a stack
    // the update had already settled, and is the accepted-and-cancelled call instead.
    if code == 400 {
        assert!(
            body.contains(status::UPDATE_IN_PROGRESS) && body.contains("cannot be deleted while"),
            "case {case:?}: expected the refusal to name the update in flight, got {body}"
        );
    } else {
        assert_eq!(
            code, 200,
            "case {case:?}: expected the delete either refused or accepted, got {code} {body}"
        );
    }
    wait_for_the_transition().await;
    wait_for_the_transition().await;

    // The stack the update left behind takes the delete, which the export in use CANCELS:
    // the stack reports the attempt and then goes back to the healthy status it holds.
    let (code, body) = call(
        &router,
        "DeleteStack",
        &[member("StackName", "in-flight-exporter")],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: the delete of the settled stack must be accepted, got {code} {body}"
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
        "case {case:?}: expected the status the in-flight update was going to reach, got \
         {settled:?}"
    );
    assert_eq!(
        reason,
        in_use_reason("in-flight-export", "in-flight-importer"),
        "case {case:?}: expected the in-use reason, got {reason:?}"
    );
}

#[tokio::test]
async fn a_delete_nothing_imports_from_completes() {
    let case = "no live stack imports what the exporter publishes";
    let router = router();
    let exports = [("free-export", "free-value")];
    deploy(&router, "free-exporter", &exporter_template(&exports)).await;

    // A stack importing ANOTHER name is no importer of this export, so it blocks nothing.
    // That other name is published by a second live stack, since an import of a name
    // NOTHING exports fails its own deployment (`unresolvable_imports.rs`).
    let other = [("some-other-export", "some-other-value")];
    deploy(&router, "other-exporter", &exporter_template(&other)).await;
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
        "case {case:?}: expected an unblocked delete to complete, got {settled:?}"
    );
    assert!(
        !reason.contains("Delete canceled"),
        "case {case:?}: expected no cancellation reason, got {reason:?}"
    );
    let published = exported_names(&router, case).await;
    assert!(
        !published.contains(&"free-export".to_string()),
        "case {case:?}: expected the export to go with its stack, got {published:?}"
    );
}
