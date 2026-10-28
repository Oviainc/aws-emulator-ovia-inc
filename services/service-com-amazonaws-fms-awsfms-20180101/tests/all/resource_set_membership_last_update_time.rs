// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `LastUpdateTime` a Firewall Manager resource set
//! reports after its MEMBERSHIP changed.
//!
//! Every test drives the service the way a client does: an awsJson1_1 `POST /`
//! through [`MultiServiceRouter::dispatch`], with the `X-Amz-Target` header and a
//! SigV4 credential scope, so the whole deserialize → handle → serialize walk
//! runs and the assertions are about the WIRE response.
//!
//! The rule under test is one sentence: `ResourceSet$LastUpdateTime` is "the last
//! time that the resource set was changed", and which resources a set holds is
//! part of the set — so every write that adds or removes a member records the
//! instant it ran as the set's last update, for every reader of it, while a write
//! that leaves the membership identical changed nothing and leaves the timestamp
//! exactly where it was.
//!
//! The table in [`cases`] is the class that rule ranges over: the rows differ in
//! which membership writer runs (`BatchAssociateResource`,
//! `BatchDisassociateResource`), in how many items the call carries (none, one,
//! several), in how many resource types the set declares, in whether the items
//! change the membership at all (a new member, a member the set already holds, a
//! resource it never held) and in whether the batch also carries items that could
//! never be members — so the two outcomes the rule has, moved and unmoved, are
//! each reached from several directions.
//!
//! Before the call under test every row's stored set is dated [`BACKDATED_BY`]
//! into the past, so "the timestamp moved" is decided at a distance no timestamp
//! precision can blur, and a moved timestamp is then pinned to the instant of the
//! call rather than merely being later than before.

use std::time::{Duration, SystemTime};

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

// The wire harness lives in one place for this test target: `policy_lifecycle`
// owns it, and the administrator helpers (`associate_admin`, `with_store`) live
// with the gate suite — so no two suites drift in how a request is signed or the
// store is reached.
use crate::administrator_gated_operations::{associate_admin, with_store};
use crate::policy_lifecycle::{call, emulator_with_engine, ACCOUNT, REGION};

/// How far into the past a row's stored resource set is dated before the call
/// under test runs. Far enough that a timestamp reported in whole seconds still
/// separates the document write from the membership change, so the assertions do
/// not depend on the wire timestamp's precision.
const BACKDATED_BY: Duration = Duration::from_secs(600);

/// A second Region, for an item whose ARN names another Region than the request.
const OTHER_REGION: &str = "eu-central-1";

/// An EC2 instance ARN of this account and Region, one per index.
fn instance(index: usize) -> String {
    format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/i-{index:017x}")
}

/// A Network Firewall ARN of this account and Region.
fn firewall(name: &str) -> String {
    format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/{name}")
}

/// One row of the table: a name for the assertion messages, the set's
/// `ResourceTypeList`, the resources associated with it before the call, the
/// membership writer the row calls with which `Items`, and whether those items
/// change which resources the set holds.
struct Case {
    name: &'static str,
    resource_types: Vec<&'static str>,
    members: Vec<String>,
    operation: &'static str,
    items: Vec<String>,
    changes_membership: bool,
}

/// The class the rule is asserted over.
///
/// The `ResourceTypeList` entries and the resource ARNs are matched to each
/// other, because `ResourceSet$ResourceTypeList` "determines the resources that
/// can be associated to the resource set" — so every row's members really are
/// its membership before the call under test runs.
fn cases() -> Vec<Case> {
    vec![
        Case {
            // The smallest change there is: one resource joins an empty set.
            name: "associates-a-first-member",
            resource_types: vec!["AWS::EC2::Instance"],
            members: Vec::new(),
            operation: "BatchAssociateResource",
            items: vec![instance(0)],
            changes_membership: true,
        },
        Case {
            // Several members at once, across two declared resource types.
            name: "associates-two-members-of-two-types",
            resource_types: vec!["AWS::EC2::Instance", "AWS::NetworkFirewall::Firewall"],
            members: Vec::new(),
            operation: "BatchAssociateResource",
            items: vec![instance(0), firewall("edge")],
            changes_membership: true,
        },
        Case {
            // One item is already a member and one is not: the set still changed.
            name: "associates-one-new-member-beside-one-already-held",
            resource_types: vec!["AWS::EC2::Instance"],
            members: vec![instance(0)],
            operation: "BatchAssociateResource",
            items: vec![instance(0), instance(1)],
            changes_membership: true,
        },
        Case {
            // Associating a URI the set already holds is the same association, so
            // nothing about the set changed.
            name: "associates-only-members-already-held",
            resource_types: vec!["AWS::EC2::Instance"],
            members: vec![instance(0), instance(1)],
            operation: "BatchAssociateResource",
            items: vec![instance(1), instance(0)],
            changes_membership: false,
        },
        Case {
            name: "associates-no-items-at-all",
            resource_types: vec!["AWS::EC2::Instance"],
            members: vec![instance(0)],
            operation: "BatchAssociateResource",
            items: Vec::new(),
            changes_membership: false,
        },
        Case {
            // A successful call whose every item is reported in `FailedItems`
            // associated nothing, so the set did not change.
            name: "associates-only-items-that-cannot-be-members",
            resource_types: vec!["AWS::EC2::Instance"],
            members: vec![instance(0)],
            operation: "BatchAssociateResource",
            items: vec![
                "this-is-not-an-arn-at-all".to_string(),
                format!("arn:aws:ec2:{OTHER_REGION}:{ACCOUNT}:instance/i-0"),
            ],
            changes_membership: false,
        },
        Case {
            name: "disassociates-one-of-two-members",
            resource_types: vec!["AWS::EC2::Instance"],
            members: vec![instance(0), instance(1)],
            operation: "BatchDisassociateResource",
            items: vec![instance(0)],
            changes_membership: true,
        },
        Case {
            name: "disassociates-every-member-of-two-types",
            resource_types: vec!["AWS::EC2::Instance", "AWS::NetworkFirewall::Firewall"],
            members: vec![instance(0), firewall("edge")],
            operation: "BatchDisassociateResource",
            items: vec![firewall("edge"), instance(0)],
            changes_membership: true,
        },
        Case {
            // Nothing to remove: the URI is associable but was never a member.
            name: "disassociates-a-resource-the-set-never-held",
            resource_types: vec!["AWS::EC2::Instance"],
            members: vec![instance(0)],
            operation: "BatchDisassociateResource",
            items: vec![instance(7)],
            changes_membership: false,
        },
        Case {
            name: "disassociates-no-items-at-all",
            resource_types: vec!["AWS::EC2::Instance"],
            members: vec![instance(0)],
            operation: "BatchDisassociateResource",
            items: Vec::new(),
            changes_membership: false,
        },
    ]
}

/// Epoch seconds as the wire reports a timestamp, for comparing a reported
/// `LastUpdateTime` with an instant this test observed.
fn epoch_seconds(when: SystemTime) -> f64 {
    when.duration_since(SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64()
}

/// Create a resource set holding `resource_types`, associate `members` with it,
/// and return the minted `Id`.
async fn create_set_with_members(
    router: &MultiServiceRouter,
    case_name: &str,
    resource_types: &[&str],
    members: &[String],
) -> String {
    let (status, created) = call(
        router,
        "PutResourceSet",
        &json!({
            "ResourceSet": {
                "Name": format!("set-{case_name}"),
                "ResourceTypeList": resource_types,
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case_name:?}: expected PutResourceSet to answer 200, got {status} with {created}"
    );
    let id = created["ResourceSet"]["Id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !id.is_empty(),
        "case {case_name:?}: expected a minted Id, got {created}"
    );

    if !members.is_empty() {
        let (status, body) = call(
            router,
            "BatchAssociateResource",
            &json!({ "ResourceSetIdentifier": id, "Items": members }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case_name:?}: expected BatchAssociateResource to answer 200, got {status} with \
             {body}"
        );
        assert_eq!(
            body["FailedItems"].as_array().map(Vec::len),
            Some(0),
            "case {case_name:?}: every seeded member must associate, got {body}"
        );
    }
    id
}

/// Date one stored resource set's last update [`BACKDATED_BY`] into the past, so
/// the instant a later membership change records is separated from it by more
/// than any timestamp precision can blur.
fn backdate(engine: &std::sync::Arc<engine::InMemoryEngine>, case_name: &str, id: &str) {
    with_store(engine, |store| {
        let record = store
            .resource_sets
            .get_mut(id)
            .unwrap_or_else(|| panic!("case {case_name:?}: the created resource set is stored"));
        record.updated_at -= BACKDATED_BY;
    });
}

/// The `LastUpdateTime` `GetResourceSet` reports for one set, as epoch seconds.
async fn reported_last_update_time(
    router: &MultiServiceRouter,
    case_name: &str,
    id: &str,
) -> f64 {
    let (status, body) = call(router, "GetResourceSet", &json!({ "Identifier": id })).await;
    assert_eq!(
        status, 200,
        "case {case_name:?}: expected GetResourceSet to answer 200, got {status} with {body}"
    );
    body["ResourceSet"]["LastUpdateTime"]
        .as_f64()
        .unwrap_or_else(|| {
            panic!(
                "case {case_name:?}: GetResourceSet must report a numeric LastUpdateTime, got \
                 {body}"
            )
        })
}

/// The `LastUpdateTime` the `ListResourceSets` summary of one set reports, as
/// epoch seconds: the second reader of the same stored instant.
async fn summarized_last_update_time(
    router: &MultiServiceRouter,
    case_name: &str,
    id: &str,
) -> f64 {
    let (status, body) = call(router, "ListResourceSets", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case_name:?}: expected ListResourceSets to answer 200, got {status} with {body}"
    );
    let summary = body["ResourceSets"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case_name:?}: no ResourceSets array in {body}"))
        .iter()
        .find(|summary| summary["Id"].as_str() == Some(id))
        .cloned()
        .unwrap_or_else(|| panic!("case {case_name:?}: the set must be listed, got {body}"));
    summary["LastUpdateTime"].as_f64().unwrap_or_else(|| {
        panic!(
            "case {case_name:?}: the summary must report a numeric LastUpdateTime, got {summary}"
        )
    })
}

/// The membership `ListResourceSetResources` reports, in ascending URI order —
/// the modelled reader of which resources the set holds.
async fn reported_membership(
    router: &MultiServiceRouter,
    case_name: &str,
    id: &str,
) -> Vec<String> {
    let (status, body) = call(
        router,
        "ListResourceSetResources",
        &json!({ "Identifier": id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case_name:?}: expected ListResourceSetResources to answer 200, got {status} with \
         {body}"
    );
    let mut reported: Vec<String> = body["Items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case_name:?}: no Items array in {body}"))
        .iter()
        .map(|item| item["URI"].as_str().unwrap_or_default().to_string())
        .collect();
    reported.sort();
    reported
}

/// A membership write moves the set's `LastUpdateTime` to the instant it ran
/// exactly when it changed which resources the set holds, and leaves it alone
/// when it changed nothing — for every row of the table, and for both readers of
/// the member.
#[tokio::test]
async fn a_membership_change_moves_the_last_update_time() {
    for case in cases() {
        let (router, engine) = emulator_with_engine();
        associate_admin(&router).await;
        let id =
            create_set_with_members(&router, case.name, &case.resource_types, &case.members).await;
        backdate(&engine, case.name, &id);

        let before_time = reported_last_update_time(&router, case.name, &id).await;
        let before_membership = reported_membership(&router, case.name, &id).await;

        // The window the call ran in, floored and ceiled so the assertion holds
        // whatever precision the wire timestamp carries.
        let started = epoch_seconds(SystemTime::now()).floor();
        let (status, body) = call(
            &router,
            case.operation,
            &json!({ "ResourceSetIdentifier": id, "Items": case.items }),
        )
        .await;
        let finished = epoch_seconds(SystemTime::now()).ceil();
        assert_eq!(
            status, 200,
            "case {:?}: expected {} to answer 200, got {status} with {body}",
            case.name, case.operation
        );

        let after_time = reported_last_update_time(&router, case.name, &id).await;
        let after_membership = reported_membership(&router, case.name, &id).await;
        let membership_changed = after_membership != before_membership;
        assert_eq!(
            membership_changed, case.changes_membership,
            "case {:?}: expected the membership to change: {}; it went from {before_membership:?} \
             to {after_membership:?}",
            case.name, case.changes_membership
        );

        if case.changes_membership {
            assert!(
                after_time > before_time,
                "case {:?}: a membership change must move LastUpdateTime, which stayed at \
                 {before_time} (now {after_time})",
                case.name
            );
            assert!(
                (started..=finished).contains(&after_time),
                "case {:?}: LastUpdateTime must be the instant the membership changed, in \
                 {started}..={finished}, got {after_time}",
                case.name
            );
        } else {
            assert_eq!(
                after_time, before_time,
                "case {:?}: a call that changed no membership must leave LastUpdateTime at \
                 {before_time}, got {after_time}",
                case.name
            );
        }

        // Both readers of the member report the one stored instant.
        let summarized = summarized_last_update_time(&router, case.name, &id).await;
        assert_eq!(
            summarized, after_time,
            "case {:?}: the ListResourceSets summary must report the same LastUpdateTime as \
             GetResourceSet ({after_time}), got {summarized}",
            case.name
        );
    }
}

/// Repeated membership writes each move the timestamp on, so a client polling
/// `LastUpdateTime` sees every change: the set's last update strictly advances
/// across a sequence of changing writes, and the reported membership is the
/// sequence's own result.
#[tokio::test]
async fn each_successive_change_advances_the_last_update_time() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;
    let id = create_set_with_members(
        &router,
        "successive",
        &["AWS::EC2::Instance", "AWS::NetworkFirewall::Firewall"],
        &[],
    )
    .await;

    // Each step: the writer, the items, and the membership the set holds after it.
    let steps: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        (
            "BatchAssociateResource",
            vec![instance(0), instance(1)],
            vec![instance(0), instance(1)],
        ),
        (
            "BatchAssociateResource",
            vec![firewall("edge")],
            vec![firewall("edge"), instance(0), instance(1)],
        ),
        (
            "BatchDisassociateResource",
            vec![instance(0)],
            vec![firewall("edge"), instance(1)],
        ),
        (
            "BatchDisassociateResource",
            vec![firewall("edge"), instance(1)],
            Vec::new(),
        ),
    ];

    let mut previous = reported_last_update_time(&router, "successive", &id).await;
    for (step, (operation, items, expected)) in steps.into_iter().enumerate() {
        let case = format!("successive step {step}");
        // Dated back before the step so the advance is visible whatever precision
        // the wire timestamp carries; the step's own instant is what must replace
        // it.
        backdate(&engine, &case, &id);
        let dated_back = reported_last_update_time(&router, &case, &id).await;
        assert!(
            dated_back < previous,
            "case {case:?}: the set was dated back to {dated_back}, which must precede {previous}"
        );

        let (status, body) = call(
            &router,
            operation,
            &json!({ "ResourceSetIdentifier": id, "Items": items }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} with {body}"
        );

        let mut want = expected;
        want.sort();
        assert_eq!(
            reported_membership(&router, &case, &id).await,
            want,
            "case {case:?}: the membership after the step must be {want:?}"
        );

        let now_reported = reported_last_update_time(&router, &case, &id).await;
        assert!(
            now_reported > dated_back,
            "case {case:?}: every change must move LastUpdateTime on from {dated_back}, got \
             {now_reported}"
        );
        previous = now_reported;
    }
}
