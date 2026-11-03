// SPDX-License-Identifier: CC0-1.0

//! The workflow lifecycle: what `CreateWorkflow` persists, and what `ListWorkflows`,
//! `DescribeWorkflow` and `DeleteWorkflow` answer from that state.
//!
//! Four rules are asserted here, each over a TABLE whose rows differ along what the
//! rule generalises over — how many workflows the store holds, the page bound, which
//! step type, which member of a step is wrong, and how deep in the request it sits:
//!
//! 1. `ListWorkflows` reports exactly the workflows state holds, in `WorkflowId` order,
//!    one page at a time: no more than `MaxResults` per call, a `NextToken` exactly
//!    while a further page remains, and the whole listing recovered by following it.
//! 2. A created workflow reads back through `DescribeWorkflow` as submitted — both step
//!    lists in the caller's order — with each documented step-detail default
//!    (`SourceFileLocation`, `OverwriteExisting`) resolved and each value the caller
//!    sent kept.
//! 3. A step that does not carry the details structure its `Type` names, or whose
//!    details do not name what the step acts on, is refused with the modelled
//!    `InvalidRequestException` (HTTP 400) and persists nothing.
//! 4. A `DeleteWorkflow` releases the workflow from every reader, is refused while a
//!    server still attaches it, and answers the modelled `ResourceNotFoundException`
//!    for an id the store does not hold.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`], so what
//! is asserted is what an SDK sees.

use serde_json::{json, Value};

use crate::harness::{assert_members, member_of, Emulator, ACCOUNT, REGION, ROLE};

/// Satisfies `CustomStepTarget`'s modelled `len 0..170`, pattern
/// `^arn:[a-z-]+:lambda:.*$`.
const LAMBDA_TARGET: &str = "arn:aws:lambda:us-east-1:000000000000:function:custom-step";

/// Satisfies `S3Bucket`'s modelled `len 3..63`, pattern
/// `^[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9]$`.
const DESTINATION_BUCKET: &str = "workflow-destination";

/// Satisfies the `StructuredLogDestinations` member's `Arn` (`len 20..1600`, pattern
/// `^arn:\S+$`).
const LOG_GROUP: &str = "arn:aws:logs:us-east-1:000000000000:log-group:transfer-workflow:*";

/// The `WorkflowStepType` value set, in the order this service's model declares it
/// (`smithy_query(query="shape", shape="com.amazonaws.transfer#WorkflowStepType")`).
const STEP_TYPES: [&str; 5] = ["COPY", "CUSTOM", "TAG", "DELETE", "DECRYPT"];

/// The refusal the service answers for a `CUSTOM` step that names no Lambda function;
/// `workflow_custom_step_function` asserts it in full over the whole class.
const CUSTOM_STEP_FUNCTION_INVALID: &str = "Invalid function provided";

/// `MaxResults`' modelled maximum (`range 1..1000`), which is also the page bound a
/// call that sends no `MaxResults` gets.
const MAX_PAGE_SIZE: usize = 1000;

/// The ONE `WorkflowStep` member each `WorkflowStepType` selects — the pairing
/// `WorkflowStep`'s own documentation states ("Details for a step that …").
fn details_member(step_type: &str) -> &'static str {
    match step_type {
        "COPY" => "CopyStepDetails",
        "CUSTOM" => "CustomStepDetails",
        "TAG" => "TagStepDetails",
        "DELETE" => "DeleteStepDetails",
        "DECRYPT" => "DecryptStepDetails",
        other => panic!("{other} is not a modelled WorkflowStepType"),
    }
}

/// Whether a step of `step_type` carries `OverwriteExisting` at all: the two step
/// types whose details shape declares the member.
fn carries_overwrite_existing(step_type: &str) -> bool {
    matches!(step_type, "COPY" | "DECRYPT")
}

/// A step of `step_type` that satisfies every documented requirement, naming nothing
/// the rule under test defaults: no `SourceFileLocation`, no `OverwriteExisting`.
fn valid_step(step_type: &str) -> Value {
    match step_type {
        "COPY" => json!({
            "Type": "COPY",
            "CopyStepDetails": {
                "Name": "copy-step",
                "DestinationFileLocation": {
                    "S3FileLocation": {"Bucket": DESTINATION_BUCKET, "Key": "processed/"},
                },
            },
        }),
        "CUSTOM" => json!({
            "Type": "CUSTOM",
            "CustomStepDetails": {
                "Name": "custom-step",
                "Target": LAMBDA_TARGET,
                "TimeoutSeconds": 120,
            },
        }),
        "TAG" => json!({
            "Type": "TAG",
            "TagStepDetails": {
                "Name": "tag-step",
                "Tags": [{"Key": "scan", "Value": "clean"}],
            },
        }),
        "DELETE" => json!({"Type": "DELETE", "DeleteStepDetails": {"Name": "delete-step"}}),
        "DECRYPT" => json!({
            "Type": "DECRYPT",
            "DecryptStepDetails": {
                "Name": "decrypt-step",
                "Type": "PGP",
                "DestinationFileLocation": {
                    "S3FileLocation": {"Bucket": DESTINATION_BUCKET, "Key": "decrypted/"},
                },
            },
        }),
        other => panic!("{other} is not a modelled WorkflowStepType"),
    }
}

/// Create the workflow `request` asks for and return its `WorkflowId`.
async fn create_workflow(emulator: &Emulator, case: &str, request: Value) -> String {
    let answer = emulator.call("CreateWorkflow", request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateWorkflow 200, got {} with body {}",
        answer.status, answer.body
    );
    let workflow_id = answer
        .get("WorkflowId")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a WorkflowId, got {}", answer.body))
        .to_string();
    // `WorkflowId` is modelled `len 19..19`, pattern `^w-([a-z0-9]{17})$`.
    assert!(
        workflow_id.len() == 19
            && workflow_id.starts_with("w-")
            && workflow_id[2..]
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
        "case {case:?}: expected a modelled WorkflowId, got {workflow_id:?}"
    );
    workflow_id
}

/// The whole listing, page by page, with `max_results` as the bound each call asks for.
///
/// Asserts the page contract on every call — the page never exceeds the bound, and a
/// `NextToken` comes back exactly while a further page remains — and returns the ids in
/// the order they were listed, plus how many calls it took.
async fn walk_pages(
    emulator: &Emulator,
    case: &str,
    max_results: Option<i64>,
    expected_total: usize,
) -> (Vec<String>, Vec<Value>) {
    let bound = max_results.map_or(MAX_PAGE_SIZE, |n| usize::try_from(n).unwrap_or(MAX_PAGE_SIZE));
    let mut ids: Vec<String> = Vec::new();
    let mut listed: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut calls = 0usize;

    loop {
        let mut request = json!({});
        if let Some(n) = max_results {
            request["MaxResults"] = json!(n);
        }
        if let Some(next) = &token {
            request["NextToken"] = json!(next);
        }
        let answer = emulator.call("ListWorkflows", request).await;
        calls += 1;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected ListWorkflows 200, got {} with body {}",
            answer.status, answer.body
        );
        let page = answer
            .get("Workflows")
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected Workflows, got {}", answer.body))
            .clone();
        assert!(
            page.len() <= bound,
            "case {case:?}: expected at most {bound} workflows per page, got {}",
            page.len()
        );
        for entry in &page {
            ids.push(
                member_of(entry, "WorkflowId")
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
            listed.push(entry.clone());
        }
        token = answer.get("NextToken").as_str().map(str::to_string);
        let remaining = expected_total.saturating_sub(ids.len());
        assert_eq!(
            token.is_some(),
            remaining > 0,
            "case {case:?}: with {remaining} workflow(s) still unlisted, expected a \
             NextToken to be present={}, got {:?}",
            remaining > 0,
            token
        );
        if token.is_none() {
            break;
        }
        assert!(
            calls <= expected_total + 2,
            "case {case:?}: paging did not terminate after {calls} calls, listed {ids:?}"
        );
    }
    (ids, listed)
}

// ---------------------------------------------------------------------------
// RULE 1: ListWorkflows reports stored state, one page at a time
// ---------------------------------------------------------------------------

/// RULE: `ListWorkflows` answers exactly the workflows the store holds, in `WorkflowId`
/// order, bounded by `MaxResults` per call, with a `NextToken` exactly while a further
/// page remains — and reading changes nothing a later caller sees.
///
/// The rows differ along how many workflows the store holds (none, then seven) and
/// along the page bound: absent, each bound below the count, the count itself, and one
/// above it.
#[tokio::test]
async fn list_workflows_reports_the_stored_workflows_one_page_at_a_time() {
    let emulator = Emulator::new();

    // An empty store lists nothing and offers no token.
    let empty = emulator.call("ListWorkflows", json!({})).await;
    assert_eq!(
        empty.status, 200,
        "case \"empty store\": expected 200, got {} with body {}",
        empty.status, empty.body
    );
    assert_eq!(
        empty.get("Workflows"),
        &json!([]),
        "case \"empty store\": expected no workflows, got {}",
        empty.body
    );
    assert!(
        empty.body.get("NextToken").is_none(),
        "case \"empty store\": expected no NextToken, got {}",
        empty.body
    );

    // Seven workflows, each with its own description and a step of a different type, so
    // a listed record can only come from the one that was created.
    let mut expected: Vec<(String, String)> = Vec::new();
    for index in 0..7usize {
        let description = format!("workflow-{index}");
        let step_type = STEP_TYPES[index % STEP_TYPES.len()];
        let id = create_workflow(
            &emulator,
            &description,
            json!({"Description": &description, "Steps": [valid_step(step_type)]}),
        )
        .await;
        expected.push((id, description));
    }
    let mut expected_ids: Vec<String> = expected.iter().map(|(id, _)| id.clone()).collect();
    expected_ids.sort();

    for max_results in [None, Some(1), Some(2), Some(3), Some(7), Some(8)] {
        let case = format!("MaxResults {max_results:?}");
        let (ids, listed) = walk_pages(&emulator, &case, max_results, expected.len()).await;
        assert_eq!(
            ids, expected_ids,
            "case {case:?}: expected every stored workflow once in WorkflowId order, got {ids:?}"
        );
        // Each listed record carries the identifier, description and ARN of the
        // workflow it stands for.
        for entry in &listed {
            let id = member_of(entry, "WorkflowId")
                .as_str()
                .unwrap_or_default()
                .to_string();
            let (_, description) = expected
                .iter()
                .find(|(created, _)| *created == id)
                .unwrap_or_else(|| panic!("case {case:?}: {id} was never created"));
            assert_members(
                &case,
                entry,
                &json!({
                    "WorkflowId": &id,
                    "Description": description,
                    "Arn": format!("arn:aws:transfer:{REGION}:{ACCOUNT}:workflow/{id}"),
                }),
            );
        }
    }
}

/// RULE (the same listing, the token's own contract): a `NextToken` this service did not
/// produce is refused with the modelled `InvalidNextTokenException` (HTTP 400), and one
/// it did produce resumes the listing after the page it was handed out for.
///
/// The rows differ along how the token is malformed: not a token at all, an odd-length
/// one, and one carrying bytes outside the codec.
#[tokio::test]
async fn an_invalid_next_token_is_refused_and_a_valid_one_resumes() {
    let emulator = Emulator::new();
    for index in 0..3usize {
        create_workflow(
            &emulator,
            "seed",
            json!({"Description": format!("token-{index}"), "Steps": [valid_step("DELETE")]}),
        )
        .await;
    }

    for (case, token) in [
        ("not a token at all", "not-a-token"),
        ("odd-length hex", "abc"),
        ("bytes outside the codec", "zzzz"),
    ] {
        let answer = emulator
            .call("ListWorkflows", json!({"NextToken": token}))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidNextTokenException",
            "case {case:?}: expected InvalidNextTokenException, got body {}",
            answer.body
        );
    }

    // The token this service hands out resumes after the page it came with.
    let first = emulator
        .call("ListWorkflows", json!({"MaxResults": 1}))
        .await;
    let token = first
        .get("NextToken")
        .as_str()
        .unwrap_or_else(|| panic!("case \"valid token\": expected a NextToken, got {}", first.body))
        .to_string();
    let resumed = emulator
        .call("ListWorkflows", json!({"NextToken": token}))
        .await;
    assert_eq!(
        resumed.status, 200,
        "case \"valid token\": expected 200, got {} with body {}",
        resumed.status, resumed.body
    );
    let first_id = member_of(&first.get("Workflows")[0], "WorkflowId").clone();
    let resumed_ids: Vec<Value> = resumed
        .get("Workflows")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| member_of(entry, "WorkflowId").clone())
        .collect();
    assert_eq!(
        resumed_ids.len(),
        2,
        "case \"valid token\": expected the two remaining workflows, got {resumed_ids:?}"
    );
    assert!(
        !resumed_ids.contains(&first_id),
        "case \"valid token\": expected the resumed page to exclude {first_id:?}, got \
         {resumed_ids:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 2: a created workflow reads back, with the documented defaults resolved
// ---------------------------------------------------------------------------

/// RULE: a workflow reads back through `DescribeWorkflow` exactly as it was created —
/// both step lists in the caller's order, each step carrying the details structure its
/// `Type` names — with every documented step-detail default resolved and every value
/// the caller sent kept.
///
/// The rows span the whole `WorkflowStepType` value set, one step type per row, and the
/// defaults are read back for each one that carries the member.
#[tokio::test]
async fn a_created_workflow_reads_back_with_its_documented_step_defaults_resolved() {
    let emulator = Emulator::new();

    for step_type in STEP_TYPES {
        let case = format!("{step_type} step with no defaults named");
        let workflow_id = create_workflow(
            &emulator,
            &case,
            json!({
                "Description": format!("defaults-{}", step_type.to_lowercase()),
                "Steps": [valid_step(step_type)],
                "OnExceptionSteps": [valid_step("DELETE")],
            }),
        )
        .await;

        let answer = emulator
            .call("DescribeWorkflow", json!({"WorkflowId": &workflow_id}))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected DescribeWorkflow 200, got {} with body {}",
            answer.status, answer.body
        );
        let workflow = answer.get("Workflow").clone();
        assert_members(
            &case,
            &workflow,
            &json!({
                "WorkflowId": &workflow_id,
                "Arn": format!("arn:aws:transfer:{REGION}:{ACCOUNT}:workflow/{workflow_id}"),
                "Description": format!("defaults-{}", step_type.to_lowercase()),
                // No tags were sent, so the tag set is reported EMPTY; no log group
                // was, so that member is not reported at all.
                "Tags": [],
                "StructuredLogDestinations": null,
            }),
        );

        let steps = member_of(&workflow, "Steps")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            steps.len(),
            1,
            "case {case:?}: expected the one submitted step, got {steps:?}"
        );
        let details = member_of(&steps[0], details_member(step_type)).clone();
        assert_eq!(
            member_of(&steps[0], "Type").as_str(),
            Some(step_type),
            "case {case:?}: expected the submitted Type, got {}",
            steps[0]
        );
        // `SourceFileLocation`: "To use the previous file as the input, enter
        // `${previous.file}` … This is the default value."
        assert_eq!(
            member_of(&details, "SourceFileLocation").as_str(),
            Some("${previous.file}"),
            "case {case:?}: expected the documented SourceFileLocation default, got {details}"
        );
        // `OverwriteExisting`: "The default is `FALSE`."
        let want_overwrite = if carries_overwrite_existing(step_type) {
            json!("FALSE")
        } else {
            Value::Null
        };
        assert_eq!(
            member_of(&details, "OverwriteExisting"),
            &want_overwrite,
            "case {case:?}: expected OverwriteExisting {want_overwrite:?}, got {details}"
        );
        // The exception steps are steps: same resolution, their own list.
        let on_exception = member_of(&workflow, "OnExceptionSteps")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            on_exception.len(),
            1,
            "case {case:?}: expected the one submitted exception step, got {on_exception:?}"
        );
        assert_eq!(
            member_of(
                &member_of(&on_exception[0], "DeleteStepDetails").clone(),
                "SourceFileLocation"
            )
            .as_str(),
            Some("${previous.file}"),
            "case {case:?}: expected the exception step to be resolved too, got {}",
            on_exception[0]
        );
    }
}

/// RULE (the same read-back, the values the caller DID send): a value the request names
/// for a defaulted member is kept, the steps keep their submitted order, and the
/// workflow's own metadata round-trips.
///
/// One row per thing the rule generalises over: a `SourceFileLocation` other than the
/// default, `OverwriteExisting` set to the non-default value, a step list longer than
/// one, and the metadata members (`Tags`, `StructuredLogDestinations`).
#[tokio::test]
async fn a_created_workflow_keeps_the_values_the_request_named() {
    let emulator = Emulator::new();

    // Every step type, in an order that is not the model's declaration order, so the
    // read-back can only come from the request.
    let submitted_order = ["TAG", "DECRYPT", "DELETE", "CUSTOM", "COPY"];
    let mut steps: Vec<Value> = submitted_order.iter().map(|t| valid_step(t)).collect();
    // `SourceFileLocation`'s other documented value, and `OverwriteExisting`'s
    // non-default one, on the two steps that carry both members.
    steps[1]["DecryptStepDetails"]["SourceFileLocation"] = json!("${original.file}");
    steps[1]["DecryptStepDetails"]["OverwriteExisting"] = json!("TRUE");
    steps[4]["CopyStepDetails"]["OverwriteExisting"] = json!("TRUE");

    let case = "every step type, submitted out of declaration order";
    let workflow_id = create_workflow(
        &emulator,
        case,
        json!({
            "Description": "kept-as-submitted",
            "Steps": steps,
            "Tags": [{"Key": "owner", "Value": "platform"}],
            "StructuredLogDestinations": [LOG_GROUP],
        }),
    )
    .await;

    let answer = emulator
        .call("DescribeWorkflow", json!({"WorkflowId": &workflow_id}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DescribeWorkflow 200, got {} with body {}",
        answer.status, answer.body
    );
    let workflow = answer.get("Workflow").clone();
    assert_members(
        case,
        &workflow,
        &json!({
            "Description": "kept-as-submitted",
            "Tags": [{"Key": "owner", "Value": "platform"}],
            "StructuredLogDestinations": [LOG_GROUP],
            // `OnExceptionSteps` is a list the workflow always holds, so a workflow
            // created without exception steps reports it as the empty list.
            "OnExceptionSteps": [],
        }),
    );

    let read_back = member_of(&workflow, "Steps")
        .as_array()
        .cloned()
        .unwrap_or_default();
    let order: Vec<&str> = read_back
        .iter()
        .map(|step| member_of(step, "Type").as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        order,
        submitted_order.to_vec(),
        "case {case:?}: expected the submitted step order, got {order:?}"
    );
    let decrypt = member_of(&read_back[1], "DecryptStepDetails").clone();
    assert_eq!(
        member_of(&decrypt, "SourceFileLocation").as_str(),
        Some("${original.file}"),
        "case {case:?}: expected the submitted SourceFileLocation, got {decrypt}"
    );
    assert_eq!(
        member_of(&decrypt, "OverwriteExisting").as_str(),
        Some("TRUE"),
        "case {case:?}: expected the submitted OverwriteExisting, got {decrypt}"
    );
    let copy = member_of(&read_back[4], "CopyStepDetails").clone();
    assert_eq!(
        member_of(&copy, "OverwriteExisting").as_str(),
        Some("TRUE"),
        "case {case:?}: expected the submitted OverwriteExisting, got {copy}"
    );
    // The custom step's own members survive the round trip.
    let custom = member_of(&read_back[3], "CustomStepDetails").clone();
    assert_members(
        case,
        &custom,
        &json!({"Name": "custom-step", "Target": LAMBDA_TARGET, "TimeoutSeconds": 120}),
    );

    // The ARN carries the region the caller signed for, not a fixed one.
    let elsewhere = "eu-west-1";
    let answer = emulator
        .call_in(
            elsewhere,
            "CreateWorkflow",
            json!({"Steps": [valid_step("DELETE")]}),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case \"signed for another region\": expected 200, got {} with body {}",
        answer.status, answer.body
    );
    let other_id = answer.get("WorkflowId").as_str().unwrap_or_default().to_string();
    let described = emulator
        .call_in(elsewhere, "DescribeWorkflow", json!({"WorkflowId": &other_id}))
        .await;
    assert_eq!(
        member_of(&described.get("Workflow").clone(), "Arn").as_str(),
        Some(format!("arn:aws:transfer:{elsewhere}:{ACCOUNT}:workflow/{other_id}").as_str()),
        "case \"signed for another region\": expected the signed region in the ARN, got {}",
        described.body
    );
}

// ---------------------------------------------------------------------------
// RULE 3: a step carries the details its Type names, and names what it acts on
// ---------------------------------------------------------------------------

/// RULE: a step that does not carry the details structure its `Type` names, carries one
/// another `Type` names, names no `Type` at all, or whose details do not name what the
/// step acts on, is refused with the modelled `InvalidRequestException` (HTTP 400) and
/// persists nothing.
///
/// The rows differ along which step type is wrong, which way it is wrong (details
/// missing, details belonging to another type, the acted-on member missing, no `Type`),
/// which of the two step lists the step sits in, and its index in that list.
///
/// A `CUSTOM` step naming no Lambda function is refused in the service's own fixed
/// wording for that defect, which names no member path and no index, so its rows name
/// that sentence instead; `workflow_custom_step_function` asserts the wording over the
/// whole class.
#[tokio::test]
async fn a_step_must_carry_the_details_its_type_names_and_what_it_acts_on() {
    let emulator = Emulator::new();

    // One workflow that IS valid, so the table cannot pass by refusing everything.
    let kept = create_workflow(
        &emulator,
        "a valid workflow created before the table",
        json!({"Description": "valid-before", "Steps": [valid_step("TAG")]}),
    )
    .await;

    // (case, request, the fragments the refusal must name)
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "COPY with no details at all",
            json!({"Steps": [{"Type": "COPY"}]}),
            vec!["Steps[0].CopyStepDetails", "Type is COPY"],
        ),
        (
            "CUSTOM with no details at all",
            json!({"Steps": [{"Type": "CUSTOM"}]}),
            vec![CUSTOM_STEP_FUNCTION_INVALID],
        ),
        (
            "TAG with no details at all",
            json!({"Steps": [{"Type": "TAG"}]}),
            vec!["Steps[0].TagStepDetails", "Type is TAG"],
        ),
        (
            "DELETE with no details at all",
            json!({"Steps": [{"Type": "DELETE"}]}),
            vec!["Steps[0].DeleteStepDetails", "Type is DELETE"],
        ),
        (
            "DECRYPT with no details at all",
            json!({"Steps": [{"Type": "DECRYPT"}]}),
            vec!["Steps[0].DecryptStepDetails", "Type is DECRYPT"],
        ),
        (
            "DELETE carrying another type's details",
            json!({"Steps": [{
                "Type": "DELETE",
                "DeleteStepDetails": {"Name": "delete-step"},
                "TagStepDetails": {"Name": "tag-step", "Tags": [{"Key": "k", "Value": "v"}]},
            }]}),
            vec!["Steps[0].TagStepDetails", "Type is TAG"],
        ),
        (
            "CUSTOM naming no Lambda target",
            json!({"Steps": [{"Type": "CUSTOM", "CustomStepDetails": {"Name": "custom-step"}}]}),
            vec![CUSTOM_STEP_FUNCTION_INVALID],
        ),
        (
            "COPY naming no destination",
            json!({"Steps": [{"Type": "COPY", "CopyStepDetails": {"Name": "copy-step"}}]}),
            vec![
                "Steps[0].CopyStepDetails.DestinationFileLocation",
                "Type is COPY",
            ],
        ),
        (
            "TAG naming no tags",
            json!({"Steps": [{"Type": "TAG", "TagStepDetails": {"Name": "tag-step"}}]}),
            vec!["Steps[0].TagStepDetails.Tags", "Type is TAG"],
        ),
        (
            "a step naming no Type",
            json!({"Steps": [{"DeleteStepDetails": {"Name": "delete-step"}}]}),
            vec!["Steps[0].Type must be specified"],
        ),
        (
            "the SECOND step is the wrong one",
            json!({"Steps": [valid_step("DELETE"), {"Type": "DECRYPT"}]}),
            vec!["Steps[1].DecryptStepDetails", "Type is DECRYPT"],
        ),
        (
            "an exception step is the wrong one",
            json!({
                "Steps": [valid_step("DELETE")],
                "OnExceptionSteps": [valid_step("TAG"), {"Type": "COPY"}],
            }),
            vec!["OnExceptionSteps[1].CopyStepDetails", "Type is COPY"],
        ),
    ];

    for (case, request, named) in cases {
        let answer = emulator.call("CreateWorkflow", request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got body {}",
            answer.body
        );
        let message = member_of(&answer.body, "Message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        for fragment in named {
            assert!(
                message.contains(fragment),
                "case {case:?}: expected the refusal to name {fragment:?}, got {message:?}"
            );
        }
        assert!(
            answer.body.get("WorkflowId").is_none(),
            "case {case:?}: a refused create must answer no identifier, got {}",
            answer.body
        );
    }

    // Nothing the table submitted was persisted: the store still holds only the
    // workflow created before it.
    let (ids, _) = walk_pages(&emulator, "the store after the whole table", None, 1).await;
    assert_eq!(
        ids,
        vec![kept],
        "case \"the store after the whole table\": expected only the accepted workflow, got {ids:?}"
    );
}

/// RULE (the refusal one level up, enforced by the boundary rather than the handler): a
/// step naming a `Type` outside `WorkflowStepType`'s modelled value set is refused with
/// HTTP 400 `ValidationException` naming the member path and the value set, and persists
/// nothing.
///
/// The rows differ along where in the request the enumerated member sits: a step's own
/// `Type`, a `DECRYPT` step's `EncryptionType`, and a `COPY` step's `OverwriteExisting`.
/// The framework's boundary validator indexes a list member from one
/// (`steps.1.member.…` is the first step), which is the wording this asserts.
#[tokio::test]
async fn a_step_naming_an_unmodelled_enum_value_is_refused_as_a_validation_error() {
    let emulator = Emulator::new();

    // (case, request, the member path and the values the message must name)
    let cases: Vec<(&str, Value, &str, Vec<&str>)> = vec![
        (
            "an unknown step type",
            json!({"Steps": [{"Type": "TRANSCODE"}]}),
            "steps.1.member.type",
            STEP_TYPES.to_vec(),
        ),
        (
            "an unknown encryption type",
            json!({"Steps": [{
                "Type": "DECRYPT",
                "DecryptStepDetails": {
                    "Type": "AES",
                    "DestinationFileLocation": {
                        "S3FileLocation": {"Bucket": DESTINATION_BUCKET, "Key": "decrypted/"},
                    },
                },
            }]}),
            "steps.1.member.decryptStepDetails.type",
            vec!["PGP"],
        ),
        (
            "an unknown OverwriteExisting",
            json!({"Steps": [{
                "Type": "COPY",
                "CopyStepDetails": {
                    "DestinationFileLocation": {
                        "S3FileLocation": {"Bucket": DESTINATION_BUCKET, "Key": "processed/"},
                    },
                    "OverwriteExisting": "MAYBE",
                },
            }]}),
            "steps.1.member.copyStepDetails.overwriteExisting",
            vec!["TRUE", "FALSE"],
        ),
    ];

    for (case, request, path, allowed) in cases {
        let answer = emulator.call("CreateWorkflow", request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got body {}",
            answer.body
        );
        let message = member_of(&answer.body, "message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            message.contains(path),
            "case {case:?}: expected the refusal to name {path:?}, got {message:?}"
        );
        for value in allowed {
            assert!(
                message.contains(value),
                "case {case:?}: expected the refusal to name the modelled value {value:?}, \
                 got {message:?}"
            );
        }
    }

    // None of them was persisted.
    let (ids, _) = walk_pages(&emulator, "the store after the enum table", None, 0).await;
    assert!(
        ids.is_empty(),
        "case \"the store after the enum table\": expected an empty store, got {ids:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 4: DeleteWorkflow releases the record, unless a server attaches it
// ---------------------------------------------------------------------------

/// RULE: `DeleteWorkflow` releases the workflow from every reader; it is refused with
/// the modelled `InvalidRequestException` while a server still attaches the workflow
/// through its `WorkflowDetails`; and an id the store does not hold answers the modelled
/// `ResourceNotFoundException`, whether it was never minted or already deleted.
///
/// The rows differ along which trigger attaches the workflow (`OnUpload`,
/// `OnPartialUpload`) and along why the id is unknown.
#[tokio::test]
async fn deleting_a_workflow_releases_it_unless_a_server_still_attaches_it() {
    let emulator = Emulator::new();

    let doomed = create_workflow(
        &emulator,
        "the workflow that is deleted",
        json!({"Description": "doomed", "Steps": [valid_step("DELETE")]}),
    )
    .await;
    let kept = create_workflow(
        &emulator,
        "the workflow that is kept",
        json!({"Description": "kept", "Steps": [valid_step("TAG")]}),
    )
    .await;

    let deleted = emulator
        .call("DeleteWorkflow", json!({"WorkflowId": &doomed}))
        .await;
    assert_eq!(
        deleted.status, 200,
        "case \"delete a stored workflow\": expected 200, got {} with body {}",
        deleted.status, deleted.body
    );

    // Gone from both readers, and the other workflow is untouched.
    let described = emulator
        .call("DescribeWorkflow", json!({"WorkflowId": &doomed}))
        .await;
    assert_eq!(
        described.status, 400,
        "case \"describe a deleted workflow\": expected HTTP 400, got {} with body {}",
        described.status, described.body
    );
    assert_eq!(
        described.error_code(),
        "ResourceNotFoundException",
        "case \"describe a deleted workflow\": expected ResourceNotFoundException, got body {}",
        described.body
    );
    assert_members(
        "describe a deleted workflow",
        &described.body,
        &json!({"Resource": &doomed, "ResourceType": "Workflow"}),
    );
    let (ids, _) = walk_pages(&emulator, "the store after the delete", None, 1).await;
    assert_eq!(
        ids,
        vec![kept.clone()],
        "case \"the store after the delete\": expected only the kept workflow, got {ids:?}"
    );

    // An id the store does not hold: never minted, or already deleted.
    for (case, workflow_id) in [
        ("already deleted", doomed.as_str()),
        ("never minted", "w-0000000000000dead"),
    ] {
        let answer = emulator
            .call("DeleteWorkflow", json!({"WorkflowId": workflow_id}))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got body {}",
            answer.body
        );
    }

    // A workflow a server attaches cannot be deleted, whichever trigger attaches it.
    for trigger in ["OnUpload", "OnPartialUpload"] {
        let case = format!("attached through {trigger}");
        let server_id = emulator
            .create_server(json!({
                "WorkflowDetails": {
                    trigger: [{"WorkflowId": &kept, "ExecutionRole": ROLE}],
                },
            }))
            .await;

        let answer = emulator
            .call("DeleteWorkflow", json!({"WorkflowId": &kept}))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got body {}",
            answer.body
        );
        let message = member_of(&answer.body, "Message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            message.contains(&server_id),
            "case {case:?}: expected the refusal to name the server {server_id:?}, got {message:?}"
        );

        // Refused, so the workflow is still there.
        let (ids, _) = walk_pages(&emulator, &case, None, 1).await;
        assert_eq!(
            ids,
            vec![kept.clone()],
            "case {case:?}: expected the attached workflow to survive the refusal, got {ids:?}"
        );

        // Detach it by dropping the server, so the next row starts from the same state.
        let deleted_server = emulator
            .call("DeleteServer", json!({"ServerId": &server_id}))
            .await;
        assert_eq!(
            deleted_server.status, 200,
            "case {case:?}: expected DeleteServer 200, got {} with body {}",
            deleted_server.status, deleted_server.body
        );
    }

    // With no server attached, the last workflow goes.
    let answer = emulator
        .call("DeleteWorkflow", json!({"WorkflowId": &kept}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case \"delete the detached workflow\": expected 200, got {} with body {}",
        answer.status, answer.body
    );
    let (ids, _) = walk_pages(&emulator, "the empty store", None, 0).await;
    assert!(
        ids.is_empty(),
        "case \"the empty store\": expected no workflows, got {ids:?}"
    );
}
