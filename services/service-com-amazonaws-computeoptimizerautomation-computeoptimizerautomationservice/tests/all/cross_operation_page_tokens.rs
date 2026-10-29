// SPDX-License-Identifier: CC0-1.0

//! A page token belongs to ONE walk: the OPERATION that issued it as well as the
//! query it was taken with.
//!
//! Every paginated operation of this service
//! (`smithy_query(query="digest")` marks nine of them `@paginated`) mints an
//! opaque `nextToken` that carries a fingerprint of the walk it resumes, and
//! accepts a token only when that fingerprint is the one it would itself have
//! minted. The operation is part of that fingerprint, so a token one listing
//! issued is refused by every other listing of the service even when the two
//! page over the same query members — `ListAutomationEvents` and
//! `ListAutomationEventSummaries` take the same filters and window over the same
//! events, and `ListRecommendedActions` and `ListRecommendedActionSummaries` the
//! same filter list, so without the issuing operation in the fingerprint one
//! would resume the other's walk and answer a page of a different result set.
//!
//! The rule is asserted twice over. First end to end: a token each listing
//! really minted is presented to every paginated operation of the service, where
//! only the one that issued it resumes. Then on the fingerprint itself, over a
//! table of listings and queries: a cursor carrying the digest of the query
//! members ALONE — the fingerprint a listing that bound no operation identity
//! would have issued — is refused, while the same cursor with the issuing
//! operation bound in is served.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::{
    AutomationEventFilter, Filter, RecommendedActionFilter,
};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::{
    self, PageCursor,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::automation_rule_lifecycle::{call, call_ok, is_next_token, member};
use crate::page_token_refusals::{assert_token_refused, paginated_operations};
use crate::shared_page_walk::emulator_with_a_fixture;

/// A `RecommendedActionType` the fixture's inventory holds
/// (`smithy_query(query="shape",shape="RecommendedActionType")`), used as an
/// event-type filter value so a filtered query is a row of the table below.
const SNAPSHOT_AND_DELETE: &str = "SnapshotAndDeleteUnattachedEbsVolume";

/// The other `RecommendedActionType` the model declares
/// (`smithy_query(query="shape",shape="RecommendedActionType")`).
const UPGRADE: &str = "UpgradeEbsVolumeType";

/// The page size that makes a listing with anything to report mint a token.
const SMALLEST_PAGE: i32 = 1;

/// The requirement a listing reports for a token it did not issue for this call,
/// taken from the one table of this service's paginated operations.
fn requirement_for(event_id: &str, operation: &str) -> &'static str {
    paginated_operations(event_id)
        .into_iter()
        .find(|(name, _, _)| *name == operation)
        .map(|(_, _, requirement)| requirement)
        .unwrap_or_else(|| panic!("{operation:?} must be a paginated operation of this service"))
}

/// The `nextToken` a listing minted, asserting it carries one at all and that it
/// has the modelled form.
fn minted_token(body: &Value, case: &str) -> String {
    let token = member(body, "nextToken")
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the fixture must leave this listing more than one page, so its first page carries a token, got {body}")
        })
        .to_string();
    assert!(
        is_next_token(&token),
        "case {case:?}: a minted token must match ^[A-Za-z0-9+/=]+$, got {token:?}"
    );
    token
}

/// One row of the fingerprint table: a listing, a query it serves, and the two
/// fingerprints a cursor can carry for that query — the digest of the query
/// members alone, and the digest that also names the issuing operation.
struct Row {
    case: &'static str,
    operation: &'static str,
    request: Value,
    query_only: String,
    operation_bound: String,
}

impl Row {
    /// A row for `operation` over `request`, whose query members digest as
    /// `query` — the value that operation's handler pages under.
    fn new<T: std::fmt::Debug + ?Sized>(
        case: &'static str,
        operation: &'static str,
        request: Value,
        query: &T,
    ) -> Self {
        Self {
            case,
            operation,
            request,
            query_only: state::fingerprint(&query),
            operation_bound: state::page_fingerprint(operation, query),
        }
    }
}

/// A cursor resuming after the first item under `fingerprint`, as the opaque
/// token a client presents.
fn token_for(fingerprint: &str) -> String {
    state::encode_page_token(&PageCursor {
        offset: 1,
        filter_fingerprint: fingerprint.to_string(),
    })
}

/// The query members each listing of the table pages under, spanning the shapes
/// of query this service's listings have: none at all, a filter list (absent and
/// present), a filter list with a time window, and the id of the single resource
/// whose children are paged.
fn fingerprint_rows(event_id: &str) -> Vec<Row> {
    let rule_filters = vec![Filter {
        name: "Status".to_string(),
        values: vec!["Active".to_string()],
    }];
    let event_filters = vec![AutomationEventFilter {
        name: "EventType".to_string(),
        values: vec![SNAPSHOT_AND_DELETE.to_string()],
    }];
    // The window bounds of an unbounded event query, as the handlers digest
    // them: `crate::state::instant_key` of each bound the caller sent.
    let no_window = (None::<(i64, u32)>, None::<(i64, u32)>);
    vec![
        Row::new(
            "rules-unfiltered",
            "ListAutomationRules",
            json!({}),
            &Vec::<Filter>::new(),
        ),
        Row::new(
            "rules-under-a-filter",
            "ListAutomationRules",
            json!({"filters": [{"name": "Status", "values": ["Active"]}]}),
            &rule_filters,
        ),
        Row::new(
            "events-unfiltered",
            "ListAutomationEvents",
            json!({}),
            &(&Vec::<AutomationEventFilter>::new(), no_window.0, no_window.1),
        ),
        Row::new(
            "events-under-a-filter",
            "ListAutomationEvents",
            json!({"filters": [{"name": "EventType", "values": [SNAPSHOT_AND_DELETE]}]}),
            &(&event_filters, no_window.0, no_window.1),
        ),
        Row::new(
            "event-summaries-unfiltered",
            "ListAutomationEventSummaries",
            json!({}),
            &(&Vec::<AutomationEventFilter>::new(), no_window.0, no_window.1),
        ),
        Row::new(
            "steps-of-one-event",
            "ListAutomationEventSteps",
            json!({"eventId": event_id}),
            &event_id.to_string(),
        ),
        Row::new(
            "recommended-actions-unfiltered",
            "ListRecommendedActions",
            json!({}),
            &Vec::<RecommendedActionFilter>::new(),
        ),
        Row::new(
            "recommended-action-summaries-unfiltered",
            "ListRecommendedActionSummaries",
            json!({}),
            &Vec::<RecommendedActionFilter>::new(),
        ),
    ]
}

/// The request a listing is asked to mint a token with: the one every other
/// case of this suite presents to it, narrowed to the smallest page.
///
/// `ListAutomationRulePreviewSummaries` pages the GROUPS of a preview, one per
/// `recommendedActionType`, so a selection naming a single type is one group and
/// one page: it is minted over both modelled types, so its walk spans two pages.
fn minting_request(operation: &str, table_request: &Value) -> Value {
    let mut request = table_request.clone();
    if operation == "ListAutomationRulePreviewSummaries" {
        request["recommendedActionTypes"] = json!([UPGRADE, SNAPSHOT_AND_DELETE]);
    }
    request["maxResults"] = json!(SMALLEST_PAGE);
    request
}

/// Present `token` to `operation` over `request` and answer the status and body,
/// so a caller can assert either that the walk resumed or that it was refused.
async fn present(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
    token: &str,
) -> (u16, Value) {
    let mut resumed = request.clone();
    resumed["nextToken"] = json!(token);
    call(router, operation, &resumed).await
}

/// A token a listing really minted resumes that listing and is refused by every
/// other paginated operation of this service, each with its own requirement.
#[tokio::test]
async fn a_minted_page_token_resumes_only_the_listing_that_issued_it() {
    let (router, event_id) = emulator_with_a_fixture().await;

    for (issuer, issuing_request, _) in paginated_operations(&event_id) {
        let minting = minting_request(issuer, &issuing_request);
        let first_page = call_ok(&router, issuer, &minting, issuer).await;
        let token = minted_token(&first_page, issuer);

        for (presenter, presenting_request, requirement) in paginated_operations(&event_id) {
            let case = format!("{issuer}-token-presented-to-{presenter}");
            if presenter == issuer {
                // The query that issued the token, which is the ONE query this
                // token may resume.
                let (status, parsed) = present(&router, presenter, &minting, &token).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the issuing listing must resume its own token, got {status} \
                     and {parsed}"
                );
                continue;
            }
            let message = assert_token_refused(
                &router,
                presenter,
                &presenting_request,
                &token,
                requirement,
                &case,
            )
            .await;
            assert!(
                !message.contains(&token),
                "case {case:?}: the refusal must not echo the rejected token, got {message:?}"
            );
        }
    }
}

/// The fingerprint a token carries names the issuing operation as well as the
/// query: a cursor carrying only the query's digest is refused, and the same
/// cursor with the operation bound in resumes the walk.
#[tokio::test]
async fn a_page_token_is_accepted_only_with_the_issuing_operation_in_its_fingerprint() {
    let (router, event_id) = emulator_with_a_fixture().await;
    let rows = fingerprint_rows(&event_id);

    for row in &rows {
        let unbound = token_for(&row.query_only);
        let message = assert_token_refused(
            &router,
            row.operation,
            &row.request,
            &unbound,
            requirement_for(&event_id, row.operation),
            row.case,
        )
        .await;
        assert!(
            !message.contains(&unbound),
            "case {:?}: the refusal must not echo the rejected token, got {message:?}",
            row.case
        );

        let bound = token_for(&row.operation_bound);
        let (status, parsed) = present(&router, row.operation, &row.request, &bound).await;
        assert_eq!(
            status, 200,
            "case {:?}: a cursor fingerprinted with the issuing operation and this query must \
             resume the walk, got {status} and {parsed}",
            row.case
        );
    }
}

/// Two listings that page the same query members digest those members
/// identically, so the issuing operation is the only thing that separates their
/// tokens: every row of the table has its own fingerprint, and the rows that
/// share a query shape share the query-only digest.
#[test]
fn listings_over_one_query_are_separated_by_the_issuing_operation() {
    // An event id of the modelled `EventId` form (`^[0-9A-Za-z]{16}$`), which
    // only the step listing's row digests.
    let rows = fingerprint_rows("Ev0000000000000a");

    for (index, left) in rows.iter().enumerate() {
        for right in &rows[index + 1..] {
            assert_ne!(
                left.operation_bound, right.operation_bound,
                "cases {:?} and {:?}: a distinct listing or query must have a distinct page-token \
                 fingerprint, got {:?} for both",
                left.case, right.case, left.operation_bound
            );
        }
    }

    // The pairs of listings that report over the very same query: a token of one
    // is distinguishable from a token of the other by nothing but the operation
    // the fingerprint names.
    for (left, right) in [
        ("events-unfiltered", "event-summaries-unfiltered"),
        (
            "recommended-actions-unfiltered",
            "recommended-action-summaries-unfiltered",
        ),
    ] {
        let find = |case: &str| {
            rows.iter()
                .find(|row| row.case == case)
                .unwrap_or_else(|| panic!("case {case:?} must be a row of the table"))
        };
        let (left, right) = (find(left), find(right));
        assert_eq!(
            left.query_only, right.query_only,
            "cases {:?} and {:?}: these listings page the same query members, so the query alone \
             must digest the same; got {:?} and {:?}",
            left.case, right.case, left.query_only, right.query_only
        );
        assert_ne!(
            left.operation_bound, right.operation_bound,
            "cases {:?} and {:?}: the issuing operation must separate their tokens, got {:?} for \
             both",
            left.case, right.case, left.operation_bound
        );
    }
}
