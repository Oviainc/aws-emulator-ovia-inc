// SPDX-License-Identifier: CC0-1.0

//! A deployment that would WITHDRAW an exported output value another live stack imports,
//! end to end over the awsQuery wire.
//!
//! RULE under test: **an exported output value another live stack still imports is PINNED,
//! so a deployment of the publishing stack that would stop publishing that export — or
//! publish a different value under it — is CANCELLED: the call is accepted (200), the
//! operation reaches `UPDATE_ROLLBACK_COMPLETE` carrying
//! `Update canceled. Cannot update export <export> as it is in use by <importing stack>.`
//! as its `StackStatusReason`, and the export goes on publishing the value the importing
//! stack resolved.** It holds for every deployment of that stack, whether a client drove it
//! with `UpdateStack` or by executing a change set.
//!
//! Why it matters: a cross-stack reference is a contract. If the producer may quietly stop
//! publishing an export, or change what it publishes, every importing stack is left holding
//! a value that is no longer the producer's — and nothing tells the client, because the
//! update is reported as having completed.
//!
//! The rows differ along what the rule generalises over: WHICH of several exports is the
//! imported one (the first, the last), HOW it is withdrawn (the export is dropped, or its
//! value changes), how MANY live stacks import it, and WHICH call deploys the revised
//! template (`UpdateStack`, `ExecuteChangeSet`). The control rows are the deployments the
//! pin does not touch: one that republishes every in-use export unchanged while changing the
//! stack elsewhere, and one that changes an export no live stack imports.

use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, deploy, describe_stack, member, router, wait_for_the_transition, xml_text, xml_texts,
};

/// The reason a cancelled update carries, as AWS words it: the fixed text around the export
/// and the stack that still imports it, ending with its period. Written out here rather than
/// read off the emulator's own formatter, so the assertion pins the wording.
fn in_use_reason(export_name: &str, importing_stack_name: &str) -> String {
    format!(
        "Update canceled. Cannot update export {export_name} as it is in use by \
         {importing_stack_name}."
    )
}

/// A template publishing one export per entry, over resources no service has to provision.
///
/// `resources` is how many of them the template declares: a change set is offered for
/// execution only where it carries changes ("the submitted information didn't contain
/// changes" is what a change set over an identical resource set FAILS with), so a revision a
/// change set deploys declares one resource more as well as revising the exports.
fn exporter_template_with(exports: &[(&str, &str)], resources: usize) -> String {
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

/// The producer's template as it is first deployed: one resource, and its exports.
fn exporter_template(exports: &[(&str, &str)]) -> String {
    exporter_template_with(exports, 1)
}

/// A template importing one export into an output of its own.
fn importer_template(export_name: &str) -> String {
    format!(
        r#"{{"Resources":{{"Handle":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}},
            "Outputs":{{"Echoed":{{"Value":{{"Fn::ImportValue":"{export_name}"}}}}}}}}"#
    )
}

/// Which call deploys the revised template: both are deployments of the same stack, and the
/// pin holds for both.
#[derive(Clone, Copy, Debug)]
enum DeployedBy {
    UpdateStack,
    ExecuteChangeSet,
}

/// One row: the exports the producer publishes, the revision it is asked to deploy, and the
/// live stacks importing one of them.
struct Row {
    case: &'static str,
    exports: Vec<(&'static str, &'static str)>,
    /// The revised exports the deployment would publish: an entry missing from it is an
    /// export the revision DROPS, and one carrying another value is an export it CHANGES.
    revised: Vec<(&'static str, &'static str)>,
    /// The export the importing stacks hold, which the cancellation must name.
    held: &'static str,
    importers: Vec<&'static str>,
    deployed_by: DeployedBy,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the FIRST of three exports changes its value, under one importer",
            exports: vec![
                ("alpha-held", "alpha-value"),
                ("beta-held", "beta-value"),
                ("gamma-held", "gamma-value"),
            ],
            revised: vec![
                ("alpha-held", "alpha-value-revised"),
                ("beta-held", "beta-value"),
                ("gamma-held", "gamma-value"),
            ],
            held: "alpha-held",
            importers: vec!["first-changed-importer"],
            deployed_by: DeployedBy::UpdateStack,
        },
        Row {
            case: "the LAST of three exports is DROPPED by the revision",
            exports: vec![
                ("alpha-held", "alpha-value"),
                ("beta-held", "beta-value"),
                ("gamma-held", "gamma-value"),
            ],
            revised: vec![("alpha-held", "alpha-value"), ("beta-held", "beta-value")],
            held: "gamma-held",
            importers: vec!["last-dropped-importer"],
            deployed_by: DeployedBy::UpdateStack,
        },
        Row {
            case: "one export, changed while TWO live stacks import it",
            exports: vec![("sole-held", "sole-value")],
            revised: vec![("sole-held", "sole-value-revised")],
            held: "sole-held",
            importers: vec!["one-held-importer", "another-held-importer"],
            deployed_by: DeployedBy::UpdateStack,
        },
        Row {
            case: "the revision is deployed by EXECUTING A CHANGE SET, and is cancelled alike",
            exports: vec![("change-set-held", "change-set-value")],
            revised: vec![("change-set-held", "change-set-value-revised")],
            held: "change-set-held",
            importers: vec!["change-set-importer"],
            deployed_by: DeployedBy::ExecuteChangeSet,
        },
        Row {
            case: "the change set DROPS the in-use export",
            exports: vec![
                ("change-set-dropped", "dropped-value"),
                ("change-set-kept", "kept-value"),
            ],
            revised: vec![("change-set-kept", "kept-value")],
            held: "change-set-dropped",
            importers: vec!["change-set-dropping-importer"],
            deployed_by: DeployedBy::ExecuteChangeSet,
        },
    ]
}

/// The `(StackStatus, StackStatusReason)` one stack reports.
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

/// The value `ListExports` reports for one export name, or the empty string where the Region
/// publishes no such export.
async fn exported_value(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (code, body) = call(router, "ListExports", &[]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: ListExports must answer 200, got {code} {body}"
    );
    let names = xml_texts(&body, "Name");
    let values = xml_texts(&body, "Value");
    names
        .iter()
        .zip(values.iter())
        .find(|(held, _)| held.as_str() == name)
        .map(|(_, value)| value.clone())
        .unwrap_or_default()
}

/// The `OutputValue` one stack reports for an output key.
async fn output_value(router: &MultiServiceRouter, case: &str, stack: &str, key: &str) -> String {
    let (code, body) = describe_stack(router, stack).await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStacks({stack}) must answer 200, got {code} {body}"
    );
    let keys = xml_texts(&body, "OutputKey");
    let values = xml_texts(&body, "OutputValue");
    keys.iter()
        .zip(values.iter())
        .find(|(output_key, _)| output_key.as_str() == key)
        .map(|(_, value)| value.clone())
        .unwrap_or_default()
}

/// Deploy `template` onto an existing stack, by the call the row names, and assert the call
/// itself was accepted.
async fn redeploy(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    template: &str,
    deployed_by: DeployedBy,
) {
    match deployed_by {
        DeployedBy::UpdateStack => {
            let (code, body) = call(
                router,
                "UpdateStack",
                &[
                    member("StackName", stack),
                    member("TemplateBody", template),
                ],
            )
            .await;
            assert_eq!(
                code, 200,
                "case {case:?}: UpdateStack must be accepted, got {code} {body}"
            );
        }
        DeployedBy::ExecuteChangeSet => {
            let change_set = format!("{stack}-revision");
            let (code, body) = call(
                router,
                "CreateChangeSet",
                &[
                    member("StackName", stack),
                    member("ChangeSetName", &change_set),
                    member("TemplateBody", template),
                ],
            )
            .await;
            assert_eq!(
                code, 200,
                "case {case:?}: CreateChangeSet must be accepted, got {code} {body}"
            );
            wait_for_the_transition().await;
            let (code, body) = call(
                router,
                "ExecuteChangeSet",
                &[
                    member("StackName", stack),
                    member("ChangeSetName", &change_set),
                ],
            )
            .await;
            assert_eq!(
                code, 200,
                "case {case:?}: ExecuteChangeSet must be accepted, got {code} {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_deployment_that_withdraws_an_imported_export_is_canceled_and_rolled_back() {
    for row in rows() {
        let router = router();
        let producer = "held-producer";
        deploy(&router, producer, &exporter_template(&row.exports)).await;
        for importer in &row.importers {
            deploy(&router, importer, &importer_template(row.held)).await;
        }
        let published_before = exported_value(&router, row.case, row.held).await;

        // A change set is executable only where it carries resource changes, so the
        // revision those rows deploy declares one resource more.
        let resources = match row.deployed_by {
            DeployedBy::UpdateStack => 1,
            DeployedBy::ExecuteChangeSet => 2,
        };
        redeploy(
            &router,
            row.case,
            producer,
            &exporter_template_with(&row.revised, resources),
            row.deployed_by,
        )
        .await;
        // Two spans: the cancelled update's own, and the rollback it opens.
        wait_for_the_transition().await;
        wait_for_the_transition().await;

        let (settled, reason) = stack_status(&router, row.case, producer).await;
        assert_eq!(
            settled,
            status::UPDATE_ROLLBACK_COMPLETE,
            "case {:?}: expected the withdrawal to be rolled back, got {settled:?} ({reason:?})",
            row.case
        );
        let accepted: Vec<String> = row
            .importers
            .iter()
            .map(|importer| in_use_reason(row.held, importer))
            .collect();
        assert!(
            accepted.contains(&reason),
            "case {:?}: expected one of {accepted:?} as the StackStatusReason, got {reason:?}",
            row.case
        );

        // NOTHING was withdrawn: the export still publishes the value its importers
        // resolved, and every other export of the producer is still published too.
        let published_after = exported_value(&router, row.case, row.held).await;
        assert_eq!(
            published_after, published_before,
            "case {:?}: expected {} to go on publishing {published_before:?}, got \
             {published_after:?}",
            row.case, row.held
        );
        for (name, value) in &row.exports {
            let held = exported_value(&router, row.case, name).await;
            assert_eq!(
                held, *value,
                "case {:?}: expected {name} to survive the cancelled update with {value:?}, \
                 got {held:?}",
                row.case
            );
        }
        for importer in &row.importers {
            let echoed = output_value(&router, row.case, importer, "Echoed").await;
            assert_eq!(
                echoed, published_before,
                "case {:?}: expected {importer} to go on reading {published_before:?}, got \
                 {echoed:?}",
                row.case
            );
        }
    }
}

/// The control rows: a deployment the pin does not touch goes through, so the guard refuses
/// only the deployments that really withdraw an export in use.
#[tokio::test]
async fn a_deployment_that_withdraws_nothing_in_use_completes() {
    // The in-use export is REPUBLISHED unchanged while the stack changes elsewhere.
    let case = "every in-use export is republished unchanged";
    let router = router();
    let exports = [("kept-export", "kept-value"), ("free-export", "free-value")];
    deploy(&router, "kept-producer", &exporter_template(&exports)).await;
    deploy(&router, "kept-importer", &importer_template("kept-export")).await;

    // The same export names and values, over a template that declares one resource more:
    // a real change to the stack that withdraws nothing.
    let revised = exporter_template_with(&exports, 2);
    redeploy(
        &router,
        case,
        "kept-producer",
        &revised,
        DeployedBy::UpdateStack,
    )
    .await;
    wait_for_the_transition().await;
    wait_for_the_transition().await;
    let (settled, reason) = stack_status(&router, case, "kept-producer").await;
    assert_eq!(
        settled,
        status::UPDATE_COMPLETE,
        "case {case:?}: expected the update to complete, got {settled:?} ({reason:?})"
    );

    // ...and an export NO live stack imports is free to change.
    let case = "the changed export is one nothing imports";
    let revised = exporter_template_with(
        &[("kept-export", "kept-value"), ("free-export", "free-value-revised")],
        2,
    );
    redeploy(
        &router,
        case,
        "kept-producer",
        &revised,
        DeployedBy::UpdateStack,
    )
    .await;
    wait_for_the_transition().await;
    wait_for_the_transition().await;
    let (settled, reason) = stack_status(&router, case, "kept-producer").await;
    assert_eq!(
        settled,
        status::UPDATE_COMPLETE,
        "case {case:?}: expected the update to complete, got {settled:?} ({reason:?})"
    );
    let published = exported_value(&router, case, "free-export").await;
    assert_eq!(
        published, "free-value-revised",
        "case {case:?}: expected the free export to carry the new value, got {published:?}"
    );
    let still_held = exported_value(&router, case, "kept-export").await;
    assert_eq!(
        still_held, "kept-value",
        "case {case:?}: expected the in-use export untouched, got {still_held:?}"
    );
}

/// An export only the PUBLISHING stack imports is not in use by another stack, so a
/// deployment that withdraws it is carried out: the pin protects the reference another stack
/// holds, not the stack's own.
#[tokio::test]
async fn an_export_no_other_stack_imports_is_not_pinned() {
    let case = "the only stack importing the export is the one publishing it";
    let router = router();
    // One stack publishes `self-held` and imports `seeded-export` from another: its own
    // export is imported by nobody.
    deploy(
        &router,
        "seed-producer",
        &exporter_template(&[("seeded-export", "seeded-value")]),
    )
    .await;
    let template = r#"{"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
        "Outputs":{"Echoed":{"Value":{"Fn::ImportValue":"seeded-export"},
                             "Export":{"Name":"self-held"}}}}"#;
    deploy(&router, "self-importer", template).await;

    let revised = r#"{"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
        "Outputs":{"Echoed":{"Value":{"Fn::ImportValue":"seeded-export"},
                             "Export":{"Name":"self-held-revised"}}}}"#;
    redeploy(
        &router,
        case,
        "self-importer",
        revised,
        DeployedBy::UpdateStack,
    )
    .await;
    wait_for_the_transition().await;
    wait_for_the_transition().await;

    let (settled, reason) = stack_status(&router, case, "self-importer").await;
    assert_eq!(
        settled,
        status::UPDATE_COMPLETE,
        "case {case:?}: expected the update to complete, got {settled:?} ({reason:?})"
    );
    let renamed = exported_value(&router, case, "self-held-revised").await;
    assert_eq!(
        renamed, "seeded-value",
        "case {case:?}: expected the renamed export to publish, got {renamed:?}"
    );
}
