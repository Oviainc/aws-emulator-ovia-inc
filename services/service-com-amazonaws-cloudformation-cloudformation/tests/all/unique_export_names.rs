// SPDX-License-Identifier: CC0-1.0

//! An exported output value's NAME is unique per Region, over the awsQuery wire.
//!
//! RULE: "Export names must be unique within a Region" per account, so a deployment that
//! would publish an export name another LIVE stack of that Region already publishes cannot
//! be carried through: the operation is accepted — the request is perfectly valid — and the
//! deployment FAILS with CloudFormation's own diagnostic naming the export and the stack
//! holding it, then unwinds exactly as any other failed deployment does (`ROLLBACK` by
//! default, `CREATE_FAILED` kept where `DisableRollback`/`OnFailure=DO_NOTHING` asked for
//! it). The stack already publishing the name is untouched and goes on publishing it.
//!
//! The rows span what the rule generalises over: the operation that would publish the
//! duplicate (`CreateStack`, `UpdateStack`, `ExecuteChangeSet`), and how the failed create
//! unwinds. The other half of the rule — the cases this must NOT fail — is a table of its
//! own: the same name in a second Region, the holding stack republishing its OWN export, and
//! the name a deleted stack has released.

use serde_json::json;
use service_com_amazonaws_cloudformation_cloudformation::state::{rollback_requested, status};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, call_in, create_stack, describe_stack, member, router, wait_for_the_transition, xml_text,
    xml_texts, REGION,
};

/// A second Region, for the row that proves the uniqueness is per-Region.
const OTHER_REGION: &str = "us-west-2";

/// The stack that holds the export name every row below collides with.
const HOLDER: &str = "export-holder";

/// What CloudFormation reports for a deployment that would publish a name another stack
/// already exports, spelled out HERE rather than taken from the implementation: the fixed
/// text with the export name and the holding stack's name interpolated.
fn already_exported(export_name: &str, holder: &str) -> String {
    format!("Export with name {export_name} is already exported by stack {holder}.")
}

/// A template publishing `export_name` from a topic it creates.
fn exporting(export_name: &str, topic: &str) -> String {
    json!({
        "Resources": {
            "Topic": {"Type": "AWS::SNS::Topic", "Properties": {"TopicName": topic}},
        },
        "Outputs": {
            "Published": {
                "Value": {"Ref": "Topic"},
                "Export": {"Name": export_name},
            },
        },
    })
    .to_string()
}

/// Deploy one stack to completion, asserting it got there.
async fn deploy(router: &MultiServiceRouter, case: &str, name: &str, template: &str) {
    let (code, body) = create_stack(router, name, template, &[]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: CreateStack({name}) must be accepted, got {code} {body}"
    );
    wait_for_the_transition().await;
    let (_, described) = describe_stack(router, name).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "case {case:?}: the stack holding the export must complete, got {described}"
    );
}

/// The `(StackStatus, StackStatusReason)` one stack reports.
async fn reported(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let (_, body) = describe_stack(router, name).await;
    (
        xml_text(&body, "StackStatus").unwrap_or_default(),
        xml_text(&body, "StackStatusReason").unwrap_or_default(),
    )
}

/// One row of the create table: how the colliding create asked to be unwound, and the
/// terminal status that leaves.
struct Colliding {
    case: &'static str,
    stack: &'static str,
    export_name: &'static str,
    /// The request members that decide the unwind.
    extra: Vec<(String, String)>,
    settles_in: &'static str,
}

fn colliding_creates() -> Vec<Colliding> {
    vec![
        Colliding {
            case: "no OnFailure at all: ROLLBACK is the default",
            stack: "export-clash-default",
            export_name: "clash-default",
            extra: Vec::new(),
            settles_in: status::ROLLBACK_COMPLETE,
        },
        Colliding {
            case: "DisableRollback=true keeps the failed state",
            stack: "export-clash-kept",
            export_name: "clash-kept",
            extra: vec![member("DisableRollback", "true")],
            settles_in: status::CREATE_FAILED,
        },
        Colliding {
            case: "OnFailure=DO_NOTHING spells the same thing",
            stack: "export-clash-do-nothing",
            export_name: "clash-do-nothing",
            extra: vec![member("OnFailure", "DO_NOTHING")],
            settles_in: status::CREATE_FAILED,
        },
    ]
}

#[tokio::test]
async fn a_create_publishing_a_name_another_stack_exports_fails_and_unwinds() {
    for row in colliding_creates() {
        let case = row.case;
        let router = router();
        deploy(
            &router,
            case,
            HOLDER,
            &exporting(row.export_name, "holder-topic"),
        )
        .await;

        let (code, body) = create_stack(
            &router,
            row.stack,
            &exporting(row.export_name, "clashing-topic"),
            &row.extra,
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: CreateStack must be ACCEPTED — the request is valid and the \
             deployment is what fails — got {code} {body}"
        );
        wait_for_the_transition().await;

        let (settled, reason) = reported(&router, row.stack).await;
        assert_eq!(
            settled, row.settles_in,
            "case {case:?}: expected the colliding create to settle in {:?}, got \
             {settled:?} ({reason:?})",
            row.settles_in
        );
        // CloudFormation's own diagnostic, naming the export and the stack that holds it.
        // The stack's `StackStatusReason` carries the failure alone, whichever way the
        // create unwound.
        let diagnostic = already_exported(row.export_name, HOLDER);
        assert_eq!(
            reason, diagnostic,
            "case {case:?}: expected the failure to report {diagnostic:?}, got {reason:?}"
        );
        // ...and the rolled-back create's own event carries that diagnostic with the
        // rollback clause, which is where a client reads WHY the stack was unwound.
        if row.settles_in == status::ROLLBACK_COMPLETE {
            let (_, events) = call(
                &router,
                "DescribeStackEvents",
                &[member("StackName", row.stack)],
            )
            .await;
            let asked_for = rollback_requested(&diagnostic);
            assert!(
                xml_texts(&events, "ResourceStatusReason").contains(&asked_for),
                "case {case:?}: expected the ROLLBACK_IN_PROGRESS event to report \
                 {asked_for:?}, got {events}"
            );
        }

        // The stack that HELD the name is untouched and goes on publishing it: the second
        // deployment's failure takes nothing away from the first.
        let (held_status, _) = reported(&router, HOLDER).await;
        assert_eq!(
            held_status,
            status::CREATE_COMPLETE,
            "case {case:?}: the holding stack must be left alone, got {held_status:?}"
        );
        let (_, listed) = call(&router, "ListExports", &[]).await;
        assert_eq!(
            xml_texts(&listed, "Name"),
            vec![row.export_name.to_string()],
            "case {case:?}: the Region must publish the holder's export and nothing else, \
             got {listed}"
        );
    }
}

#[tokio::test]
async fn an_update_or_an_executed_change_set_publishing_a_held_name_fails_too() {
    for by_change_set in [false, true] {
        let case = if by_change_set {
            "an executed change set publishing a held export name"
        } else {
            "an update publishing a held export name"
        };
        let router = router();
        deploy(&router, case, HOLDER, &exporting("taken-by-holder", "holder-topic")).await;
        // The stack the second operation runs against: live, publishing a name of its own.
        deploy(
            &router,
            case,
            "export-mover",
            &exporting("mover-own-name", "mover-topic"),
        )
        .await;

        // The change-set arm renames the topic too: a change set over a template whose
        // RESOURCES are identical carries no changes at all, which is a refusal of its own.
        let wanted = if by_change_set {
            exporting("taken-by-holder", "mover-topic-renamed")
        } else {
            exporting("taken-by-holder", "mover-topic")
        };
        if by_change_set {
            let members = [
                member("StackName", "export-mover"),
                member("ChangeSetName", "take-the-name"),
                member("TemplateBody", &wanted),
            ];
            let (code, body) = call(&router, "CreateChangeSet", &members).await;
            assert_eq!(
                code, 200,
                "case {case:?}: CreateChangeSet must be accepted, got {code} {body}"
            );
            wait_for_the_transition().await;
            let (code, body) = call(
                &router,
                "ExecuteChangeSet",
                &[
                    member("StackName", "export-mover"),
                    member("ChangeSetName", "take-the-name"),
                ],
            )
            .await;
            assert_eq!(
                code, 200,
                "case {case:?}: ExecuteChangeSet must be accepted, got {code} {body}"
            );
        } else {
            let (code, body) = call(
                &router,
                "UpdateStack",
                &[
                    member("StackName", "export-mover"),
                    member("TemplateBody", &wanted),
                ],
            )
            .await;
            assert_eq!(
                code, 200,
                "case {case:?}: UpdateStack must be accepted, got {code} {body}"
            );
        }
        wait_for_the_transition().await;
        wait_for_the_transition().await;

        let (settled, reason) = reported(&router, "export-mover").await;
        assert_eq!(
            settled,
            status::UPDATE_ROLLBACK_COMPLETE,
            "case {case:?}: expected the failed operation to roll back, got {settled:?} \
             ({reason:?})"
        );
        assert_eq!(
            reason,
            already_exported("taken-by-holder", HOLDER),
            "case {case:?}: expected the already-exported diagnostic, got {reason:?}"
        );
        // The rollback put the stack's own export back, and the holder still has its name.
        let (_, listed) = call(&router, "ListExports", &[]).await;
        let mut names = xml_texts(&listed, "Name");
        names.sort();
        assert_eq!(
            names,
            vec!["mover-own-name".to_string(), "taken-by-holder".to_string()],
            "case {case:?}: expected both stacks' own exports, got {listed}"
        );
    }
}

#[tokio::test]
async fn a_name_no_live_stack_of_the_region_publishes_is_free() {
    let router = router();
    let case = "the negatives of the uniqueness rule";
    deploy(&router, case, HOLDER, &exporting("per-region-name", "holder-topic")).await;

    // A second REGION is a second namespace: "export names must be unique within a Region",
    // so the same name there is a second export and both coexist.
    let (code, body) = call_in(
        &router,
        OTHER_REGION,
        "CreateStack",
        &[
            member("StackName", "other-region-holder"),
            member("TemplateBody", &exporting("per-region-name", "other-topic")),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: the other Region's create must be accepted, got {code} {body}"
    );
    wait_for_the_transition().await;
    let (_, described) = call_in(
        &router,
        OTHER_REGION,
        "DescribeStacks",
        &[member("StackName", "other-region-holder")],
    )
    .await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "case {case:?}: the same export name in another Region must deploy, got {described}"
    );

    // The stack that HOLDS the name may publish it again: a re-deploy republishes its own
    // export rather than colliding with it.
    let (code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", HOLDER),
            member(
                "TemplateBody",
                &exporting("per-region-name", "holder-topic-renamed"),
            ),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: the holder's own re-deploy must be accepted, got {code} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;
    let (settled, reason) = reported(&router, HOLDER).await;
    assert_eq!(
        settled,
        status::UPDATE_COMPLETE,
        "case {case:?}: a stack republishing its OWN export must complete, got \
         {settled:?} ({reason:?})"
    );

    // ...and once the holder is gone the name is free: a deleted stack publishes nothing.
    let (code, body) = call(&router, "DeleteStack", &[member("StackName", HOLDER)]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: DeleteStack must be accepted, got {code} {body}"
    );
    wait_for_the_transition().await;
    let (code, body) = create_stack(
        &router,
        "export-successor",
        &exporting("per-region-name", "successor-topic"),
        &[],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: the successor's create must be accepted, got {code} {body}"
    );
    wait_for_the_transition().await;
    let (settled, reason) = reported(&router, "export-successor").await;
    assert_eq!(
        settled,
        status::CREATE_COMPLETE,
        "case {case:?}: the name a deleted stack released must deploy, got {settled:?} \
         ({reason:?})"
    );
    let (_, listed) = call_in(&router, REGION, "ListExports", &[]).await;
    assert_eq!(
        xml_texts(&listed, "Name"),
        vec!["per-region-name".to_string()],
        "case {case:?}: the Region must publish the successor's export alone, got {listed}"
    );
}
