// SPDX-License-Identifier: CC0-1.0

//! `DeleteAutomatedReasoningPolicyBuildWorkflow` end-to-end: the build workflow
//! `StartAutomatedReasoningPolicyBuildWorkflow` started is removed from the store, over the
//! same `MultiServiceRouter` the emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A deleted build workflow is gone from every read**: the delete answers `202` with no
//!    members, a later `GetAutomatedReasoningPolicyBuildWorkflow` and
//!    `CancelAutomatedReasoningPolicyBuildWorkflow` are both `404` carrying the service's
//!    wording for a build workflow it does not hold, and
//!    `ListAutomatedReasoningPolicyBuildWorkflows` reports exactly the policy's remaining
//!    workflows, unchanged. The rows span every `AutomatedReasoningPolicyBuildWorkflowStatus`
//!    the model admits — the operation documents no status precondition — several workflow
//!    types, policies holding one and several workflows, which of them the delete addresses,
//!    and a `lastUpdatedAt` spelled as the read reported it and truncated to whole
//!    milliseconds.
//! 2. **`lastUpdatedAt` is the optimistic concurrency token**: a delete naming any instant
//!    other than the write the workflow holds is refused with `ConflictException` and leaves
//!    the workflow readable, over instants on both sides of the stored one, at millisecond
//!    and second distance, and at the epoch and far in the future.
//! 3. **A workflow is addressed by its policy AND its id**: a `policyArn` naming no policy of
//!    this account, a `buildWorkflowId` nothing started, and one policy's workflow id under
//!    another policy are each refused `404` and leave the stored workflow where it was.
//! 4. **A deleted workflow takes its artifacts with it, and frees the policy**: the test runs
//!    a test workflow scheduled against it are gone, the policy's tests themselves survive,
//!    the policy a deleted workflow was still building accepts another build, and a start
//!    replaying the `clientRequestToken` of a deleted workflow mints a NEW one.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="DeleteAutomatedReasoningPolicyBuildWorkflow")`:
//! `@http(code: 202, method: "DELETE")`, `lastUpdatedAt` bound to the `updatedAt` query
//! parameter as a `date-time` timestamp, `ConflictException` 400, `ResourceNotFoundException`
//! 404), and from its documentation ("Deletes an Automated Reasoning policy build workflow
//! and its associated artifacts. This permanently removes the workflow history and any
//! generated assets"; `lastUpdatedAt` "is used for optimistic concurrency control to prevent
//! accidental deletion of workflows that have been modified"). The requests go through
//! `crate::support` — the ONE harness every suite in this crate drives the emulator with.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

use crate::support::{
    age_build_workflow, at_millis, emulator, epoch_millis, label, seed_build_workflow, Emulator,
    Reply, ACCOUNT, REGION,
};

/// The status `DeleteAutomatedReasoningPolicyBuildWorkflow` answers a delete it accepted with
/// (`@http(code: 202)`).
const DELETE_ACCEPTED: u16 = 202;

/// The `ResourceNotFoundException` message a request ADDRESSED BY a build workflow id the
/// store does not hold is answered with — the wording Amazon Bedrock answers for that error
/// class, which names neither the policy nor the id the request carried, so it is a FIXED
/// text with no variable part to fill.
const WORKFLOW_NOT_FOUND_MESSAGE: &str = "Build WorkflowExecution not found";

/// The message the TEST surface answers for the same miss, its request NAMING the workflow
/// whose runs it reads rather than addressing that workflow: a different sentence of this
/// service, as the guardrail a request BODY provides is a different sentence from the one an
/// operation addressed by a guardrail identifier answers.
const REQUEST_WORKFLOW_NOT_FOUND_MESSAGE: &str = "Build workflow not found";

/// A source document's bytes on the wire: `AutomatedReasoningPolicyBuildDocumentBlob` is a
/// `blob` (`@length(1, 5000000)`), which this protocol carries base64-encoded — this is
/// `policy rules`.
const DOCUMENT_BLOB: &str = "cG9saWN5IHJ1bGVz";

/// The workflow type a row that is not about the type itself starts
/// (`AutomatedReasoningPolicyBuildWorkflowType$INGEST_CONTENT`).
const INGEST_CONTENT: &str = "INGEST_CONTENT";

/// How far a workflow a start accepted must be aged to be past the completion the clock
/// owes it: the whole progression the start arms, and a little more, so a workflow aged this
/// much must hold the terminal status.
const PAST_THE_BUILD: Duration =
    bedrock::state::BUILD_WORKFLOW_COMPLETE_AFTER.saturating_add(Duration::from_millis(150));

/// Long enough that the test runs a test workflow scheduled are past the completion the
/// clock owes them: the emulator makes that transition due a fraction of a second after the
/// run was scheduled, so a caller that waits this long must see the terminal status.
const PAST_THE_TEST_RUN: Duration = Duration::from_millis(900);

/// A `buildWorkflowId` nothing has started: one `AutomatedReasoningPolicyBuildWorkflowId`
/// admits (`^[a-f0-9]{8}-?[a-f0-9]{4}-?4[a-f0-9]{3}-?[89ab][a-f0-9]{3}-?[a-f0-9]{12}$`), so
/// it is refused on the store holding no such workflow rather than on its spelling.
const UNSTARTED_WORKFLOW_ID: &str = "00000000-0000-4000-8000-000000000000";

/// The three in-flight values `AutomatedReasoningPolicyBuildWorkflowStatus` admits while a
/// build is running, named by `bedrock::state` as the rest of the enum is
/// (`smithy_query(query="shape",shape="AutomatedReasoningPolicyBuildWorkflowStatus")`).
const PREPROCESSING: &str = bedrock::state::BUILD_WORKFLOW_STATUS_PREPROCESSING;
const BUILDING: &str = bedrock::state::BUILD_WORKFLOW_STATUS_BUILDING;
const TESTING: &str = bedrock::state::BUILD_WORKFLOW_STATUS_TESTING;

/// Create a policy and hand back the `policyArn` it minted.
async fn policy(emu: &Emulator, name: &str) -> String {
    let created = emu
        .call(
            "POST",
            "/automated-reasoning-policies",
            Some(json!({ "name": name })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: CreateAutomatedReasoningPolicy must answer 200, got {} {:?}",
        created.status, created.body
    );
    created.text("policyArn")
}

/// `StartAutomatedReasoningPolicyBuildWorkflow`: the `sourceContent` is the request's
/// `@httpPayload`, the policy ARN and the workflow type are path labels, and the
/// `@idempotencyToken` travels in the `x-amz-client-token` HEADER.
async fn start(
    emu: &Emulator,
    policy_arn: &str,
    workflow_type: &str,
    client_request_token: Option<&str>,
) -> Reply {
    let path = format!(
        "/automated-reasoning-policies/{}/build-workflows/{workflow_type}/start",
        label(policy_arn)
    );
    let source = json!({
        "workflowContent": {
            "documents": [{
                "document": DOCUMENT_BLOB,
                "documentContentType": "txt",
                "documentName": "returns.txt",
            }],
        },
    });
    let headers: Vec<(&str, &str)> = client_request_token
        .map(|token| vec![("x-amz-client-token", token)])
        .unwrap_or_default();
    emu.call_with_headers(REGION, "POST", &path, Some(source), &headers)
        .await
}

/// Start a build workflow of `policy_arn` and hand back the `buildWorkflowId` it minted.
async fn started_workflow(emu: &Emulator, policy_arn: &str, workflow_type: &str) -> String {
    let started = start(emu, policy_arn, workflow_type, None).await;
    assert_eq!(
        started.status, 200,
        "case {workflow_type:?}: the start must answer 200, got {} {:?}",
        started.status, started.body
    );
    started.text("buildWorkflowId")
}

async fn get_workflow(emu: &Emulator, policy_arn: &str, build_workflow_id: &str) -> Reply {
    emu.call(
        "GET",
        &format!(
            "/automated-reasoning-policies/{}/build-workflows/{build_workflow_id}",
            label(policy_arn)
        ),
        None,
    )
    .await
}

/// `CancelAutomatedReasoningPolicyBuildWorkflow`: the policy ARN and the workflow id are
/// path labels, and the request carries no body at all.
async fn cancel_workflow(emu: &Emulator, policy_arn: &str, build_workflow_id: &str) -> Reply {
    emu.call(
        "POST",
        &format!(
            "/automated-reasoning-policies/{}/build-workflows/{build_workflow_id}/cancel",
            label(policy_arn)
        ),
        None,
    )
    .await
}

/// One `DELETE /automated-reasoning-policies/{policyArn}/build-workflows/{buildWorkflowId}`,
/// carrying `last_updated_at` in the `updatedAt` query parameter the model binds it to.
///
/// The ARN is a path LABEL and the token is a query VALUE; both carry `:`, which is
/// percent-encoded so each stays one path segment or one parameter, exactly as an SDK writes
/// them.
async fn delete_workflow(
    emu: &Emulator,
    policy_arn: &str,
    build_workflow_id: &str,
    last_updated_at: &str,
) -> Reply {
    emu.call(
        "DELETE",
        &format!(
            "/automated-reasoning-policies/{}/build-workflows/{}?updatedAt={}",
            label(policy_arn),
            label(build_workflow_id),
            label(last_updated_at)
        ),
        None,
    )
    .await
}

/// Every `buildWorkflowId` a listing of `policy_arn` reports, in the order it reports them.
async fn listed_ids(case: &str, emu: &Emulator, policy_arn: &str) -> Vec<String> {
    let listed = emu
        .call(
            "GET",
            &format!(
                "/automated-reasoning-policies/{}/build-workflows?maxResults=1000",
                label(policy_arn)
            ),
            None,
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: the listing must answer 200, got {} {:?}",
        listed.status, listed.body
    );
    listed
        .get("automatedReasoningPolicyBuildWorkflowSummaries")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the summaries are a required list, got {:?}",
                listed.body
            )
        })
        .iter()
        .map(|summary| {
            summary
                .get("buildWorkflowId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `updatedAt` a read of the workflow reports, as the `date-time` text the wire carries
/// — the value a caller echoes back as `lastUpdatedAt`.
async fn stored_updated_at(emu: &Emulator, policy_arn: &str, build_workflow_id: &str) -> String {
    let read = get_workflow(emu, policy_arn, build_workflow_id).await;
    assert_eq!(
        read.status, 200,
        "case {build_workflow_id:?}: the workflow must read back before its updatedAt is \
         taken, got {} {:?}",
        read.status, read.body
    );
    read.get("updatedAt")
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {build_workflow_id:?}: updatedAt is modelled \
                 @timestampFormat(\"date-time\"), so the read must report it as an RFC 3339 \
                 string, got {:?}",
                read.body
            )
        })
        .to_string()
}

/// Assert `reply` is the answer the delete gives a request it accepted: the modelled status
/// and a body carrying no members (`DeleteAutomatedReasoningPolicyBuildWorkflowResponse`
/// declares none).
fn assert_accepted(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, DELETE_ACCEPTED,
        "case {case:?}: the delete must answer {DELETE_ACCEPTED}, got {} {:?}",
        reply.status, reply.body
    );
    assert!(
        reply.body == Value::Null || reply.body == json!({}),
        "case {case:?}: the delete's answer carries no members, got {:?}",
        reply.body
    );
    assert!(
        reply.error.is_none(),
        "case {case:?}: an accepted delete carries no error, got {:?}",
        reply.error
    );
}

/// Assert `reply` is the modelled 404 a request ADDRESSED BY a build workflow id the store
/// does not hold is refused with, carrying the service's wording for that error class.
fn assert_not_found(case: &str, reply: &Reply) {
    assert_not_found_with(case, reply, WORKFLOW_NOT_FOUND_MESSAGE);
}

/// Assert `reply` is that same modelled 404 carrying `message` — the sentence of the surface
/// that reported the miss: a request addressed BY the workflow id answers
/// [`WORKFLOW_NOT_FOUND_MESSAGE`], and the TEST surface, whose request NAMES the workflow its
/// runs belong to, answers [`REQUEST_WORKFLOW_NOT_FOUND_MESSAGE`].
fn assert_not_found_with(case: &str, reply: &Reply, message: &str) {
    assert_eq!(
        (reply.status, reply.error.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case {case:?}: the request must be refused 404, got {} {:?}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.get("message"),
        json!(message),
        "case {case:?}: the refusal must carry the service's wording for a build workflow it \
         does not hold, got {:?}",
        reply.body
    );
}

/// Store the build workflow an admitted `StartAutomatedReasoningPolicyBuildWorkflow` would
/// have written `created_ago` ago, holding `status`, and hand back its `buildWorkflowId`.
///
/// Seeded through the ONE harness every suite of this crate seeds this record with
/// ([`seed_build_workflow`]), so a seeded workflow is the workflow a start would have left.
/// Seeding is what lets a row fix the STATUS the delete finds the workflow in — every value
/// the enum admits, including the one no span of this emulator's clock leaves a workflow in
/// (`FAILED`) — without sleeping for any of them: the workflow is left owing no transition,
/// so no due instant is set and its `updatedAt` stays where the baseline put it.
fn seed_workflow(
    emu: &Emulator,
    policy_arn: &str,
    workflow_type: &str,
    status: &'static str,
    created_ago: Duration,
) -> String {
    seed_build_workflow(emu, policy_arn, workflow_type, created_ago, |record| {
        record.status = status;
        record.forget_owed_transitions();
    })
}

// ---------------------------------------------------------------------------------------
// 1. A deleted build workflow is gone from every read.
// ---------------------------------------------------------------------------------------

/// How a row spells the `lastUpdatedAt` it sends.
#[derive(Debug, Clone, Copy)]
enum Token {
    /// Exactly the text the read reported.
    AsRead,
    /// The same instant truncated to whole milliseconds — what a client carrying
    /// millisecond instants echoes back after reading a finer-grained one.
    WholeMilliseconds,
}

#[tokio::test]
async fn a_deleted_build_workflow_is_gone_from_every_read() {
    // Rows differ along the status the workflow holds when the delete addresses it — every
    // value `AutomatedReasoningPolicyBuildWorkflowStatus` admits
    // (`smithy_query(query="shape",shape="AutomatedReasoningPolicyBuildWorkflowStatus")`),
    // because the operation documents no status precondition — along the workflow type, how
    // many workflows the policy holds, which of them the delete addresses, and how the token
    // is spelled.
    struct Row {
        case: &'static str,
        status: &'static str,
        workflow_type: &'static str,
        held: usize,
        victim: usize,
        token: Token,
    }

    let rows = [
        Row {
            case: "a scheduled build, the only workflow of its policy",
            status: bedrock::state::BUILD_WORKFLOW_STATUS_SCHEDULED,
            workflow_type: INGEST_CONTENT,
            held: 1,
            victim: 0,
            token: Token::AsRead,
        },
        Row {
            case: "a preprocessing build, the first of three",
            status: PREPROCESSING,
            workflow_type: "REFINE_POLICY",
            held: 3,
            victim: 0,
            token: Token::AsRead,
        },
        Row {
            case: "a building build, a middle workflow of three",
            status: BUILDING,
            workflow_type: "IMPORT_POLICY",
            held: 3,
            victim: 1,
            token: Token::WholeMilliseconds,
        },
        Row {
            case: "a testing build, the last of three",
            status: TESTING,
            workflow_type: "GENERATE_FIDELITY_REPORT",
            held: 3,
            victim: 2,
            token: Token::AsRead,
        },
        Row {
            case: "a build winding down from a cancel",
            status: bedrock::state::BUILD_WORKFLOW_STATUS_CANCEL_REQUESTED,
            workflow_type: "GENERATE_POLICY_SCENARIOS",
            held: 2,
            victim: 1,
            token: Token::WholeMilliseconds,
        },
        Row {
            case: "a completed build",
            status: bedrock::state::BUILD_WORKFLOW_STATUS_COMPLETED,
            workflow_type: "RESOLVE_POLICY_AMBIGUITIES",
            held: 2,
            victim: 0,
            token: Token::AsRead,
        },
        Row {
            case: "a failed build",
            status: bedrock::state::BUILD_WORKFLOW_STATUS_FAILED,
            workflow_type: "ITERATIVELY_REFINE_POLICY",
            held: 1,
            victim: 0,
            token: Token::WholeMilliseconds,
        },
        Row {
            case: "a cancelled build",
            status: bedrock::state::BUILD_WORKFLOW_STATUS_CANCELLED,
            workflow_type: INGEST_CONTENT,
            held: 3,
            victim: 1,
            token: Token::AsRead,
        },
    ];

    let emu = emulator();

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // `AutomatedReasoningPolicyName` is `[0-9a-zA-Z-_ ]+`, so the row's index names its
        // policy rather than the case text.
        let policy_arn = policy(&emu, &format!("workflow deletion policy {index}")).await;
        // The workflows this policy holds, oldest first. Only the victim carries the row's
        // status; the others are terminal, so the policy admits every one of them.
        let ids: Vec<String> = (0..row.held)
            .map(|held| {
                let status = if held == row.victim {
                    row.status
                } else {
                    bedrock::state::BUILD_WORKFLOW_STATUS_COMPLETED
                };
                seed_workflow(
                    &emu,
                    &policy_arn,
                    row.workflow_type,
                    status,
                    Duration::from_secs((row.held - held) as u64),
                )
            })
            .collect();
        let victim = ids[row.victim].clone();

        // The bodies of the workflows that must survive, read BEFORE the delete, so the
        // assertion afterwards is against what the store held rather than against a
        // hand-written expectation.
        let mut survivors: Vec<(String, Value)> = Vec::new();
        for id in ids.iter().filter(|id| **id != victim) {
            survivors.push((id.clone(), get_workflow(&emu, &policy_arn, id).await.body));
        }

        let reported = stored_updated_at(&emu, &policy_arn, &victim).await;
        let token = match row.token {
            Token::AsRead => reported.clone(),
            Token::WholeMilliseconds => at_millis(case, epoch_millis(case, &reported)),
        };

        assert_accepted(case, &delete_workflow(&emu, &policy_arn, &victim, &token).await);

        assert_not_found(
            &format!("{case}: read after the delete"),
            &get_workflow(&emu, &policy_arn, &victim).await,
        );
        assert_not_found(
            &format!("{case}: cancel after the delete"),
            &cancel_workflow(&emu, &policy_arn, &victim).await,
        );
        // A delete of a workflow that is already gone is refused the same way: the pair
        // names no workflow this store holds.
        assert_not_found(
            &format!("{case}: delete after the delete"),
            &delete_workflow(&emu, &policy_arn, &victim, &token).await,
        );

        let expected: Vec<String> = ids.iter().filter(|id| **id != victim).cloned().collect();
        assert_eq!(
            listed_ids(case, &emu, &policy_arn).await,
            expected,
            "case {case:?}: the listing must report exactly the policy's remaining workflows"
        );
        for (id, before) in &survivors {
            let after = get_workflow(&emu, &policy_arn, id).await;
            assert_eq!(
                after.body, *before,
                "case {case:?}: the delete must leave workflow {id:?} exactly as it was"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// 2. `lastUpdatedAt` is the optimistic concurrency token.
// ---------------------------------------------------------------------------------------

/// The `ConflictException` message a delete whose `lastUpdatedAt` is not the write the
/// addressed workflow holds is refused with — the wording this service answers for that
/// error class, whose variable parts are the id and the policy ARN the REQUEST carried.
fn conflict_message(build_workflow_id: &str, policy_arn: &str) -> String {
    format!(
        "Conflict with deleting Build WorkflowExecution with buildWorkflowId: \
         {build_workflow_id} and policyArn: {policy_arn}"
    )
}

#[tokio::test]
async fn the_token_must_name_the_write_the_build_workflow_holds() {
    let emu = emulator();
    let policy_arn = policy(&emu, "concurrency token policy").await;
    // Two workflows, so a refusal that dropped the wrong record would show up as the other
    // one going missing. Both are terminal, so neither moves while the rows run.
    let victim = seed_workflow(
        &emu,
        &policy_arn,
        INGEST_CONTENT,
        bedrock::state::BUILD_WORKFLOW_STATUS_COMPLETED,
        Duration::from_secs(2),
    );
    let survivor = seed_workflow(
        &emu,
        &policy_arn,
        "REFINE_POLICY",
        bedrock::state::BUILD_WORKFLOW_STATUS_CANCELLED,
        Duration::from_secs(1),
    );

    let reported = stored_updated_at(&emu, &policy_arn, &victim).await;
    let held = epoch_millis("the stored instant", &reported);
    let before = get_workflow(&emu, &policy_arn, &victim).await.body;

    // Rows differ along the DISTANCE and the direction from the write the workflow holds,
    // from one millisecond either side out to the epoch and far into the future.
    let rows: Vec<(&str, String)> = vec![
        ("one millisecond earlier", at_millis("earlier", held - 1)),
        ("one millisecond later", at_millis("later", held + 1)),
        (
            "one second earlier",
            at_millis("a second earlier", held - 1_000),
        ),
        ("one second later", at_millis("a second later", held + 1_000)),
        ("a day earlier", at_millis("a day earlier", held - 86_400_000)),
        ("the epoch", at_millis("the epoch", 0)),
        // 2099-12-31T23:59:59Z, far past any instant this store can mint.
        (
            "far in the future",
            at_millis("far future", 4_102_444_799_000),
        ),
    ];

    for (case, token) in &rows {
        let refused = delete_workflow(&emu, &policy_arn, &victim, token).await;
        assert_eq!(
            (refused.status, refused.error.as_deref()),
            (400, Some("ConflictException")),
            "case {case:?}: {token:?} is not the write the workflow holds, so the delete must \
             be refused, got {} {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.get("message"),
            json!(conflict_message(&victim, &policy_arn)),
            "case {case:?}: the refusal must carry the service's wording for a conflicting \
             delete, got {:?}",
            refused.body
        );
        let read = get_workflow(&emu, &policy_arn, &victim).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: a refused delete must leave the workflow readable, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, before,
            "case {case:?}: a refused delete must change nothing about the workflow"
        );
    }

    // The token the workflow does hold deletes it, so every row above was refused on the
    // token and not on the pair.
    assert_accepted(
        "the stored instant",
        &delete_workflow(&emu, &policy_arn, &victim, &reported).await,
    );
    assert_eq!(
        listed_ids("after the accepted delete", &emu, &policy_arn).await,
        vec![survivor],
        "case after the accepted delete: only the workflow the delete did not address is left"
    );
}

// ---------------------------------------------------------------------------------------
// 3. A workflow is addressed by its policy AND its id.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_is_addressed_by_its_policy_and_its_id() {
    let emu = emulator();
    let policy_arn = policy(&emu, "owning policy").await;
    let other_policy_arn = policy(&emu, "other policy").await;
    let id = started_workflow(&emu, &policy_arn, INGEST_CONTENT).await;
    let other_id = seed_workflow(
        &emu,
        &other_policy_arn,
        "REFINE_POLICY",
        bedrock::state::BUILD_WORKFLOW_STATUS_COMPLETED,
        Duration::from_secs(1),
    );
    let token = stored_updated_at(&emu, &policy_arn, &id).await;

    // Rows: an ARN of a policy nothing minted, a well-formed ARN of another region, and the
    // policy's own versioned ARN — `AutomatedReasoningPolicyArn` admits a `:<version>`
    // suffix and this policy has no versions. None names a policy of this account, so none
    // holds the workflow the id names.
    let unknown_policies: Vec<(&str, String)> = vec![
        (
            "a policy id nothing minted",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:automated-reasoning-policy/00000000000z"),
        ),
        (
            "a policy ARN of another region",
            policy_arn.replace(REGION, "eu-west-1"),
        ),
        ("a version nothing minted", format!("{policy_arn}:1")),
    ];
    for (case, arn) in &unknown_policies {
        assert_not_found(case, &delete_workflow(&emu, arn, &id, &token).await);
    }

    // Rows: an id nothing started, and the id of a workflow ANOTHER policy of this account
    // does hold. Neither names a workflow of the policy addressed.
    let unknown_workflows: Vec<(&str, &str, &str)> = vec![
        ("an id nothing started", &policy_arn, UNSTARTED_WORKFLOW_ID),
        (
            "another policy's workflow id",
            &policy_arn,
            other_id.as_str(),
        ),
        ("this policy's workflow id elsewhere", &other_policy_arn, &id),
    ];
    for (case, arn, workflow_id) in &unknown_workflows {
        assert_not_found(case, &delete_workflow(&emu, arn, workflow_id, &token).await);
    }

    // Every row above left both workflows where they were, and deleting each THROUGH its own
    // policy removes it.
    for (case, arn, workflow_id) in [
        ("the started pair", &policy_arn, &id),
        ("the other policy's pair", &other_policy_arn, &other_id),
    ] {
        let read = get_workflow(&emu, arn, workflow_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the workflow must still read back, got {} {:?}",
            read.status, read.body
        );
        let held = stored_updated_at(&emu, arn, workflow_id).await;
        assert_accepted(case, &delete_workflow(&emu, arn, workflow_id, &held).await);
        assert!(
            listed_ids(case, &emu, arn).await.is_empty(),
            "case {case:?}: the policy holds no workflow once its only one is deleted"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 4. A deleted workflow takes its artifacts with it, and frees the policy it was building.
// ---------------------------------------------------------------------------------------

/// `CreateAutomatedReasoningPolicyTestCase`: the policy ARN is a path label and the test's
/// content travels in the body.
async fn create_test_case(emu: &Emulator, policy_arn: &str, guard_content: &str) -> String {
    let created = emu
        .call(
            "POST",
            &format!(
                "/automated-reasoning-policies/{}/test-cases",
                label(policy_arn)
            ),
            Some(json!({
                "guardContent": guard_content,
                // One value of `AutomatedReasoningCheckResult`
                // (`smithy_query(query="shape",shape="AutomatedReasoningCheckResult")`).
                "expectedAggregatedFindingsResult": "VALID",
            })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {guard_content:?}: CreateAutomatedReasoningPolicyTestCase must answer 200, got \
         {} {:?}",
        created.status, created.body
    );
    created.text("testCaseId")
}

/// `GetAutomatedReasoningPolicyTestResult`: the policy ARN, the build workflow id and the
/// test id are all path labels.
async fn get_test_result(
    emu: &Emulator,
    policy_arn: &str,
    build_workflow_id: &str,
    test_case_id: &str,
) -> Reply {
    emu.call(
        "GET",
        &format!(
            "/automated-reasoning-policies/{}/build-workflows/{build_workflow_id}/test-cases/\
             {test_case_id}/test-results",
            label(policy_arn)
        ),
        None,
    )
    .await
}

#[tokio::test]
async fn a_deleted_build_workflow_takes_its_artifacts_with_it() {
    let emu = emulator();
    let policy_arn = policy(&emu, "artifact policy").await;

    // Tests differing in their content, so the rule is asserted over several runs of one
    // workflow rather than over a single one.
    let test_case_ids: Vec<String> = {
        let mut ids = Vec::new();
        for claim in ["returns close after 30 days", "refunds are issued in full"] {
            ids.push(create_test_case(&emu, &policy_arn, claim).await);
        }
        ids
    };

    let built = started_workflow(&emu, &policy_arn, INGEST_CONTENT).await;
    // A test workflow runs only against a COMPLETED build, so the build is aged past the
    // progression the start armed instead of sleeping through it.
    age_build_workflow(&emu, &policy_arn, &built, PAST_THE_BUILD);
    let scheduled = emu
        .call(
            "POST",
            &format!(
                "/automated-reasoning-policies/{}/build-workflows/{built}/test-workflows",
                label(&policy_arn)
            ),
            Some(json!({})),
        )
        .await;
    assert_eq!(
        scheduled.status, 200,
        "case the test workflow: StartAutomatedReasoningPolicyTestWorkflow must answer 200, \
         got {} {:?}",
        scheduled.status, scheduled.body
    );
    tokio::time::sleep(PAST_THE_TEST_RUN).await;

    for test_case_id in &test_case_ids {
        let read = get_test_result(&emu, &policy_arn, &built, test_case_id).await;
        assert_eq!(
            read.status, 200,
            "case {test_case_id:?}: the run scheduled against the workflow must read back \
             before the delete, got {} {:?}",
            read.status, read.body
        );
    }

    let token = stored_updated_at(&emu, &policy_arn, &built).await;
    assert_accepted(
        "the built workflow",
        &delete_workflow(&emu, &policy_arn, &built, &token).await,
    );

    // "Deletes an Automated Reasoning policy build workflow and its associated artifacts":
    // a run is addressed THROUGH the workflow it tested, so it is gone with it.
    for test_case_id in &test_case_ids {
        assert_not_found_with(
            &format!("the run of {test_case_id:?} after the delete"),
            &get_test_result(&emu, &policy_arn, &built, test_case_id).await,
            REQUEST_WORKFLOW_NOT_FOUND_MESSAGE,
        );
    }

    // The TESTS themselves belong to the policy rather than to the workflow that ran them,
    // so they are still there.
    for test_case_id in &test_case_ids {
        let read = emu
            .call(
                "GET",
                &format!(
                    "/automated-reasoning-policies/{}/test-cases/{test_case_id}",
                    label(&policy_arn)
                ),
                None,
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {test_case_id:?}: the test must survive the deletion of the workflow that \
             ran it, got {} {:?}",
            read.status, read.body
        );
    }

    // A workflow that is still building holds the policy, and deleting it frees the policy
    // for another build: the start refused while it was there is accepted once it is gone.
    let working = start(&emu, &policy_arn, INGEST_CONTENT, Some("build-token-1")).await;
    assert_eq!(
        working.status, 200,
        "case the second build: the start must answer 200 once the policy is free, got {} {:?}",
        working.status, working.body
    );
    let working_id = working.text("buildWorkflowId");
    let blocked = start(&emu, &policy_arn, INGEST_CONTENT, None).await;
    assert_eq!(
        (blocked.status, blocked.error.as_deref()),
        (400, Some("ResourceInUseException")),
        "case the blocked build: a policy a workflow is still building admits no second \
         build, got {} {:?}",
        blocked.status,
        blocked.body
    );
    let working_token = stored_updated_at(&emu, &policy_arn, &working_id).await;
    assert_accepted(
        "the working workflow",
        &delete_workflow(&emu, &policy_arn, &working_id, &working_token).await,
    );
    let freed = start(&emu, &policy_arn, INGEST_CONTENT, None).await;
    assert_eq!(
        freed.status, 200,
        "case the freed policy: a start must be accepted once the workflow holding the policy \
         is deleted, got {} {:?}",
        freed.status, freed.body
    );
    let freed_id = freed.text("buildWorkflowId");

    // The `clientRequestToken` of a deleted workflow no longer names it: a replay mints a
    // NEW workflow rather than answering with the one that is gone. The freed policy's
    // workflow is aged past its build first, so the replay is not refused on it.
    age_build_workflow(&emu, &policy_arn, &freed_id, PAST_THE_BUILD);
    let replay = start(&emu, &policy_arn, INGEST_CONTENT, Some("build-token-1")).await;
    assert_eq!(
        replay.status, 200,
        "case the replayed token: the start must answer 200, got {} {:?}",
        replay.status, replay.body
    );
    let reminted = replay.text("buildWorkflowId");
    assert_ne!(
        reminted, working_id,
        "case the replayed token: the replay must mint a new workflow, not name the deleted one"
    );
    let mut expected = vec![freed_id, reminted];
    expected.sort();
    let mut listed = listed_ids("after the replay", &emu, &policy_arn).await;
    listed.sort();
    assert_eq!(
        listed, expected,
        "case after the replay: the policy holds exactly the workflows that are left"
    );
}
