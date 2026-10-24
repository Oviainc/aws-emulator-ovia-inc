// SPDX-License-Identifier: CC0-1.0

//! `ListInvestigations`, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. A list reports exactly the investigations the creates PERSISTED on that
//!    detector, each summary member carrying what `GetInvestigation` reports for the
//!    same investigation — and nothing for the members the analysis would have
//!    produced. A detector that holds none reports an empty list, not a canned one.
//! 2. `SortCriteria` orders the answer by the named `InvestigationSortField` in the
//!    named `OrderBy`, with the investigation id breaking ties so the order is total;
//!    a request that names no criterion is answered newest-first.
//! 3. `MaxResults`/`NextToken` page that ordered list: whatever the page size, each
//!    investigation appears exactly once, in the unpaged order, and only the last page
//!    carries no `NextToken`.
//! 4. A `DetectorId` the account/Region does not hold is an AUTHORIZATION failure
//!    (HTTP 403), a value that cannot be a `DetectorId` at all is a parameter failure
//!    (HTTP 400) — never an empty page; so are a sort attribute the model does not
//!    carry and a `NextToken` the service never issued.
//! 5. The operation is served exactly through the Regions its documentation lists.
//! 6. Listing is a REPORT: it never moves an investigation's status early and never
//!    changes what a later list or read sees; the only transition it materialises is
//!    the one the CLOCK already owed, which a read reports identically.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, new_router, Router, ABSENT_DETECTOR_ID,
};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    investigation_sort_fields, INVESTIGATION_REGIONS, INVESTIGATION_TRANSITION_MILLIS,
};

/// The `InvestigationStatus` members a listed investigation can be in here: it starts
/// `RUNNING` and reaches `FAILED` once the clock owes it, because this emulator runs no
/// analysis (`smithy_query(shape="InvestigationStatus")`).
const RUNNING: &str = "RUNNING";
const FAILED: &str = "FAILED";

/// The `OrderBy` members, `ASC | DESC` (`smithy_query(shape="OrderBy")`).
const ORDERS: [&str; 2] = ["ASC", "DESC"];

/// Long enough for the clock to owe the terminal status, plus slack.
fn past_the_transition() -> std::time::Duration {
    std::time::Duration::from_millis(INVESTIGATION_TRANSITION_MILLIS as u64 + 250)
}

async fn list(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/investigation/list"),
        Some(body),
    )
    .await
}

/// The `Investigations` a successful list reports.
async fn listed(router: &Router, detector_id: &str, body: Value) -> Vec<Value> {
    let (status, answered) = list(router, detector_id, body.clone()).await;
    assert_eq!(
        status, 200,
        "list-investigations {body:?}: expected 200, got {status} {answered:?}"
    );
    assert!(
        answered.get("nextToken").is_none(),
        "list-investigations {body:?}: a single page over this detector must not carry a nextToken, got {answered:?}"
    );
    answered["investigations"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("list-investigations {body:?}: expected an investigations list, got {answered:?}")
        })
        .clone()
}

/// Initiate an investigation and return its id. `CreateInvestigation`'s success status
/// is 202 (`smithy_query(operation="CreateInvestigation")`).
async fn initiated(router: &Router, detector_id: &str, prompt: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/investigation"),
        Some(json!({"triggerPrompt": prompt})),
    )
    .await;
    assert_eq!(
        status, 202,
        "create-investigation {prompt:?}: expected 202, got {status} {body:?}"
    );
    body["investigationId"]
        .as_str()
        .unwrap_or_else(|| panic!("create-investigation {prompt:?}: expected an id, got {body:?}"))
        .to_string()
}

/// The `Investigation` `GetInvestigation` reports.
async fn read(router: &Router, detector_id: &str, investigation_id: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/investigation/{investigation_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "get-investigation {investigation_id}: expected 200, got {status} {body:?}"
    );
    body["investigation"].clone()
}

fn id_of(summary: &Value) -> String {
    summary["investigationId"]
        .as_str()
        .unwrap_or_else(|| panic!("a summary without an investigationId: {summary:?}"))
        .to_string()
}

fn ids(summaries: &[Value]) -> Vec<String> {
    summaries.iter().map(id_of).collect()
}

/// A detector holding investigations in BOTH statuses: the first batch has had the
/// clock owe it its terminal status, the second has not. That spans what the sort keys
/// can carry — a present and an absent `EndTime`, two different `Status` values.
async fn detector_with_mixed_investigations(router: &Router) -> (String, Vec<String>) {
    let detector_id = enabled_detector(router).await;
    let mut created = Vec::new();
    for n in 1..=3 {
        created.push(initiated(router, &detector_id, &format!("settled {n}")).await);
    }
    tokio::time::sleep(past_the_transition()).await;
    for n in 1..=3 {
        created.push(initiated(router, &detector_id, &format!("running {n}")).await);
    }
    (detector_id, created)
}

// ---------------------------------------------------------------------------
// Rule 1: a list reports what the creates persisted, and agrees with a read.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_list_reports_exactly_the_persisted_investigations() {
    let router = new_router();

    // A detector that holds none reports an empty list — not absent, not canned.
    // Its own router: GuardDuty allows one detector per account per Region.
    let empty_router = new_router();
    let empty_detector = enabled_detector(&empty_router).await;
    assert_eq!(
        listed(&empty_router, &empty_detector, json!({})).await,
        Vec::<Value>::new(),
        "list-investigations over a detector holding none: expected []"
    );

    let (detector_id, created) = detector_with_mixed_investigations(&router).await;
    let summaries = listed(&router, &detector_id, json!({})).await;

    assert_eq!(
        summaries.len(),
        created.len(),
        "list-investigations: expected {} summaries, got {summaries:?}",
        created.len()
    );
    assert_eq!(
        ids(&summaries).into_iter().collect::<BTreeSet<_>>(),
        created.iter().cloned().collect::<BTreeSet<_>>(),
        "list-investigations: expected exactly the created ids, got {summaries:?}"
    );

    let account = engine::emulator_account_id();
    for summary in &summaries {
        let id = id_of(summary);
        let got = read(&router, &detector_id, &id).await;

        // Every member the summary shares with the `Investigation` a read reports is
        // the SAME stored value: one record, two views of it.
        for member in ["investigationId", "status", "triggerPrompt", "startTime"] {
            assert_eq!(
                summary[member], got[member],
                "case {id}: summary {member} must match the read, got {summary:?} vs {got:?}"
            );
        }
        assert_eq!(
            summary.get("endTime"),
            got.get("endTime"),
            "case {id}: summary endTime must match the read, got {summary:?} vs {got:?}"
        );
        // "The Amazon Web Services account ID associated with the investigation"
        // (`smithy_query(shape="InvestigationSummary")`): the account that initiated
        // it, which the read reports as `TriggeredBy`.
        assert_eq!(
            summary["accountId"],
            json!(account),
            "case {id}: expected accountId {account}, got {summary:?}"
        );
        assert_eq!(
            summary["accountId"], got["triggeredBy"],
            "case {id}: accountId must be the account the read reports, got {summary:?} vs {got:?}"
        );

        // The analysis is what produces an assessment, and this emulator runs none:
        // those members are ABSENT rather than synthesised.
        for member in ["riskLevel", "confidence", "title"] {
            assert!(
                summary.get(member).is_none(),
                "case {id}: {member} must be absent while no analysis produced one, got {summary:?}"
            );
        }

        // A settled investigation reports when it ended; a running one does not.
        let status = summary["status"].as_str().unwrap_or_default();
        assert!(
            [RUNNING, FAILED].contains(&status),
            "case {id}: expected an InvestigationStatus member, got {summary:?}"
        );
        assert_eq!(
            summary.get("endTime").is_some(),
            status == FAILED,
            "case {id}: endTime is reported exactly when the investigation ended, got {summary:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: SortCriteria orders the answer.
// ---------------------------------------------------------------------------

/// The comparison key one `InvestigationSortField` member resolves to, as the summary
/// reports it. The fields are the enum's own members — `START_TIME | END_TIME | STATUS
/// | RISK_LEVEL | CONFIDENCE` (`smithy_query(shape="InvestigationSortField")`) — and
/// the table is driven by [`investigation_sort_fields`], so a field the model gains is
/// a row here too.
fn sort_key(summary: &Value, field: &str) -> (f64, String) {
    match field {
        "START_TIME" => (summary["startTime"].as_f64().unwrap_or(0.0), String::new()),
        // Absent until the investigation ends, and ordering by an absent instant is
        // ordering by the epoch.
        "END_TIME" => (summary["endTime"].as_f64().unwrap_or(0.0), String::new()),
        "STATUS" => (
            0.0,
            summary["status"].as_str().unwrap_or_default().to_string(),
        ),
        // Both are the ANALYSIS's conclusions, which nothing produced here, so every
        // record carries the same key and the id tiebreak is the whole order.
        "RISK_LEVEL" | "CONFIDENCE" => (0.0, String::new()),
        other => panic!("sort field {other} has no key in this test"),
    }
}

/// Assert that `summaries` are ordered by `field` in `descending` order or not, with
/// the investigation id breaking ties.
fn assert_ordered(summaries: &[Value], field: &str, descending: bool, case: &str) {
    for pair in summaries.windows(2) {
        let (left, right) = (&pair[0], &pair[1]);
        let (left_key, right_key) = (sort_key(left, field), sort_key(right, field));
        let primary = left_key
            .0
            .partial_cmp(&right_key.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left_key.1.cmp(&right_key.1));
        match primary {
            std::cmp::Ordering::Equal => assert!(
                id_of(left) < id_of(right),
                "case {case}: equal {field} keys must be ordered by investigation id, got {:?} then {:?}",
                id_of(left),
                id_of(right)
            ),
            std::cmp::Ordering::Less => assert!(
                !descending,
                "case {case}: expected descending {field}, got {left_key:?} before {right_key:?}"
            ),
            std::cmp::Ordering::Greater => assert!(
                descending,
                "case {case}: expected ascending {field}, got {left_key:?} before {right_key:?}"
            ),
        }
    }
}

#[tokio::test]
async fn a_sort_criterion_orders_the_answer_and_keeps_the_same_set() {
    let router = new_router();
    let (detector_id, created) = detector_with_mixed_investigations(&router).await;
    let expected: BTreeSet<String> = created.iter().cloned().collect();

    // Every modelled field, in every modelled order — plus a field with no explicit
    // order, which the service answers ascending for the attribute the caller named.
    for field in investigation_sort_fields() {
        for order in ORDERS {
            let case = format!("{field}/{order}");
            let summaries = listed(
                &router,
                &detector_id,
                json!({"sortCriteria": {"attributeName": field, "orderBy": order}}),
            )
            .await;
            assert_eq!(
                ids(&summaries).into_iter().collect::<BTreeSet<_>>(),
                expected,
                "case {case}: a sort reorders the answer, it never changes the set, got {summaries:?}"
            );
            assert_ordered(&summaries, field, order == "DESC", &case);
        }

        let case = format!("{field}/no orderBy");
        let summaries = listed(
            &router,
            &detector_id,
            json!({"sortCriteria": {"attributeName": field}}),
        )
        .await;
        assert_ordered(&summaries, field, false, &case);
    }

    // No criterion at all: newest first, which is `START_TIME` descending.
    for body in [json!({}), json!({"sortCriteria": {}})] {
        let summaries = listed(&router, &detector_id, body.clone()).await;
        assert_eq!(
            ids(&summaries).into_iter().collect::<BTreeSet<_>>(),
            expected,
            "case {body:?}: expected the whole set, got {summaries:?}"
        );
        assert_ordered(&summaries, "START_TIME", true, &format!("{body:?}"));
    }
}

// ---------------------------------------------------------------------------
// Rule 3: paging the ordered list.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn paging_yields_each_investigation_exactly_once_in_order() {
    let router = new_router();
    let (detector_id, created) = detector_with_mixed_investigations(&router).await;

    // The page sizes span the modelled `MaxResults` range 1..50
    // (`smithy_query(operation="ListInvestigations")`): both published bounds, a size
    // that divides the set, one that does not, and one past the set.
    for max_results in [1, 2, 4, 6, 7, 50] {
        for sort in [
            json!(null),
            json!({"attributeName": "START_TIME", "orderBy": "ASC"}),
            json!({"attributeName": "STATUS", "orderBy": "DESC"}),
        ] {
            let case = format!("maxResults {max_results}, sortCriteria {sort}");
            let mut body = json!({"maxResults": max_results});
            if !sort.is_null() {
                body["sortCriteria"] = sort.clone();
            }

            // The unpaged answer under the same ordering is what the pages must add
            // up to.
            let mut unpaged_body = json!({});
            if !sort.is_null() {
                unpaged_body["sortCriteria"] = sort.clone();
            }
            let unpaged = ids(&listed(&router, &detector_id, unpaged_body).await);

            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut request = body.clone();
                if let Some(token) = token.as_deref() {
                    request["nextToken"] = json!(token);
                }
                let (status, answered) = list(&router, &detector_id, request.clone()).await;
                assert_eq!(
                    status, 200,
                    "case {case}: page {pages} expected 200, got {status} {answered:?}"
                );
                let page = answered["investigations"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case}: expected a page, got {answered:?}"))
                    .clone();
                assert!(
                    !page.is_empty() && page.len() <= max_results.min(unpaged.len()),
                    "case {case}: page {pages} must carry 1..={max_results} summaries, got {page:?}"
                );
                seen.extend(ids(&page));
                pages += 1;
                assert!(
                    pages <= unpaged.len() + 1,
                    "case {case}: paging did not terminate after {pages} pages"
                );
                token = answered
                    .get("nextToken")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if token.is_none() {
                    break;
                }
            }

            assert_eq!(
                seen, unpaged,
                "case {case}: paging must yield the unpaged order exactly once each, got {seen:?}"
            );
            assert_eq!(
                seen.iter().cloned().collect::<BTreeSet<_>>(),
                created.iter().cloned().collect::<BTreeSet<_>>(),
                "case {case}: paging must yield every created investigation, got {seen:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: what a refusal looks like.
// ---------------------------------------------------------------------------

/// Values that cannot be a `DetectorId` at all: the id is 32 hexadecimal characters
/// ([`service_com_amazonaws_guardduty_guarddutyapiservice::state::is_detector_id`]),
/// and CASE is not significant, so an upper-cased id is well formed and is covered by
/// the ownership failure above instead.
const MALFORMED_DETECTOR_IDS: [(&str, &str); 4] = [
    ("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz", "32 non-hex characters"),
    ("not-a-detector", "not an id at all"),
    ("0", "one character where 32 are required"),
    (
        "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        "40 hexadecimal characters",
    ),
];

#[tokio::test]
async fn a_list_over_an_unresolvable_detector_is_refused_not_emptied() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    initiated(&router, &detector_id, "a real investigation").await;

    // A well-formed id this account/Region does not hold: the caller's permission on
    // it is decided before it is resolved, so the answer says nothing about whether it
    // names anything. Case is not significant in the FORM, so an upper-cased id is
    // well formed too and is denied the same way.
    for (absent, case) in [
        (ABSENT_DETECTOR_ID, "a well-formed id nothing minted"),
        (
            "DEADBEEFDEADBEEFDEADBEEFDEADBEEF",
            "the same id upper-cased",
        ),
    ] {
        let (status, denied) = list(&router, absent, json!({})).await;
        assert_eq!(
            status, 403,
            "list over {case}: expected 403, got {status} {denied:?}"
        );
        assert_eq!(
            error_code(&denied),
            "AccessDeniedException",
            "list over {case}: got {denied:?}"
        );
    }

    for (malformed, case) in MALFORMED_DETECTOR_IDS {
        let (status, refused) = list(&router, malformed, json!({})).await;
        assert_eq!(
            status, 400,
            "list with {case}: expected 400, got {status} {refused:?}"
        );
    }

    // Dropping the detector drops the investigations that hung off it: the list that
    // answered a moment ago is now a detector the account does not hold.
    let (status, body) = call(&router, "DELETE", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(status, 200, "delete-detector: got {status} {body:?}");
    let (status, denied) = list(&router, &detector_id, json!({})).await;
    assert_eq!(
        status, 403,
        "list after the detector was deleted: expected 403, got {status} {denied:?}"
    );
}

#[tokio::test]
async fn an_unmodelled_sort_attribute_and_an_unissued_token_are_refused() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    for n in 1..=3 {
        initiated(&router, &detector_id, &format!("investigation {n}")).await;
    }

    // Not `InvestigationSortField` members: a field of another sort's catalogue, the
    // right member spelled as a response member, and a value of the WRONG enum.
    for (attribute, case) in [
        ("createdAt", "another operation's sort attribute"),
        ("startTime", "the summary member rather than the enum member"),
        ("ASC", "an OrderBy member in the attribute slot"),
    ] {
        let (status, refused) = list(
            &router,
            &detector_id,
            json!({"sortCriteria": {"attributeName": attribute}}),
        )
        .await;
        assert_eq!(
            status, 400,
            "sort attribute {case}: expected 400, got {status} {refused:?}"
        );
    }

    // `NextToken` matches the modelled pattern but is not a token this service issued:
    // it is not an offset at all, or it points past the end — which the service never
    // hands out, because the last page carries no token.
    for (token, case) in [
        ("Zm9vYmFy", "a token the service never issued"),
        ("999", "an offset past the end of the list"),
        ("-", "a value that is no offset"),
    ] {
        let (status, refused) = list(&router, &detector_id, json!({"nextToken": token})).await;
        assert_eq!(
            status, 400,
            "nextToken {case}: expected 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidInputException",
            "nextToken {case}: got {refused:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: the Regions the operation is served through.
// ---------------------------------------------------------------------------

/// Regions GuardDuty itself serves that are NOT in the Investigation catalogue,
/// spanning four geographies so the rows are not one endpoint.
const REGIONS_WITHOUT_INVESTIGATIONS: [&str; 4] =
    ["us-west-1", "eu-south-1", "ap-southeast-2", "sa-east-1"];

#[tokio::test]
async fn the_list_is_served_exactly_through_the_documented_regions() {
    for region in INVESTIGATION_REGIONS {
        let router = new_router();
        let (status, created) =
            call_in(&router, region, "POST", "/detector", Some(json!({"enable": true}))).await;
        assert_eq!(
            status, 200,
            "create-detector in {region}: got {status} {created:?}"
        );
        let detector_id = created["detectorId"].as_str().unwrap_or_default().to_string();

        let (status, body) = call_in(
            &router,
            region,
            "POST",
            &format!("/detector/{detector_id}/investigation"),
            Some(json!({"triggerPrompt": "investigate in a published Region"})),
        )
        .await;
        assert_eq!(
            status, 202,
            "create-investigation in {region}: expected 202, got {status} {body:?}"
        );
        let id = body["investigationId"].as_str().unwrap_or_default().to_string();

        let (status, answered) = call_in(
            &router,
            region,
            "POST",
            &format!("/detector/{detector_id}/investigation/list"),
            Some(json!({})),
        )
        .await;
        assert_eq!(
            status, 200,
            "list-investigations in {region}: expected 200, got {status} {answered:?}"
        );
        assert_eq!(
            ids(answered["investigations"].as_array().unwrap_or(&Vec::new())),
            vec![id.clone()],
            "list-investigations in {region}: expected the investigation it was initiated with, got {answered:?}"
        );
    }

    for region in REGIONS_WITHOUT_INVESTIGATIONS {
        let router = new_router();
        let (status, created) =
            call_in(&router, region, "POST", "/detector", Some(json!({"enable": true}))).await;
        assert_eq!(
            status, 200,
            "create-detector in {region}: got {status} {created:?}"
        );
        let detector_id = created["detectorId"].as_str().unwrap_or_default().to_string();

        let (status, answered) = call_in(
            &router,
            region,
            "POST",
            &format!("/detector/{detector_id}/investigation/list"),
            Some(json!({})),
        )
        .await;
        assert_eq!(
            status, 400,
            "list-investigations in {region}: expected 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "InvalidInputException",
            "list-investigations in {region}: got {answered:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 6: listing is a report.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_never_moves_an_investigation_early_and_agrees_with_a_read() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let id = initiated(&router, &detector_id, "summarize suspicious IAM activity").await;

    // The transient status is observable: the span is never zero, so a list straight
    // after the create still reports RUNNING — and repeating the list does not move it.
    for attempt in 1..=3 {
        let summaries = listed(&router, &detector_id, json!({})).await;
        assert_eq!(
            summaries.len(),
            1,
            "attempt {attempt}: expected one summary, got {summaries:?}"
        );
        assert_eq!(
            summaries[0]["status"],
            json!(RUNNING),
            "attempt {attempt}: a list must not settle an investigation early, got {summaries:?}"
        );
        assert!(
            summaries[0].get("endTime").is_none(),
            "attempt {attempt}: a running investigation has no endTime, got {summaries:?}"
        );
    }

    // A read taken after those lists sees the same transient status: a list changed
    // nothing a later caller sees.
    assert_eq!(
        read(&router, &detector_id, &id).await["status"],
        json!(RUNNING),
        "a read after the lists must still report the transient status"
    );

    tokio::time::sleep(past_the_transition()).await;

    // Once the clock owes it, the list reports the terminal status — and so does the
    // read, with the same instants: one record, settled once.
    let settled = listed(&router, &detector_id, json!({})).await;
    assert_eq!(
        settled[0]["status"],
        json!(FAILED),
        "after the transition: expected the terminal status, got {settled:?}"
    );
    let got = read(&router, &detector_id, &id).await;
    for member in ["status", "startTime", "endTime"] {
        assert_eq!(
            settled[0][member], got[member],
            "after the transition: the list and the read must agree on {member}, got {settled:?} vs {got:?}"
        );
    }

    // And it never moves again: the terminal status is stable across later lists.
    tokio::time::sleep(past_the_transition()).await;
    let again = listed(&router, &detector_id, json!({})).await;
    assert_eq!(
        again, settled,
        "a terminal investigation must never move again, got {again:?} after {settled:?}"
    );
}
