// SPDX-License-Identifier: CC0-1.0

//! The ONE `AppliedLevelEnum` level filter this crate applies, driven over the wire
//! through a listing of EACH kind that applies it.
//!
//! The rule: a listing filtered to a level reports the records that sit at exactly that
//! level, `ALL` reports the records of every level, and a caller that names no level
//! filters to `ACCOUNT` — the documented default of the member on both the
//! applied-quota listing (`smithy_query(query="doc",
//! shape="ListServiceQuotasRequest", member="QuotaAppliedAtLevel")`: "ACCOUNT is the
//! default") and the change-history listings (the identical sentence on
//! `QuotaRequestedAtLevel`). `ListServiceQuotas` asks it of the plane each applied
//! record sits on, the history listings of the level each request was made at; one
//! predicate answers for all of them.
//!
//! The table spans both listings, every published `AppliedLevelEnum` member plus the
//! absent case, and two page sizes (the whole listing in one page, and one record per
//! page, so the filter the pagination token's label carries is walked too). The
//! expectation for each row is computed from the `ALL` listing of that same row by the
//! rule itself, so it pins the rule rather than a record count: state is seeded with
//! records on BOTH planes first, which the row for `ALL` asserts.
//!
//! The harness (`Client`, `walk_pages`) is the one `quota_workflow.rs` owns, and the
//! quota-code lookup the one `resource_quota_context.rs` owns.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_servicequotas_servicequotasv20190624::ops::types::APPLIED_LEVEL_ENUM_VALUES;
use service_com_amazonaws_servicequotas_servicequotasv20190624::state::{
    CONTEXT_ID_EVERY_RESOURCE, LEVEL_ACCOUNT, LEVEL_ALL, LEVEL_RESOURCE,
};

use crate::quota_workflow::{client, emulator, walk_pages, Client, ACCOUNT};
use crate::resource_quota_context::quota_code_for;

/// Comfortably past BOTH steps the clock owes a quota increase request
/// (`crate::state::SUPPORT_CASE_OPEN_DELAY`, then
/// `crate::state::REQUEST_DECISION_DELAY` once its support case is open), so a request
/// made before this wait has settled afterwards and its value is in force.
const PAST_DECISION: Duration = Duration::from_millis(1_800);

/// The quota the catalog publishes as adjustable per resource AND account-wide, so one
/// quota carries records on both planes (`src/state.rs`, the `es` rows).
const RESOURCE_QUOTA_NAME: &str = "Instances per domain";

/// One listing that applies the level rule: how to call it, where its records are, the
/// request member that names the level filter, and the member each record reports its
/// own level at.
struct Listing {
    operation: &'static str,
    base: Value,
    items: &'static str,
    filter_member: &'static str,
    level_member: &'static str,
}

/// Walk one listing with the level filter of this row, to the end.
async fn listed(
    caller: &Client,
    listing: &Listing,
    level: Option<&str>,
    page_size: Option<i64>,
) -> Vec<Value> {
    let mut body = listing.base.clone();
    if let Some(level) = level {
        body[listing.filter_member] = json!(level);
    }
    walk_pages(caller, listing.operation, &body, listing.items, page_size).await
}

/// The rule itself: whether a record at `level` belongs in a listing filtered to
/// `requested`.
fn keeps(level: &str, requested: Option<&str>) -> bool {
    let filter = requested.unwrap_or(LEVEL_ACCOUNT);
    filter == LEVEL_ALL || level == filter
}

/// Both the applied-quota listing and the change-history listing report exactly the
/// records the one level rule keeps, for every published filter value and the absent
/// case.
#[tokio::test]
async fn every_listing_applies_the_one_applied_level_filter_rule() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    let quota_code = quota_code_for(&caller, "es", RESOURCE_QUOTA_NAME).await;
    let alpha = format!("arn:aws:es:us-east-1:{ACCOUNT}:domain/alpha");

    // Records on BOTH planes of one quota: an account-wide increase, one applied to a
    // single resource, and one applied to every resource with no value of its own.
    for (desired, context) in [
        (400.0, None),
        (120.0, Some(alpha.as_str())),
        (130.0, Some(CONTEXT_ID_EVERY_RESOURCE)),
    ] {
        let mut body = json!({
            "ServiceCode": "es",
            "QuotaCode": quota_code,
            "DesiredValue": desired,
        });
        if let Some(context) = context {
            body["ContextId"] = json!(context);
        }
        caller.ok("RequestServiceQuotaIncrease", body).await;
    }
    tokio::time::sleep(PAST_DECISION).await;

    let listings = [
        Listing {
            operation: "ListServiceQuotas",
            base: json!({"ServiceCode": "es"}),
            items: "Quotas",
            filter_member: "QuotaAppliedAtLevel",
            level_member: "QuotaAppliedAtLevel",
        },
        Listing {
            operation: "ListRequestedServiceQuotaChangeHistoryByQuota",
            base: json!({"ServiceCode": "es", "QuotaCode": quota_code}),
            items: "RequestedQuotas",
            filter_member: "QuotaRequestedAtLevel",
            level_member: "QuotaRequestedAtLevel",
        },
    ];

    // The filter values come from the generated catalog of the model's enum
    // (`smithy_query(query="shape", shape="AppliedLevelEnum")`), so the table cannot
    // drift from the model, plus the absent case the default covers.
    let levels: Vec<Option<&str>> = std::iter::once(None)
        .chain(APPLIED_LEVEL_ENUM_VALUES.iter().copied().map(Some))
        .collect();

    for listing in &listings {
        let operation = listing.operation;
        let every = listed(&caller, listing, Some(LEVEL_ALL), None).await;
        let level_of = |record: &Value| {
            record[listing.level_member]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {operation} ALL: every record must report its level, got {record}")
                })
                .to_string()
        };
        // The rows below only bite over state spanning both planes, so assert it does.
        for plane in [LEVEL_ACCOUNT, LEVEL_RESOURCE] {
            assert!(
                every.iter().any(|record| level_of(record) == plane),
                "case {operation} ALL: expected a record at the {plane} level, got {every:?}"
            );
        }

        for level in &levels {
            let expected: Vec<Value> = every
                .iter()
                .filter(|record| keeps(&level_of(record), *level))
                .cloned()
                .collect();
            // Walked whole, and one record per page: the filter also labels the
            // pagination token, so a token cut under the default must resume the
            // listing the default selects.
            for page_size in [None, Some(1)] {
                let got = listed(&caller, listing, *level, page_size).await;
                assert_eq!(
                    got, expected,
                    "case {operation} level={level:?} MaxResults={page_size:?}: expected \
                     {expected:?}, got {got:?}"
                );
            }
        }

        // The absent case and `ACCOUNT` name the same listing, and `ALL` is the union
        // of the two planes — the halves of the rule, stated once per listing.
        let defaulted = listed(&caller, listing, None, None).await;
        let account = listed(&caller, listing, Some(LEVEL_ACCOUNT), None).await;
        let resource = listed(&caller, listing, Some(LEVEL_RESOURCE), None).await;
        assert_eq!(
            defaulted, account,
            "case {operation} with no level named: expected the ACCOUNT listing {account:?}, got \
             {defaulted:?}"
        );
        assert_eq!(
            every.len(),
            account.len() + resource.len(),
            "case {operation} ALL: expected both planes ({} + {}), got {every:?}",
            account.len(),
            resource.len()
        );
    }
}
