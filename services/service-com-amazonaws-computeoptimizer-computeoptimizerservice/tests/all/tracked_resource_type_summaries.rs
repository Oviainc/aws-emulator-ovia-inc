// SPDX-License-Identifier: CC0-1.0

//! The ORDER `GetRecommendationSummaries` rolls an account's findings up in, and the
//! vocabulary that order walks.
//!
//! The rule under test: the groups a summary page reports are the whole
//! `RecommendationSourceType` vocabulary `RecommendationSummary$recommendationResourceType`
//! names, in the order the model declares it, and paging walks that one order — so a client
//! that indexes the response, or derives the tracked resource-type set from it, reads the
//! same groups in the same places however it pages.
//!
//! That the entries exist at all for an account whose analysis has produced nothing, and what
//! each of them counts, is asserted in `tests/all/summary_resource_type_surface.rs` and
//! `tests/all/recommendation_summaries.rs`; this suite pins the LIST the handler walks
//! against the model's own enum, so a value the model adds or renames cannot be missed, and
//! pins the reported order against it.
//!
//! Every call goes through the shared wire harness (`tests/all/harness.rs`), i.e. the same
//! `MultiServiceRouter::dispatch` the emulator's front door uses.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::RECOMMENDATION_SOURCE_TYPE_VALUES;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    ANALYSIS_DELAY, TRACKED_SOURCE_TYPES,
};

use crate::harness::{call, emulator, opt_in};

/// Read one page of summaries, asserting the call succeeded.
async fn summaries(
    router: &smithy_server_runtime::MultiServiceRouter,
    request: Value,
) -> Vec<Value> {
    let (status, body) = call(router, "GetRecommendationSummaries", request.clone()).await;
    assert_eq!(
        status, 200,
        "summaries for {request}: expected 200, got {status}: {body}"
    );
    body["recommendationSummaries"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("summaries for {request}: recommendationSummaries must be a list, got {body}")
        })
        .clone()
}

/// The resource types one response reports, in the order it reports them.
fn resource_types_of(reported: &[Value]) -> Vec<&str> {
    reported
        .iter()
        .map(|summary| {
            summary["recommendationResourceType"]
                .as_str()
                .unwrap_or_default()
        })
        .collect()
}

/// The resource types the service rolls findings up for are EXACTLY the model's
/// `RecommendationSourceType` vocabulary — no value missing and none invented, each once — so
/// the list the handler walks cannot drift from the model's enum as that enum grows.
#[test]
fn the_tracked_resource_types_are_the_modelled_vocabulary() {
    let tracked: BTreeSet<&str> = TRACKED_SOURCE_TYPES.iter().copied().collect();
    let modelled: BTreeSet<&str> = RECOMMENDATION_SOURCE_TYPE_VALUES.iter().copied().collect();
    assert_eq!(
        tracked, modelled,
        "the tracked resource types must be the modelled RecommendationSourceType values"
    );
    assert_eq!(
        tracked.len(),
        TRACKED_SOURCE_TYPES.len(),
        "no resource type may be tracked twice, got {TRACKED_SOURCE_TYPES:?}"
    );
}

/// One page reports the tracked vocabulary in the order the model declares it, and paging
/// walks that same order whatever page size is asked for — including a size that divides the
/// vocabulary unevenly, where a page boundary falls inside it.
#[tokio::test]
async fn the_summaries_are_reported_and_paged_in_the_tracked_order() {
    let router = emulator();
    opt_in(&router).await;
    // Read with the analysis the enrollment started due, so the counts differ between groups
    // and this order is not the order of an all-zero response alone.
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;

    let unpaged = summaries(&router, json!({})).await;
    assert_eq!(
        resource_types_of(&unpaged),
        TRACKED_SOURCE_TYPES.to_vec(),
        "expected one summary per tracked resource type, in the tracked order, got {unpaged:?}"
    );

    // Rows: page sizes that divide the vocabulary evenly and unevenly, one page per entry,
    // and one page holding the lot.
    for page_size in [1, 3, 5, TRACKED_SOURCE_TYPES.len()] {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<Value> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let (status, body) = call(&router, "GetRecommendationSummaries", request.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from {request}, got {status}: {body}"
            );
            let page = body["recommendationSummaries"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} summaries, got {}",
                page.len()
            );
            walked.extend(page);
            assert!(
                walked.len() <= TRACKED_SOURCE_TYPES.len(),
                "case {case:?}: paging must terminate within the vocabulary, got {} entries",
                walked.len()
            );
            next_token = body["nextToken"].as_str().map(ToString::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            resource_types_of(&walked),
            TRACKED_SOURCE_TYPES.to_vec(),
            "case {case:?}: paging must walk exactly the tracked vocabulary, in the tracked \
             order"
        );
        assert_eq!(
            walked, unpaged,
            "case {case:?}: paging must not change what each summary says"
        );
    }
}
