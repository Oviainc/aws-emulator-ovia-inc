// SPDX-License-Identifier: CC0-1.0

//! Tagging a WAFv2 resource: `TagResource` and `ListTagsForResource`, end to end
//! over the wire.
//!
//! The rule these suites assert is: a tag key under the prefix AWS keeps for its
//! own system tags is refused with 400 `WAFTagOperationException` wherever a
//! request supplies tags — the tagging operation and every create that takes a
//! `TagList` — and the resource's tags are left exactly as they were, while any
//! other key is accepted, stored against the resource its ARN names and read back
//! by `ListTagsForResource`, a key applied twice taking its latest value.
//!
//! Each suite drives that over a TABLE spanning the class rather than one request:
//! the refused rows differ in the form of the reserved key (a plain one, a
//! namespaced one, the bare prefix) and the accepted rows in how `aws` appears in a
//! key that is NOT reserved (nowhere, as a whole key, as a prefix of a longer word,
//! in the middle of a namespaced key), crossed with each of the four resource kinds
//! WAF tags and with both scopes. The harness is the one in
//! [`crate::web_acl_lifecycle`]: every call goes through the router's front door,
//! so what these tests observe is what an SDK observes.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    MAX_TAGS_PER_RESOURCE, SCOPE_CLOUDFRONT, SCOPE_REGIONAL,
};

use crate::permission_policy::error_message;
use crate::regex_pattern_set_lifecycle::create_regex_pattern_set;
use crate::rule_group_lifecycle::create_rule_group;
use crate::web_acl_lifecycle::{create_ip_set, create_web_acl, rule, sqli_statement, Waf};

/// `WAFTagOperationException`'s sentence for a key under the reserved prefix, as
/// recorded from WAF. Spelled out here independently of the service's own constant,
/// so this suite pins the text and not merely its reuse.
pub(crate) const RESERVED_TAG_KEY: &str = "You tried to tag your resources with system tag \
     keys. These tags have an aws: prefix and are reserved by AWS. Change your tag \
     keys, and try again.";

/// `WAFNonexistentItemException`'s sentence, as recorded from WAF.
pub(crate) const NONEXISTENT_ITEM: &str =
    "AWS WAF couldn’t perform the operation because your resource doesn’t exist.";

/// Keys AWS keeps for itself: every one of them begins with the `aws:` prefix the
/// tagging documentation reserves ("These tags have an aws: prefix and are reserved
/// by AWS"), in the forms AWS's own system tags take — a plain reserved key, a
/// namespaced one, and the bare prefix itself.
pub(crate) const RESERVED_KEYS: &[&str] = &["aws:Reserved", "aws:cloudformation:stack-name", "aws:"];

/// Keys that are a caller's own, including every way `aws` can appear in a key
/// WITHOUT it being a system tag key. Each satisfies `TagKey`'s own `@pattern`
/// (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, from
/// `smithy_query(query="shape",shape="TagKey")`).
pub(crate) const ACCEPTED_KEYS: &[&str] = &["Team", "aws", "awsome", "team:aws:owner", "Cost.Center/1"];

/// The four resource kinds WAF tags — "web ACLs, rule groups, IP sets, and regex
/// pattern sets" (`TagResource`'s own doc) — as the case name and the ARN of one
/// freshly created in `scope`.
pub(crate) async fn taggable_resources(waf: &Waf, scope: &str) -> Vec<(String, String)> {
    let (_id, ip_set_arn, _lock) = create_ip_set(waf, scope, "tagged-ips").await;
    let (_id, web_acl_arn, _lock) = create_web_acl(waf, scope, "tagged-acl", json!([])).await;
    let (_id, rule_group_arn, _lock) = create_rule_group(
        waf,
        scope,
        "tagged-group",
        50,
        json!([rule("inspect", sqli_statement())]),
    )
    .await;
    let (_id, regex_arn, _lock) =
        create_regex_pattern_set(waf, scope, "tagged-regex", &["^/admin"], None).await;
    vec![
        (format!("{scope} IP set"), ip_set_arn),
        (format!("{scope} web ACL"), web_acl_arn),
        (format!("{scope} rule group"), rule_group_arn),
        (format!("{scope} regex pattern set"), regex_arn),
    ]
}

/// A `TagList` of one pair.
pub(crate) fn tags(key: &str, value: &str) -> Value {
    json!([{ "Key": key, "Value": value }])
}

/// The pairs a listing must answer for the collection `expected` describes: a
/// resource's tags are reported in KEY order, which is the order a [`BTreeMap`]
/// enumerates in. ONE statement of that order, shared with the sibling suites.
pub(crate) fn pairs(expected: &BTreeMap<String, String>) -> Vec<(String, String)> {
    expected
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// The tags `ListTagsForResource` reports for `arn`, as `(key, value)` pairs in the
/// order the listing answers them.
pub(crate) async fn listed_tags(waf: &Waf, arn: &str) -> Vec<(String, String)> {
    let listed = waf
        .ok("ListTagsForResource", json!({ "ResourceARN": arn }))
        .await;
    assert_eq!(
        listed["TagInfoForResource"]["ResourceARN"],
        json!(arn),
        "the listing must report the resource it was asked about, got {listed}"
    );
    listed["TagInfoForResource"]["TagList"]
        .as_array()
        .expect("TagInfoForResource.TagList")
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// A key under the reserved prefix is refused on every taggable resource kind, in
/// both scopes, and changes nothing: the resource keeps the caller's own tag it
/// already carried and gains none of the refused request's keys.
#[tokio::test]
async fn a_reserved_tag_key_is_refused_and_stores_nothing() {
    for scope in [SCOPE_REGIONAL, SCOPE_CLOUDFRONT] {
        let waf = Waf::new();
        for (kind, arn) in taggable_resources(&waf, scope).await {
            // The caller's own tag, which every refusal below must leave alone.
            waf.ok(
                "TagResource",
                json!({ "ResourceARN": &arn, "Tags": tags("Team", "sim2real") }),
            )
            .await;

            for reserved in RESERVED_KEYS {
                let case = format!("{kind}, key {reserved:?}");
                // The reserved key alone, and the reserved key alongside an
                // acceptable one: one bad key refuses the whole request.
                for request_tags in [
                    tags(reserved, "x"),
                    json!([
                        { "Key": "Owner", "Value": "platform" },
                        { "Key": reserved, "Value": "x" },
                    ]),
                ] {
                    let refused = waf
                        .refused(
                            "TagResource",
                            json!({ "ResourceARN": &arn, "Tags": request_tags }),
                            "WAFTagOperationException",
                            &case,
                        )
                        .await;
                    assert_eq!(
                        error_message(&refused),
                        RESERVED_TAG_KEY,
                        "case {case:?}: the refusal must carry WAF's own sentence, \
                         got {refused}"
                    );
                }

                let listed = listed_tags(&waf, &arn).await;
                assert_eq!(
                    listed,
                    vec![("Team".to_string(), "sim2real".to_string())],
                    "case {case:?}: a refused tagging must change nothing, got {listed:?}"
                );
            }
        }
    }
}

/// A key of the caller's own is accepted on every taggable resource kind, in both
/// scopes, and is surfaced by `ListTagsForResource` with the value it was given;
/// applying the same key again replaces that value rather than adding a second
/// pair.
#[tokio::test]
async fn an_accepted_tag_key_is_stored_and_read_back() {
    for scope in [SCOPE_REGIONAL, SCOPE_CLOUDFRONT] {
        let waf = Waf::new();
        for (kind, arn) in taggable_resources(&waf, scope).await {
            // What the resource must carry, by key — the listing reports a
            // resource's tags in key order, the order every listing of this service
            // enumerates in.
            let mut expected: BTreeMap<String, String> = BTreeMap::new();
            for key in ACCEPTED_KEYS {
                let case = format!("{kind}, key {key:?}");
                waf.ok(
                    "TagResource",
                    json!({ "ResourceARN": &arn, "Tags": tags(key, "first") }),
                )
                .await;
                expected.insert((*key).to_string(), "first".to_string());
                let listed = listed_tags(&waf, &arn).await;
                assert_eq!(
                    listed,
                    pairs(&expected),
                    "case {case:?}: the accepted key must be surfaced by the listing, \
                     got {listed:?}"
                );

                // The same key again: a tag is a key:value PAIR identified by its
                // key, so this replaces the value and adds no second pair.
                waf.ok(
                    "TagResource",
                    json!({ "ResourceARN": &arn, "Tags": tags(key, "second") }),
                )
                .await;
                expected.insert((*key).to_string(), "second".to_string());
                let listed = listed_tags(&waf, &arn).await;
                assert_eq!(
                    listed,
                    pairs(&expected),
                    "case {case:?}: re-tagging a key must replace its value, got {listed:?}"
                );
            }
        }
    }
}

/// One create per taggable kind, with `Tags` carrying a key: the reserved ones are
/// refused and create nothing, and the accepted ones are stored and read back by
/// `ListTagsForResource` — the same rule the tagging operation applies, applied
/// wherever a request supplies tags.
#[tokio::test]
async fn a_create_applies_the_same_reserved_key_rule() {
    /// One create, as the operation name and a body builder taking the `Tags` to
    /// carry and the name to create under.
    struct Create {
        operation: &'static str,
        body: fn(&str, &str, Value) -> Value,
    }
    let creates = [
        Create {
            operation: "CreateIPSet",
            body: |name, scope, tags| {
                json!({
                    "Name": name,
                    "Scope": scope,
                    "IPAddressVersion": "IPV4",
                    "Addresses": ["10.0.0.0/8"],
                    "Tags": tags,
                })
            },
        },
        Create {
            operation: "CreateWebACL",
            body: |name, scope, tags| {
                json!({
                    "Name": name,
                    "Scope": scope,
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": {
                        "SampledRequestsEnabled": true,
                        "CloudWatchMetricsEnabled": true,
                        "MetricName": name,
                    },
                    "Tags": tags,
                })
            },
        },
        Create {
            operation: "CreateRuleGroup",
            body: |name, scope, tags| {
                json!({
                    "Name": name,
                    "Scope": scope,
                    "Capacity": 10,
                    "Rules": [],
                    "VisibilityConfig": {
                        "SampledRequestsEnabled": true,
                        "CloudWatchMetricsEnabled": true,
                        "MetricName": name,
                    },
                    "Tags": tags,
                })
            },
        },
        Create {
            operation: "CreateRegexPatternSet",
            body: |name, scope, tags| {
                json!({
                    "Name": name,
                    "Scope": scope,
                    "RegularExpressionList": [{ "RegexString": "^/admin" }],
                    "Tags": tags,
                })
            },
        },
    ];

    let waf = Waf::new();
    for create in creates {
        for (index, reserved) in RESERVED_KEYS.iter().enumerate() {
            let case = format!("{} with {reserved:?}", create.operation);
            let name = format!("reserved-{index}");
            let refused = waf
                .refused(
                    create.operation,
                    (create.body)(&name, SCOPE_REGIONAL, tags(reserved, "x")),
                    "WAFTagOperationException",
                    &case,
                )
                .await;
            assert_eq!(
                error_message(&refused),
                RESERVED_TAG_KEY,
                "case {case:?}: the refusal must carry WAF's own sentence, got {refused}"
            );
            // Nothing was created under that name: the same create succeeds now,
            // which it could not if the refused one had stored a resource (a
            // duplicate name is refused).
            let created = waf
                .ok(
                    create.operation,
                    (create.body)(&name, SCOPE_REGIONAL, tags("Team", "sim2real")),
                )
                .await;
            let arn = created["Summary"]["ARN"]
                .as_str()
                .expect("Summary.ARN")
                .to_string();
            let listed = listed_tags(&waf, &arn).await;
            assert_eq!(
                listed,
                vec![("Team".to_string(), "sim2real".to_string())],
                "case {case:?}: a create's own tags must be surfaced by the listing, \
                 got {listed:?}"
            );
        }
    }
}

/// An address that resolves to no taggable resource of this account is refused
/// without storing anything — malformed as the parameter failure, well formed as
/// the nonexistent-item failure — and so is a listing of one.
#[tokio::test]
async fn an_address_that_resolves_to_nothing_is_refused() {
    /// A string long enough to satisfy `ResourceArn`'s `@length(20..2048)` that is
    /// not an ARN at all, so the modelled constraints admit it and the handler is
    /// what refuses it.
    const MALFORMED_ARN: &str = "this-is-not-an-arn-but-it-is-long-enough";
    /// A well-formed wafv2 IP set ARN that no create minted.
    const MISSING_ARN: &str = "arn:aws:wafv2:us-west-2:000000000000:regional/ipset/\
         probe-absent-ips/00000000-1111-2222-3333-444444444444";

    let waf = Waf::new();
    let (_id, arn, _lock) = create_ip_set(&waf, SCOPE_REGIONAL, "addressed-ips").await;
    // A CLOUDFRONT-scoped resource's ARN carries another Region than the REGIONAL
    // one above, so the same name in the other scope is a different resource.
    let (_id, other_scope_arn, _lock) =
        create_ip_set(&waf, SCOPE_CLOUDFRONT, "addressed-ips").await;
    assert_ne!(
        arn, other_scope_arn,
        "a scope is a namespace, so the two IP sets must carry different ARNs"
    );

    for (case, address, code) in [
        ("a string that is not an ARN", MALFORMED_ARN, "WAFInvalidParameterException"),
        ("an ARN no create minted", MISSING_ARN, "WAFNonexistentItemException"),
    ] {
        for operation in ["TagResource", "ListTagsForResource"] {
            let case = format!("{operation}, {case}");
            let mut request = json!({ "ResourceARN": address });
            if operation == "TagResource" {
                request["Tags"] = tags("Team", "sim2real");
            }
            let refused = waf.refused(operation, request, code, &case).await;
            if code == "WAFNonexistentItemException" {
                assert_eq!(
                    error_message(&refused),
                    NONEXISTENT_ITEM,
                    "case {case:?}: the refusal must carry WAF's own sentence, \
                     got {refused}"
                );
            }
        }
    }

    // The resources that DO exist are untouched by any of the refusals above.
    for arn in [&arn, &other_scope_arn] {
        let listed = listed_tags(&waf, arn).await;
        assert!(
            listed.is_empty(),
            "a resource created with no tags must list none, got {listed:?}"
        );
    }
}

/// A resource holds at most the documented number of tags: the request that fills
/// the quota is accepted, the one past it is refused and stores nothing, and
/// replacing the value of a key already held adds no tag, so it is accepted even on
/// a full resource.
#[tokio::test]
async fn a_resource_holds_at_most_the_documented_number_of_tags() {
    let waf = Waf::new();
    let (_id, arn, _lock) = create_ip_set(&waf, SCOPE_REGIONAL, "quota-ips").await;

    // The published bound, in one request: `TagList` is a list, so the quota is
    // about the resource and not about how many calls filled it.
    let fill: Vec<Value> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| json!({ "Key": format!("key-{index}"), "Value": "v" }))
        .collect();
    waf.ok(
        "TagResource",
        json!({ "ResourceARN": &arn, "Tags": Value::Array(fill) }),
    )
    .await;
    let listed = listed_tags(&waf, &arn).await;
    assert_eq!(
        listed.len(),
        MAX_TAGS_PER_RESOURCE,
        "the resource must carry the tags that filled the quota, got {listed:?}"
    );

    // One past the bound.
    waf.refused(
        "TagResource",
        json!({ "ResourceARN": &arn, "Tags": tags("one-too-many", "v") }),
        "WAFLimitsExceededException",
        "one tag past the quota",
    )
    .await;
    let after = listed_tags(&waf, &arn).await;
    assert_eq!(
        after, listed,
        "case {:?}: a refused tagging must change nothing, got {after:?}",
        "one tag past the quota"
    );

    // A key the resource already holds, on a full resource: no new tag, so it is
    // accepted and the value changes.
    waf.ok(
        "TagResource",
        json!({ "ResourceARN": &arn, "Tags": tags("key-0", "replaced") }),
    )
    .await;
    let replaced = listed_tags(&waf, &arn).await;
    assert_eq!(
        replaced.len(),
        MAX_TAGS_PER_RESOURCE,
        "replacing a value must add no tag, got {replaced:?}"
    );
    assert!(
        replaced.contains(&("key-0".to_string(), "replaced".to_string())),
        "the replaced value must be surfaced by the listing, got {replaced:?}"
    );
}

/// `ListTagsForResource` pages the collection: a `Limit` hands back that many tags,
/// a marker while tags remain resumes after the last key of the page, and the pages
/// together are the whole collection in the listing's own order. A listing that has
/// handed back everything carries no position to resume from, which is what ends the
/// loop.
#[tokio::test]
async fn a_tag_listing_pages_the_collection() {
    let waf = Waf::new();
    let (_id, arn, _lock) = create_ip_set(&waf, SCOPE_REGIONAL, "paged-ips").await;
    let keys = ["alpha", "beta", "gamma", "delta", "epsilon"];
    for key in keys {
        waf.ok(
            "TagResource",
            json!({ "ResourceARN": &arn, "Tags": tags(key, "v") }),
        )
        .await;
    }
    let whole = listed_tags(&waf, &arn).await;
    assert_eq!(
        whole.len(),
        keys.len(),
        "every applied key must be listed, got {whole:?}"
    );

    for limit in [1usize, 2, keys.len(), keys.len() + 1] {
        let case = format!("Limit {limit}");
        let mut walked: Vec<(String, String)> = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let mut request = json!({ "ResourceARN": &arn, "Limit": limit });
            if let Some(marker) = &marker {
                request["NextMarker"] = json!(marker);
            }
            let page = waf.ok("ListTagsForResource", request).await;
            let listed = page["TagInfoForResource"]["TagList"]
                .as_array()
                .expect("TagInfoForResource.TagList")
                .clone();
            assert!(
                listed.len() <= limit,
                "case {case:?}: a page must hold at most the limit, got {page}"
            );
            walked.extend(listed.iter().map(|tag| {
                (
                    tag["Key"].as_str().unwrap_or_default().to_string(),
                    tag["Value"].as_str().unwrap_or_default().to_string(),
                )
            }));
            // A marker only while tags remain: an empty one (or none at all) is the
            // end of the collection, and every tag must already have been walked by
            // then.
            let answered = page["NextMarker"].as_str().unwrap_or_default().to_string();
            if answered.is_empty() {
                assert_eq!(
                    walked.len(),
                    whole.len(),
                    "case {case:?}: the listing may only stop handing back a marker \
                     once it has handed back every tag, got {walked:?}"
                );
                break;
            }
            assert_eq!(
                Some(answered.as_str()),
                walked.last().map(|(key, _value)| key.as_str()),
                "case {case:?}: a page that leaves tags behind must name its last \
                 key, got {page}"
            );
            marker = Some(answered);
        }
        assert_eq!(
            walked, whole,
            "case {case:?}: the pages together must be the whole collection, got {walked:?}"
        );
    }

    // A `Limit` outside the model's `PaginationLimit` range (`range 1..100`) is a
    // MODELLED constraint, so the request boundary refuses it before the handler
    // runs: the refusal is the framework's `ValidationException`, and a caller never
    // receives a silently empty terminal page. One row per published bound and one
    // past each.
    for limit in [1_i64, 100] {
        let page = waf
            .ok(
                "ListTagsForResource",
                json!({ "ResourceARN": &arn, "Limit": limit }),
            )
            .await;
        assert!(
            !page["TagInfoForResource"]["TagList"]
                .as_array()
                .expect("TagInfoForResource.TagList")
                .is_empty(),
            "case Limit={limit}: a limit on the bound must be honoured, got {page}"
        );
    }
    for limit in [0_i64, -1, 101] {
        let (status, body) = waf
            .call(
                "ListTagsForResource",
                json!({ "ResourceARN": &arn, "Limit": limit }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case Limit={limit}: a limit outside 1..100 must be refused, got status \
             {status} and body {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ValidationException"),
            "case Limit={limit}: a limit outside the modelled range must be refused \
             by the request boundary, got {body}"
        );
    }
}
