// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ONE invariant a workload's Region lists carry.
//!
//! The rule under test: "Either `AwsRegions`, `NonAwsRegions`, or both must be
//! specified" (`smithy_query(query="doc", shape="CreateWorkload")`) is an invariant of
//! the WORKLOAD, not a rule about one request — a stored workload always names at least
//! one Region it runs in. So every call that writes a workload is judged on the record
//! it would leave behind, and a merge that would leave none is refused with the modelled
//! `ValidationException` (HTTP 400) naming both members, leaving the stored record
//! exactly as it was.
//!
//! Every request goes through the real `MultiServiceRouter`, so the boundary walk and
//! the handler run exactly as they do in production.

use serde_json::{json, Value};

use super::workload_harness::{created_id, minimal_create, Client};

/// The one sentence that states the cross-member rule, as `state::either_regions_exception`
/// words it for the create path.
const SENTENCE: &str = "Either AwsRegions or NonAwsRegions is required.";

/// Create a workload holding exactly `aws` and `non_aws` and answer its `WorkloadId`.
///
/// `aws` entries are Amazon Web Services Regions from the published list the emulator
/// carries (`state::AWS_REGIONS`, whose source is named there); `non_aws` names Regions
/// outside AWS, which no published list bounds.
async fn seeded(client: &Client, name: &str, aws: &[&str], non_aws: &[&str]) -> String {
    let mut request = minimal_create(name, &format!("tok-{name}"));
    request["AwsRegions"] = json!(aws);
    request["NonAwsRegions"] = json!(non_aws);
    created_id(&client.create(request).await, name)
}

/// The ONE `Workload` a `GetWorkload` reports for `workload_id` — read once, so every
/// assertion a row makes is about the same snapshot of the stored record.
async fn stored_workload(client: &Client, workload_id: &str) -> Value {
    client
        .get(workload_id)
        .await
        .body
        .get("Workload")
        .cloned()
        .unwrap_or(Value::Null)
}

/// One row of either table: the Regions the workload is created with, the update sent
/// to it, and (for an accepted row) the Regions the merge must leave stored.
struct Row {
    case: &'static str,
    aws: &'static [&'static str],
    non_aws: &'static [&'static str],
    update: Value,
    merged_aws: &'static [&'static str],
    merged_non_aws: &'static [&'static str],
}

/// An update whose merged result would name no Region at all is refused, and changes
/// nothing — not the Regions, and not the other members the same request carried.
///
/// Rows differ along which member the update empties, along which lists the stored
/// workload already held (AWS only, non-AWS only, both), and along whether the same
/// request also asks for an unrelated change.
#[tokio::test]
async fn an_update_that_would_leave_no_region_is_refused() {
    let client = Client::new();

    // A refused update stores nothing, so each row's merged lists are the ones it was
    // created with.
    let rows = [
        Row {
            case: "both-emptied",
            aws: &["us-east-1"],
            non_aws: &[],
            update: json!({ "AwsRegions": [], "NonAwsRegions": [] }),
            merged_aws: &["us-east-1"],
            merged_non_aws: &[],
        },
        Row {
            case: "aws-emptied-with-no-stored-non-aws",
            aws: &["us-west-2", "eu-west-1"],
            non_aws: &[],
            update: json!({ "AwsRegions": [] }),
            merged_aws: &["us-west-2", "eu-west-1"],
            merged_non_aws: &[],
        },
        Row {
            case: "non-aws-emptied-with-no-stored-aws",
            aws: &[],
            non_aws: &["on-prem-dc-1"],
            update: json!({ "NonAwsRegions": [] }),
            merged_aws: &[],
            merged_non_aws: &["on-prem-dc-1"],
        },
        Row {
            case: "both-emptied-from-both-populated",
            aws: &["ap-south-1"],
            non_aws: &["colo-1", "colo-2"],
            update: json!({ "AwsRegions": [], "NonAwsRegions": [] }),
            merged_aws: &["ap-south-1"],
            merged_non_aws: &["colo-1", "colo-2"],
        },
        Row {
            case: "both-emptied-beside-a-rename",
            aws: &["ca-central-1"],
            non_aws: &["colo-7"],
            update: json!({
                "AwsRegions": [],
                "NonAwsRegions": [],
                "WorkloadName": "regions-rule-rename",
                "Description": "an update that breaks the Regions invariant",
            }),
            merged_aws: &["ca-central-1"],
            merged_non_aws: &["colo-7"],
        },
    ];

    for Row {
        case,
        aws,
        non_aws,
        update,
        merged_aws,
        merged_non_aws,
    } in rows
    {
        let workload_id = seeded(&client, case, aws, non_aws).await;
        let answer = client.update(&workload_id, update).await;

        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected the error code ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Reason").and_then(Value::as_str),
            Some("FIELD_VALIDATION_FAILED"),
            "case {case:?}: the rule is about the members' values, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Fields"),
            Some(&json!([
                { "Name": "AwsRegions", "Message": SENTENCE },
                { "Name": "NonAwsRegions", "Message": SENTENCE },
            ])),
            "case {case:?}: both members the rule covers must be reported, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Message").and_then(Value::as_str),
            Some(format!("[Validation] {SENTENCE}").as_str()),
            "case {case:?}: Message states the rule once, not once per member, got {answer:?}"
        );

        let stored = stored_workload(&client, &workload_id).await;
        assert_eq!(
            (stored.get("AwsRegions"), stored.get("NonAwsRegions")),
            (Some(&json!(merged_aws)), Some(&json!(merged_non_aws))),
            "case {case:?}: a refused update must leave the stored Regions alone, got {stored:?}"
        );
        assert_eq!(
            stored.get("WorkloadName").and_then(Value::as_str),
            Some(case),
            "case {case:?}: a refused update must apply none of its other members, got {stored:?}"
        );
    }

    // The refused rename left the name index alone too: the name it asked for is still
    // free, so a later create may take it.
    let taken = client
        .create(minimal_create("regions-rule-rename", "tok-rename-free"))
        .await;
    created_id(&taken, "rename-name-still-free");
}

/// An update is judged on the MERGED record, so emptying one list is accepted whenever
/// the other still names a Region — the stored list where the request names no
/// replacement, the list the same request supplies where it does.
///
/// Rows differ along which member is emptied, along where the surviving Region comes
/// from (the stored record, or the same request), along replacing rather than clearing a
/// list, and along an update that names neither member at all.
#[tokio::test]
async fn an_update_that_still_leaves_a_region_is_applied() {
    let client = Client::new();

    let rows = [
        Row {
            case: "aws-emptied-stored-non-aws-remains",
            aws: &["us-east-2"],
            non_aws: &["on-prem-dc-2"],
            update: json!({ "AwsRegions": [] }),
            merged_aws: &[],
            merged_non_aws: &["on-prem-dc-2"],
        },
        Row {
            case: "non-aws-emptied-stored-aws-remains",
            aws: &["sa-east-1"],
            non_aws: &["colo-3"],
            update: json!({ "NonAwsRegions": [] }),
            merged_aws: &["sa-east-1"],
            merged_non_aws: &[],
        },
        Row {
            case: "aws-emptied-and-non-aws-supplied-together",
            aws: &["us-east-1"],
            non_aws: &[],
            update: json!({ "AwsRegions": [], "NonAwsRegions": ["dc-berlin"] }),
            merged_aws: &[],
            merged_non_aws: &["dc-berlin"],
        },
        Row {
            case: "regions-replaced",
            aws: &["us-east-1"],
            non_aws: &[],
            update: json!({ "AwsRegions": ["eu-north-1", "ap-northeast-3"] }),
            merged_aws: &["eu-north-1", "ap-northeast-3"],
            merged_non_aws: &[],
        },
        Row {
            case: "neither-member-supplied",
            aws: &["us-gov-west-1"],
            non_aws: &["colo-4"],
            update: json!({ "Description": "an update that names no Region member at all" }),
            merged_aws: &["us-gov-west-1"],
            merged_non_aws: &["colo-4"],
        },
    ];

    for Row {
        case,
        aws,
        non_aws,
        update,
        merged_aws,
        merged_non_aws,
    } in rows
    {
        let workload_id = seeded(&client, case, aws, non_aws).await;
        let answer = client.update(&workload_id, update).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the merged workload still names a Region, got {answer:?}"
        );

        // Read it back through GetWorkload: the merge was stored, not merely accepted.
        let stored = stored_workload(&client, &workload_id).await;
        assert_eq!(
            (stored.get("AwsRegions"), stored.get("NonAwsRegions")),
            (Some(&json!(merged_aws)), Some(&json!(merged_non_aws))),
            "case {case:?}: GetWorkload must report the merged Regions, got {stored:?}"
        );
        let answered = answer.body.get("Workload").cloned().unwrap_or(Value::Null);
        assert_eq!(
            (
                answered.get("AwsRegions"),
                answered.get("NonAwsRegions")
            ),
            (Some(&json!(merged_aws)), Some(&json!(merged_non_aws))),
            "case {case:?}: the update's own answer must report both merged Region lists, got {answer:?}"
        );
    }
}
