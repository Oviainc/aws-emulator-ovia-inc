// SPDX-License-Identifier: CC0-1.0

//! The ONE status/level filter-and-view rule BOTH quota increase request history
//! listings apply, driven through each of them over the wire.
//!
//! The rule: of the requests a history listing selects — every request of the account
//! (`ListRequestedServiceQuotaChangeHistory`) or one quota's
//! (`ListRequestedServiceQuotaChangeHistoryByQuota`) — it reports the ones whose stored
//! status equals the `Status` asked for (every status when none is asked for) and which
//! were made at a level `QuotaRequestedAtLevel` keeps (`ALL` keeps every level,
//! `ACCOUNT` is the documented default), each rendered as the same
//! `RequestedServiceQuotaChange` record. The two listings therefore differ ONLY in
//! which requests they select, never in how they filter or render them.
//!
//! The table below is the cross product of every published `RequestStatus` (plus the
//! absent case) with every published `AppliedLevelEnum` member (plus the absent case),
//! walked twice: while the requests are all still open, and once the clock has decided
//! them, so the status column moves under the same rows. Both listings are driven on
//! each row, over a set of requests spanning two levels, two decided statuses and two
//! services, and each is compared against the expectation computed here from what was
//! minted — not against the other listing alone.
//!
//! The harness (`Client`, `walk_pages`) is the one `quota_workflow.rs` owns, and the
//! quota-code lookup the one `resource_quota_context.rs` owns.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_servicequotas_servicequotasv20190624::ops::types::{
    APPLIED_LEVEL_ENUM_VALUES, REQUEST_STATUS_VALUES,
};
use service_com_amazonaws_servicequotas_servicequotasv20190624::state::{
    LEVEL_ACCOUNT, LEVEL_ALL, LEVEL_RESOURCE,
};

use crate::quota_workflow::{client, emulator, walk_pages, Client, ACCOUNT};
use crate::resource_quota_context::quota_code_for;

/// Comfortably past BOTH steps the clock owes a quota increase request
/// (`crate::state::SUPPORT_CASE_OPEN_DELAY`, then
/// `crate::state::REQUEST_DECISION_DELAY` once its support case is open), so a request
/// made before this wait has settled afterwards.
const PAST_DECISION: Duration = Duration::from_millis(1_800);

/// The account-wide history listing — the first of the two listings the rule covers.
const HISTORY: &str = "ListRequestedServiceQuotaChangeHistory";

/// The per-quota history listing — the second one.
const HISTORY_BY_QUOTA: &str = "ListRequestedServiceQuotaChangeHistoryByQuota";

/// The quota the catalog publishes as adjustable per resource AND account-wide, so one
/// quota can carry requests made at both levels (`src/state.rs`, the `es` rows).
const RESOURCE_QUOTA_NAME: &str = "Instances per domain";

/// A quota of ANOTHER service, so the account-wide listing selects requests the
/// per-quota listing never sees.
const OTHER_QUOTA_NAME: &str = "VPCs per Region";

/// One minted request, with the level it was made at and the status the clock settles
/// it to.
struct Minted {
    id: String,
    level: &'static str,
    settled: &'static str,
}

/// Ask for an increase and return its minted `Id`.
async fn raise(
    caller: &Client,
    service_code: &str,
    quota_code: &str,
    desired: f64,
    context: Option<&str>,
    support_case_allowed: bool,
) -> String {
    let mut body = json!({
        "ServiceCode": service_code,
        "QuotaCode": quota_code,
        "DesiredValue": desired,
        "SupportCaseAllowed": support_case_allowed,
    });
    if let Some(context) = context {
        body["ContextId"] = json!(context);
    }
    let response = caller.ok("RequestServiceQuotaIncrease", body).await;
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

/// The rule itself, over what was minted: the ids a listing selecting `selected` must
/// report for `status` / `level`, NEWEST FIRST.
///
/// `status` of `None` keeps every status; `level` of `None` is the documented default
/// `ACCOUNT`, and `ALL` keeps every level.
fn expected_ids(
    selected: &[&Minted],
    status: Option<&str>,
    level: Option<&str>,
    decided: bool,
) -> Vec<String> {
    let wanted_level = level.unwrap_or(LEVEL_ACCOUNT);
    selected
        .iter()
        .rev()
        .filter(|minted| {
            let carried = if decided { minted.settled } else { "PENDING" };
            status.is_none_or(|status| status == carried)
        })
        .filter(|minted| wanted_level == LEVEL_ALL || minted.level == wanted_level)
        .map(|minted| minted.id.clone())
        .collect()
}

/// The filter body for one row of the table.
fn filters(status: Option<&str>, level: Option<&str>) -> Value {
    let mut body = json!({});
    if let Some(status) = status {
        body["Status"] = json!(status);
    }
    if let Some(level) = level {
        body["QuotaRequestedAtLevel"] = json!(level);
    }
    body
}

/// Merge two JSON objects, so a row's filters compose with a listing's own selection.
fn with(base: &Value, extra: &Value) -> Value {
    let mut body = base.clone();
    if let (Some(body), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            body.insert(key.clone(), value.clone());
        }
    }
    body
}

/// Both history listings apply the one status/level filter-and-view rule, over the
/// whole cross product of the published filter values, before and after the clock has
/// decided the requests.
#[tokio::test]
async fn both_history_listings_apply_the_one_status_and_level_rule() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    let quota_code = quota_code_for(&caller, "es", RESOURCE_QUOTA_NAME).await;
    let other_code = quota_code_for(&caller, "vpc", OTHER_QUOTA_NAME).await;
    let alpha = format!("arn:aws:es:us-east-1:{ACCOUNT}:domain/alpha");
    let beta = format!("arn:aws:es:us-east-1:{ACCOUNT}:domain/beta");

    // Four requests spanning both levels, both decided statuses and two services, each
    // on a plane of its own so none blocks another. Mint order is listing order
    // reversed: the history reports the newest first.
    let minted = [
        Minted {
            id: raise(&caller, "es", &quota_code, 400.0, None, true).await,
            level: LEVEL_ACCOUNT,
            settled: "APPROVED",
        },
        Minted {
            id: raise(&caller, "es", &quota_code, 120.0, Some(&alpha), true).await,
            level: LEVEL_RESOURCE,
            settled: "APPROVED",
        },
        Minted {
            id: raise(&caller, "es", &quota_code, 130.0, Some(&beta), false).await,
            level: LEVEL_RESOURCE,
            settled: "NOT_APPROVED",
        },
        Minted {
            id: raise(&caller, "vpc", &other_code, 25.0, None, false).await,
            level: LEVEL_ACCOUNT,
            settled: "NOT_APPROVED",
        },
    ];

    // What each listing SELECTS, before the shared rule runs: the account's whole
    // history, the one narrowed to a service, and the one quota's own.
    let selections: Vec<(&str, Value, Vec<&Minted>)> = vec![
        (HISTORY, json!({}), minted.iter().collect()),
        (
            HISTORY,
            json!({"ServiceCode": "es"}),
            minted.iter().take(3).collect(),
        ),
        (
            HISTORY_BY_QUOTA,
            json!({"ServiceCode": "es", "QuotaCode": quota_code}),
            minted.iter().take(3).collect(),
        ),
        (
            HISTORY_BY_QUOTA,
            json!({"ServiceCode": "vpc", "QuotaCode": other_code}),
            minted.iter().skip(3).collect(),
        ),
    ];

    // The status column of every row moves when the clock decides the requests, so the
    // table is walked once while they are open and once afterwards.
    for decided in [false, true] {
        if decided {
            tokio::time::sleep(PAST_DECISION).await;
        }

        // One row per (Status, QuotaRequestedAtLevel), each member of the published
        // value sets plus the absent case. The value sets come from the model
        // (`smithy_query(query="shape", shape="RequestStatus")` and
        // `shape="AppliedLevelEnum"`), reached through the generated catalogs so the
        // table cannot drift from it.
        let statuses: Vec<Option<&str>> = std::iter::once(None)
            .chain(REQUEST_STATUS_VALUES.iter().copied().map(Some))
            .collect();
        let levels: Vec<Option<&str>> = std::iter::once(None)
            .chain(APPLIED_LEVEL_ENUM_VALUES.iter().copied().map(Some))
            .collect();

        for status in &statuses {
            for level in &levels {
                let row = filters(*status, *level);
                for (operation, selection, selected) in &selections {
                    let body = with(selection, &row);
                    let listed =
                        walk_pages(&caller, operation, &body, "RequestedQuotas", None).await;
                    let got = ids(&listed);
                    let expected = expected_ids(selected, *status, *level, decided);
                    assert_eq!(
                        got, expected,
                        "case {operation} {body} decided={decided}: expected {expected:?}, got \
                         {got:?}"
                    );
                    // Every record reported must itself carry the status and the level
                    // it was kept for: the filter and the rendered record agree.
                    for record in &listed {
                        if let Some(status) = status {
                            assert_eq!(
                                record["Status"],
                                json!(status),
                                "case {operation} {body} decided={decided}: a record of another \
                                 status was reported, got {record}"
                            );
                        }
                        let kept = level.unwrap_or(LEVEL_ACCOUNT);
                        if kept != LEVEL_ALL {
                            assert_eq!(
                                record["QuotaRequestedAtLevel"],
                                json!(kept),
                                "case {operation} {body} decided={decided}: a record of another \
                                 level was reported, got {record}"
                            );
                        }
                    }
                }

                // The two listings RENDER the records they share identically: the
                // service-narrowed account-wide listing and the quota's own history
                // select the same requests here, so their pages must be equal record
                // for record.
                let account_wide = walk_pages(
                    &caller,
                    HISTORY,
                    &with(&json!({"ServiceCode": "es"}), &row),
                    "RequestedQuotas",
                    None,
                )
                .await;
                let per_quota = walk_pages(
                    &caller,
                    HISTORY_BY_QUOTA,
                    &with(
                        &json!({"ServiceCode": "es", "QuotaCode": quota_code}),
                        &row,
                    ),
                    "RequestedQuotas",
                    None,
                )
                .await;
                assert_eq!(
                    account_wide, per_quota,
                    "case {row} decided={decided}: both listings must report the shared records \
                     identically, got {account_wide:?} and {per_quota:?}"
                );
            }
        }
    }
}
