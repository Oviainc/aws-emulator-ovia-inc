// SPDX-License-Identifier: CC0-1.0

//! What a listing of this service serves a caller that names NO `MaxResults`.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, over ONE shared engine, so what is exercised is the wire
//! behaviour.
//!
//! The RULE, asserted over a TABLE spanning the listings this service answers WHOLE and
//! listing lengths from empty to past the largest page a caller may ask for:
//! `MaxResults` bounds what a CALLER may ask for and NOT what such a listing serves when
//! the caller names none, so an unasked listing carries its WHOLE set in one body with
//! no `NextToken`, and a `NextToken` follows only a page a caller cut short with
//! `MaxResults` — which still resumes exactly where it stopped.
//!
//! The APPLIED-quota listing is not among them: `ListServiceQuotas` has an unasked page
//! of its own (`crate::state::APPLIED_QUOTA_PAGE_SIZE`), and `applied_quota_listing_pages.rs`
//! pins that half of the contract.
//!
//! The rows differ along what the rule generalises over: the operation (the service
//! inventory, the default quota listing, both request histories), the length of the set
//! (none, one, a handful, dozens, hundreds) and the narrowing the request asks for. The
//! harness (`Client`, `walk_pages`) is the one `quota_workflow.rs` owns.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_servicequotas_servicequotasv20190624::state::MAX_PAGE_SIZE;

use crate::quota_workflow::{client, emulator, walk_pages, Client};

/// Comfortably past BOTH steps the clock owes a quota increase request
/// (`crate::state::SUPPORT_CASE_OPEN_DELAY`, then
/// `crate::state::REQUEST_DECISION_DELAY` once its support case is open), so a request
/// made before this wait has settled afterwards and two reads of the history are
/// comparable record for record.
const PAST_DECISION: Duration = Duration::from_millis(1_800);

/// The account-wide request history, whose name is too long to repeat in every row.
const HISTORY: &str = "ListRequestedServiceQuotaChangeHistory";

/// The per-quota request history.
const HISTORY_BY_QUOTA: &str = "ListRequestedServiceQuotaChangeHistoryByQuota";

/// Service codes whose default-quota listings span the lengths this rule generalises
/// over: `iam` publishes a handful of quotas, `acm` and `lambda` dozens, `ec2` hundreds —
/// more than the largest page a caller may ask for. None of them is the code the
/// reproduction used.
const QUOTA_SERVICES: &[&str] = &["iam", "acm", "lambda", "ec2"];

/// How many increase requests the history rows are seeded with: comfortably more than
/// a handful, so a history row is longer than any small page size.
const SEEDED_REQUESTS: usize = 12;

/// One row of the table: a listing, the request that asks for it, and the output
/// member carrying the items.
struct Row {
    /// What names the case in every assertion.
    case: String,
    /// The operation to call.
    operation: &'static str,
    /// The request body, carrying no `MaxResults` and no `NextToken`.
    body: Value,
    /// The output member the items arrive in.
    items: &'static str,
}

/// The items a response carries under `member`.
fn items(case: &str, response: &Value, member: &str) -> Vec<Value> {
    response[member]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a {member} list, got {response}"))
        .clone()
}

/// The adjustable quotas of `service_code`, as `(QuotaCode, value in force)`, in
/// listing order — the quotas an increase may be requested on.
async fn adjustable(client: &Client, service_code: &str) -> Vec<(String, f64)> {
    walk_pages(
        client,
        "ListServiceQuotas",
        &json!({"ServiceCode": service_code}),
        "Quotas",
        Some(MAX_PAGE_SIZE as i64),
    )
    .await
    .into_iter()
    .filter(|quota| quota["Adjustable"].as_bool().unwrap_or(false))
    .filter_map(|quota| {
        Some((
            quota["QuotaCode"].as_str()?.to_string(),
            quota["Value"].as_f64()?,
        ))
    })
    .collect()
}

/// Request one increase above the value in force and return the minted `Id`.
async fn raise(client: &Client, service_code: &str, quota_code: &str, desired: f64) -> String {
    let response = client
        .ok(
            "RequestServiceQuotaIncrease",
            json!({
                "ServiceCode": service_code,
                "QuotaCode": quota_code,
                "DesiredValue": desired,
            }),
        )
        .await;
    response["RequestedQuota"]["Id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {service_code}/{quota_code}: expected a minted Id, got {response}")
        })
        .to_string()
}

/// Seed the request store with increase requests on distinct `ec2` quotas, and return
/// the quota the per-quota history rows are asked for, plus one quota that was never
/// requested on.
async fn seed_requests(client: &Client) -> (String, String) {
    let quotas = adjustable(client, "ec2").await;
    assert!(
        quotas.len() > SEEDED_REQUESTS,
        "case seeding: ec2 must publish more than {SEEDED_REQUESTS} adjustable quotas to seed \
         distinct requests, got {}",
        quotas.len()
    );

    for (quota_code, in_force) in quotas.iter().take(SEEDED_REQUESTS) {
        raise(client, "ec2", quota_code, in_force + 1.0).await;
    }
    // Every decision the clock owes comes due, so the records are terminal.
    tokio::time::sleep(PAST_DECISION).await;

    (quotas[0].0.clone(), quotas[SEEDED_REQUESTS].0.clone())
}

/// The whole table: every listing this service paginates, over the set lengths the rule
/// generalises over.
async fn rows(client: &Client) -> Vec<Row> {
    let (requested_quota, untouched_quota) = seed_requests(client).await;

    let mut rows = vec![Row {
        case: "ListServices, the whole inventory".to_string(),
        operation: "ListServices",
        body: json!({}),
        items: "Services",
    }];

    for service_code in QUOTA_SERVICES {
        rows.push(Row {
            case: format!("ListAWSDefaultServiceQuotas {service_code}"),
            operation: "ListAWSDefaultServiceQuotas",
            body: json!({"ServiceCode": service_code}),
            items: "Quotas",
        });
    }

    // A service the catalog lists but publishes no quotas for: the empty set is a
    // whole set too, and it is the last page.
    rows.push(Row {
        case: "ListAWSDefaultServiceQuotas kinesisanalytics, a service with no seeded quotas"
            .to_string(),
        operation: "ListAWSDefaultServiceQuotas",
        body: json!({"ServiceCode": "kinesisanalytics"}),
        items: "Quotas",
    });

    // The request histories: the account-wide listing, the same listing narrowed to the
    // service the requests were made against, one narrowed to a service with none, and
    // the per-quota listing for a quota that was requested on and for one that was not.
    rows.push(Row {
        case: format!("{HISTORY}, every stored request"),
        operation: HISTORY,
        body: json!({}),
        items: "RequestedQuotas",
    });
    rows.push(Row {
        case: format!("{HISTORY} ec2"),
        operation: HISTORY,
        body: json!({"ServiceCode": "ec2", "Status": "APPROVED"}),
        items: "RequestedQuotas",
    });
    rows.push(Row {
        case: format!("{HISTORY} lambda, a service with no requests"),
        operation: HISTORY,
        body: json!({"ServiceCode": "lambda"}),
        items: "RequestedQuotas",
    });
    rows.push(Row {
        case: format!("{HISTORY_BY_QUOTA} ec2/{requested_quota}"),
        operation: HISTORY_BY_QUOTA,
        body: json!({"ServiceCode": "ec2", "QuotaCode": requested_quota}),
        items: "RequestedQuotas",
    });
    rows.push(Row {
        case: format!("{HISTORY_BY_QUOTA} ec2/{untouched_quota}, a quota with no requests"),
        operation: HISTORY_BY_QUOTA,
        body: json!({"ServiceCode": "ec2", "QuotaCode": untouched_quota}),
        items: "RequestedQuotas",
    });

    rows
}

/// RULE: a listing called with no `MaxResults` carries its WHOLE set in one body, with
/// no `NextToken` after it — whatever the operation and however long the set is.
#[tokio::test]
async fn an_unasked_listing_carries_its_whole_set_in_one_page() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");
    let rows = rows(&client).await;

    let mut past_a_handful = 0_usize;
    let mut past_the_largest_asked_page = 0_usize;
    let mut empty = 0_usize;

    for row in &rows {
        let Row {
            case,
            operation,
            body,
            items: member,
        } = row;

        // The set this listing publishes, as the token chain yields it at the largest
        // page size a caller may ask for: the reference every assertion is against.
        let whole = walk_pages(
            &client,
            operation,
            body,
            member,
            Some(MAX_PAGE_SIZE as i64),
        )
        .await;

        let unasked = client.ok(operation, body.clone()).await;
        let page = items(case, &unasked, member);
        assert_eq!(
            page.len(),
            whole.len(),
            "case {case}: an unasked listing carries the whole set of {} items, got {}",
            whole.len(),
            page.len()
        );
        assert_eq!(
            page, whole,
            "case {case}: the unasked page must be the published listing itself"
        );
        assert!(
            unasked["NextToken"].is_null(),
            "case {case}: the whole set is the last page, so no NextToken follows it, got {}",
            unasked["NextToken"]
        );

        // Reading it again serves the same body: a listing reports, it does not consume.
        let again = client.ok(operation, body.clone()).await;
        assert_eq!(
            items(case, &again, member),
            page,
            "case {case}: a second unasked read must serve the same set"
        );

        if whole.is_empty() {
            empty += 1;
        }
        if whole.len() > 10 {
            past_a_handful += 1;
        }
        if whole.len() > MAX_PAGE_SIZE {
            past_the_largest_asked_page += 1;
        }
    }

    // The table must actually span the lengths the rule generalises over: an empty
    // set, sets past a handful of items, and a set past the largest page a caller may
    // ask for (which no caller-nameable `MaxResults` could have served).
    assert!(
        empty >= 1 && past_a_handful >= 3 && past_the_largest_asked_page >= 1,
        "case table: expected rows spanning empty, past-a-handful and past-{MAX_PAGE_SIZE} \
         listings, got {empty}, {past_a_handful} and {past_the_largest_asked_page}"
    );
}

/// RULE, the other half: a caller that DOES name `MaxResults` gets exactly that page
/// size, with a `NextToken` while items remain that resumes where the page stopped, and
/// a continuation naming no page size serves the whole remainder in one page.
#[tokio::test]
async fn an_asked_page_size_still_bounds_every_listing() {
    let engine = emulator();
    let client = client(&engine, "us-west-2");
    let rows = rows(&client).await;

    for row in &rows {
        let Row {
            case,
            operation,
            body,
            items: member,
        } = row;

        // The reference set, walked at the largest page size a caller may ask for, so
        // this suite does not lean on the unasked page it is the other half of.
        let whole = walk_pages(
            &client,
            operation,
            body,
            member,
            Some(MAX_PAGE_SIZE as i64),
        )
        .await;
        if whole.is_empty() {
            // An empty set has no page to cut short; its last-page shape is asserted
            // by the unasked suite.
            continue;
        }

        // Rows: both published bounds of the modelled `MaxResults` @range 1..100 and
        // two sizes between them that divide no listing evenly.
        for size in [1_usize, 7, 63, MAX_PAGE_SIZE] {
            let case = format!("{case} MaxResults={size}");
            let asked = client
                .ok(operation, {
                    let mut asked = body.clone();
                    asked["MaxResults"] = json!(size);
                    asked
                })
                .await;
            let page = items(&case, &asked, member);
            let taken = size.min(whole.len());
            assert_eq!(
                page.len(),
                taken,
                "case {case}: an asked page holds the asked size, expected {taken}, got {}",
                page.len()
            );
            assert_eq!(
                page,
                whole[..taken],
                "case {case}: the first page must be the first items of the published order"
            );

            let token = asked["NextToken"].as_str().unwrap_or_default().to_string();
            assert_eq!(
                !token.is_empty(),
                taken < whole.len(),
                "case {case}: a NextToken follows this page exactly while items remain ({} of \
                 {} served), got {asked}",
                taken,
                whole.len()
            );
            if token.is_empty() {
                continue;
            }

            // The continuation names no page size, so it serves the whole remainder in
            // one page: the unasked page size governs a continuation the same way.
            let resumed = client
                .ok(operation, {
                    let mut resume = body.clone();
                    resume["NextToken"] = json!(token);
                    resume
                })
                .await;
            let rest = items(&format!("{case} resumed"), &resumed, member);
            assert_eq!(
                rest,
                whole[taken..],
                "case {case} resumed: an unasked continuation carries the remaining {} items, \
                 got {}",
                whole.len() - taken,
                rest.len()
            );
            assert!(
                resumed["NextToken"].is_null(),
                "case {case} resumed: the remainder is the last page, got {}",
                resumed["NextToken"]
            );
        }
    }
}
