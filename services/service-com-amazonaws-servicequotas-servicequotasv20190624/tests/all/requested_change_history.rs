// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT-WIDE quota increase request history
//! (`ListRequestedServiceQuotaChangeHistory`), end to end over the wire.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, over ONE shared engine, so what is exercised is the HTTP
//! behaviour and the persisted state behind it. The harness (`Client`, `walk_pages`)
//! is the one `quota_workflow.rs` owns.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one case:
//!
//! * the listing reports the requests this account has STORED, newest first, across
//!   every service when the caller names no `ServiceCode` and narrowed to one service
//!   when it does — so the service-wide listing is the union of its per-service
//!   narrowings, and each record is the one `GetRequestedServiceQuotaChange` answers
//!   with for the same `Id`;
//! * `Status` keeps the requests that carry that `RequestStatus` at the instant of the
//!   call, so the statuses partition the listing, and `QuotaRequestedAtLevel` keeps the
//!   requests made at that level, defaulting to `ACCOUNT` and keeping every level under
//!   `ALL`;
//! * the listing is per region, and it refuses a `ServiceCode` the catalog does not
//!   publish and a `NextToken` it did not issue;
//! * paging yields the one ordered listing whatever page size it is walked with.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};

use crate::error_envelope::error_of;
use crate::quota_workflow::{base64_of, client, emulator, walk_pages, Client};

/// Comfortably past `crate::state::REQUEST_DECISION_DELAY` (500 ms), so a request made
/// before this wait is due for its decision afterwards.
const PAST_DECISION: Duration = Duration::from_millis(1_200);

/// The account-wide history listing, whose name is too long to repeat in every row.
const HISTORY: &str = "ListRequestedServiceQuotaChangeHistory";

/// The per-quota history listing — the same records, narrowed to one quota. Used here
/// as ANOTHER listing, to show a token is only good for the listing that issued it.
const HISTORY_BY_QUOTA: &str = "ListRequestedServiceQuotaChangeHistoryByQuota";

/// Adjustable quotas to raise, as `(ServiceCode, QuotaCode, DesiredValue)`.
///
/// Source: the seeded catalog in `src/state.rs` (the rows AWS publishes as
/// `Adjustable`), with a desired value above each quota's published default. The rows
/// span four services and several quota shapes, and Service Quotas allows one OPEN
/// request per quota, so each row is a distinct quota.
const RAISEABLE: &[(&str, &str, f64)] = &[
    ("vpc", "L-F678F1CE", 10.0),
    ("vpc", "L-A4707A72", 25.0),
    ("lambda", "L-B99A9384", 2_000.0),
    ("lambda", "L-2ACBD22F", 150.0),
    ("dynamodb", "L-F98FE922", 5_000.0),
    ("s3", "L-DC2B2D3D", 20_000.0),
];

/// Every `RequestStatus` the model declares
/// (`smithy_query(query="shape", shape="RequestStatus")`), so a status table covers the
/// published value set rather than the two statuses this emulator mints.
const EVERY_STATUS: &[&str] = &[
    "APPROVED",
    "CASE_CLOSED",
    "CASE_OPENED",
    "DENIED",
    "INVALID_REQUEST",
    "NOT_APPROVED",
    "PENDING",
];

/// Raise `desired` on one quota and return the minted `Id`.
async fn raise(client: &Client, service_code: &str, quota_code: &str, desired: f64) -> String {
    let body = json!({
        "ServiceCode": service_code,
        "QuotaCode": quota_code,
        "DesiredValue": desired,
    });
    let response = client.ok("RequestServiceQuotaIncrease", body).await;
    response["RequestedQuota"]["Id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {service_code}/{quota_code}: expected a minted Id, got {response}")
        })
        .to_string()
}

/// The `Id`s a history listing reports, in listing order.
fn ids(requests: &[Value]) -> Vec<String> {
    requests
        .iter()
        .map(|request| {
            request["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case a history record: expected an Id, got {request}"))
                .to_string()
        })
        .collect()
}

/// The account-wide listing carries every stored request, newest first, is the union of
/// its per-service narrowings, pages to one ordered listing, and agrees field for field
/// with the single read of each record.
#[tokio::test]
async fn the_account_wide_history_lists_every_stored_request_newest_first() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    // Nothing stored yet: the listing is empty and final, not a refusal.
    let empty = client.ok(HISTORY, json!({})).await;
    assert_eq!(
        empty["RequestedQuotas"],
        json!([]),
        "case an account with no requests: expected an empty listing, got {empty}"
    );
    assert_eq!(
        empty["NextToken"],
        Value::Null,
        "case an account with no requests: expected no NextToken, got {empty}"
    );

    let mut minted: Vec<(String, &str)> = Vec::new();
    for (service_code, quota_code, desired) in RAISEABLE {
        minted.push((
            raise(&client, service_code, quota_code, *desired).await,
            service_code,
        ));
    }

    // Let every decision the clock owes come due, so the records are terminal and two
    // walks of the listing are comparable field for field.
    tokio::time::sleep(PAST_DECISION).await;

    let whole = walk_pages(&client, HISTORY, &json!({}), "RequestedQuotas", None).await;
    let newest_first: Vec<String> = minted
        .iter()
        .rev()
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        ids(&whole),
        newest_first,
        "case the whole history: expected the newest request first, got {:?}",
        ids(&whole)
    );

    // Paging yields the ONE ordered listing, at every page size, including the bounds
    // of the model's `MaxResults` @range 1..100.
    for size in [1_i64, 2, 5, 100] {
        let paged = walk_pages(&client, HISTORY, &json!({}), "RequestedQuotas", Some(size)).await;
        assert_eq!(
            paged, whole,
            "case the whole history MaxResults={size}: paging must yield the one listing, got \
             {paged:?}"
        );
    }

    // Reading it again changes nothing: a listing is a report.
    let again = walk_pages(&client, HISTORY, &json!({}), "RequestedQuotas", None).await;
    assert_eq!(
        again, whole,
        "case the whole history, read twice: expected the same listing, got {again:?}"
    );

    // Narrowed to one service: exactly that service's requests, in the same order, and
    // the narrowings partition the account-wide listing.
    let mut union: Vec<Value> = Vec::new();
    let services: BTreeSet<&str> = minted.iter().map(|(_, service)| *service).collect();
    for service_code in &services {
        let narrowed = walk_pages(
            &client,
            HISTORY,
            &json!({"ServiceCode": service_code}),
            "RequestedQuotas",
            None,
        )
        .await;
        let expected: Vec<String> = minted
            .iter()
            .rev()
            .filter(|(_, service)| service == service_code)
            .map(|(id, _)| id.clone())
            .collect();
        assert_eq!(
            ids(&narrowed),
            expected,
            "case ServiceCode={service_code}: expected only that service's requests, newest \
             first, got {:?}",
            ids(&narrowed)
        );
        for request in &narrowed {
            assert_eq!(
                request["ServiceCode"],
                json!(service_code),
                "case ServiceCode={service_code}: a record of another service was listed, got \
                 {request}"
            );
        }
        union.extend(narrowed);
    }
    let listed: BTreeSet<String> = ids(&whole).into_iter().collect();
    let narrowed_ids: BTreeSet<String> = ids(&union).into_iter().collect();
    assert_eq!(
        narrowed_ids, listed,
        "case the per-service narrowings: expected them to cover the account-wide listing, got \
         {narrowed_ids:?}"
    );

    // A quota's own history is the account-wide listing filtered to that quota, and
    // each record is the one the single read answers with.
    for (service_code, quota_code, _) in RAISEABLE {
        let per_quota = walk_pages(
            &client,
            HISTORY_BY_QUOTA,
            &json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
            "RequestedQuotas",
            None,
        )
        .await;
        let from_account: Vec<Value> = whole
            .iter()
            .filter(|request| {
                request["ServiceCode"] == json!(service_code)
                    && request["QuotaCode"] == json!(quota_code)
            })
            .cloned()
            .collect();
        assert_eq!(
            per_quota, from_account,
            "case {service_code}/{quota_code}: the account-wide listing must carry the same \
             records as the quota's own history, got {per_quota:?}"
        );

        for record in &from_account {
            let id = record["Id"].as_str().unwrap_or_default().to_string();
            let fetched = client
                .ok("GetRequestedServiceQuotaChange", json!({"RequestId": id}))
                .await;
            assert_eq!(
                fetched["RequestedQuota"], *record,
                "case {service_code}/{quota_code}: the listed record must equal the single read, \
                 got {fetched}"
            );
        }
    }
}

/// `Status` keeps the requests carrying that status NOW, so the published statuses
/// partition the listing — before the decisions are due and after.
#[tokio::test]
async fn the_history_filters_on_the_status_a_request_carries_now() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    // Two requests on two quotas of two services, differing in the one input that
    // decides their outcome: the one that opted out of its support case is NOT_APPROVED
    // when its decision comes due, the other APPROVED.
    let approved_id = raise(&client, "vpc", "L-F678F1CE", 10.0).await;
    let not_approved_id = client
        .ok(
            "RequestServiceQuotaIncrease",
            json!({
                "ServiceCode": "lambda",
                "QuotaCode": "L-B99A9384",
                "DesiredValue": 2_000.0,
                "SupportCaseAllowed": false,
            }),
        )
        .await["RequestedQuota"]["Id"]
        .as_str()
        .expect("the second request reports an Id")
        .to_string();

    // While both are still open, PENDING carries both and every other published status
    // carries none.
    let open: BTreeSet<String> = [approved_id.clone(), not_approved_id.clone()]
        .into_iter()
        .collect();
    for status in EVERY_STATUS {
        let listed = walk_pages(
            &client,
            HISTORY,
            &json!({"Status": status}),
            "RequestedQuotas",
            None,
        )
        .await;
        let got: BTreeSet<String> = ids(&listed).into_iter().collect();
        let expected = if *status == "PENDING" {
            open.clone()
        } else {
            BTreeSet::new()
        };
        assert_eq!(
            got, expected,
            "case Status={status}, both requests open: expected {expected:?}, got {got:?}"
        );
    }

    // Once the clock has made both decisions due, each request is filtered on the
    // status it NOW carries.
    tokio::time::sleep(PAST_DECISION).await;

    let decided: Vec<(&str, BTreeSet<String>)> = EVERY_STATUS
        .iter()
        .map(|status| {
            let expected: BTreeSet<String> = match *status {
                "APPROVED" => [approved_id.clone()].into_iter().collect(),
                "NOT_APPROVED" => [not_approved_id.clone()].into_iter().collect(),
                _ => BTreeSet::new(),
            };
            (*status, expected)
        })
        .collect();

    let mut covered: BTreeSet<String> = BTreeSet::new();
    for (status, expected) in &decided {
        let listed = walk_pages(
            &client,
            HISTORY,
            &json!({"Status": status}),
            "RequestedQuotas",
            None,
        )
        .await;
        let got: BTreeSet<String> = ids(&listed).into_iter().collect();
        assert_eq!(
            got, *expected,
            "case Status={status}, both decided: expected {expected:?}, got {got:?}"
        );
        for record in &listed {
            assert_eq!(
                record["Status"],
                json!(status),
                "case Status={status}: a record of another status was listed, got {record}"
            );
        }
        covered.extend(got);
    }

    // The statuses partition the listing: filtering on none reports exactly the union.
    let unfiltered = walk_pages(&client, HISTORY, &json!({}), "RequestedQuotas", None).await;
    let whole: BTreeSet<String> = ids(&unfiltered).into_iter().collect();
    assert_eq!(
        covered, whole,
        "case the status filters: expected them to partition the listing {whole:?}, got {covered:?}"
    );

    // A status narrowing composes with a service narrowing rather than replacing it.
    let narrowed = walk_pages(
        &client,
        HISTORY,
        &json!({"ServiceCode": "lambda", "Status": "NOT_APPROVED"}),
        "RequestedQuotas",
        None,
    )
    .await;
    assert_eq!(
        ids(&narrowed),
        vec![not_approved_id.clone()],
        "case ServiceCode=lambda Status=NOT_APPROVED: expected the one request, got {:?}",
        ids(&narrowed)
    );
    let crossed = walk_pages(
        &client,
        HISTORY,
        &json!({"ServiceCode": "vpc", "Status": "NOT_APPROVED"}),
        "RequestedQuotas",
        None,
    )
    .await;
    assert!(
        crossed.is_empty(),
        "case ServiceCode=vpc Status=NOT_APPROVED: expected no requests, got {crossed:?}"
    );
}

/// `QuotaRequestedAtLevel` keeps the requests made at that level, defaults to `ACCOUNT`
/// and keeps every level under `ALL`.
#[tokio::test]
async fn the_history_filters_on_the_level_a_request_was_made_at() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (service_code, quota_code, desired) in RAISEABLE.iter().take(3) {
        minted.insert(raise(&client, service_code, quota_code, *desired).await);
    }

    // One row per published `AppliedLevelEnum` member (ACCOUNT | ALL | RESOURCE,
    // `smithy_query(query="shape", shape="AppliedLevelEnum")`) plus the absent case,
    // whose documented default is ACCOUNT. Every request above was made at the ACCOUNT
    // level — the level the quotas raised are adjustable at — so ACCOUNT, ALL and the
    // default keep all of them and RESOURCE keeps none.
    let rows: Vec<(Value, bool)> = vec![
        (json!({}), true),
        (json!({"QuotaRequestedAtLevel": "ACCOUNT"}), true),
        (json!({"QuotaRequestedAtLevel": "ALL"}), true),
        (json!({"QuotaRequestedAtLevel": "RESOURCE"}), false),
    ];

    for (body, keeps) in rows {
        let listed = walk_pages(&client, HISTORY, &body, "RequestedQuotas", None).await;
        let got: BTreeSet<String> = ids(&listed).into_iter().collect();
        let expected = if keeps { minted.clone() } else { BTreeSet::new() };
        assert_eq!(
            got, expected,
            "case {body}: expected {expected:?}, got {got:?}"
        );
        for record in &listed {
            assert_eq!(
                record["QuotaRequestedAtLevel"],
                json!("ACCOUNT"),
                "case {body}: an account-level request must report its level, got {record}"
            );
        }
    }
}

/// The listing is per region, and it refuses what it cannot serve: a `ServiceCode` the
/// catalog does not publish, and a `NextToken` it did not issue.
#[tokio::test]
async fn the_history_is_per_region_and_refuses_what_it_cannot_serve() {
    let engine = emulator();
    let home = client(&engine, "us-east-1");
    let elsewhere = client(&engine, "eu-west-1");

    let here = raise(&home, "vpc", "L-F678F1CE", 10.0).await;
    let there = raise(&elsewhere, "vpc", "L-F678F1CE", 20.0).await;

    // Service Quotas is regional: each endpoint reports its OWN region's requests.
    for (caller, own, other) in [(&home, &here, &there), (&elsewhere, &there, &here)] {
        let listed = walk_pages(caller, HISTORY, &json!({}), "RequestedQuotas", None).await;
        let got = ids(&listed);
        assert!(
            got.contains(own) && !got.contains(other),
            "case a regional history: expected {own} and not {other}, got {got:?}"
        );
    }

    // A `ServiceCode` the catalog does not publish names no service, so the listing is
    // a lookup failure — on the client-fault status this awsJson front end answers a
    // modelled client error on — rather than an empty page. The rows differ in the form
    // of the unknown code and in which other filters accompany it.
    let unknown: Vec<Value> = vec![
        json!({"ServiceCode": "not-a-real-service-xyz"}),
        json!({"ServiceCode": "Vpc"}),
        json!({"ServiceCode": "not-a-real-service-xyz", "Status": "PENDING"}),
        json!({"ServiceCode": "not-a-real-service-xyz", "MaxResults": 1}),
    ];
    for body in unknown {
        let (status, parsed) = home.call(HISTORY, &body).await;
        let (code, message) = error_of(&parsed);
        assert_eq!(
            status, 400,
            "case {body}: expected HTTP 400, got {status} with {parsed}"
        );
        assert!(
            code.contains("NoSuchResourceException"),
            "case {body}: expected NoSuchResourceException, got {code:?} in {parsed}"
        );
        assert!(
            message.as_deref().is_some_and(|text| !text.is_empty()),
            "case {body}: expected a non-empty Message member, got {parsed}"
        );
    }

    // A token this listing did not issue resumes nothing. The rows span: a token of
    // ANOTHER listing of the same records, an offset past the end of this one, and
    // opaque bytes that are not a token at all.
    let tokens = [
        base64_of(&format!("{HISTORY_BY_QUOTA}:0")),
        base64_of("ListServices:0"),
        base64_of(&format!("{HISTORY}:999")),
        base64_of("not-a-token"),
    ];
    for token in &tokens {
        let (status, parsed) = home.call(HISTORY, &json!({"NextToken": token})).await;
        let (code, _) = error_of(&parsed);
        assert_eq!(
            status, 400,
            "case NextToken={token}: expected HTTP 400, got {status} with {parsed}"
        );
        assert!(
            code.contains("InvalidPaginationTokenException"),
            "case NextToken={token}: expected InvalidPaginationTokenException, got {code:?} in \
             {parsed}"
        );
    }

    // And symmetrically: a token THIS listing issued is good here and nowhere else.
    for (service_code, quota_code, desired) in RAISEABLE.iter().skip(1) {
        raise(&home, service_code, quota_code, *desired).await;
    }
    let first = home.ok(HISTORY, json!({"MaxResults": 1})).await;
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case MaxResults=1: expected a NextToken, got {first}"))
        .to_string();
    let second = home
        .ok(HISTORY, json!({"MaxResults": 1, "NextToken": &token}))
        .await;
    assert_ne!(
        first["RequestedQuotas"], second["RequestedQuotas"],
        "case MaxResults=1: the second page repeated the first, got {second}"
    );
    let (status, parsed) = home
        .call(
            HISTORY_BY_QUOTA,
            &json!({
                "ServiceCode": "vpc",
                "QuotaCode": "L-F678F1CE",
                "NextToken": &token,
            }),
        )
        .await;
    let (code, _) = error_of(&parsed);
    assert_eq!(
        status, 400,
        "case this listing's token on the per-quota listing: expected HTTP 400, got {status} with \
         {parsed}"
    );
    assert!(
        code.contains("InvalidPaginationTokenException"),
        "case this listing's token on the per-quota listing: expected \
         InvalidPaginationTokenException, got {code:?} in {parsed}"
    );
}
