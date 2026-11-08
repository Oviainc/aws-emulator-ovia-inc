// SPDX-License-Identifier: CC0-1.0

//! `ListChangeSets`, end to end over the awsQuery wire.
//!
//! The rule under test: `ListChangeSets` reports one `ChangeSetSummary` per change set the
//! store still holds for the stack named by `StackName`, in creation order and one bounded
//! page at a time, each summary projecting the SAME stored record `DescribeChangeSet`
//! reports in full — its id and name, the stack it plans changes for, the creation and
//! execution statuses it has reached with the reason that came with the first of them, the
//! instant it was created and the settings it was created with. A change set a request
//! deleted, an execution superseded or a completed stack deletion took with it is no longer
//! held and so no longer listed ("Returns the ID and status of each active change set for a
//! stack"), and a `StackName` that resolves to no stack is refused rather than answered
//! with an empty list.
//!
//! The recorded trace this suite was written for — one `CREATE` change set on a stack in
//! `REVIEW_IN_PROGRESS` — is ONE row of the table below: the rows span the change-set type,
//! the number of change sets a stack holds (none, one, several, the whole per-stack quota),
//! both template spellings, the statuses a creation settles into, and the optional members
//! a creation carries or leaves out. Every case drives the real request path (form-encoded
//! body in, XML envelope out) through the harness in `stack_lifecycle.rs`, so what is
//! asserted is what a client calling `ListChangeSets` sees.

use std::collections::BTreeSet;

use crate::change_set_lifecycle::settle_the_store;
use crate::stack_lifecycle::{
    call, call_in, create_stack, member, router_with_engine, xml_blocks, xml_text, REGION,
};
// The per-element XML reader and the timestamp shape this suite needs already exist,
// shared, in the `DescribeStackResources` suite.
use crate::stack_resources::{element, is_iso8601_instant};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    self, change_set_status, change_set_type, execution_status, MAX_CHANGE_SETS_PER_STACK,
    NO_CHANGES_REASON, PAGE_SIZE,
};
use smithy_server_runtime::MultiServiceRouter;

/// One `ChangeSetSummary` as the response carries it.
///
/// Read member by member inside its own `<member>` block, so each summary's name stays
/// paired with its own id, statuses and timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WireSummary {
    stack_id: Option<String>,
    stack_name: Option<String>,
    change_set_id: Option<String>,
    change_set_name: Option<String>,
    execution_status: Option<String>,
    status: Option<String>,
    status_reason: Option<String>,
    creation_time: Option<String>,
    description: Option<String>,
    include_nested_stacks: Option<String>,
    parent_change_set_id: Option<String>,
    root_change_set_id: Option<String>,
    import_existing_resources: Option<String>,
}

/// Every `ChangeSetSummary` in a `ListChangeSets` response, in wire order.
fn wire_summaries(xml: &str) -> Vec<WireSummary> {
    let Some(block) = xml_blocks(xml, "Summaries").into_iter().next() else {
        return Vec::new();
    };
    xml_blocks(&block, "member")
        .iter()
        .map(|entry| WireSummary {
            stack_id: element(entry, "StackId"),
            stack_name: element(entry, "StackName"),
            change_set_id: element(entry, "ChangeSetId"),
            change_set_name: element(entry, "ChangeSetName"),
            execution_status: element(entry, "ExecutionStatus"),
            status: element(entry, "Status"),
            status_reason: element(entry, "StatusReason"),
            creation_time: element(entry, "CreationTime"),
            description: element(entry, "Description"),
            include_nested_stacks: element(entry, "IncludeNestedStacks"),
            parent_change_set_id: element(entry, "ParentChangeSetId"),
            root_change_set_id: element(entry, "RootChangeSetId"),
            import_existing_resources: element(entry, "ImportExistingResources"),
        })
        .collect()
}

/// The summary's value for one wire member, so a check can walk the members by name.
fn member_of(summary: &WireSummary, tag: &str) -> Option<String> {
    match tag {
        "ChangeSetId" => summary.change_set_id.clone(),
        "ChangeSetName" => summary.change_set_name.clone(),
        "StackId" => summary.stack_id.clone(),
        "StackName" => summary.stack_name.clone(),
        "Status" => summary.status.clone(),
        "StatusReason" => summary.status_reason.clone(),
        "ExecutionStatus" => summary.execution_status.clone(),
        "CreationTime" => summary.creation_time.clone(),
        "Description" => summary.description.clone(),
        "IncludeNestedStacks" => summary.include_nested_stacks.clone(),
        "ImportExistingResources" => summary.import_existing_resources.clone(),
        other => panic!("{other} is not a member of a ChangeSetSummary"),
    }
}

async fn list_change_sets(
    router: &MultiServiceRouter,
    members: &[(String, String)],
) -> (u16, String) {
    call(router, "ListChangeSets", members).await
}

/// `ListChangeSets` for one stack, the common case.
async fn change_sets_of(router: &MultiServiceRouter, name_or_id: &str) -> (u16, String) {
    list_change_sets(router, &[member("StackName", name_or_id)]).await
}

/// The change set names a response reports, in wire order.
fn listed_names(summaries: &[WireSummary]) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| summary.change_set_name.clone().unwrap_or_default())
        .collect()
}

/// One `CreateChangeSet` request a case makes.
struct Creation<'a> {
    change_set_name: &'a str,
    change_set_type: &'a str,
    template: &'a str,
    description: Option<&'a str>,
    include_nested_stacks: bool,
}

/// An `UPDATE` change set carrying nothing optional: the shape most rows need.
fn updating<'a>(change_set_name: &'a str, template: &'a str) -> Creation<'a> {
    Creation {
        change_set_name,
        change_set_type: change_set_type::UPDATE,
        template,
        description: None,
        include_nested_stacks: false,
    }
}

/// What a row expects one creation to be summarised as, once the store has settled.
struct Summarised {
    change_set_name: &'static str,
    status: &'static str,
    execution_status: &'static str,
    status_reason: Option<&'static str>,
    description: Option<&'static str>,
    include_nested_stacks: bool,
}

/// One row of the listed-change-sets table.
struct ListCase {
    case: &'static str,
    stack_name: &'static str,
    /// The template `CreateStack` deploys first, for a row whose change sets update an
    /// existing stack; `None` for a row whose first change set is what brings the stack
    /// into existence.
    seed: Option<&'static str>,
    creations: Vec<Creation<'static>>,
    expected: Vec<Summarised>,
}

/// One topic, as JSON.
const TOPIC: &str = r#"{"Resources":{"A":{"Type":"AWS::SNS::Topic"}}}"#;
/// A second topic beside it: an addition, as JSON.
const TWO_TOPICS: &str =
    r#"{"Resources":{"A":{"Type":"AWS::SNS::Topic"},"B":{"Type":"AWS::SNS::Topic"}}}"#;
/// The same one topic with a property set: a modification, as YAML.
const TOPIC_WITH_DISPLAY_NAME: &str = concat!(
    "Resources:\n",
    "  A:\n",
    "    Type: AWS::SNS::Topic\n",
    "    Properties:\n",
    "      DisplayName: listed\n",
);

/// The class the rule covers: a stack holding no change set, one, or several of either
/// type, whose creations settle to the statuses a creation can reach and carry or leave out
/// the optional members. The recorded trace's own case — a single `CREATE` change set on a
/// stack nothing had created yet — is the first row.
fn list_cases() -> Vec<ListCase> {
    vec![
        ListCase {
            case: "one CREATE change set, the recorded trace's own flow",
            stack_name: "cs-list-create",
            seed: None,
            creations: vec![Creation {
                change_set_name: "cs-list-create-set",
                change_set_type: change_set_type::CREATE,
                template: TOPIC,
                description: None,
                include_nested_stacks: false,
            }],
            expected: vec![Summarised {
                change_set_name: "cs-list-create-set",
                status: change_set_status::CREATE_COMPLETE,
                execution_status: execution_status::AVAILABLE,
                status_reason: None,
                description: None,
                include_nested_stacks: false,
            }],
        },
        ListCase {
            case: "a stack holding no change set at all",
            stack_name: "cs-list-none",
            seed: Some(TOPIC),
            creations: Vec::new(),
            expected: Vec::new(),
        },
        ListCase {
            case: "two UPDATE change sets, the second described and nested-aware",
            stack_name: "cs-list-two",
            seed: Some(TOPIC),
            creations: vec![
                updating("cs-list-two-first", TWO_TOPICS),
                Creation {
                    change_set_name: "cs-list-two-second",
                    change_set_type: change_set_type::UPDATE,
                    template: TOPIC_WITH_DISPLAY_NAME,
                    description: Some("gives the topic a display name"),
                    include_nested_stacks: true,
                },
            ],
            expected: vec![
                Summarised {
                    change_set_name: "cs-list-two-first",
                    status: change_set_status::CREATE_COMPLETE,
                    execution_status: execution_status::AVAILABLE,
                    status_reason: None,
                    description: None,
                    include_nested_stacks: false,
                },
                Summarised {
                    change_set_name: "cs-list-two-second",
                    status: change_set_status::CREATE_COMPLETE,
                    execution_status: execution_status::AVAILABLE,
                    status_reason: None,
                    description: Some("gives the topic a display name"),
                    include_nested_stacks: true,
                },
            ],
        },
        ListCase {
            case: "a change set whose plan is empty, beside one that changes something",
            stack_name: "cs-list-failed",
            seed: Some(TOPIC),
            creations: vec![
                updating("cs-list-failed-empty", TOPIC),
                updating("cs-list-failed-real", TWO_TOPICS),
            ],
            expected: vec![
                Summarised {
                    change_set_name: "cs-list-failed-empty",
                    status: change_set_status::FAILED,
                    execution_status: execution_status::UNAVAILABLE,
                    status_reason: Some(NO_CHANGES_REASON),
                    description: None,
                    include_nested_stacks: false,
                },
                Summarised {
                    change_set_name: "cs-list-failed-real",
                    status: change_set_status::CREATE_COMPLETE,
                    execution_status: execution_status::AVAILABLE,
                    status_reason: None,
                    description: None,
                    include_nested_stacks: false,
                },
            ],
        },
    ]
}

/// Create one change set, with the members the caller varies spelled as an SDK sends them,
/// answered with the `Id` and `StackId` `CreateChangeSet` minted.
async fn create_change_set(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    creation: &Creation<'_>,
) -> (String, String) {
    let mut members = vec![
        member("StackName", stack_name),
        member("ChangeSetName", creation.change_set_name),
        member("ChangeSetType", creation.change_set_type),
        member("TemplateBody", creation.template),
    ];
    if let Some(description) = creation.description {
        members.push(member("Description", description));
    }
    if creation.include_nested_stacks {
        members.push(member("IncludeNestedStacks", "true"));
    }
    let (status, body) = call(router, "CreateChangeSet", &members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateChangeSet to succeed, got {status} {body}"
    );
    let id = xml_text(&body, "Id")
        .unwrap_or_else(|| panic!("case {case:?}: CreateChangeSet answered no Id, got {body}"));
    let stack_id = xml_text(&body, "StackId").unwrap_or_else(|| {
        panic!("case {case:?}: CreateChangeSet answered no StackId, got {body}")
    });
    (id, stack_id)
}

/// Seed one row's stack and change sets, answered with the stack id they all belong to and
/// the change set ids `CreateChangeSet` minted, in creation order.
async fn seed_case(
    router: &MultiServiceRouter,
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    case: &ListCase,
) -> (Option<String>, Vec<String>) {
    let mut stack_id = None;
    if let Some(seed) = case.seed {
        let (status, body) = create_stack(router, case.stack_name, seed, &[]).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateStack to succeed, got {status} {body}",
            case.case
        );
        stack_id = xml_text(&body, "StackId");
        settle_the_store(router, engine).await;
    }
    let mut created_ids = Vec::new();
    for creation in &case.creations {
        let (id, created_stack_id) =
            create_change_set(router, case.case, case.stack_name, creation).await;
        created_ids.push(id);
        stack_id = Some(created_stack_id);
        // Settle between creations, so each change set is planned against a stack whose own
        // last operation has completed.
        settle_the_store(router, engine).await;
    }
    settle_the_store(router, engine).await;
    (stack_id, created_ids)
}

#[tokio::test]
async fn a_stack_lists_every_change_set_it_holds_in_creation_order() {
    for row in list_cases() {
        // A router per row, so each row's stack owns the resources its templates name.
        let (router, engine) = router_with_engine();
        let (stack_id, created_ids) = seed_case(&router, &engine, &row).await;
        let case = row.case;

        let (status, body) = change_sets_of(&router, row.stack_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListChangeSets to succeed, got {status} {body}"
        );
        let summaries = wire_summaries(&body);
        assert_eq!(
            listed_names(&summaries),
            row.expected
                .iter()
                .map(|expected| expected.change_set_name.to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected one summary per held change set, in creation order, got \
             {body}"
        );
        assert_eq!(
            xml_text(&body, "NextToken"),
            None,
            "case {case:?}: a single-page answer carries no NextToken, got {body}"
        );

        for (index, (expected, actual)) in row.expected.iter().zip(summaries.iter()).enumerate()
        {
            assert_eq!(
                actual.change_set_id.as_deref(),
                Some(created_ids[index].as_str()),
                "case {case:?}: the summary carries the id CreateChangeSet minted, got \
                 {actual:?}"
            );
            assert_eq!(
                actual.stack_id.as_deref(),
                stack_id.as_deref(),
                "case {case:?}: the summary names the stack the change set plans for, got \
                 {actual:?}"
            );
            assert_eq!(
                actual.stack_name.as_deref(),
                Some(row.stack_name),
                "case {case:?}: the summary names that stack by name, got {actual:?}"
            );
            assert_eq!(
                actual.status.as_deref(),
                Some(expected.status),
                "case {case:?}: expected the settled ChangeSetStatus, got {actual:?}"
            );
            assert_eq!(
                actual.execution_status.as_deref(),
                Some(expected.execution_status),
                "case {case:?}: expected the settled ExecutionStatus, got {actual:?}"
            );
            assert_eq!(
                actual.status_reason.as_deref(),
                expected.status_reason,
                "case {case:?}: expected the reason that came with the status, got {actual:?}"
            );
            assert_eq!(
                actual.description.as_deref(),
                expected.description,
                "case {case:?}: expected the description the creation carried, got {actual:?}"
            );
            assert_eq!(
                actual.include_nested_stacks.as_deref(),
                Some(if expected.include_nested_stacks {
                    "true"
                } else {
                    "false"
                }),
                "case {case:?}: expected the IncludeNestedStacks setting, got {actual:?}"
            );
            assert_eq!(
                actual.import_existing_resources.as_deref(),
                Some("false"),
                "case {case:?}: no creation imported existing resources, got {actual:?}"
            );
            let creation_time = actual.creation_time.clone().unwrap_or_default();
            assert!(
                is_iso8601_instant(&creation_time),
                "case {case:?}: expected the instant the change set was created, got \
                 {creation_time:?}"
            );
            assert_eq!(
                (
                    actual.parent_change_set_id.as_deref(),
                    actual.root_change_set_id.as_deref()
                ),
                (None, None),
                "case {case:?}: nothing here is a nested change set, so it has no parent or \
                 root, got {actual:?}"
            );
        }
    }
}

/// `ListChangeSets` and `DescribeChangeSet` are two reads of ONE stored change set, so they
/// agree on every member the summary projects.
#[tokio::test]
async fn the_listed_and_described_change_sets_of_one_stack_agree() {
    for row in list_cases() {
        let (router, engine) = router_with_engine();
        seed_case(&router, &engine, &row).await;
        let case = row.case;

        let (_, body) = change_sets_of(&router, row.stack_name).await;
        for summary in wire_summaries(&body) {
            let change_set_id = summary.change_set_id.clone().unwrap_or_default();
            let (status, described) = call(
                &router,
                "DescribeChangeSet",
                &[member("ChangeSetName", &change_set_id)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: every listed change set must be describable, got {status} \
                 {described}"
            );
            for tag in [
                "ChangeSetId",
                "ChangeSetName",
                "StackId",
                "StackName",
                "Status",
                "StatusReason",
                "ExecutionStatus",
                "CreationTime",
                "Description",
                "IncludeNestedStacks",
                "ImportExistingResources",
            ] {
                assert_eq!(
                    member_of(&summary, tag),
                    xml_text(&described, tag),
                    "case {case:?}: the listed and described {tag} of {change_set_id:?} must \
                     agree, got {described}"
                );
            }
        }
    }
}

/// The stack is named the way every other read of a stack names it, and a name that
/// resolves to no stack in the Region the request was signed for is refused.
#[tokio::test]
async fn the_stack_is_named_by_name_or_by_stack_id_and_an_unknown_name_is_refused() {
    let case = "naming the stack";
    let (router, engine) = router_with_engine();
    let (status, body) = create_stack(&router, "cs-list-named", TOPIC, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateStack to succeed, got {status} {body}"
    );
    let stack_id = xml_text(&body, "StackId").expect("CreateStack answers a StackId");
    settle_the_store(&router, &engine).await;
    create_change_set(
        &router,
        case,
        "cs-list-named",
        &updating("cs-list-named-set", TWO_TOPICS),
    )
    .await;
    settle_the_store(&router, &engine).await;

    // (case, how the stack is named)
    for (case, name) in [
        ("the stack's name", "cs-list-named"),
        ("the stack's unique id", stack_id.as_str()),
    ] {
        let (status, body) = change_sets_of(&router, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListChangeSets to succeed, got {status} {body}"
        );
        assert_eq!(
            listed_names(&wire_summaries(&body)),
            vec!["cs-list-named-set".to_string()],
            "case {case:?}: the same stack's change sets, however it is named, got {body}"
        );
    }

    // (case, the StackName asked for)
    for (case, name) in [
        ("a name no stack has", "cs-list-absent"),
        (
            "a name differing only in case from a live stack",
            "CS-LIST-NAMED",
        ),
    ] {
        let (status, body) = change_sets_of(&router, name).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an unaddressable stack is refused, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected ValidationError, got {body}"
        );
    }

    // A stack belongs to the Region the call that created it was signed for, so a request
    // signed for another Region resolves nothing by that name.
    let elsewhere = "us-west-2";
    assert_ne!(
        elsewhere, REGION,
        "the cross-Region case must be signed for another Region"
    );
    let (status, body) = call_in(
        &router,
        elsewhere,
        "ListChangeSets",
        &[member("StackName", "cs-list-named")],
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"another Region's request\": the stack is not in that Region, got {status} \
         {body}"
    );
}

/// A change set stops being listed when it stops being held: deleted by request, deleted
/// because another change set of its stack was executed, or taken by its stack's deletion.
#[tokio::test]
async fn a_change_set_that_is_no_longer_held_is_no_longer_listed() {
    let case = "a change set that is no longer held";
    let (router, engine) = router_with_engine();
    let (_, body) = create_stack(&router, "cs-list-gone", TOPIC, &[]).await;
    let stack_id = xml_text(&body, "StackId").expect("CreateStack answers a StackId");
    settle_the_store(&router, &engine).await;

    for creation in [
        updating("cs-list-gone-a", TWO_TOPICS),
        updating("cs-list-gone-b", TOPIC_WITH_DISPLAY_NAME),
        updating("cs-list-gone-c", TWO_TOPICS),
    ] {
        create_change_set(&router, case, "cs-list-gone", &creation).await;
        settle_the_store(&router, &engine).await;
    }
    let (_, body) = change_sets_of(&router, "cs-list-gone").await;
    assert_eq!(
        listed_names(&wire_summaries(&body)),
        vec![
            "cs-list-gone-a".to_string(),
            "cs-list-gone-b".to_string(),
            "cs-list-gone-c".to_string()
        ],
        "case \"three change sets of one stack\": all three are held, got {body}"
    );

    let (status, body) = call(
        &router,
        "DeleteChangeSet",
        &[
            member("ChangeSetName", "cs-list-gone-b"),
            member("StackName", "cs-list-gone"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a change set deleted by request\": expected DeleteChangeSet to succeed, got \
         {status} {body}"
    );
    let (_, body) = change_sets_of(&router, "cs-list-gone").await;
    assert_eq!(
        listed_names(&wire_summaries(&body)),
        vec!["cs-list-gone-a".to_string(), "cs-list-gone-c".to_string()],
        "case \"a change set deleted by request\": the deleted one is no longer listed, got \
         {body}"
    );

    // "When you execute a change set, CloudFormation deletes all other change sets
    // associated with the stack": the executed one is still held, and it alone is listed.
    let (status, body) = call(
        &router,
        "ExecuteChangeSet",
        &[
            member("ChangeSetName", "cs-list-gone-a"),
            member("StackName", "cs-list-gone"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a change set superseded by an execution\": expected ExecuteChangeSet to \
         succeed, got {status} {body}"
    );
    settle_the_store(&router, &engine).await;
    let (_, body) = change_sets_of(&router, "cs-list-gone").await;
    let summaries = wire_summaries(&body);
    assert_eq!(
        listed_names(&summaries),
        vec!["cs-list-gone-a".to_string()],
        "case \"a change set superseded by an execution\": only the executed one is held, \
         got {body}"
    );
    assert_eq!(
        summaries[0].execution_status.as_deref(),
        Some(execution_status::EXECUTE_COMPLETE),
        "case \"a change set superseded by an execution\": the executed one reports its \
         execution, got {body}"
    );

    let (status, body) =
        call(&router, "DeleteStack", &[member("StackName", "cs-list-gone")]).await;
    assert_eq!(
        status, 200,
        "case \"a change set taken by its stack's deletion\": expected DeleteStack to \
         succeed, got {status} {body}"
    );
    settle_the_store(&router, &engine).await;
    // A deleted stack is addressable by its unique id alone, and holds no change set.
    let (status, body) = change_sets_of(&router, &stack_id).await;
    assert_eq!(
        status, 200,
        "case \"a change set taken by its stack's deletion\": a deleted stack is still read \
         by its id, got {status} {body}"
    );
    assert_eq!(
        listed_names(&wire_summaries(&body)),
        Vec::<String>::new(),
        "case \"a change set taken by its stack's deletion\": its change sets went with it, \
         got {body}"
    );
}

/// Every change set a stack can hold fits one page: the per-stack quota is below the page
/// bound, so a full stack is answered whole and carries no `NextToken`.
#[tokio::test]
async fn a_stack_at_its_change_set_quota_is_answered_in_one_page() {
    let case = "a stack at its change set quota";
    // Read into locals so the invariant is asserted with its values named.
    let (quota, page) = (MAX_CHANGE_SETS_PER_STACK, PAGE_SIZE);
    assert!(
        quota <= page,
        "case {case:?}: a stack's change sets cannot exceed one page: quota {quota}, page \
         {page}"
    );
    let (router, engine) = router_with_engine();
    let names: Vec<String> = (0..MAX_CHANGE_SETS_PER_STACK)
        .map(|index| format!("cs-list-quota-set-{index:02}"))
        .collect();
    for name in &names {
        create_change_set(
            &router,
            case,
            "cs-list-quota",
            &Creation {
                change_set_name: name,
                change_set_type: change_set_type::CREATE,
                template: TOPIC,
                description: None,
                include_nested_stacks: false,
            },
        )
        .await;
    }
    settle_the_store(&router, &engine).await;

    let (status, body) = change_sets_of(&router, "cs-list-quota").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListChangeSets to succeed, got {status} {body}"
    );
    let listed = listed_names(&wire_summaries(&body));
    assert_eq!(
        listed, names,
        "case {case:?}: every change set once, in creation order, got {listed:?}"
    );
    let distinct: BTreeSet<&String> = listed.iter().collect();
    assert_eq!(
        distinct.len(),
        MAX_CHANGE_SETS_PER_STACK,
        "case {case:?}: no change set is listed twice, got {} of {MAX_CHANGE_SETS_PER_STACK}",
        distinct.len()
    );
    assert_eq!(
        xml_text(&body, "NextToken"),
        None,
        "case {case:?}: one page holds them all, got {body}"
    );
}

#[tokio::test]
async fn a_next_token_this_operation_never_issued_is_refused() {
    let (router, engine) = router_with_engine();
    let (_, stack_id) = create_change_set(
        &router,
        "a token this operation never issued",
        "cs-list-token",
        &Creation {
            change_set_name: "cs-list-token-set",
            change_set_type: change_set_type::CREATE,
            template: TOPIC,
            description: None,
            include_nested_stacks: false,
        },
    )
    .await;
    settle_the_store(&router, &engine).await;

    // A cursor past the end of the stack's change sets is not one this operation handed out
    // for it, and neither is one issued for a different collection, nor an opaque string
    // this service never minted.
    let cases: Vec<(&str, String)> = vec![
        (
            "a cursor past the end",
            state::encode_next_token(state::PageOf::ChangeSetSummaries, 2),
        ),
        (
            "a cursor issued for another collection",
            state::encode_next_token(state::PageOf::Changes, 0),
        ),
        (
            "a string this service never issued",
            "not-a-token-we-issued".to_string(),
        ),
    ];
    for (case, token) in cases {
        let (status, body) = list_change_sets(
            &router,
            &[
                member("StackName", "cs-list-token"),
                member("NextToken", &token),
            ],
        )
        .await;
        assert_eq!(status, 400, "case {case:?}: refused, got {status} {body}");
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected ValidationError, got {body}"
        );
    }

    // The cursor at the start of THIS stack's collection IS accepted, so the refusals above
    // are about the token and not about the operation: the token a paginated read of one
    // stack hands out is scoped to that stack.
    let first = state::encode_scoped_next_token(
        state::PageOf::ChangeSetSummaries,
        state::PageScope(&stack_id),
        0,
    );
    let (status, body) = list_change_sets(
        &router,
        &[
            member("StackName", "cs-list-token"),
            member("NextToken", &first),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a cursor at the start of this collection\": accepted, got {status} {body}"
    );
    assert_eq!(
        listed_names(&wire_summaries(&body)),
        vec!["cs-list-token-set".to_string()],
        "case \"a cursor at the start of this collection\": the whole stack, got {body}"
    );
}
