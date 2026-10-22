// SPDX-License-Identifier: CC0-1.0

//! The rescore-execution-plan lifecycle, end to end over the wire: create, describe,
//! list (paged), rescore and delete, every answer computed from the plans this
//! service persisted.
//!
//! Each suite asserts a RULE over a table of inputs rather than one recorded call:
//! the ordering rule `Rescore` applies, the paging rule `ListRescoreExecutionPlans`
//! applies, the id/ARN rule `CreateRescoreExecutionPlan` applies, and the refusal rule
//! for a request outside a constraint.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, call_in, create_active_plan, create_plan, emulator, error_code,
    sleep_past_the_transition,
};

/// A 36-character `RescoreExecutionPlanId` that no `CreateRescoreExecutionPlan` call in
/// these tests mints, for the not-found cases.
const UNKNOWN_PLAN_ID: &str = "00000000-0000-4000-8000-000000000000";

/// The `DocumentId`s of a `Rescore` answer, in the order the service returned them.
fn result_document_ids(body: &Value) -> Vec<String> {
    body["ResultItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["DocumentId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `Score`s of a `Rescore` answer, in the order the service returned them.
fn result_scores(body: &Value) -> Vec<f64> {
    body["ResultItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["Score"].as_f64().unwrap_or(f64::NAN))
        .collect()
}

#[tokio::test]
async fn the_plan_lifecycle_is_served_from_the_persisted_plan() {
    let router = emulator();

    // CREATE — a fresh 36-character id and an ARN that carries it.
    let (status, created) = call(
        &router,
        "CreateRescoreExecutionPlan",
        json!({
            "Name": "my-plan",
            "Description": "demo",
            "CapacityUnits": { "RescoreCapacityUnits": 2 },
            "Tags": [{ "Key": "team", "Value": "search" }],
            "ClientToken": "tok-1",
        }),
    )
    .await;
    assert_eq!(status, 200, "create: unexpected answer {created}");
    let id = created["Id"].as_str().expect("create: an Id").to_string();
    assert_eq!(id.len(), 36, "create: Id must be 36 characters, got {id:?}");
    let arn = created["Arn"].as_str().expect("create: an Arn").to_string();
    assert_eq!(
        arn,
        format!(
            "arn:aws:kendra-ranking:us-east-1:{}:rescore-execution-plan/{id}",
            engine::EMULATOR_ACCOUNT_ID
        ),
        "create: unexpected Arn {arn}"
    );

    // DESCRIBE — every member read back from what create persisted. Creation is
    // asynchronous, so the first read still reports the transient CREATING status; the
    // plan is read again below once the clock owes it ACTIVE.
    let (status, described) =
        call(&router, "DescribeRescoreExecutionPlan", json!({ "Id": id })).await;
    assert_eq!(status, 200, "describe: unexpected answer {described}");
    assert_eq!(described["Id"], json!(id), "describe: got {described}");
    assert_eq!(described["Arn"], json!(arn), "describe: got {described}");
    assert_eq!(
        described["Name"],
        json!("my-plan"),
        "describe: got {described}"
    );
    assert_eq!(
        described["Description"],
        json!("demo"),
        "describe: got {described}"
    );
    assert_eq!(
        described["CapacityUnits"]["RescoreCapacityUnits"],
        json!(2),
        "describe: got {described}"
    );
    assert_eq!(
        described["Status"],
        json!("CREATING"),
        "describe: a plan whose creation has not settled is CREATING, got {described}"
    );
    assert!(
        described["CreatedAt"].is_number() && described["UpdatedAt"].is_number(),
        "describe: CreatedAt/UpdatedAt must be set, got {described}"
    );
    assert!(
        described["ErrorMessage"].is_null(),
        "describe: a healthy plan carries no ErrorMessage, got {described}"
    );

    // LIST — the plan appears once, with its persisted name and status.
    let (status, listed) = call(&router, "ListRescoreExecutionPlans", json!({})).await;
    assert_eq!(status, 200, "list: unexpected answer {listed}");
    let summaries = listed["SummaryItems"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(summaries.len(), 1, "list: got {listed}");
    assert_eq!(summaries[0]["Id"], json!(id), "list: got {listed}");
    assert_eq!(summaries[0]["Name"], json!("my-plan"), "list: got {listed}");
    assert_eq!(
        summaries[0]["Status"],
        json!("CREATING"),
        "list: a listing reports the same status as a describe, got {listed}"
    );
    assert!(
        listed["NextToken"].is_null(),
        "list: the final page carries no NextToken, got {listed}"
    );

    // The clock owes CREATING -> ACTIVE: once it is due, every reader reports ACTIVE.
    sleep_past_the_transition();
    let (status, described) =
        call(&router, "DescribeRescoreExecutionPlan", json!({ "Id": id })).await;
    assert_eq!(status, 200, "describe after settling: got {described}");
    assert_eq!(
        described["Status"],
        json!("ACTIVE"),
        "describe after settling: got {described}"
    );
    let (_, listed) = call(&router, "ListRescoreExecutionPlans", json!({})).await;
    assert_eq!(
        listed["SummaryItems"][0]["Status"],
        json!("ACTIVE"),
        "list after settling: got {listed}"
    );

    // RESCORE — one scored item per submitted document, under the live plan.
    let (status, rescored) = call(
        &router,
        "Rescore",
        json!({
            "RescoreExecutionPlanId": id,
            "SearchQuery": "intelligent ranking",
            "Documents": [
                { "Id": "d1", "OriginalScore": 0.4, "Title": "intelligent ranking guide", "Body": "a guide" },
                { "Id": "d2", "OriginalScore": 0.9, "Body": "unrelated" },
                { "Id": "d3", "OriginalScore": 0.1, "Title": "ranking", "Body": "notes" },
            ],
        }),
    )
    .await;
    assert_eq!(status, 200, "rescore: unexpected answer {rescored}");
    assert_eq!(
        result_document_ids(&rescored),
        vec!["d1", "d2", "d3"],
        "rescore: got {rescored}"
    );
    let rescore_id = rescored["RescoreId"].as_str().unwrap_or_default();
    assert!(
        !rescore_id.is_empty() && rescore_id.len() <= 36,
        "rescore: RescoreId must be 1..36 characters, got {rescore_id:?}"
    );

    // DELETE — an empty 200, and the plan is gone from every reader.
    let (status, deleted) = call(&router, "DeleteRescoreExecutionPlan", json!({ "Id": id })).await;
    assert_eq!(status, 200, "delete: unexpected answer {deleted}");

    let (status, described) =
        call(&router, "DescribeRescoreExecutionPlan", json!({ "Id": id })).await;
    assert_eq!(status, 400, "describe after delete: got {described}");
    assert_eq!(
        error_code(&described),
        "ResourceNotFoundException",
        "describe after delete: got {described}"
    );

    let (status, listed) = call(&router, "ListRescoreExecutionPlans", json!({})).await;
    assert_eq!(status, 200, "list after delete: got {listed}");
    assert_eq!(
        listed["SummaryItems"],
        json!([]),
        "list after delete: got {listed}"
    );
}

#[tokio::test]
async fn repeating_a_client_token_returns_the_one_plan_it_created() {
    let router = emulator();

    // The rule spans the token's value and the payload beside it: the same token names
    // the same plan, a different token a different one.
    let tokens = ["tok-a", "x", "a-client-token-with-mixed_chars.123"];
    let mut minted: BTreeMap<String, String> = BTreeMap::new();
    for token in &tokens {
        let (status, first) = call(
            &router,
            "CreateRescoreExecutionPlan",
            json!({ "Name": "plan", "ClientToken": token }),
        )
        .await;
        assert_eq!(status, 200, "token {token:?}: got {first}");
        let (status, second) = call(
            &router,
            "CreateRescoreExecutionPlan",
            // A different payload under the same token: the token decides.
            json!({ "Name": "plan-again", "Description": "second call", "ClientToken": token }),
        )
        .await;
        assert_eq!(status, 200, "token {token:?} repeated: got {second}");
        assert_eq!(
            first["Id"], second["Id"],
            "token {token:?}: a repeated token must return the same Id, got {second}"
        );
        assert_eq!(
            first["Arn"], second["Arn"],
            "token {token:?}: a repeated token must return the same Arn, got {second}"
        );
        minted.insert(
            token.to_string(),
            first["Id"].as_str().unwrap_or_default().to_string(),
        );
    }

    // Distinct tokens created distinct plans, and only those.
    let ids: std::collections::BTreeSet<&String> = minted.values().collect();
    assert_eq!(
        ids.len(),
        tokens.len(),
        "each distinct ClientToken creates its own plan, got {minted:?}"
    );
    let (_, listed) = call(&router, "ListRescoreExecutionPlans", json!({})).await;
    assert_eq!(
        listed["SummaryItems"].as_array().map(Vec::len),
        Some(tokens.len()),
        "only one plan per token exists, got {listed}"
    );
}

#[tokio::test]
async fn an_id_no_plan_was_created_for_is_not_found_on_every_operation_that_addresses_one() {
    let router = emulator();
    // A live plan beside it, so the refusal is about the id and not about an empty
    // store. ACTIVE, so the delete below is about the id and not about the plan's
    // status.
    let live = create_active_plan(&router, "live-plan").await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "DescribeRescoreExecutionPlan",
            json!({ "Id": UNKNOWN_PLAN_ID }),
        ),
        (
            "DeleteRescoreExecutionPlan",
            json!({ "Id": UNKNOWN_PLAN_ID }),
        ),
        (
            "Rescore",
            json!({
                "RescoreExecutionPlanId": UNKNOWN_PLAN_ID,
                "SearchQuery": "anything",
                "Documents": [{ "Id": "d1", "OriginalScore": 0.5, "Body": "a body" }],
            }),
        ),
    ];

    for (operation, body) in cases {
        let (status, answer) = call(&router, operation, body).await;
        // A client-side failure of this RPC service travels with the protocol's
        // client-error status; the body's error type says which failure it was.
        assert_eq!(status, 400, "case {operation}: got {answer}");
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {operation}: got {answer}"
        );
    }

    // And deleting the live plan twice: the second delete is the same refusal.
    let (status, first) = call(
        &router,
        "DeleteRescoreExecutionPlan",
        json!({ "Id": live }),
    )
    .await;
    assert_eq!(status, 200, "first delete: got {first}");
    let (status, second) = call(
        &router,
        "DeleteRescoreExecutionPlan",
        json!({ "Id": live }),
    )
    .await;
    assert_eq!(status, 400, "second delete: got {second}");
    assert_eq!(
        error_code(&second),
        "ResourceNotFoundException",
        "second delete: got {second}"
    );
}

/// One row of the `Rescore` ranking table.
struct RescoreCase {
    /// What this row varies.
    case: &'static str,
    query: &'static str,
    documents: Value,
    /// The document ids in the order the ranking rule puts them, where the rule pins
    /// the whole order; `None` where it does not.
    expected_order: Option<Vec<&'static str>>,
}

#[tokio::test]
async fn rescore_ranks_exactly_the_submitted_documents_by_descending_score() {
    let router = emulator();
    let plan = create_active_plan(&router, "ranking-plan").await;

    // The rows differ along document count, which text fields carry the query's terms,
    // the text vs. pre-tokenized form of those fields, the sign and magnitude of
    // `OriginalScore`, and repeated document ids.
    let cases = vec![
        RescoreCase {
            case: "a title carrying every query term outranks a higher original score",
            query: "intelligent ranking",
            documents: json!([
                { "Id": "d1", "OriginalScore": 0.4, "Title": "intelligent ranking guide", "Body": "a guide" },
                { "Id": "d2", "OriginalScore": 0.9, "Body": "unrelated" },
                { "Id": "d3", "OriginalScore": 0.1, "Title": "ranking", "Body": "notes" },
            ]),
            expected_order: Some(vec!["d1", "d2", "d3"]),
        },
        RescoreCase {
            case: "a single document",
            query: "lonely",
            documents: json!([{ "Id": "only", "OriginalScore": 0.5, "Title": "lonely", "Body": "a note" }]),
            expected_order: Some(vec!["only"]),
        },
        RescoreCase {
            case: "no document carries a query term, so the original order decides",
            query: "nothing matches here",
            documents: json!([
                { "Id": "a", "OriginalScore": 0.1, "Body": "apples" },
                { "Id": "b", "OriginalScore": 0.75, "Body": "pears" },
                { "Id": "c", "OriginalScore": 0.5, "Body": "plums" },
            ]),
            expected_order: Some(vec!["b", "c", "a"]),
        },
        RescoreCase {
            case: "a title match outranks a body match, which outranks no match",
            query: "cloud search",
            documents: json!([
                { "Id": "t-both", "OriginalScore": 0.0, "Title": "cloud search engine", "Body": "notes" },
                { "Id": "t-one", "OriginalScore": 0.0, "Title": "cloud", "Body": "notes" },
                { "Id": "b-both", "OriginalScore": 0.0, "Body": "cloud search tuning" },
                { "Id": "tok", "OriginalScore": 0.0, "TokenizedBody": ["cloud", "search"] },
                { "Id": "plain", "OriginalScore": 0.2, "Body": "notes" },
            ]),
            expected_order: Some(vec!["t-both", "t-one", "b-both", "tok", "plain"]),
        },
        RescoreCase {
            case: "original scores at both published bounds",
            query: "bounds",
            documents: json!([
                { "Id": "hi", "OriginalScore": 100000.0, "Body": "a note" },
                { "Id": "lo", "OriginalScore": -100000.0, "Body": "a note" },
            ]),
            expected_order: Some(vec!["hi", "lo"]),
        },
        RescoreCase {
            case: "a repeated document id still gets one result item each",
            query: "repeat",
            documents: json!([
                { "Id": "dup", "OriginalScore": 0.3, "Body": "a note" },
                { "Id": "dup", "OriginalScore": 0.3, "Body": "a note" },
            ]),
            expected_order: Some(vec!["dup", "dup"]),
        },
        RescoreCase {
            case: "a query whose terms repeat, over pre-tokenized titles",
            query: "alpha alpha beta",
            documents: json!([
                { "Id": "both", "OriginalScore": 0.0, "TokenizedTitle": ["Alpha", "BETA"], "Body": "a note" },
                { "Id": "one", "OriginalScore": 0.0, "TokenizedTitle": ["alpha"], "Body": "a note" },
            ]),
            expected_order: Some(vec!["both", "one"]),
        },
    ];

    for case in cases {
        let submitted: Vec<String> = case
            .documents
            .as_array()
            .expect("the case's documents")
            .iter()
            .map(|d| d["Id"].as_str().unwrap_or_default().to_string())
            .collect();
        let (status, answer) = call(
            &router,
            "Rescore",
            json!({
                "RescoreExecutionPlanId": plan,
                "SearchQuery": case.query,
                "Documents": case.documents,
            }),
        )
        .await;
        assert_eq!(status, 200, "case {:?}: got {answer}", case.case);

        let returned = result_document_ids(&answer);
        assert_eq!(
            returned.len(),
            submitted.len(),
            "case {:?}: one result item per submitted document, got {returned:?}",
            case.case
        );
        let mut sorted_returned = returned.clone();
        sorted_returned.sort();
        let mut sorted_submitted = submitted.clone();
        sorted_submitted.sort();
        assert_eq!(
            sorted_returned, sorted_submitted,
            "case {:?}: every DocumentId comes from the request, got {returned:?}",
            case.case
        );

        let scores = result_scores(&answer);
        for window in scores.windows(2) {
            assert!(
                window[0] >= window[1],
                "case {:?}: the results must be in descending order, got {scores:?}",
                case.case
            );
        }
        for score in &scores {
            assert!(
                (-100_000.0..=100_000.0).contains(score),
                "case {:?}: Score must stay inside the modelled range, got {score}",
                case.case
            );
        }
        if let Some(expected) = case.expected_order {
            assert_eq!(
                returned, expected,
                "case {:?}: unexpected ranking, scores {scores:?}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn a_pre_tokenized_field_scores_the_same_as_its_text_form() {
    let router = emulator();
    let plan = create_active_plan(&router, "tokenized-plan").await;

    // `Title`/`TokenizedTitle` and `Body`/`TokenizedBody` are two forms of one field,
    // so the same words must score the same whichever form carries them.
    let (status, answer) = call(
        &router,
        "Rescore",
        json!({
            "RescoreExecutionPlanId": plan,
            "SearchQuery": "alpha beta",
            "Documents": [
                { "Id": "text", "OriginalScore": 0.2, "Title": "Alpha beta!", "Body": "a note" },
                { "Id": "tokens", "OriginalScore": 0.2, "TokenizedTitle": ["alpha", "BETA"], "Body": "a note" },
            ],
        }),
    )
    .await;
    assert_eq!(status, 200, "got {answer}");
    let scores = result_scores(&answer);
    assert_eq!(scores.len(), 2, "got {answer}");
    assert!(
        (scores[0] - scores[1]).abs() < 1e-6,
        "the text and pre-tokenized forms of one title must score alike, got {scores:?}"
    );
}

#[tokio::test]
async fn a_paged_listing_visits_every_plan_exactly_once() {
    // The rule generalises over how many plans exist and how large a page is asked for,
    // including no page size at all and the top of `MaxResults`'s published range.
    let cases: Vec<(usize, Option<i64>)> = vec![
        (0, None),
        (0, Some(1)),
        (1, Some(1)),
        (3, None),
        (5, Some(2)),
        (7, Some(50)),
        (4, Some(4)),
    ];

    for (plan_count, page_size) in cases {
        let router = emulator();
        let mut created = Vec::new();
        for n in 0..plan_count {
            created.push(create_plan(&router, &format!("plan-{n}")).await);
        }
        created.sort();

        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({});
            if let Some(size) = page_size {
                body["MaxResults"] = json!(size);
            }
            if let Some(token) = &next_token {
                body["NextToken"] = json!(token);
            }
            let (status, answer) = call(&router, "ListRescoreExecutionPlans", body.clone()).await;
            assert_eq!(
                status, 200,
                "case {plan_count} plans / page {page_size:?}: got {answer}"
            );
            pages += 1;
            let page: Vec<String> = answer["SummaryItems"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|item| item["Id"].as_str().unwrap_or_default().to_string())
                .collect();
            if let Some(size) = page_size {
                assert!(
                    page.len() as i64 <= size,
                    "case {plan_count} plans / page {page_size:?}: a page must not exceed \
                     MaxResults, got {} items",
                    page.len()
                );
            }
            seen.extend(page);
            next_token = answer["NextToken"].as_str().map(str::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= plan_count + 2,
                "case {plan_count} plans / page {page_size:?}: paging did not terminate \
                 after {pages} pages"
            );
        }

        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {plan_count} plans / page {page_size:?}: a plan was listed twice, got {seen:?}"
        );
        let mut sorted_seen = seen.clone();
        sorted_seen.sort();
        assert_eq!(
            sorted_seen, created,
            "case {plan_count} plans / page {page_size:?}: the walk must visit every plan, \
             got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_pagination_token_this_service_did_not_issue_is_refused() {
    let router = emulator();
    create_plan(&router, "one-plan").await;

    // Rows: too short, too long, the right length but outside the id character set,
    // and a hex blob of the right length that is not an id either.
    for token in [
        "not-a-token",
        "zz",
        &"0".repeat(37),
        "0000-0000-0000-0000-0000-0000-00!!!!",
        &::hex::encode("0000-0000"),
    ] {
        let (status, answer) = call(
            &router,
            "ListRescoreExecutionPlans",
            json!({ "NextToken": token }),
        )
        .await;
        assert_eq!(status, 400, "token {token:?}: got {answer}");
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "token {token:?}: got {answer}"
        );
    }
}

#[tokio::test]
async fn a_request_outside_a_constraint_is_refused_with_validation_exception() {
    let router = emulator();
    let plan = create_active_plan(&router, "validating-plan").await;

    // One row per constraint class: the name's pattern and length, an absent required
    // member, the empty document list, a value outside a published numeric range, an id
    // of the wrong length, `MaxResults` past each of its bounds, and the documented
    // "Title or TokenizedTitle, not both" rule.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "a Name with a character outside its pattern",
            "CreateRescoreExecutionPlan",
            json!({ "Name": "bad name!" }),
        ),
        (
            "an empty Name",
            "CreateRescoreExecutionPlan",
            json!({ "Name": "" }),
        ),
        (
            "a Name past its 1000-character bound",
            "CreateRescoreExecutionPlan",
            json!({ "Name": "a".repeat(1001) }),
        ),
        (
            "an absent required Name",
            "CreateRescoreExecutionPlan",
            json!({ "Description": "no name" }),
        ),
        (
            "a negative RescoreCapacityUnits",
            "CreateRescoreExecutionPlan",
            json!({ "Name": "plan", "CapacityUnits": { "RescoreCapacityUnits": -1 } }),
        ),
        (
            "an empty Documents list",
            "Rescore",
            json!({ "RescoreExecutionPlanId": plan, "SearchQuery": "q", "Documents": [] }),
        ),
        (
            "an OriginalScore past its published range",
            "Rescore",
            json!({
                "RescoreExecutionPlanId": plan,
                "SearchQuery": "q",
                "Documents": [{ "Id": "d1", "OriginalScore": 100001.0 }],
            }),
        ),
        (
            "a document providing both Title and TokenizedTitle",
            "Rescore",
            json!({
                "RescoreExecutionPlanId": plan,
                "SearchQuery": "q",
                "Documents": [{
                    "Id": "d1",
                    "OriginalScore": 0.5,
                    "Title": "both",
                    "TokenizedTitle": ["both"],
                    "Body": "a note",
                }],
            }),
        ),
        (
            "a document providing both Body and TokenizedBody",
            "Rescore",
            json!({
                "RescoreExecutionPlanId": plan,
                "SearchQuery": "q",
                "Documents": [{
                    "Id": "d1",
                    "OriginalScore": 0.5,
                    "Body": "both",
                    "TokenizedBody": ["both"],
                }],
            }),
        ),
        (
            "a plan id that is not 36 characters",
            "DescribeRescoreExecutionPlan",
            json!({ "Id": "too-short" }),
        ),
        (
            "MaxResults below its published range",
            "ListRescoreExecutionPlans",
            json!({ "MaxResults": 0 }),
        ),
        (
            "MaxResults above its published range",
            "ListRescoreExecutionPlans",
            json!({ "MaxResults": 51 }),
        ),
    ];

    for (case, operation, body) in cases {
        let (status, answer) = call(&router, operation, body).await;
        assert_eq!(status, 400, "case {case:?} on {operation}: got {answer}");
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?} on {operation}: got {answer}"
        );
    }

    // The per-request document ceiling: accepted AT the bound, refused one past it.
    let ceiling =
        service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::state::MAX_DOCUMENTS_PER_RESCORE;
    for (case, count, expected_status) in [
        ("documents at the per-request ceiling", ceiling, 200),
        ("documents one past the ceiling", ceiling + 1, 400),
    ] {
        let documents: Vec<Value> = (0..count)
            .map(|n| json!({ "Id": format!("d{n}"), "OriginalScore": 0.5, "Body": "a note" }))
            .collect();
        let (status, answer) = call(
            &router,
            "Rescore",
            json!({
                "RescoreExecutionPlanId": plan,
                "SearchQuery": "batch",
                "Documents": documents,
            }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?} ({count} documents): got {answer}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case:?}: got {answer}"
            );
        } else {
            assert_eq!(
                answer["ResultItems"].as_array().map(Vec::len),
                Some(count),
                "case {case:?}: one result item per document, got {answer}"
            );
        }
    }

    // A refused request changed nothing: the one plan created above is still the only one.
    let (_, listed) = call(&router, "ListRescoreExecutionPlans", json!({})).await;
    assert_eq!(
        listed["SummaryItems"].as_array().map(Vec::len),
        Some(1),
        "a refused request must not create a plan, got {listed}"
    );
}

#[tokio::test]
async fn a_plan_past_the_quota_is_refused_with_the_modelled_quota_error() {
    let router = emulator();
    // Fill the store to its quota, then the next create is refused — and the store is
    // unchanged, so the plans already there still list.
    for n in 0..service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::state::MAX_RESCORE_EXECUTION_PLANS
    {
        create_plan(&router, &format!("quota-plan-{n}")).await;
    }

    let (status, answer) = call(
        &router,
        "CreateRescoreExecutionPlan",
        json!({ "Name": "one-too-many" }),
    )
    .await;
    // The quota refusal is a client-side failure, so it travels with this service's
    // client-error status like every other one; its error TYPE is what names it.
    assert_eq!(status, 400, "a create past the quota: got {answer}");
    assert_eq!(
        error_code(&answer),
        "ServiceQuotaExceededException",
        "a create past the quota: got {answer}"
    );

    // Deleting a plan frees its slot, so the quota is a live count and not a one-way
    // latch. The plan has to be ACTIVE first: a delete mid-provisioning is a conflict.
    let freed = first_listed_plan(&router).await;
    sleep_past_the_transition();
    let (status, deleted) = call(
        &router,
        "DeleteRescoreExecutionPlan",
        json!({ "Id": freed }),
    )
    .await;
    assert_eq!(status, 200, "freeing one plan: got {deleted}");
    let (status, created) = call(
        &router,
        "CreateRescoreExecutionPlan",
        json!({ "Name": "room-again" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a create after a delete freed room: got {created}"
    );
}

/// The id of the first plan a listing reports, for freeing a slot at the quota.
async fn first_listed_plan(router: &MultiServiceRouter) -> String {
    let (_, listed) = call(router, "ListRescoreExecutionPlans", json!({ "MaxResults": 1 })).await;
    listed["SummaryItems"][0]["Id"]
        .as_str()
        .expect("a listed plan")
        .to_string()
}

#[tokio::test]
async fn a_plan_arn_names_the_region_the_request_was_signed_for() {
    // The ARN rule spans every region a caller can sign for, not only the default one.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2", "us-gov-west-1"] {
        let router = emulator();
        let (status, created) = call_in(
            &router,
            region,
            "CreateRescoreExecutionPlan",
            json!({ "Name": "regional-plan" }),
        )
        .await;
        assert_eq!(status, 200, "region {region}: got {created}");
        let id = created["Id"].as_str().unwrap_or_default().to_string();
        let expected = format!(
            "arn:aws:kendra-ranking:{region}:{}:rescore-execution-plan/{id}",
            engine::EMULATOR_ACCOUNT_ID
        );
        assert_eq!(
            created["Arn"],
            json!(expected),
            "region {region}: got {created}"
        );

        // And the stored ARN is what Describe reports, whichever region reads it.
        let (status, described) =
            call_in(&router, "us-east-1", "DescribeRescoreExecutionPlan", json!({ "Id": id })).await;
        assert_eq!(status, 200, "region {region}: got {described}");
        assert_eq!(
            described["Arn"],
            json!(expected),
            "region {region}: Describe must report the stored ARN, got {described}"
        );
    }
}

#[tokio::test]
async fn no_operation_of_this_service_is_a_scaffold_any_more() {
    let router = emulator();
    let plan = create_plan(&router, "census-plan").await;
    let (_, described) =
        call(&router, "DescribeRescoreExecutionPlan", json!({ "Id": plan })).await;
    let arn = described["Arn"].as_str().unwrap_or_default().to_string();

    // The stub census (`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/`) is now
    // empty for this service — the tagging operations are implemented in
    // `tests/all/resource_tags.rs`'s suites and `UpdateRescoreExecutionPlan` in
    // `tests/all/plan_update.rs` — so no operation the service models may answer 501
    // for a well-formed request. (What each one answers instead is its own suite's
    // subject; this row only pins that nothing is a scaffold.)
    let cases: Vec<(&str, Value)> = vec![
        (
            "CreateRescoreExecutionPlan",
            json!({ "Name": "census-created" }),
        ),
        ("DescribeRescoreExecutionPlan", json!({ "Id": plan })),
        ("ListRescoreExecutionPlans", json!({})),
        (
            "UpdateRescoreExecutionPlan",
            json!({ "Id": plan, "Name": "renamed" }),
        ),
        (
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": [{ "Key": "k", "Value": "v" }] }),
        ),
        ("ListTagsForResource", json!({ "ResourceARN": arn })),
        (
            "UntagResource",
            json!({ "ResourceARN": arn, "TagKeys": ["k"] }),
        ),
        (
            "Rescore",
            json!({
                "RescoreExecutionPlanId": plan,
                "SearchQuery": "census",
                "Documents": [{ "Id": "d", "OriginalScore": 0.5, "Body": "a note" }],
            }),
        ),
        ("DeleteRescoreExecutionPlan", json!({ "Id": plan })),
    ];
    for (operation, body) in cases {
        let (status, answer) = call(&router, operation, body).await;
        assert_ne!(
            status, 501,
            "case {operation}: a scaffold 501 is left, got {answer}"
        );
    }
}
