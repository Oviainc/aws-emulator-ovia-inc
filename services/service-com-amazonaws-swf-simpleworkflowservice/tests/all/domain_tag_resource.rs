// SPDX-License-Identifier: CC0-1.0

//! `TagResource`, end to end over the service's own wire protocol (`awsJson1_0`,
//! `POST /` with `X-Amz-Target: SimpleWorkflowService.<OperationName>`), through the
//! same `MultiServiceRouter` the emulator mounts in production. The harness is the one
//! `workflow_execution_lifecycle` already builds, and every assertion reads the tags
//! back through `ListTagsForResource`, so a handler that stored nothing cannot pass.
//!
//! The rule under test: the tags a call carries are MERGED into the stored domain's own
//! tag list — a key the domain already carries takes the new value, a key it does not is
//! added — an ARN that resolves to no stored domain is HTTP 400 `UnknownResourceFault`,
//! and a merge that would leave the domain above the documented maximum of 50 tags is
//! HTTP 400 `TooManyTagsFault` and stores nothing. The tables vary what the rule
//! generalises over: how many tags a call carries, whether each key is new or already
//! stored, the form of a tag value (present, empty, absent), the order the calls arrive
//! in, the way an ARN fails to name a stored domain, and the documented bound plus one
//! past it.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::workflow_execution_lifecycle::{
    register_domain_body, string_at, Emulator, ACCOUNT, REGION,
};

/// Tags one SWF domain may carry, from `TagResource`'s own doc: "Amazon SWF supports a
/// maximum of 50 tags per resource." Mirrors the crate's `MAX_TAGS_PER_DOMAIN`.
const MAX_TAGS_PER_DOMAIN: usize = 50;

/// Register `domain` and return the ARN the service minted for it, read back through
/// `DescribeDomain` — the way a client learns a resource ARN.
async fn registered(emulator: &Emulator, case: &str, domain: &str) -> String {
    emulator
        .ok(case, "RegisterDomain", &register_domain_body(domain, "7"))
        .await;
    let described = emulator
        .ok(case, "DescribeDomain", &json!({ "name": domain }))
        .await;
    let arn = string_at(&described, "/domainInfo/arn");
    assert_eq!(
        arn,
        format!("arn:aws:swf:{REGION}:{ACCOUNT}:/domain/{domain}"),
        "case {case:?}: the ARN a tagging call addresses is the stored domain's own, got \
         {described:?}"
    );
    arn
}

/// One tag on the wire: `value` is optional, so a row can carry it present, present and
/// empty, or absent entirely.
fn tag(key: &str, value: Option<&str>) -> Value {
    match value {
        Some(value) => json!({ "key": key, "value": value }),
        None => json!({ "key": key }),
    }
}

/// The tags `ListTagsForResource` reports, as a key -> value map. AWS documents no
/// ordering for a tag list, so the comparison is by key, never by position; an absent
/// `value` member is read as the empty string, which is how the wire carries "no value".
async fn stored_tags(emulator: &Emulator, case: &str, resource_arn: &str) -> BTreeMap<String, String> {
    let body = emulator
        .ok(
            case,
            "ListTagsForResource",
            &json!({ "resourceArn": resource_arn }),
        )
        .await;
    let reported = body
        .get("tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a tags array, got {body:?}"))
        .clone();
    let tags: BTreeMap<String, String> = reported
        .iter()
        .map(|entry| {
            (
                string_at(entry, "/key"),
                entry
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    assert_eq!(
        tags.len(),
        reported.len(),
        "case {case:?}: a domain carries each tag key once, got {body:?}"
    );
    tags
}

/// The expected map for a table row.
fn expected(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One row of the merge table: the case name, the tags the call carries, and the tag
/// set the domain must report afterwards.
type MergeRow = (&'static str, Vec<Value>, Vec<(&'static str, &'static str)>);

#[tokio::test]
async fn tagging_merges_the_request_into_the_stored_domain() {
    let emulator = Emulator::new();
    let arn = registered(&emulator, "merge", "merge-domain").await;

    // The rows arrive in order against the SAME domain, so each one is checked against
    // the state the one before it left: a new key, several at once, an overwrite of a
    // stored key, a value that is empty, a value that is absent, and a repeat that
    // changes nothing.
    let rows: [MergeRow; 6] = [
        (
            "one new key",
            vec![tag("owner", Some("platform"))],
            vec![("owner", "platform")],
        ),
        (
            "two more keys at once",
            vec![tag("env", Some("prod")), tag("tier", Some("gold"))],
            vec![("env", "prod"), ("owner", "platform"), ("tier", "gold")],
        ),
        (
            "an overwrite of a stored key",
            vec![tag("env", Some("staging"))],
            vec![("env", "staging"), ("owner", "platform"), ("tier", "gold")],
        ),
        (
            "a key whose value is empty",
            vec![tag("note", Some(""))],
            vec![
                ("env", "staging"),
                ("note", ""),
                ("owner", "platform"),
                ("tier", "gold"),
            ],
        ),
        (
            "a key with no value member at all",
            vec![tag("bare", None)],
            vec![
                ("bare", ""),
                ("env", "staging"),
                ("note", ""),
                ("owner", "platform"),
                ("tier", "gold"),
            ],
        ),
        (
            "the same tag again",
            vec![tag("owner", Some("platform"))],
            vec![
                ("bare", ""),
                ("env", "staging"),
                ("note", ""),
                ("owner", "platform"),
                ("tier", "gold"),
            ],
        ),
    ];

    for (case, tags, want) in rows {
        emulator
            .ok(
                case,
                "TagResource",
                &json!({ "resourceArn": arn, "tags": tags }),
            )
            .await;
        assert_eq!(
            stored_tags(&emulator, case, &arn).await,
            expected(&want),
            "case {case:?}: the tags read back are the merge of the request into the stored \
             domain"
        );
    }
}

#[tokio::test]
async fn tagging_addresses_one_domain_by_its_whole_arn() {
    let emulator = Emulator::new();
    let first = registered(&emulator, "scoped", "scoped-one").await;
    let second = registered(&emulator, "scoped", "scoped-two").await;

    emulator
        .ok(
            "scoped",
            "TagResource",
            &json!({ "resourceArn": first, "tags": [tag("only", Some("first"))] }),
        )
        .await;
    assert_eq!(
        stored_tags(&emulator, "scoped", &first).await,
        expected(&[("only", "first")]),
        "case \"scoped\": the addressed domain carries the tag"
    );
    assert_eq!(
        stored_tags(&emulator, "scoped", &second).await,
        expected(&[]),
        "case \"scoped\": no other domain is touched by it"
    );

    // The ways an ARN fails to name a stored domain: an unknown name, the same name in
    // another region or account, another service's ARN, and a value that is no ARN at
    // all. Each is the modelled `UnknownResourceFault`, and nothing is stored.
    for (case, resource_arn) in [
        (
            "an unknown domain name",
            format!("arn:aws:swf:{REGION}:{ACCOUNT}:/domain/never-registered"),
        ),
        (
            "the same domain name in another region",
            format!("arn:aws:swf:eu-west-1:{ACCOUNT}:/domain/scoped-one"),
        ),
        (
            "the same domain name in another account",
            format!("arn:aws:swf:{REGION}:999999999999:/domain/scoped-one"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:sqs:{REGION}:{ACCOUNT}:scoped-one"),
        ),
        ("not an ARN at all", "scoped-one".to_string()),
    ] {
        emulator
            .refused(
                case,
                "TagResource",
                &json!({ "resourceArn": resource_arn, "tags": [tag("k", Some("v"))] }),
                "UnknownResourceFault",
            )
            .await;
    }
    assert_eq!(
        stored_tags(&emulator, "after the refusals", &first).await,
        expected(&[("only", "first")]),
        "case \"after the refusals\": a refused call stores nothing"
    );
}

#[tokio::test]
async fn tagging_past_the_documented_maximum_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let case = "the documented maximum";
    let arn = registered(&emulator, case, "bounded-domain").await;

    // Fill the domain to exactly the documented maximum, in one call: the bound is on
    // what the domain carries, not on what one request names.
    let full: Vec<Value> = (0..MAX_TAGS_PER_DOMAIN)
        .map(|index| tag(&format!("key{index}"), Some("v")))
        .collect();
    emulator
        .ok(
            case,
            "TagResource",
            &json!({ "resourceArn": arn, "tags": full }),
        )
        .await;
    let at_bound = stored_tags(&emulator, case, &arn).await;
    assert_eq!(
        at_bound.len(),
        MAX_TAGS_PER_DOMAIN,
        "case {case:?}: the domain carries the documented maximum"
    );

    // One key past it is refused, and the stored tags are untouched.
    let case = "one past the documented maximum";
    emulator
        .refused(
            case,
            "TagResource",
            &json!({ "resourceArn": arn, "tags": [tag("one-too-many", Some("v"))] }),
            "TooManyTagsFault",
        )
        .await;
    assert_eq!(
        stored_tags(&emulator, case, &arn).await,
        at_bound,
        "case {case:?}: a refused call leaves the stored tags exactly as they were"
    );

    // A key the domain ALREADY carries is accepted at the bound: the merge does not grow
    // the list, so the maximum is not exceeded.
    let case = "an overwrite at the documented maximum";
    emulator
        .ok(
            case,
            "TagResource",
            &json!({ "resourceArn": arn, "tags": [tag("key0", Some("changed"))] }),
        )
        .await;
    let after = stored_tags(&emulator, case, &arn).await;
    assert_eq!(
        after.len(),
        MAX_TAGS_PER_DOMAIN,
        "case {case:?}: an overwrite does not grow the tag list"
    );
    assert_eq!(
        after.get("key0").map(String::as_str),
        Some("changed"),
        "case {case:?}: the overwritten key carries the new value, got {after:?}"
    );

    // And a single call that carries MORE than the maximum is refused on its own batch,
    // whatever the domain holds: a fresh domain cannot be taken over the bound either.
    let case = "a batch above the documented maximum";
    let fresh = registered(&emulator, case, "batch-bound-domain").await;
    let oversized: Vec<Value> = (0..=MAX_TAGS_PER_DOMAIN)
        .map(|index| tag(&format!("batch{index}"), Some("v")))
        .collect();
    emulator
        .refused(
            case,
            "TagResource",
            &json!({ "resourceArn": fresh, "tags": oversized }),
            "TooManyTagsFault",
        )
        .await;
    assert_eq!(
        stored_tags(&emulator, case, &fresh).await,
        expected(&[]),
        "case {case:?}: the refused batch stored nothing"
    );
}
