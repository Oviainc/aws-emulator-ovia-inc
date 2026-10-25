// SPDX-License-Identifier: CC0-1.0

//! The Automated Reasoning policy BUILD WORKFLOW end-to-end:
//! `StartAutomatedReasoningPolicyBuildWorkflow` starts it and
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
//! 2. **A workflow a start has just accepted is not built yet**: it reports the
//!    non-terminal status until the clock makes its completion due, and the terminal one
//!    afterwards — and a read never moves it, so two reads after the span report ONE
//!    instant.
//! 3. **One build at a time per policy**: a second start for a policy a workflow is still
//!    working on is refused with `ResourceInUseException`, while the same start is
//!    accepted once that workflow is terminal.
//! 4. **A workflow is addressed by its policy AND its id**: an ARN naming no policy of
//!    this account cannot be built, an id nothing started reads back as nothing, and one
//!    policy's workflow id names no workflow of another policy.
//! 5. **A replayed `clientRequestToken` starts nothing new**, and two policies' starts are
//!    two workflows.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="StartAutomatedReasoningPolicyBuildWorkflow,GetAutomatedReasoningPolicyBuildWorkflow")`
//! / `query="shape"`). The requests go through `crate::support` — the ONE harness every
//! suite in this crate drives the emulator with.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};

use crate::support::{emulator, label, Emulator, Reply, REGION};

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

/// How many characters a hyphenated `AutomatedReasoningPolicyBuildWorkflowId` carries
/// (`@length(max: 36)`, pattern
/// `^[a-f0-9]{8}-?[a-f0-9]{4}-?4[a-f0-9]{3}-?[89ab][a-f0-9]{3}-?[a-f0-9]{12}$`).
const WORKFLOW_ID_LENGTH: usize = 36;

/// A source document's bytes on the wire: `AutomatedReasoningPolicyBuildDocumentBlob` is a
/// `blob` (`@length(1, 5000000)`), which this protocol carries base64-encoded — this is
/// `policy rules`.
const DOCUMENT_BLOB: &str = "cG9saWN5IHJ1bGVz";

/// Long enough that a workflow a start accepted is past the completion the clock owes it:
/// the emulator makes that transition due a fraction of a second after the start, so a
/// caller that waits this long must see the terminal status.
const PAST_THE_BUILD: Duration = Duration::from_millis(900);

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
        let created_at = before.instant("createdAt");

        tokio::time::sleep(PAST_THE_BUILD).await;

        let after = get_workflow(&emu, &policy_arn, &id).await;
        assert_eq!(
            after.text("status"),
            COMPLETED,
            "case {case:?}: the workflow is owed its completion a fraction of a second \
             after the start, got {:?}",
            after.get("status")
        );
        assert!(
            (after.instant("createdAt") - created_at).abs() < f64::EPSILON,
            "case {case:?}: the completion must not move createdAt, got {:?}",
            after.get("createdAt")
        );
        let settled_at = after.instant("updatedAt");
        assert!(
            settled_at > created_at,
            "case {case:?}: the completion must move updatedAt past createdAt, got \
             createdAt={created_at} updatedAt={settled_at}"
        );

        // A read reports; it must not move anything a later caller sees.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let again = get_workflow(&emu, &policy_arn, &id).await;
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

        // Once the first workflow is terminal the policy is free again.
        tokio::time::sleep(PAST_THE_BUILD).await;
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
