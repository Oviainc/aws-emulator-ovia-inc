// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource`' continuation marker, end to end over the wire.
//!
//! `ListTagsForResourceResponse$NextMarker` is returned "if the number of objects
//! that are still available for retrieval exceeds the limit" — so the RULE asserted
//! here is: the tagging listing hands back a position to resume from only while tags
//! remain beyond the page it just served, and a listing that handed back the whole
//! collection hands back none. That is what makes a paginator terminate: a client
//! loops while the marker is there, so a marker naming a key INSIDE the page it came
//! with is a loop that either repeats a page forever or never ends.
//!
//! Each suite asserts that over a TABLE spanning the class: a resource with one tag,
//! with a few, and with as many as one resource may hold, crossed with every way a
//! call can ask for a page — no `Limit` at all, a `Limit` smaller than the
//! collection, exactly its size, larger than it, and each published bound — and over
//! two of the resource kinds WAF tags, in both scopes.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    MAX_TAGS_PER_RESOURCE, SCOPE_CLOUDFRONT, SCOPE_REGIONAL,
};

use crate::web_acl_lifecycle::{create_ip_set, create_web_acl, Waf};

/// The bounds `PaginationLimit` allows
/// (`smithy_query(query="shape",shape="PaginationLimit")` → `range 1..100`).
const MIN_PAGE_LIMIT: i64 = 1;
/// See [`MIN_PAGE_LIMIT`].
const MAX_PAGE_LIMIT: i64 = 100;

/// One `ListTagsForResource` call, as a 200.
async fn list(waf: &Waf, arn: &str, limit: Option<i64>, marker: Option<&str>) -> Value {
    let mut request = json!({ "ResourceARN": arn });
    if let Some(limit) = limit {
        request["Limit"] = json!(limit);
    }
    if let Some(marker) = marker {
        request["NextMarker"] = json!(marker);
    }
    waf.ok("ListTagsForResource", request).await
}

/// The keys a page lists, in the order it listed them.
fn keys_on(page: &Value, case: &str) -> Vec<String> {
    page["TagInfoForResource"]["TagList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: a page must carry a TagList, got {page}"))
        .iter()
        .map(|tag| tag["Key"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The marker a page handed back, as a client reads it: `None` when there is no
/// position to resume from — no member, or an empty one.
fn marker_on(page: &Value) -> Option<String> {
    match page["NextMarker"].as_str() {
        Some(marker) if !marker.is_empty() => Some(marker.to_string()),
        _ => None,
    }
}

/// Tag `arn` with `count` keys, and answer them in the order a listing reports them
/// (a resource's tags are reported in key order).
async fn tag_keys(waf: &Waf, arn: &str, count: usize) -> Vec<String> {
    let mut keys: Vec<String> = (0..count).map(|index| format!("key-{index:02}")).collect();
    for key in &keys {
        waf.ok(
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": [{ "Key": key, "Value": "v" }] }),
        )
        .await;
    }
    keys.sort();
    keys
}

/// A listing that handed back the whole collection hands back no position to resume
/// from — whatever `Limit` it was asked with, and whatever the resource holds.
#[tokio::test]
async fn a_complete_listing_hands_back_no_marker() {
    for scope in [SCOPE_REGIONAL, SCOPE_CLOUDFRONT] {
        let waf = Waf::new();
        for (index, count) in [1_usize, 3, MAX_TAGS_PER_RESOURCE].into_iter().enumerate() {
            let name = format!("marker-ips-{index}");
            let (_id, arn, _lock) = create_ip_set(&waf, scope, &name).await;
            let keys = tag_keys(&waf, &arn, count).await;

            // Every Limit that holds the whole collection, and the call that names
            // none at all.
            let limits: Vec<Option<i64>> = vec![
                None,
                Some(i64::try_from(count).unwrap_or(MAX_PAGE_LIMIT)),
                Some(MAX_PAGE_LIMIT),
            ];
            for limit in limits {
                let case = format!("{scope}, {count} tag(s), Limit {limit:?}");
                let page = list(&waf, &arn, limit, None).await;
                assert_eq!(
                    keys_on(&page, &case),
                    keys,
                    "case {case:?}: the page must hold the whole collection"
                );
                assert_eq!(
                    marker_on(&page),
                    None,
                    "case {case:?}: a complete listing must hand back no position to \
                     resume from, got {page}"
                );
            }
        }
    }
}

/// While tags remain, the marker is the key of the page's LAST tag — the position
/// the next call resumes after — and the walk terminates having seen every tag
/// exactly once.
#[tokio::test]
async fn a_marker_is_handed_back_only_while_tags_remain() {
    let waf = Waf::new();
    for (index, count) in [1_usize, 2, 5].into_iter().enumerate() {
        let name = format!("paged-ips-{index}");
        let (_id, arn, _lock) = create_ip_set(&waf, SCOPE_REGIONAL, &name).await;
        let keys = tag_keys(&waf, &arn, count).await;

        for limit in [MIN_PAGE_LIMIT, 2, 3] {
            let case = format!("{count} tag(s), Limit {limit}");
            let mut walked: Vec<String> = Vec::new();
            let mut marker: Option<String> = None;
            let mut pages = 0_usize;
            loop {
                let page = list(&waf, &arn, Some(limit), marker.as_deref()).await;
                let listed = keys_on(&page, &case);
                assert!(
                    listed.len() <= usize::try_from(limit).unwrap_or(usize::MAX),
                    "case {case:?}: a page must hold at most the limit, got {page}"
                );
                walked.extend(listed.iter().cloned());
                pages += 1;
                assert!(
                    pages <= count + 1,
                    "case {case:?}: the walk must terminate within one page per tag, \
                     got {page}"
                );
                match marker_on(&page) {
                    Some(next) => {
                        assert_eq!(
                            Some(next.as_str()),
                            listed.last().map(String::as_str),
                            "case {case:?}: a marker must name the page's last key, \
                             got {page}"
                        );
                        assert!(
                            walked.len() < keys.len(),
                            "case {case:?}: a marker must only be handed back while \
                             tags remain, got {page}"
                        );
                        marker = Some(next);
                    }
                    None => break,
                }
            }
            assert_eq!(
                walked, keys,
                "case {case:?}: the pages together must be the whole collection, \
                 exactly once each"
            );
        }
    }
}

/// Resuming after the LAST key of the collection is a complete listing of what is
/// left: an empty page with no position to resume from. Asserted on a web ACL too,
/// because the marker is the tagging listing's and not one resource kind's.
#[tokio::test]
async fn resuming_past_the_last_tag_ends_the_walk() {
    let waf = Waf::new();
    let (_id, ip_set_arn, _lock) = create_ip_set(&waf, SCOPE_REGIONAL, "resumed-ips").await;
    let (_id, web_acl_arn, _lock) =
        create_web_acl(&waf, SCOPE_REGIONAL, "resumed-acl", json!([])).await;

    for (kind, arn) in [("IP set", ip_set_arn), ("web ACL", web_acl_arn)] {
        let keys = tag_keys(&waf, &arn, 4).await;
        let last = keys.last().cloned().unwrap_or_default();
        for marker in [last.as_str(), "zzz-past-every-key"] {
            let case = format!("{kind} resumed after {marker:?}");
            let page = list(&waf, &arn, None, Some(marker)).await;
            assert_eq!(
                keys_on(&page, &case),
                Vec::<String>::new(),
                "case {case:?}: nothing follows the last key, got {page}"
            );
            assert_eq!(
                marker_on(&page),
                None,
                "case {case:?}: an exhausted listing must hand back no position to \
                 resume from, got {page}"
            );
        }
    }
}
