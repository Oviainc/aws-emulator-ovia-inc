// SPDX-License-Identifier: CC0-1.0

//! A `nextToken` is accepted only when it is one the listing could have issued
//! — in its FORM *and* in the offset it carries.
//!
//! `crate::state::cut_page` is the only issuer of a token and encodes the
//! position after a row it has just reported while a further selected row is
//! still unreported, so the offsets a listing can hand out are exactly
//! `1..stored_rows`. A token carrying any other offset — zero, or one at or
//! past the end of the stored collection — is as unissued as a token of the
//! other listing or of no listing at all, and is refused with the modelled
//! `ValidationException` (HTTP 400) in the one wording this service refuses
//! every unissued token in, rather than answered as "the listing is
//! exhausted".
//!
//! Both paginated operations decode through that one helper, so the table below
//! runs against both.

use serde_json::{json, Value};
use service_com_amazonaws_freetier_awsfreetierservice::ops::types::ACTIVITY_STATUS_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::free_tier::{call, error_code, harness, ok, refused, text, ACTIVITIES_PAGE_SIZE_CEILING};

/// The largest page `ListAccountActivities` serves
/// ([`ACTIVITIES_PAGE_SIZE_CEILING`]), which both paginated operations accept,
/// so one call reports the whole collection.
const MAX_RESULTS_CEILING: usize = ACTIVITIES_PAGE_SIZE_CEILING as usize;

/// This service's paginated operations: the operation, the output member
/// carrying its page, and the listing kind its tokens name.
const PAGINATED: [(&str, &str, &str); 2] = [
    ("ListAccountActivities", "activities", "activities"),
    ("GetFreeTierUsage", "freeTierUsages", "freeTierUsages"),
];

/// A token of `kind` carrying `offset`, in the encoding the service itself
/// issues (pinned against a real issued token by
/// [`the_probe_tokens_are_in_the_encoding_the_service_issues`]).
fn token(kind: &str, offset: &str) -> String {
    ::hex::encode(format!("ftv1:{kind}:{offset}"))
}

/// Every row one paginated operation reports when asked for the whole
/// collection at once.
async fn whole_collection(
    router: &MultiServiceRouter,
    operation: &str,
    items_member: &str,
) -> Vec<Value> {
    let payload = ok(router, operation, &json!({"maxResults": MAX_RESULTS_CEILING})).await;
    payload[items_member]
        .as_array()
        .unwrap_or_else(|| panic!("case {operation}: {items_member} must be a list, got {payload}"))
        .clone()
}

/// A filter each operation accepts and which selects a strict subset of the
/// stored rows, so the filtered view is shorter than the offsets probed under
/// it.
fn narrowing_filter(operation: &str) -> Value {
    match operation {
        // One modelled `ActivityStatus` out of the whole enum, so the view is
        // narrower than the catalog the offsets index into.
        "ListAccountActivities" => json!({"filterActivityStatuses": [ACTIVITY_STATUS_VALUES[0]]}),
        _ => json!({"filter": {"Dimensions": {
            "Key": "REGION",
            "Values": ["no-such-region-1"],
            "MatchOptions": ["EQUALS"],
        }}}),
    }
}

/// The probe tokens below are built by hand, so pin that the hand-built form is
/// the form the service issues: the token a one-row page of each listing hands
/// out is exactly the token for offset 1 of that listing.
#[tokio::test]
async fn the_probe_tokens_are_in_the_encoding_the_service_issues() {
    let router = harness().0;

    for (operation, _, kind) in PAGINATED {
        let page = ok(&router, operation, &json!({"maxResults": 1})).await;
        let issued = text(&page, "nextToken");
        assert_eq!(
            issued,
            token(kind, "1"),
            "case {operation}: the token a one-row page issues must be the probe encoding for \
             offset 1, got {issued}"
        );
    }
}

/// An offset no page of the listing could have issued is refused, in the same
/// wording every other unissued token is refused in — never served as an empty
/// page.
///
/// The table spans the offsets around the bound exhaustively (zero, the last
/// issuable offset, the first unissuable one, and past the end) plus offsets far
/// beyond any collection and one too large for an offset at all, against both
/// paginated operations.
#[tokio::test]
async fn an_offset_no_page_could_have_issued_is_refused() {
    let router = harness().0;

    for (operation, items_member, kind) in PAGINATED {
        let total = whole_collection(&router, operation, items_member)
            .await
            .len();
        assert!(
            total >= 2,
            "case {operation}: the account must hold at least two rows for an offset bound to be \
             observable, got {total}"
        );

        // The wording this service refuses an unissued token in, taken from a
        // token that is not even hex, so the rows below are asserted to refuse
        // in the SAME wording rather than against a sentence spelled here.
        let (status, payload) = call(&router, operation, &json!({"nextToken": "not-a-token"})).await;
        assert_eq!(
            status, 400,
            "case {operation}: a token that is not even hex must be refused, got {status} with \
             {payload}"
        );
        let wording = text(&payload, "message");

        let mut unissued: Vec<String> = vec![
            "0".to_string(),
            total.to_string(),
            (total + 1).to_string(),
            (total + 2).to_string(),
            (10 * total).to_string(),
            "999".to_string(),
            // One past the largest offset an offset can even be, so the tail
            // parses as a number nowhere.
            format!("{}0", usize::MAX),
        ];
        unissued.dedup();

        for offset in unissued {
            let probe = token(kind, &offset);
            let body = json!({"nextToken": probe});
            refused(&router, operation, &body, 400, "ValidationException").await;

            let (_, payload) = call(&router, operation, &body).await;
            assert_eq!(
                text(&payload, "message"),
                wording,
                "case {operation} offset {offset}: an unissued token must be refused in the one \
                 wording, got {payload}"
            );
        }
    }
}

/// Every offset the issuer CAN hand out is still served, and resumes the
/// listing exactly there — the bound refuses only what no page could have
/// issued.
#[tokio::test]
async fn every_issuable_offset_still_resumes_the_listing() {
    let router = harness().0;

    for (operation, items_member, kind) in PAGINATED {
        let rows = whole_collection(&router, operation, items_member).await;

        for offset in 1..rows.len() {
            let payload = ok(
                &router,
                operation,
                &json!({"maxResults": MAX_RESULTS_CEILING, "nextToken": token(kind, &offset.to_string())}),
            )
            .await;
            let reported = payload[items_member]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {operation} offset {offset}: {items_member} must be a list, got {payload}")
                })
                .clone();
            assert_eq!(
                reported,
                rows[offset..].to_vec(),
                "case {operation} offset {offset}: the page must resume at the offset the token \
                 names, got {reported:?}"
            );
        }
    }
}

/// The bound is the STORED collection's, not the filtered view's: a token
/// naming a stored row is served even when the request's filter selects fewer
/// rows than the offset, so a caller may change its filter between pages.
#[tokio::test]
async fn the_bound_is_the_stored_collection_not_the_filtered_view() {
    let router = harness().0;

    for (operation, items_member, kind) in PAGINATED {
        let total = whole_collection(&router, operation, items_member).await.len();
        let filter = narrowing_filter(operation);

        for offset in 1..total {
            let mut body = filter.clone();
            body["maxResults"] = json!(MAX_RESULTS_CEILING);
            body["nextToken"] = json!(token(kind, &offset.to_string()));

            let (status, payload) = call(&router, operation, &body).await;
            assert_eq!(
                status, 200,
                "case {operation} offset {offset} under a filter: a token naming a stored row \
                 must be served, got {status} with {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "",
                "case {operation} offset {offset} under a filter: expected no error, got {payload}"
            );
        }

        // The offset past the end stays refused whatever the filter selects.
        let mut body = filter;
        body["maxResults"] = json!(MAX_RESULTS_CEILING);
        body["nextToken"] = json!(token(kind, &total.to_string()));
        refused(&router, operation, &body, 400, "ValidationException").await;
    }
}
