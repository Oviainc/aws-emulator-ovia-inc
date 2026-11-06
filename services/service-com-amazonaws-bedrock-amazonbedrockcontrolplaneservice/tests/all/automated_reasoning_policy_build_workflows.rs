// SPDX-License-Identifier: CC0-1.0

//! The Automated Reasoning policy BUILD WORKFLOW end-to-end:
//! `StartAutomatedReasoningPolicyBuildWorkflow` starts it,
//! `CancelAutomatedReasoningPolicyBuildWorkflow` stops it and
//! `GetAutomatedReasoningPolicyBuildWorkflow` reads it back, over the same
//! `MultiServiceRouter` the emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A started workflow round-trips**: the read reports the workflow type the start
//!    named and the source document it carried, for requests spanning every
//!    `AutomatedReasoningPolicyBuildWorkflowType` and every form
//!    `AutomatedReasoningPolicyWorkflowTypeContent` takes (a single document, the fidelity
//!    report's documents, the iterative refinement's documents, repair annotations, and a
//!    source carrying a policy definition only). The minted `buildWorkflowId` is one
//!    `AutomatedReasoningPolicyBuildWorkflowId` admits, and every start mints a fresh one.
//!    (Which status such a row reports depends on how long the request took, which rule 2
//!    pins on its own, so this one only requires one the model admits.)
//! 2. **A workflow a start has just accepted is not built yet**: it reports the status the
//!    start accepted until the clock makes its next move due, and the terminal one once the
//!    whole progression has elapsed — and a read never moves it, so two reads after the span
//!    report ONE instant. (The processing statuses the progression passes through on the way
//!    there are pinned by `automated_reasoning_policy_build_workflow_progression.rs`.)
//! 3. **One build at a time per policy**: a second start for a policy a workflow is still
//!    working on is refused with `ResourceInUseException`, while the same start is
//!    accepted once that workflow is terminal.
//! 4. **A workflow is addressed by its policy AND its id**: an ARN naming no policy of
//!    this account cannot be built, an id nothing started reads back as nothing, and one
//!    policy's workflow id names no workflow of another policy.
//! 5. **A replayed `clientRequestToken` starts nothing new**, and two policies' starts are
//!    two workflows.
//! 6. **A cancel stops a workflow that is still working**: it is reported
//!    `CANCEL_REQUESTED` from the cancel onwards, the clock owes it `CANCELLED` rather
//!    than the completion the start owed it, and the policy it was building is free again
//!    once it is terminal.
//! 7. **A cancel with nothing left to stop changes nothing**: a workflow already winding
//!    down, or already terminal, is accepted and keeps the status and the instants it had
//!    — while a pair naming no stored workflow is refused with `ResourceNotFoundException`
//!    carrying the service's wording for a missing build workflow.
//! 8. **A listing of a policy's build workflows reports what a read of each one reports**,
//!    in creation order, in pages that partition the workflows: over policies holding 0,
//!    1, 3 and 7 workflows, walked with every page size, the pages concatenate to the
//!    whole history with neither a gap nor a repeat; a listing is scoped to its own
//!    policy; an ARN naming no policy of this account is refused rather than answered
//!    empty; and a `nextToken` the listing never issued is refused
//!    `ValidationException`.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="StartAutomatedReasoningPolicyBuildWorkflow,GetAutomatedReasoningPolicyBuildWorkflow")`
//! / `query="shape"`). The requests go through `crate::support` — the ONE harness every
//! suite in this crate drives the emulator with.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

use crate::support::{
    age_build_workflow, arm_build_workflow, date_time_seconds, emulator, label,
    seed_build_workflow, Emulator, Reply, ACCOUNT, REGION,
};

/// Every `AutomatedReasoningPolicyBuildWorkflowType` the model admits
/// (`smithy_query(query="shape",shape="AutomatedReasoningPolicyBuildWorkflowType")`).
const WORKFLOW_TYPES: &[&str] = &[
    "INGEST_CONTENT",
    "REFINE_POLICY",
    "IMPORT_POLICY",
    "GENERATE_FIDELITY_REPORT",
    "GENERATE_POLICY_SCENARIOS",
    "RESOLVE_POLICY_AMBIGUITIES",
    "ITERATIVELY_REFINE_POLICY",
];

/// Every `AutomatedReasoningPolicyBuildDocumentContentType` the model admits
/// (`smithy_query(query="shape",shape="AutomatedReasoningPolicyBuildWorkflowDocument")`).
const DOCUMENT_CONTENT_TYPES: &[&str] = &["pdf", "txt"];

/// The status a build workflow a start has just accepted carries, before the clock owes it
/// its completion (`AutomatedReasoningPolicyBuildWorkflowStatus$SCHEDULED`).
const SCHEDULED: &str = "SCHEDULED";

/// The terminal status the clock owes a started build workflow
/// (`AutomatedReasoningPolicyBuildWorkflowStatus$COMPLETED`).
const COMPLETED: &str = "COMPLETED";

/// The status a build workflow carries from the cancel that asked it to stop until the
/// clock owes it the terminal one
/// (`AutomatedReasoningPolicyBuildWorkflowStatus$CANCEL_REQUESTED`).
const CANCEL_REQUESTED: &str = "CANCEL_REQUESTED";

/// The terminal status the clock owes a workflow whose cancellation was asked for
/// (`AutomatedReasoningPolicyBuildWorkflowStatus$CANCELLED`).
const CANCELLED: &str = "CANCELLED";

/// How many characters a hyphenated `AutomatedReasoningPolicyBuildWorkflowId` carries
/// (`@length(max: 36)`, pattern
/// `^[a-f0-9]{8}-?[a-f0-9]{4}-?4[a-f0-9]{3}-?[89ab][a-f0-9]{3}-?[a-f0-9]{12}$`).
const WORKFLOW_ID_LENGTH: usize = 36;

/// A source document's bytes on the wire: `AutomatedReasoningPolicyBuildDocumentBlob` is a
/// `blob` (`@length(1, 5000000)`), which this protocol carries base64-encoded — this is
/// `policy rules`.
const DOCUMENT_BLOB: &str = "cG9saWN5IHJ1bGVz";

/// How far a workflow a start accepted must be aged to be past the completion the clock
/// owes it: the whole progression the start arms, and a little more, so a workflow aged this
/// much must hold the terminal status.
const PAST_THE_BUILD: Duration = bedrock::state::BUILD_WORKFLOW_COMPLETE_AFTER
    .saturating_add(Duration::from_millis(150));

/// Long enough that a workflow a cancel took over is past the cancellation the clock owes
/// it, for the same reason [`PAST_THE_BUILD`] is long enough for the completion.
const PAST_THE_CANCELLATION: Duration = Duration::from_millis(900);

/// The `ResourceNotFoundException` message a request addressing a build workflow nothing
/// holds is answered with — the wording Amazon Bedrock answers for that error class, which
/// names neither the policy nor the id the request carried, so it is a FIXED text with no
/// variable part to fill.
const WORKFLOW_NOT_FOUND_MESSAGE: &str = "Build WorkflowExecution not found";

/// The `ResourceNotFoundException` message a request addressing an Automated Reasoning
/// policy this account holds none of is answered with — a FIXED text like
/// [`WORKFLOW_NOT_FOUND_MESSAGE`], naming neither the ARN the request carried nor the
/// account.
const POLICY_NOT_FOUND_MESSAGE: &str = "Policy does not exist";

/// The status a cancel answers with: `CancelAutomatedReasoningPolicyBuildWorkflow` carries
/// no output members and is modelled `@http(code: 202)`
/// (`smithy_query(query="operation",operation="CancelAutomatedReasoningPolicyBuildWorkflow")`).
const CANCEL_ACCEPTED: u16 = 202;

/// Create a policy and hand back the `policyArn` it minted.
async fn policy(emu: &Emulator, name: &str) -> String {
    let created = emu
        .call("POST", "/automated-reasoning-policies", Some(json!({ "name": name })))
        .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: CreateAutomatedReasoningPolicy must answer 200, got {} {:?}",
        created.status, created.body
    );
    created.text("policyArn")
}

/// `StartAutomatedReasoningPolicyBuildWorkflow`: the `sourceContent` is the request's
/// `@httpPayload`, and the policy ARN and the workflow type are path labels.
async fn start(emu: &Emulator, policy_arn: &str, workflow_type: &str, source: Value) -> Reply {
    start_with_token(emu, policy_arn, workflow_type, source, None).await
}

/// The same start, with the `@idempotencyToken` the model binds to the
/// `x-amz-client-token` HEADER.
async fn start_with_token(
    emu: &Emulator,
    policy_arn: &str,
    workflow_type: &str,
    source: Value,
    client_request_token: Option<&str>,
) -> Reply {
    let path = format!(
        "/automated-reasoning-policies/{}/build-workflows/{workflow_type}/start",
        label(policy_arn)
    );
    let headers: Vec<(&str, &str)> = client_request_token
        .map(|token| vec![("x-amz-client-token", token)])
        .unwrap_or_default();
    emu.call_with_headers(REGION, "POST", &path, Some(source), &headers)
        .await
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

/// One source document, as `AutomatedReasoningPolicyBuildWorkflowDocument` spells it.
fn document(name: &str, content_type: &str, description: Option<&str>) -> Value {
    let mut document = json!({
        "document": DOCUMENT_BLOB,
        "documentContentType": content_type,
        "documentName": name,
    });
    if let Some(description) = description {
        document["documentDescription"] = json!(description);
    }
    document
}

/// `sourceContent` carrying one document under `workflowContent$documents`, which is
/// modelled `@length(1, 1)`.
fn documents_source(name: &str, content_type: &str, description: Option<&str>) -> Value {
    json!({
        "workflowContent": { "documents": [document(name, content_type, description)] },
    })
}

/// `sourceContent` carrying the fidelity report content's own documents
/// (`AutomatedReasoningPolicyGenerateFidelityReportContent$documents`, `@length(0, 5)`).
fn fidelity_report_source(documents: &[(&str, &str)]) -> Value {
    let documents: Vec<Value> = documents
        .iter()
        .map(|(name, content_type)| document(name, content_type, None))
        .collect();
    json!({
        "workflowContent": { "generateFidelityReportContent": { "documents": documents } },
    })
}

/// `sourceContent` carrying the iterative refinement content
/// (`AutomatedReasoningPolicyIterativeRefinementContent`, whose `documents` is
/// `@length(1, 5)`).
fn iterative_refinement_source(documents: &[(&str, &str)], feedback: &str) -> Value {
    let documents: Vec<Value> = documents
        .iter()
        .map(|(name, content_type)| document(name, content_type, None))
        .collect();
    json!({
        "workflowContent": {
            "iterativeRefinementContent": { "documents": documents, "feedback": feedback },
        },
    })
}

/// `sourceContent` carrying repair annotations rather than a document
/// (`AutomatedReasoningPolicyBuildWorkflowRepairContent$annotations`).
fn repair_source() -> Value {
    json!({
        "workflowContent": {
            "policyRepairAssets": {
                "annotations": [
                    { "deleteRule": { "ruleId": "R00000000001" } },
                    { "addRuleFromNaturalLanguage": { "naturalLanguage": "returns close after 30 days" } },
                ],
            },
        },
    })
}

/// `sourceContent` carrying only the existing policy definition it starts from.
fn definition_source() -> Value {
    json!({
        "policyDefinition": {
            "version": "1",
            "types": [],
            "variables": [],
            "rules": [{ "id": "R00000000001", "expression": "returnWindow == Open" }],
        },
    })
}

/// Assert `id` is one `AutomatedReasoningPolicyBuildWorkflowId` admits: the hyphenated
/// 36-character form, lowercase hex, with RFC 4122's version nibble `4` and a variant
/// digit in `[89ab]` — the `4` and the `[89ab]` the member's own pattern fixes.
fn assert_workflow_id_shape(case: &str, id: &str) {
    assert_eq!(
        id.len(),
        WORKFLOW_ID_LENGTH,
        "case {case}: a buildWorkflowId carries {WORKFLOW_ID_LENGTH} characters, got {id:?}"
    );
    let groups: Vec<&str> = id.split('-').collect();
    assert_eq!(
        groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12],
        "case {case}: expected the 8-4-4-4-12 grouping, got {id:?}"
    );
    assert!(
        groups.iter().all(|group| group
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))),
        "case {case}: a buildWorkflowId is lowercase hex, got {id:?}"
    );
    assert!(
        groups[2].starts_with('4'),
        "case {case}: the version nibble must be 4, got {id:?}"
    );
    assert!(
        groups[3].starts_with(['8', '9', 'a', 'b']),
        "case {case}: the variant digit must be one of [89ab], got {id:?}"
    );
}

// ---------------------------------------------------------------------------------------
// 1. What a start stores is what a read reports.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_started_build_workflow_round_trips_through_get() {
    /// (case, buildWorkflowType, sourceContent, the document metadata the read must
    /// report as (documentName, documentContentType, documentDescription)).
    struct Row {
        case: &'static str,
        workflow_type: &'static str,
        source: Value,
        document: Option<(&'static str, &'static str, Option<&'static str>)>,
    }

    // Rows span every workflow type the enum admits, both document content types, and
    // every form the source content takes — including the two that name no document.
    let mut rows = vec![
        Row {
            case: "one txt document, described",
            workflow_type: WORKFLOW_TYPES[0],
            source: documents_source("returns.txt", "txt", Some("the returns policy")),
            document: Some(("returns.txt", "txt", Some("the returns policy"))),
        },
        Row {
            case: "one pdf document, undescribed",
            workflow_type: WORKFLOW_TYPES[1],
            source: documents_source("returns.pdf", "pdf", None),
            document: Some(("returns.pdf", "pdf", None)),
        },
        Row {
            case: "the fidelity report's documents",
            workflow_type: WORKFLOW_TYPES[3],
            source: fidelity_report_source(&[("fidelity.txt", "txt"), ("second.pdf", "pdf")]),
            document: Some(("fidelity.txt", "txt", None)),
        },
        Row {
            case: "the iterative refinement's documents",
            workflow_type: WORKFLOW_TYPES[6],
            source: iterative_refinement_source(&[("refine.pdf", "pdf")], "tighten the rules"),
            document: Some(("refine.pdf", "pdf", None)),
        },
        Row {
            case: "repair annotations, no document",
            workflow_type: WORKFLOW_TYPES[1],
            source: repair_source(),
            document: None,
        },
        Row {
            case: "a policy definition only, no document",
            workflow_type: WORKFLOW_TYPES[2],
            source: definition_source(),
            document: None,
        },
    ];
    // Every remaining workflow type, so the rule is not keyed to the ones above.
    for (index, workflow_type) in WORKFLOW_TYPES.iter().enumerate() {
        rows.push(Row {
            case: workflow_type,
            workflow_type,
            source: documents_source(
                "catalog.txt",
                DOCUMENT_CONTENT_TYPES[index % DOCUMENT_CONTENT_TYPES.len()],
                None,
            ),
            document: Some((
                "catalog.txt",
                DOCUMENT_CONTENT_TYPES[index % DOCUMENT_CONTENT_TYPES.len()],
                None,
            )),
        });
    }

    let emu = emulator();
    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // One policy per row: a policy admits one build workflow at a time, which rule 3
        // is about.
        let policy_arn = policy(&emu, &format!("workflow policy {index}")).await;

        let started = start(&emu, &policy_arn, row.workflow_type, row.source.clone()).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: StartAutomatedReasoningPolicyBuildWorkflow must answer 200, \
             got {} {:?}",
            started.status, started.body
        );
        assert_eq!(
            started.text("policyArn"),
            policy_arn,
            "case {case:?}: the start must report the policy it was addressed for"
        );
        let id = started.text("buildWorkflowId");
        assert_workflow_id_shape(case, &id);
        assert!(
            minted.insert(id.clone()),
            "case {case:?}: every start mints a fresh buildWorkflowId, got {id:?} twice"
        );

        let read = get_workflow(&emu, &policy_arn, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetAutomatedReasoningPolicyBuildWorkflow must answer 200, \
             got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.text("buildWorkflowId"),
            id,
            "case {case:?}: the read must report the id the start minted"
        );
        assert_eq!(
            read.text("policyArn"),
            policy_arn,
            "case {case:?}: the read must report the policy the workflow builds"
        );
        assert_eq!(
            read.text("buildWorkflowType"),
            row.workflow_type,
            "case {case:?}: the read must report the type the start named, got {:?}",
            read.get("buildWorkflowType")
        );
        // Which status this row reports depends on how long the request took, which rule
        // 2 pins on its own; here it must be one the model admits for a workflow this
        // store started.
        let status = read.text("status");
        assert!(
            status == SCHEDULED || status == COMPLETED,
            "case {case:?}: a started workflow reports the status the start accepted or \
             the one the clock owed it, got {status:?}"
        );
        let expected_name = row.document.map_or(Value::Null, |(name, _, _)| json!(name));
        let expected_type = row
            .document
            .map_or(Value::Null, |(_, content_type, _)| json!(content_type));
        let expected_description = row
            .document
            .and_then(|(_, _, description)| description)
            .map_or(Value::Null, |description| json!(description));
        assert_eq!(
            read.get("documentName"),
            expected_name,
            "case {case:?}: expected documentName {expected_name:?}, got {:?}",
            read.get("documentName")
        );
        assert_eq!(
            read.get("documentContentType"),
            expected_type,
            "case {case:?}: expected documentContentType {expected_type:?}, got {:?}",
            read.get("documentContentType")
        );
        assert_eq!(
            read.get("documentDescription"),
            expected_description,
            "case {case:?}: expected documentDescription {expected_description:?}, got {:?}",
            read.get("documentDescription")
        );
        let created_at = read.instant("createdAt");
        let updated_at = read.instant("updatedAt");
        assert!(
            status == COMPLETED || (created_at - updated_at).abs() < f64::EPSILON,
            "case {case:?}: a workflow nothing has changed reports one instant for both, \
             got createdAt={created_at} updatedAt={updated_at}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 2. The status the clock owes a started workflow.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_started_workflow_reaches_its_terminal_status_on_the_clock() {
    // Rows differ along the workflow type and the source form, so the transition is not
    // keyed to one of either.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a document ingestion",
            WORKFLOW_TYPES[0],
            documents_source("ingest.txt", "txt", None),
        ),
        ("a policy repair", WORKFLOW_TYPES[1], repair_source()),
        (
            "a scenario generation",
            WORKFLOW_TYPES[4],
            definition_source(),
        ),
    ];

    let emu = emulator();
    // Every row is started first and read afterwards, so the suite waits through the
    // progression the clock owes a build ONCE rather than once per row.
    let mut started_rows: Vec<(&str, String, String, f64)> = Vec::new();
    for (index, (case, workflow_type, source)) in rows.iter().enumerate() {
        let policy_arn = policy(&emu, &format!("clock policy {index}")).await;
        let started = start(&emu, &policy_arn, workflow_type, source.clone()).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: the start must answer 200, got {} {:?}",
            started.status, started.body
        );
        let id = started.text("buildWorkflowId");

        let before = get_workflow(&emu, &policy_arn, &id).await;
        assert_eq!(
            before.text("status"),
            SCHEDULED,
            "case {case:?}: a caller reading straight after the start still sees the \
             status the start accepted, got {:?}",
            before.get("status")
        );
        started_rows.push((case, policy_arn, id, before.instant("createdAt")));
    }

    tokio::time::sleep(PAST_THE_BUILD).await;

    for (case, policy_arn, id, created_at) in &started_rows {
        let after = get_workflow(&emu, policy_arn, id).await;
        assert_eq!(
            after.text("status"),
            COMPLETED,
            "case {case:?}: the workflow is owed its completion once the whole progression \
             the start armed has elapsed, got {:?}",
            after.get("status")
        );
        assert!(
            (after.instant("createdAt") - created_at).abs() < f64::EPSILON,
            "case {case:?}: the completion must not move createdAt, got {:?}",
            after.get("createdAt")
        );
        let settled_at = after.instant("updatedAt");
        assert!(
            settled_at > *created_at,
            "case {case:?}: the completion must move updatedAt past createdAt, got \
             createdAt={created_at} updatedAt={settled_at}"
        );

        // A read reports; it must not move anything a later caller sees.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let again = get_workflow(&emu, policy_arn, id).await;
        assert_eq!(
            again.text("status"),
            COMPLETED,
            "case {case:?}: a terminal workflow stays terminal, got {:?}",
            again.get("status")
        );
        assert!(
            (again.instant("updatedAt") - settled_at).abs() < f64::EPSILON,
            "case {case:?}: a read must report the instant the completion was owed at, \
             not the instant of the read, got {:?}",
            again.get("updatedAt")
        );
    }
}

// ---------------------------------------------------------------------------------------
// 3. One build at a time per policy.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_policy_a_workflow_is_still_building_is_in_use() {
    // Rows differ along the pair of workflow types asked for, so the refusal is about the
    // policy being in use rather than about one type.
    let rows: &[(&str, &str, &str)] = &[
        ("the same type twice", WORKFLOW_TYPES[0], WORKFLOW_TYPES[0]),
        ("two different types", WORKFLOW_TYPES[1], WORKFLOW_TYPES[5]),
        (
            "a refinement after an ingestion",
            WORKFLOW_TYPES[0],
            WORKFLOW_TYPES[6],
        ),
    ];

    let emu = emulator();
    for (index, (case, first_type, second_type)) in rows.iter().enumerate() {
        let policy_arn = policy(&emu, &format!("in use policy {index}")).await;
        let first = start(
            &emu,
            &policy_arn,
            first_type,
            documents_source("first.txt", "txt", None),
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first start must answer 200, got {} {:?}",
            first.status, first.body
        );

        let second = start(
            &emu,
            &policy_arn,
            second_type,
            documents_source("second.txt", "txt", None),
        )
        .await;
        if second.status == 200 {
            // The clock had already completed the first workflow, so the policy was free
            // and the rule does not refuse this start: assert exactly that, rather than a
            // refusal nothing was in use for.
            let first_read = get_workflow(&emu, &policy_arn, &first.text("buildWorkflowId")).await;
            assert_eq!(
                first_read.text("status"),
                COMPLETED,
                "case {case:?}: a second start is admitted only once the first workflow \
                 is terminal, got {:?}",
                first_read.get("status")
            );
        } else {
            assert_eq!(
                second.status, 400,
                "case {case:?}: a policy a workflow is still building cannot take \
                 another, got {} {:?}",
                second.status, second.body
            );
            assert_eq!(
                second.error.as_deref(),
                Some("ResourceInUseException"),
                "case {case:?}: expected ResourceInUseException, got {:?}",
                second.error
            );
        }

        // Once the first workflow is terminal the policy is free again: the workflow is
        // aged past the progression the start armed instead of sleeping through it.
        age_build_workflow(
            &emu,
            &policy_arn,
            &first.text("buildWorkflowId"),
            PAST_THE_BUILD,
        );
        let third = start(
            &emu,
            &policy_arn,
            second_type,
            documents_source("third.txt", "txt", None),
        )
        .await;
        assert_eq!(
            third.status, 200,
            "case {case:?}: a policy whose workflow is terminal takes another, got {} {:?}",
            third.status, third.body
        );
        assert_ne!(
            third.text("buildWorkflowId"),
            first.text("buildWorkflowId"),
            "case {case:?}: the second start is a second workflow"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 4. A workflow is addressed by its policy AND its id.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_pair_naming_no_stored_workflow_is_not_found() {
    let emu = emulator();
    let policy_arn = policy(&emu, "addressed policy").await;
    let other_arn = policy(&emu, "other policy").await;
    let started = start(
        &emu,
        &policy_arn,
        WORKFLOW_TYPES[0],
        documents_source("addressed.txt", "txt", None),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case setup: the start must answer 200, got {} {:?}",
        started.status, started.body
    );
    let id = started.text("buildWorkflowId");

    // A start whose policy ARN names no policy of this account: an id nothing minted, the
    // minted policy in another region, and the policy's own versioned ARN —
    // `AutomatedReasoningPolicyArn` admits a `:<version>` suffix, and this policy has no
    // versions.
    let unminted = format!(
        "arn:aws:bedrock:{REGION}:000000000000:automated-reasoning-policy/00000000000z"
    );
    let elsewhere = policy_arn.replace(&format!(":bedrock:{REGION}:"), ":bedrock:eu-west-1:");
    let versioned = format!("{policy_arn}:1");
    for (case, arn) in [
        ("an id nothing minted", &unminted),
        ("the minted policy in another region", &elsewhere),
        ("a version nothing minted", &versioned),
    ] {
        let refused = start(
            &emu,
            arn,
            WORKFLOW_TYPES[0],
            documents_source("nothing.txt", "txt", None),
        )
        .await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: {arn:?} names no policy of this account, so the start must \
             answer 404, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            refused.error
        );
    }

    // A read whose pair names no stored workflow. The ids are well-formed
    // `AutomatedReasoningPolicyBuildWorkflowId`s, so the refusal is about the store.
    for (case, read_arn, read_id) in [
        (
            "an id nothing started",
            policy_arn.as_str(),
            "00000000-0000-4000-8000-000000000000",
        ),
        ("the workflow of another policy", other_arn.as_str(), id.as_str()),
        ("a policy nothing minted", unminted.as_str(), id.as_str()),
    ] {
        let read = get_workflow(&emu, read_arn, read_id).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: {read_arn:?}/{read_id:?} names no stored workflow, so the \
             read must answer 404, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            read.error
        );
    }

    // The pair the start minted still reads back, so the rows above refused on the pair
    // rather than on the store.
    let read = get_workflow(&emu, &policy_arn, &id).await;
    assert_eq!(
        read.status, 200,
        "case the started pair: the read must answer 200, got {} {:?}",
        read.status, read.body
    );
}

// ---------------------------------------------------------------------------------------
// 5. The idempotency token.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_request_token_starts_nothing_new() {
    // Tokens differing in the characters `IdempotencyToken`
    // (`^[a-zA-Z0-9]([-a-zA-Z0-9]{0,254}[a-zA-Z0-9])?$`) admits.
    let tokens = ["token1", "A-token-2", "0123456789abcdef"];

    let emu = emulator();
    let mut started: BTreeSet<String> = BTreeSet::new();
    for (index, token) in tokens.iter().enumerate() {
        // One policy per token: a replay must answer with the workflow its token already
        // started rather than be refused for the policy being in use.
        let policy_arn = policy(&emu, &format!("token policy {index}")).await;
        let source = documents_source("token.txt", "txt", Some("the token row"));

        let first = start_with_token(
            &emu,
            &policy_arn,
            WORKFLOW_TYPES[0],
            source.clone(),
            Some(token),
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case {token:?}: the first start must answer 200, got {} {:?}",
            first.status, first.body
        );
        let replay = start_with_token(
            &emu,
            &policy_arn,
            WORKFLOW_TYPES[0],
            source,
            Some(token),
        )
        .await;
        assert_eq!(
            replay.status, 200,
            "case {token:?}: a replay must answer 200, got {} {:?}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.body, first.body,
            "case {token:?}: a replay must answer with the workflow the token already \
             started"
        );

        let id = first.text("buildWorkflowId");
        assert!(
            started.insert(id.clone()),
            "case {token:?}: a distinct token must start a distinct workflow, got {id:?} \
             twice"
        );
    }
    assert_eq!(
        started.len(),
        tokens.len(),
        "case all: {} distinct tokens must start {} workflows, got {started:?}",
        tokens.len(),
        tokens.len()
    );
}

// ---------------------------------------------------------------------------------------
// 6. A cancel stops a workflow that is still working.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_cancel_stops_a_workflow_that_is_still_building() {
    // Rows differ along the workflow type and the source form, so the cancellation is not
    // keyed to one of either.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a document ingestion",
            WORKFLOW_TYPES[0],
            documents_source("cancel.txt", "txt", Some("the cancelled build")),
        ),
        ("a policy repair", WORKFLOW_TYPES[1], repair_source()),
        (
            "an ambiguity resolution",
            WORKFLOW_TYPES[5],
            definition_source(),
        ),
        (
            "an iterative refinement",
            WORKFLOW_TYPES[6],
            iterative_refinement_source(&[("cancel.pdf", "pdf")], "stop here"),
        ),
    ];

    let emu = emulator();
    for (index, (case, workflow_type, source)) in rows.iter().enumerate() {
        let policy_arn = policy(&emu, &format!("cancel policy {index}")).await;
        let started = start(&emu, &policy_arn, workflow_type, source.clone()).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: the start must answer 200, got {} {:?}",
            started.status, started.body
        );
        let id = started.text("buildWorkflowId");

        let cancelled = cancel_workflow(&emu, &policy_arn, &id).await;
        assert_eq!(
            cancelled.status, CANCEL_ACCEPTED,
            "case {case:?}: the cancel of a workflow that is still working must answer \
             {CANCEL_ACCEPTED}, got {} {:?}",
            cancelled.status, cancelled.body
        );

        // The cancel is recorded, so a caller reading straight after it sees the status
        // that cancel wrote rather than the one the start accepted.
        let requested = get_workflow(&emu, &policy_arn, &id).await;
        assert_eq!(
            requested.text("status"),
            CANCEL_REQUESTED,
            "case {case:?}: a cancelled workflow winds down from the cancel onwards, got \
             {:?}",
            requested.get("status")
        );
        let created_at = requested.instant("createdAt");
        let requested_at = requested.instant("updatedAt");
        assert!(
            requested_at >= created_at,
            "case {case:?}: the cancel must move updatedAt to the cancel, got \
             createdAt={created_at} updatedAt={requested_at}"
        );

        tokio::time::sleep(PAST_THE_CANCELLATION).await;

        // The completion the START owed this workflow was due BEFORE the cancellation is,
        // so a workflow reported COMPLETED here would be one the cancel never stopped.
        let after = get_workflow(&emu, &policy_arn, &id).await;
        assert_eq!(
            after.text("status"),
            CANCELLED,
            "case {case:?}: the workflow is owed its cancellation, not the completion the \
             start owed it, got {:?}",
            after.get("status")
        );
        assert!(
            (after.instant("createdAt") - created_at).abs() < f64::EPSILON,
            "case {case:?}: the cancellation must not move createdAt, got {:?}",
            after.get("createdAt")
        );
        let cancelled_at = after.instant("updatedAt");
        assert!(
            cancelled_at > requested_at,
            "case {case:?}: the cancellation must move updatedAt past the cancel, got \
             updatedAt={cancelled_at} cancel={requested_at}"
        );

        // A read reports; it must not move anything a later caller sees.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let again = get_workflow(&emu, &policy_arn, &id).await;
        assert_eq!(
            again.text("status"),
            CANCELLED,
            "case {case:?}: a terminal workflow stays terminal, got {:?}",
            again.get("status")
        );
        assert!(
            (again.instant("updatedAt") - cancelled_at).abs() < f64::EPSILON,
            "case {case:?}: a read must report the instant the cancellation was owed at, \
             not the instant of the read, got {:?}",
            again.get("updatedAt")
        );

        // A cancelled workflow is terminal, so the policy it was building is free again.
        let next = start(
            &emu,
            &policy_arn,
            workflow_type,
            documents_source("after-the-cancel.txt", "txt", None),
        )
        .await;
        assert_eq!(
            next.status, 200,
            "case {case:?}: a policy whose workflow was cancelled takes another, got {} \
             {:?}",
            next.status, next.body
        );
        assert_ne!(
            next.text("buildWorkflowId"),
            id,
            "case {case:?}: the start after the cancel is a second workflow"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 7. A cancel with nothing left to stop.
// ---------------------------------------------------------------------------------------

/// What a cancel of a workflow that has nothing left to stop must leave untouched: the
/// status it already carried and both instants it already reported.
fn workflow_snapshot(read: &Reply) -> (String, f64, f64) {
    (
        read.text("status"),
        read.instant("createdAt"),
        read.instant("updatedAt"),
    )
}

#[tokio::test]
async fn a_cancel_with_nothing_left_to_stop_changes_nothing() {
    /// How a row brings its workflow to the state the cancel finds it in.
    #[derive(Clone, Copy)]
    enum Reached {
        /// The clock completed it before anything asked it to stop.
        Completion,
        /// A cancel already took it over and it is still winding down.
        CancelRequested,
        /// A cancel already took it over and the clock has wound it down.
        Cancellation,
    }

    // Rows span every state a workflow a cancel can find with no work left to stop, and
    // differ along the workflow type and the source form as well.
    let rows: Vec<(&str, Reached, &str, Value)> = vec![
        (
            "a workflow the clock completed",
            Reached::Completion,
            WORKFLOW_TYPES[0],
            documents_source("completed.txt", "txt", None),
        ),
        (
            "a workflow already winding down",
            Reached::CancelRequested,
            WORKFLOW_TYPES[2],
            definition_source(),
        ),
        (
            "a workflow already cancelled",
            Reached::Cancellation,
            WORKFLOW_TYPES[4],
            repair_source(),
        ),
    ];

    let emu = emulator();
    for (index, (case, reached, workflow_type, source)) in rows.iter().enumerate() {
        let policy_arn = policy(&emu, &format!("noop cancel policy {index}")).await;
        let started = start(&emu, &policy_arn, workflow_type, source.clone()).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: the start must answer 200, got {} {:?}",
            started.status, started.body
        );
        let id = started.text("buildWorkflowId");

        let expected_status = match reached {
            Reached::Completion => {
                age_build_workflow(&emu, &policy_arn, &id, PAST_THE_BUILD);
                COMPLETED
            }
            Reached::CancelRequested => {
                let first = cancel_workflow(&emu, &policy_arn, &id).await;
                assert_eq!(
                    first.status, CANCEL_ACCEPTED,
                    "case {case:?}: the first cancel must answer {CANCEL_ACCEPTED}, got {} \
                     {:?}",
                    first.status, first.body
                );
                CANCEL_REQUESTED
            }
            Reached::Cancellation => {
                let first = cancel_workflow(&emu, &policy_arn, &id).await;
                assert_eq!(
                    first.status, CANCEL_ACCEPTED,
                    "case {case:?}: the first cancel must answer {CANCEL_ACCEPTED}, got {} \
                     {:?}",
                    first.status, first.body
                );
                tokio::time::sleep(PAST_THE_CANCELLATION).await;
                CANCELLED
            }
        };

        let before = get_workflow(&emu, &policy_arn, &id).await;
        assert_eq!(
            before.text("status"),
            expected_status,
            "case {case:?}: the row must reach {expected_status}, got {:?}",
            before.get("status")
        );
        let snapshot = workflow_snapshot(&before);

        let again = cancel_workflow(&emu, &policy_arn, &id).await;
        assert_eq!(
            again.status, CANCEL_ACCEPTED,
            "case {case:?}: a cancel with nothing left to stop is accepted, got {} {:?}",
            again.status, again.body
        );

        let after = get_workflow(&emu, &policy_arn, &id).await;
        let after_snapshot = workflow_snapshot(&after);
        assert_eq!(
            after_snapshot.0, snapshot.0,
            "case {case:?}: expected the status to stay {:?}, got {:?}",
            snapshot.0, after_snapshot.0
        );
        assert!(
            (after_snapshot.1 - snapshot.1).abs() < f64::EPSILON
                && (after_snapshot.2 - snapshot.2).abs() < f64::EPSILON,
            "case {case:?}: expected the instants to stay {:?}, got {after_snapshot:?}",
            snapshot
        );
    }
}

#[tokio::test]
async fn a_cancel_of_a_pair_naming_no_stored_workflow_is_not_found() {
    let emu = emulator();
    let policy_arn = policy(&emu, "cancelled policy").await;
    let other_arn = policy(&emu, "other cancelled policy").await;
    let started = start(
        &emu,
        &policy_arn,
        WORKFLOW_TYPES[0],
        documents_source("addressed.txt", "txt", None),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case setup: the start must answer 200, got {} {:?}",
        started.status, started.body
    );
    let id = started.text("buildWorkflowId");
    let unminted =
        format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:automated-reasoning-policy/00000000000z");
    let versioned = format!("{policy_arn}:1");

    // Every row carries a well-formed `AutomatedReasoningPolicyBuildWorkflowId`, so the
    // refusal is about the store rather than about the id's form.
    for (case, cancel_arn, cancel_id) in [
        (
            "an id nothing started",
            policy_arn.as_str(),
            "00000000-0000-4000-8000-000000000000",
        ),
        (
            "the workflow of another policy",
            other_arn.as_str(),
            id.as_str(),
        ),
        ("a policy nothing minted", unminted.as_str(), id.as_str()),
        ("a version nothing minted", versioned.as_str(), id.as_str()),
    ] {
        let refused = cancel_workflow(&emu, cancel_arn, cancel_id).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: {cancel_arn:?}/{cancel_id:?} names no stored workflow, so the \
             cancel must answer 404, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            refused.error
        );
        assert_eq!(
            refused.text("message"),
            WORKFLOW_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the message {WORKFLOW_NOT_FOUND_MESSAGE:?}, got {:?}",
            refused.get("message")
        );
    }

    // A read of a pair naming no stored workflow is refused the same way: the wording
    // belongs to the resource, not to one operation.
    let read = get_workflow(&emu, &policy_arn, "00000000-0000-4000-8000-000000000000").await;
    assert_eq!(
        read.status, 404,
        "case a read of an id nothing started: expected 404, got {} {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.text("message"),
        WORKFLOW_NOT_FOUND_MESSAGE,
        "case a read of an id nothing started: expected the message \
         {WORKFLOW_NOT_FOUND_MESSAGE:?}, got {:?}",
        read.get("message")
    );

    // The pair the start minted still cancels, so the rows above refused on the pair
    // rather than on the store.
    let accepted = cancel_workflow(&emu, &policy_arn, &id).await;
    assert_eq!(
        accepted.status, CANCEL_ACCEPTED,
        "case the started pair: the cancel must answer {CANCEL_ACCEPTED}, got {} {:?}",
        accepted.status, accepted.body
    );
}

// ---------------------------------------------------------------------------------------
// 8. The listing of a policy's build workflows.
// ---------------------------------------------------------------------------------------

/// The page size a listing request that names no `maxResults` takes: the member's own
/// modelled `default`
/// (`smithy_query(query="operation",operation="ListAutomatedReasoningPolicyBuildWorkflows")`).
const DEFAULT_PAGE_SIZE: usize = 25;

/// The output member the listing carries its page under.
const SUMMARIES: &str = "automatedReasoningPolicyBuildWorkflowSummaries";

/// `ListAutomatedReasoningPolicyBuildWorkflows`: the policy ARN is a path label, and both
/// page parameters are `@httpQuery` members.
async fn list_workflows(emu: &Emulator, policy_arn: &str, query: &str) -> Reply {
    let base = format!(
        "/automated-reasoning-policies/{}/build-workflows",
        label(policy_arn)
    );
    let path = if query.is_empty() {
        base
    } else {
        format!("{base}?{query}")
    };
    emu.call("GET", &path, None).await
}

/// The page a listing answered with. The member is modelled `@required` with
/// `@length(min: 0)`, so a policy nothing has built reports `[]` rather than omitting it.
fn summaries(case: &str, reply: &Reply) -> Vec<Value> {
    let page = reply.get(SUMMARIES);
    page.as_array()
        .unwrap_or_else(|| {
            panic!("case {case}: {SUMMARIES} is a required list, got {:?}", reply.body)
        })
        .clone()
}

/// One string member of one summary.
fn member(case: &str, summary: &Value, name: &str) -> String {
    summary
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: the summary has no string member {name}: {summary:?}"))
        .to_string()
}

/// Walk every page of one policy's listing and hand back the `buildWorkflowId`s in the
/// order the pages reported them, checking each page's own invariants on the way: a page
/// carries at most `maxResults` summaries, and a `nextToken` is issued only when the page
/// was truncated — so a caller that follows the tokens sees each workflow exactly once.
async fn walk_pages(
    emu: &Emulator,
    case: &str,
    policy_arn: &str,
    max_results: Option<usize>,
) -> Vec<String> {
    let bound = max_results.unwrap_or(DEFAULT_PAGE_SIZE);
    let mut ids: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // More than enough pages for any row below; a listing that keeps issuing tokens is a
    // listing that does not terminate, which this turns into a failure rather than a hang.
    for _ in 0..64 {
        let mut query: Vec<String> = Vec::new();
        if let Some(max) = max_results {
            query.push(format!("maxResults={max}"));
        }
        if let Some(token) = token.as_deref() {
            query.push(format!("nextToken={}", label(token)));
        }
        let reply = list_workflows(emu, policy_arn, &query.join("&")).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: the listing must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        let page = summaries(case, &reply);
        assert!(
            page.len() <= bound,
            "case {case}: a page carries at most {bound} summaries, got {}",
            page.len()
        );
        for summary in &page {
            assert_eq!(
                member(case, summary, "policyArn"),
                policy_arn,
                "case {case}: every summary names the policy the listing addressed, got {summary:?}"
            );
            ids.push(member(case, summary, "buildWorkflowId"));
        }
        match reply.get("nextToken") {
            Value::Null => return ids,
            value => {
                let next = value
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: nextToken is a string, got {value:?}"))
                    .to_string();
                assert_eq!(
                    page.len(),
                    bound,
                    "case {case}: a nextToken is issued only for a truncated page, got a page \
                     of {} with the token {next:?}",
                    page.len()
                );
                token = Some(next);
            }
        }
    }
    panic!("case {case}: the listing kept issuing pagination tokens past 64 pages");
}

/// Seed `count` workflows of one policy, oldest first, and hand back their ids in the
/// order they were started — which is the order the listing must report them in.
///
/// The rows are spread a second apart so each one's start instant is distinct, and the
/// workflow type cycles through the enum so nothing about the order can be keyed to one
/// type.
fn seed_history(emu: &Emulator, policy_arn: &str, count: usize) -> Vec<String> {
    (0..count)
        .map(|index| {
            seed_build_workflow(
                emu,
                policy_arn,
                WORKFLOW_TYPES[index % WORKFLOW_TYPES.len()],
                // The oldest row is the one seeded first, so the ids come back in start
                // order.
                Duration::from_secs((count - index) as u64),
                |_| {},
            )
        })
        .collect()
}

#[tokio::test]
async fn a_listed_build_workflow_agrees_with_a_read_of_it() {
    // Rows differ along the workflow type and the form the source content takes, so the
    // summary is not keyed to one of either.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a document ingestion",
            WORKFLOW_TYPES[0],
            documents_source("listed.txt", "txt", Some("the listed build")),
        ),
        ("a policy repair", WORKFLOW_TYPES[1], repair_source()),
        (
            "a fidelity report",
            WORKFLOW_TYPES[3],
            fidelity_report_source(&[("listed.pdf", "pdf")]),
        ),
        (
            "an iterative refinement",
            WORKFLOW_TYPES[6],
            iterative_refinement_source(&[("listed.pdf", "pdf")], "again"),
        ),
    ];

    let emu = emulator();
    for (index, (case, workflow_type, source)) in rows.iter().enumerate() {
        let policy_arn = policy(&emu, &format!("listed policy {index}")).await;
        let started = start(&emu, &policy_arn, workflow_type, source.clone()).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: the start must answer 200, got {} {:?}",
            started.status, started.body
        );
        let id = started.text("buildWorkflowId");
        // Past the completion the clock owes the workflow, so the read and the listing
        // below both report the terminal status rather than racing the transition.
        age_build_workflow(&emu, &policy_arn, &id, PAST_THE_BUILD);

        let read = get_workflow(&emu, &policy_arn, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the read must answer 200, got {} {:?}",
            read.status, read.body
        );

        let listed = list_workflows(&emu, &policy_arn, "").await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: the listing must answer 200, got {} {:?}",
            listed.status, listed.body
        );
        let page = summaries(case, &listed);
        assert_eq!(
            page.len(),
            1,
            "case {case:?}: a policy one start has built reports one summary, got {page:?}"
        );
        let summary = &page[0];
        assert_workflow_id_shape(case, &member(case, summary, "buildWorkflowId"));
        // Every member the summary models is the member the read reports for the same
        // workflow: one record, reported one way.
        for name in [
            "policyArn",
            "buildWorkflowId",
            "status",
            "buildWorkflowType",
        ] {
            assert_eq!(
                member(case, summary, name),
                read.text(name),
                "case {case:?}: the summary's {name} must be what the read reports, got {summary:?}"
            );
        }
        for name in ["createdAt", "updatedAt"] {
            // `AutomatedReasoningPolicyBuildWorkflowSummary`'s two `Timestamp` members
            // target a shape carrying `@timestampFormat("date-time")`, so the wire form is
            // an RFC 3339 STRING and never the epoch number the protocol default writes.
            let value = summary.get(name).cloned().unwrap_or(Value::Null);
            assert!(
                value.is_string(),
                "case {case:?}: {name} is a @timestampFormat(\"date-time\") member, so it \
                 goes onto the wire as a string, got {value}"
            );
            let summarised = date_time_seconds(case, &member(case, summary, name));
            let got = read.instant(name);
            assert!(
                (summarised - got).abs() < f64::EPSILON,
                "case {case:?}: the summary's {name} must be what the read reports ({got}), \
                 got {summarised}"
            );
        }
        assert_eq!(
            member(case, summary, "buildWorkflowType"),
            *workflow_type,
            "case {case:?}: the summary reports the type the start named, got {summary:?}"
        );
        assert_eq!(
            member(case, summary, "status"),
            COMPLETED,
            "case {case:?}: a workflow past its span is terminal, got {summary:?}"
        );
    }
}

#[tokio::test]
async fn the_pages_of_a_listing_partition_a_policys_build_workflows() {
    // Rows span a policy nothing has built, one with a single workflow, and ones holding
    // several; the page sizes span one row per page, a part page, the modelled default and
    // the whole modelled span (`maxResults` is `range 1..1000`).
    let counts = [0usize, 1, 3, 7];
    let page_sizes = [None, Some(1), Some(2), Some(5), Some(1000)];

    let emu = emulator();
    for count in counts {
        let policy_arn = policy(&emu, &format!("history policy of {count}")).await;
        let started = seed_history(&emu, &policy_arn, count);
        for page_size in page_sizes {
            let case = format!("{count} workflows read {page_size:?} at a time");
            let walked = walk_pages(&emu, &case, &policy_arn, page_size).await;
            assert_eq!(
                walked, started,
                "case {case:?}: the pages must concatenate to the history in start order, \
                 expected {started:?}, got {walked:?}"
            );
            let unique: BTreeSet<&String> = walked.iter().collect();
            assert_eq!(
                unique.len(),
                walked.len(),
                "case {case:?}: no workflow is reported twice, got {walked:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_listing_is_scoped_to_the_policy_it_addresses() {
    let emu = emulator();
    let first_arn = policy(&emu, "scoped policy").await;
    let second_arn = policy(&emu, "other scoped policy").await;
    let empty_arn = policy(&emu, "unbuilt scoped policy").await;
    let first = seed_history(&emu, &first_arn, 3);
    let second = seed_history(&emu, &second_arn, 2);

    for (case, policy_arn, expected) in [
        ("the first policy", &first_arn, first.clone()),
        ("the second policy", &second_arn, second.clone()),
        ("a policy nothing has built", &empty_arn, Vec::new()),
    ] {
        let walked = walk_pages(&emu, case, policy_arn, None).await;
        assert_eq!(
            walked, expected,
            "case {case:?}: a listing reports its own policy's workflows and no other's, \
             expected {expected:?}, got {walked:?}"
        );
    }

    // An ARN naming no policy of this account is refused rather than answered with an
    // empty listing: a listing of nothing and a listing of a policy that does not exist
    // are different answers.
    let unminted =
        format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:automated-reasoning-policy/00000000000z");
    let elsewhere = first_arn.replace(REGION, "eu-west-1");
    let versioned = format!("{first_arn}:1");
    for (case, policy_arn) in [
        ("a policy nothing minted", unminted.as_str()),
        ("the policy of another region", elsewhere.as_str()),
        ("a version nothing minted", versioned.as_str()),
    ] {
        let refused = list_workflows(&emu, policy_arn, "").await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: {policy_arn:?} names no policy of this account, so the listing \
             must answer 404, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            refused.error
        );
        assert_eq!(
            refused.text("message"),
            POLICY_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the wording this service answers for a policy it holds \
             none of, got {:?}",
            refused.get("message")
        );
    }
}

#[tokio::test]
async fn a_pagination_token_this_listing_never_issued_is_refused() {
    let emu = emulator();
    let policy_arn = policy(&emu, "token paged policy").await;
    let started = seed_history(&emu, &policy_arn, 3);

    // Rows span the ways a token can fail to be one this listing issues: a word, a bare
    // number, an instant half of the wrong width, a non-digit in that half, and a cursor
    // naming no workflow at all. Every row satisfies `PaginationToken`
    // (`@length(1, 2048)`, `^\S*$`), so each is refused by the listing rather than by the
    // constraint walk.
    for case in [
        "nextpage",
        "17",
        "12345:00000000-0000-4000-8000-000000000000",
        "0000000000000000000x:00000000-0000-4000-8000-000000000000",
        "00000000000000000000:",
    ] {
        let refused = list_workflows(&emu, &policy_arn, &format!("nextToken={}", label(case))).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a token this listing never issued must be refused 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            refused.error
        );
    }

    // The token the listing DOES issue resumes it at the row the page stopped before.
    let first = list_workflows(&emu, &policy_arn, "maxResults=1").await;
    assert_eq!(
        first.status, 200,
        "case the issued token: the first page must answer 200, got {} {:?}",
        first.status, first.body
    );
    let token = first.text("nextToken");
    let resumed = list_workflows(
        &emu,
        &policy_arn,
        &format!("maxResults=1&nextToken={}", label(&token)),
    )
    .await;
    assert_eq!(
        resumed.status, 200,
        "case the issued token: the resumed page must answer 200, got {} {:?}",
        resumed.status, resumed.body
    );
    let page = summaries("the issued token", &resumed);
    assert_eq!(
        page.len(),
        1,
        "case the issued token: a page of one carries one summary, got {page:?}"
    );
    assert_eq!(
        member("the issued token", &page[0], "buildWorkflowId"),
        started[1],
        "case the issued token: the resumed page starts on the second workflow, got {page:?}"
    );
}

#[tokio::test]
async fn a_listing_reports_the_status_the_clock_owes_and_changes_nothing() {
    // Rows span every state a stored workflow can be in when a listing reads it: the two
    // the clock has already made due, and the two it has not. Each row fixes how long ago
    // its build was started, so the status is the one the elapsed time owes it.
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        &str,
        &str,
        Duration,
        Box<dyn FnOnce(&mut bedrock::state::AutomatedReasoningPolicyBuildWorkflowRecord)>,
    )> = vec![
        (
            "a completion the clock has made due",
            COMPLETED,
            PAST_THE_BUILD,
            Box::new(|_| {}),
        ),
        (
            "a build the clock has not finished",
            SCHEDULED,
            Duration::from_secs(5),
            Box::new(|record| {
                // A build whose every stage is still ahead of it: the progression is
                // re-armed on a start a minute from now.
                arm_build_workflow(record, SystemTime::now() + Duration::from_secs(60));
            }),
        ),
        (
            "a cancellation the clock has made due",
            CANCELLED,
            Duration::from_secs(5),
            Box::new(|record| {
                record.forget_owed_transitions();
                record.status = bedrock::state::BUILD_WORKFLOW_STATUS_CANCEL_REQUESTED;
                record.cancelled_due_at = Some(record.created_at);
            }),
        ),
        (
            "a cancellation still winding down",
            CANCEL_REQUESTED,
            Duration::from_secs(5),
            Box::new(|record| {
                record.forget_owed_transitions();
                record.status = bedrock::state::BUILD_WORKFLOW_STATUS_CANCEL_REQUESTED;
                record.cancelled_due_at = Some(SystemTime::now() + Duration::from_secs(60));
            }),
        ),
    ];

    let emu = emulator();
    for (index, (case, expected, created_ago, tweak)) in rows.into_iter().enumerate() {
        let policy_arn = policy(&emu, &format!("settled policy {index}")).await;
        let id = seed_build_workflow(
            &emu,
            &policy_arn,
            WORKFLOW_TYPES[index % WORKFLOW_TYPES.len()],
            created_ago,
            tweak,
        );

        let listed = list_workflows(&emu, &policy_arn, "").await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: the listing must answer 200, got {} {:?}",
            listed.status, listed.body
        );
        let page = summaries(case, &listed);
        assert_eq!(
            page.len(),
            1,
            "case {case:?}: the seeded policy holds one workflow, got {page:?}"
        );
        assert_eq!(
            member(case, &page[0], "status"),
            expected,
            "case {case:?}: expected the status {expected:?}, got {:?}",
            page[0]
        );
        assert_eq!(
            member(case, &page[0], "buildWorkflowId"),
            id,
            "case {case:?}: the listing reports the seeded workflow, got {:?}",
            page[0]
        );

        // A read of the same workflow agrees with the listing, and a second listing
        // reports exactly what the first one did: reading changes nothing a later caller
        // sees.
        let read = get_workflow(&emu, &policy_arn, &id).await;
        assert_eq!(
            read.text("status"),
            member(case, &page[0], "status"),
            "case {case:?}: a read and a listing report one record one way, got {:?} and {:?}",
            read.get("status"),
            page[0]
        );
        let again = summaries(case, &list_workflows(&emu, &policy_arn, "").await);
        assert_eq!(
            again, page,
            "case {case:?}: a second listing must report what the first one did, got {again:?}"
        );
    }
}
