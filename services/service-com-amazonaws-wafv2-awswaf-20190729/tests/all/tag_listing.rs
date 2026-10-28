// SPDX-License-Identifier: CC0-1.0

//! What `ListTagsForResource` reports, end to end over the wire.
//!
//! The rule these suites assert is: `ListTagsForResource` REPORTS the collection
//! held against the resource its `ResourceARN` names at the moment of the call —
//! exactly that resource's pairs and none of any other resource's, whatever kind,
//! scope or name the others carry — and reporting it changes nothing a later caller
//! sees: the collection, the pages a marker walks and the quota the resource has
//! left are what they were before the read, however many reads there were. An ARN
//! the service no longer holds names no resource at all, so a listing of it is the
//! nonexistent-item refusal rather than an empty collection.
//!
//! Each suite drives that over a TABLE spanning the class rather than one request:
//! the rows differ in the resource kind (each of the four WAF tags), in the scope
//! (both values `Scope` declares), in how many tags the resource holds (none, one,
//! several), in whether the read supplies a `Limit` and a `NextMarker` at all, and
//! in how many times the same read is repeated. The harness is the one in
//! [`crate::web_acl_lifecycle`]: every call goes through the router's front door, so
//! what these tests observe is what an SDK observes.

use std::collections::BTreeMap;

use serde_json::json;
use service_com_amazonaws_wafv2_awswaf_20190729::state::{SCOPE_CLOUDFRONT, SCOPE_REGIONAL};

use crate::permission_policy::error_message;
use crate::regex_pattern_set_lifecycle::create_regex_pattern_set;
use crate::resource_tagging::{listed_tags, pairs, tags, NONEXISTENT_ITEM};
use crate::rule_group_lifecycle::create_rule_group;
use crate::web_acl_lifecycle::{create_ip_set, create_web_acl, rule, sqli_statement, Waf};

/// The four resource kinds WAF tags — "web ACLs, rule groups, IP sets, and regex
/// pattern sets" (`ListTagsForResource`'s own doc) — as the name a case calls the
/// kind and the operation that deletes one of it.
const KINDS: &[(&str, &str)] = &[
    ("IP set", "DeleteIPSet"),
    ("web ACL", "DeleteWebACL"),
    ("rule group", "DeleteRuleGroup"),
    ("regex pattern set", "DeleteRegexPatternSet"),
];

/// Create one resource of `kind` under `name` in `scope`, and answer
/// `(id, arn, lock token)`.
///
/// The per-kind creates are the sibling suites' own helpers; this is only the one
/// place that picks between them by the kind a table row names.
async fn create_of_kind(
    waf: &Waf,
    kind: &str,
    scope: &str,
    name: &str,
) -> (String, String, String) {
    match kind {
        "IP set" => create_ip_set(waf, scope, name).await,
        "web ACL" => create_web_acl(waf, scope, name, json!([])).await,
        "rule group" => {
            create_rule_group(waf, scope, name, 50, json!([rule("inspect", sqli_statement())])).await
        }
        "regex pattern set" => create_regex_pattern_set(waf, scope, name, &["^/admin"], None).await,
        other => panic!("unknown resource kind {other:?}"),
    }
}

/// A listing answers the tags of the resource its ARN names and of no other: the
/// four kinds created under ONE name in both scopes are four times two distinct
/// resources, each carrying its own collection, and each listing is exactly the
/// collection of the resource it addressed.
#[tokio::test]
async fn a_listing_reports_only_the_tags_of_the_resource_its_arn_names() {
    /// The one name every resource below is created under: a kind and a scope are
    /// each a namespace, so the same name names a different resource in each.
    const SHARED_NAME: &str = "shared-name";
    /// The key every resource holds, under a value of its own — so a listing that
    /// reported another resource's collection would be caught on the VALUE and not
    /// merely on a missing key.
    const SHARED_KEY: &str = "Owner";

    let waf = Waf::new();
    let mut rows: Vec<(String, String, BTreeMap<String, String>)> = Vec::new();
    for scope in [SCOPE_REGIONAL, SCOPE_CLOUDFRONT] {
        for (kind, _delete) in KINDS {
            let case = format!("{scope} {kind}");
            let (_id, arn, _lock) = create_of_kind(&waf, kind, scope, SHARED_NAME).await;
            // A key of this resource's own beside the shared one, so the row's
            // collection differs from every other row's in both key and value.
            let unique_key = format!("probe:{scope}:{kind}");
            let mut expected: BTreeMap<String, String> = BTreeMap::new();
            expected.insert(SHARED_KEY.to_string(), case.clone());
            expected.insert(unique_key.clone(), "held".to_string());
            waf.ok(
                "TagResource",
                json!({
                    "ResourceARN": &arn,
                    "Tags": [
                        { "Key": SHARED_KEY, "Value": &case },
                        { "Key": &unique_key, "Value": "held" },
                    ],
                }),
            )
            .await;
            rows.push((case, arn, expected));
        }
    }

    for (index, (case, arn, _expected)) in rows.iter().enumerate() {
        for (other_case, other_arn, _other) in &rows[index + 1..] {
            assert_ne!(
                arn, other_arn,
                "case {case:?} and case {other_case:?} must be different resources, \
                 got the one ARN {arn:?}"
            );
        }
    }

    // Every resource was tagged before any of them was read, so a listing that
    // leaked across resources shows up here and not merely in creation order.
    for (case, arn, expected) in &rows {
        let listed = listed_tags(&waf, arn).await;
        assert_eq!(
            listed,
            pairs(expected),
            "case {case:?}: expected only this resource's own pairs, got {listed:?}"
        );
    }
}

/// Reading a listing changes nothing a later caller sees: the same read repeated
/// answers the same collection, a read of ONE page leaves the next read the whole
/// collection from its start, and the tags a resource may still be given afterwards
/// are undiminished by however many reads there were.
#[tokio::test]
async fn reading_a_listing_changes_nothing_a_later_caller_sees() {
    /// How many times each read below is repeated: more than once, so a read that
    /// consumed anything diverges on the second.
    const REPEATS: usize = 3;

    for scope in [SCOPE_REGIONAL, SCOPE_CLOUDFRONT] {
        let waf = Waf::new();
        // How many tags the resource holds: none at all, one, and several — the
        // counts the reporting rule generalises over.
        for count in [0usize, 1, 2, 5] {
            let case = format!("{scope}, {count} tag(s)");
            let (_id, arn, _lock) =
                create_of_kind(&waf, "IP set", scope, &format!("read-{count}-ips")).await;
            let mut expected: BTreeMap<String, String> = BTreeMap::new();
            for index in 0..count {
                let key = format!("key-{index}");
                waf.ok(
                    "TagResource",
                    json!({ "ResourceARN": &arn, "Tags": tags(&key, "v") }),
                )
                .await;
                expected.insert(key, "v".to_string());
            }

            // The whole collection, read again and again.
            for repeat in 0..REPEATS {
                let listed = listed_tags(&waf, &arn).await;
                assert_eq!(
                    listed,
                    pairs(&expected),
                    "case {case:?}, read {repeat}: a read must report the collection \
                     unchanged, got {listed:?}"
                );
            }

            // ONE page of it, read again and again: a listing holds no cursor of its
            // own, so every first page is the same first page.
            for repeat in 0..REPEATS {
                let page = waf
                    .ok(
                        "ListTagsForResource",
                        json!({ "ResourceARN": &arn, "Limit": 1 }),
                    )
                    .await;
                let listed = page["TagInfoForResource"]["TagList"]
                    .as_array()
                    .expect("TagInfoForResource.TagList")
                    .clone();
                let head: Vec<(String, String)> = expected
                    .iter()
                    .take(1)
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect();
                let actual: Vec<(String, String)> = listed
                    .iter()
                    .map(|tag| {
                        (
                            tag["Key"].as_str().unwrap_or_default().to_string(),
                            tag["Value"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect();
                assert_eq!(
                    actual, head,
                    "case {case:?}, page read {repeat}: every first page must be the \
                     same first page, got {actual:?}"
                );
            }

            // A marker read, repeated: resuming after the first key answers the rest
            // of the collection every time, and takes nothing away from it.
            if let Some(first_key) = expected.keys().next().cloned() {
                let rest: Vec<(String, String)> = expected
                    .iter()
                    .filter(|(key, _value)| key.as_str() > first_key.as_str())
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect();
                for repeat in 0..REPEATS {
                    let page = waf
                        .ok(
                            "ListTagsForResource",
                            json!({ "ResourceARN": &arn, "NextMarker": &first_key }),
                        )
                        .await;
                    let actual: Vec<(String, String)> = page["TagInfoForResource"]["TagList"]
                        .as_array()
                        .expect("TagInfoForResource.TagList")
                        .iter()
                        .map(|tag| {
                            (
                                tag["Key"].as_str().unwrap_or_default().to_string(),
                                tag["Value"].as_str().unwrap_or_default().to_string(),
                            )
                        })
                        .collect();
                    assert_eq!(
                        actual, rest,
                        "case {case:?}, marker read {repeat}: a marker read must report \
                         the keys after the marker, got {actual:?}"
                    );
                }
            }

            // After all those reads the collection is still the whole one, and the
            // resource still takes a further tag: no read consumed a pair or a slot.
            let whole = listed_tags(&waf, &arn).await;
            assert_eq!(
                whole,
                pairs(&expected),
                "case {case:?}: the reads must leave the collection whole, got {whole:?}"
            );
            waf.ok(
                "TagResource",
                json!({ "ResourceARN": &arn, "Tags": tags("Added", "after the reads") }),
            )
            .await;
            expected.insert("Added".to_string(), "after the reads".to_string());
            let grown = listed_tags(&waf, &arn).await;
            assert_eq!(
                grown,
                pairs(&expected),
                "case {case:?}: a tagging after the reads must add exactly its pair, \
                 got {grown:?}"
            );
        }
    }
}

/// An ARN the service no longer holds names no resource, so its listing is the
/// nonexistent-item refusal — and a resource created again under the same name is a
/// new resource, carrying none of the tags the deleted one held.
#[tokio::test]
async fn a_deleted_resource_lists_no_tags() {
    let waf = Waf::new();
    for (index, (kind, delete)) in KINDS.iter().enumerate() {
        let case = format!("{kind}, deleted");
        let name = format!("deleted-{index}");
        let (id, arn, lock) = create_of_kind(&waf, kind, SCOPE_REGIONAL, &name).await;
        waf.ok(
            "TagResource",
            json!({ "ResourceARN": &arn, "Tags": tags("Team", "sim2real") }),
        )
        .await;
        let listed = listed_tags(&waf, &arn).await;
        assert_eq!(
            listed,
            vec![("Team".to_string(), "sim2real".to_string())],
            "case {case:?}: the live resource must carry the tag it was given, got {listed:?}"
        );

        waf.ok(
            delete,
            json!({ "Name": &name, "Scope": SCOPE_REGIONAL, "Id": &id, "LockToken": &lock }),
        )
        .await;
        let refused = waf
            .refused(
                "ListTagsForResource",
                json!({ "ResourceARN": &arn }),
                "WAFNonexistentItemException",
                &case,
            )
            .await;
        assert_eq!(
            error_message(&refused),
            NONEXISTENT_ITEM,
            "case {case:?}: the refusal must carry WAF's own sentence, got {refused}"
        );

        // The same name again: a create mints a new resource, so the collection the
        // deleted one held is gone rather than inherited.
        let (_new_id, new_arn, _new_lock) = create_of_kind(&waf, kind, SCOPE_REGIONAL, &name).await;
        assert_ne!(
            new_arn, arn,
            "case {case:?}: a create after a delete must mint a new ARN, got {new_arn:?}"
        );
        let fresh = listed_tags(&waf, &new_arn).await;
        assert!(
            fresh.is_empty(),
            "case {case:?}: the new resource must carry no tag of the deleted one, \
             got {fresh:?}"
        );
    }
}
