// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `ListTagsForResource`.
//!
//! The RULE under test, asserted over tables rather than one recorded request:
//! `ListTagsForResource` REPORTS the tag set the store holds for the resource the
//! `resourceArn` label names, whatever wrote it and whichever resource kind the ARN
//! names — so it agrees entry for entry with every other reader of that same set, a
//! resource carrying no tags reports an EMPTY `tags` map rather than an absent one, and an
//! ARN that names no resource this service holds is refused with the service's
//! `ResourceNotFoundException` (404). Being a report, it changes nothing a later caller
//! sees.
//!
//! The rows differ along what the rule generalises over: which operation wrote the set
//! (`CreateDomain`'s `tags`, then `TagResource`), how many entries it holds (none, one,
//! several, up to the per-resource cap), the FORM of the key and the value (ASCII,
//! non-ASCII, spaces and symbols, an empty value, each at its published maximum length),
//! which resource kind the ARN names (a `Domain` and a `Subscription`), and how an ARN
//! fails to resolve (wrong id, wrong region, wrong account, wrong service, not an ARN at
//! all, or a resource that has since been deleted).
//!
//! Sources for every bound and catalog written by hand here:
//!
//! * `smithy_query(query="operation",operation="ListTagsForResource",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — `GET /tags/{resourceArn}` answering 200, `resourceArn` an `@httpLabel` and
//!   required, the single output member `tags` targeting `TagMap`, `@readonly`, and
//!   `errors: []` (so the exception asserted here is the shape the SERVICE models,
//!   widened into this operation exactly as `TagResource` widens it).
//! * `smithy_query(query="shape",shape="TagMap,TagKey,TagValue",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — `TagKey` is `@length 1..128` and `TagValue` is `@length 0..256`; `TagMap` declares
//!   no length, so the count limit is the service's own (`state::MAX_TAGS_PER_RESOURCE`).

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::state::{
    MAX_TAGS_PER_RESOURCE, MAX_TAG_KEY_LEN, MAX_TAG_VALUE_LEN,
};

use crate::domain_subscription_lifecycle::{error_code, string_at, tag_map, Emulator};
use crate::resource_tagging::{
    domain_tags, entries, tag_resource, tags_path, under_another_account,
};

/// `ListTagsForResource`, asserting the modelled success: 200 carrying a `tags` map, and
/// the entries of that map.
///
/// An absent `tags` member is a failure rather than "no tags": the member is a map of the
/// resource's tags, and a resource with none has an empty one.
pub(crate) async fn list_tags(
    emulator: &Emulator,
    case: &str,
    arn: &str,
) -> BTreeMap<String, String> {
    let (status, body) = emulator.send("GET", &tags_path(arn), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource {arn} must answer 200, got {status} with {body}"
    );
    let reported = body
        .get("tags")
        .and_then(Value::as_object)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the 200 must carry a tags map, got {body}")
        });
    reported
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: tag {key:?} must be a string in {body}")
                    })
                    .to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn list_tags_reports_the_set_the_store_holds_for_the_addressed_resource() {
    let emulator = Emulator::new("us-east-1");
    // The domain starts with the tags `CreateDomain` wrote, so the first read reports a
    // set this operation did not write itself.
    let created = emulator
        .create_domain(&json!({ "name": "clinic", "tags": { "env": "prod" } }))
        .await;
    let domain_id = string_at(&created, "domainId");
    let arn = string_at(&created, "arn");

    // A key and a value at each published maximum length, built here rather than taken
    // from a recorded request.
    let longest_key = "k".repeat(MAX_TAG_KEY_LEN);
    let longest_value = "v".repeat(MAX_TAG_VALUE_LEN);

    let mut expected: BTreeMap<String, String> = [("env".to_string(), "prod".to_string())]
        .into_iter()
        .collect();
    let reported = list_tags(&emulator, "the set CreateDomain wrote", &arn).await;
    assert_eq!(
        reported, expected,
        "the tags CreateDomain stored must be the set this reader reports, got {reported:?}"
    );

    // Each row is one `TagResource` call; the expectation after it is computed by
    // applying the merge rule to the previous expectation, never by restating a response.
    let rows: Vec<(&str, Value)> = vec![
        ("a single new key", json!({ "team": "cardiology" })),
        ("an existing key, overwritten", json!({ "env": "staging" })),
        (
            "several keys at once",
            json!({ "a": "1", "b": "2", "c": "3" }),
        ),
        ("an empty value", json!({ "retired": "" })),
        (
            "non-ASCII key and value",
            json!({ "clínica": "ü-ok", "日本語": "値" }),
        ),
        ("spaces and symbols", json!({ "cost center": "acct/42 +x" })),
        (
            "a key and a value at their maximum length",
            json!({ longest_key.clone(): longest_value.clone() }),
        ),
        ("no entries at all", json!({})),
    ];
    for (case, tags) in &rows {
        tag_resource(&emulator, case, &arn, tags).await;
        expected.extend(entries(tags));
        let reported = list_tags(&emulator, case, &arn).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: after tagging with {tags} the reader must report the merged \
             set, got {reported:?}"
        );
        // One store per resource: the ARN-addressed reader and the domain's own reader
        // project the same entries.
        let through_get_domain = domain_tags(&emulator, &domain_id).await;
        assert_eq!(
            reported, through_get_domain,
            "case {case:?}: ListTagsForResource and GetDomain must report the same set, \
             got {reported:?} and {through_get_domain:?}"
        );
    }
}

#[tokio::test]
async fn a_resource_with_no_tags_reports_an_empty_map() {
    let emulator = Emulator::new("eu-central-1");
    // Three ways to carry no tags: never tagged, created with an explicitly empty map,
    // and a subscription, which no create tags at all.
    let never_tagged = emulator.create_domain(&json!({ "name": "clinic" })).await;
    let empty_map = emulator
        .create_domain(&json!({ "name": "clinic-empty", "tags": {} }))
        .await;
    let domain_id = string_at(&never_tagged, "domainId");
    let (status, subscription) = emulator
        .send(
            "POST",
            &format!("/domains/{domain_id}/subscriptions"),
            Some(&json!({})),
        )
        .await;
    assert_eq!(status, 201, "CreateSubscription got {subscription}");

    for (case, arn) in [
        ("a domain created with no tags member", string_at(&never_tagged, "arn")),
        ("a domain created with an empty tags map", string_at(&empty_map, "arn")),
        ("a subscription, which a create never tags", string_at(&subscription, "arn")),
    ] {
        let reported = list_tags(&emulator, case, &arn).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: an untagged resource reports an EMPTY map, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn each_resource_kind_reports_its_own_set_at_its_own_arn() {
    let emulator = Emulator::new("ap-south-1");
    let created = emulator
        .create_domain(&json!({ "name": "clinic", "tags": { "env": "prod" } }))
        .await;
    let domain_id = string_at(&created, "domainId");
    let domain_arn = string_at(&created, "arn");
    let (status, first) = emulator
        .send(
            "POST",
            &format!("/domains/{domain_id}/subscriptions"),
            Some(&json!({})),
        )
        .await;
    assert_eq!(status, 201, "CreateSubscription got {first}");
    let (status, second) = emulator
        .send(
            "POST",
            &format!("/domains/{domain_id}/subscriptions"),
            Some(&json!({})),
        )
        .await;
    assert_eq!(status, 201, "CreateSubscription got {second}");
    let first_arn = string_at(&first, "arn");
    let second_arn = string_at(&second, "arn");

    // One write per resource, each a different set, so a reader that confused two ARNs
    // for one store would report the wrong entries somewhere.
    let writes: Vec<(&str, &str, Value)> = vec![
        (
            "the domain",
            domain_arn.as_str(),
            json!({ "tier": "silver", "env": "prod" }),
        ),
        (
            "the first subscription",
            first_arn.as_str(),
            json!({ "tier": "gold" }),
        ),
        (
            "the second subscription",
            second_arn.as_str(),
            json!({ "tier": "bronze", "owner": "ops" }),
        ),
    ];
    for (case, arn, tags) in &writes {
        tag_resource(&emulator, case, arn, tags).await;
    }
    for (case, arn, tags) in &writes {
        let reported = list_tags(&emulator, case, arn).await;
        assert_eq!(
            reported,
            entries(tags).into_iter().collect::<BTreeMap<_, _>>(),
            "case {case:?}: {arn} must report only its own set, got {reported:?}"
        );
    }

    // And the whole set a resource may hold reads back, not just a handful of entries.
    let seeded = tag_map(MAX_TAGS_PER_RESOURCE, 12, 20);
    let full = emulator
        .create_domain(&json!({ "name": "clinic-full", "tags": seeded.clone() }))
        .await;
    let reported = list_tags(&emulator, "a resource at the tag cap", &string_at(&full, "arn")).await;
    assert_eq!(
        reported,
        entries(&seeded).into_iter().collect::<BTreeMap<_, _>>(),
        "a resource holding {MAX_TAGS_PER_RESOURCE} tags must report all of them, \
         got {} entries",
        reported.len()
    );
}

#[tokio::test]
async fn an_arn_that_names_no_resource_is_refused() {
    let emulator = Emulator::new("us-west-2");
    let created = emulator
        .create_domain(&json!({ "name": "clinic", "tags": { "env": "prod" } }))
        .await;
    let domain_id = string_at(&created, "domainId");
    let arn = string_at(&created, "arn");
    let (status, subscription) = emulator
        .send(
            "POST",
            &format!("/domains/{domain_id}/subscriptions"),
            Some(&json!({})),
        )
        .await;
    assert_eq!(status, 201, "CreateSubscription got {subscription}");
    let subscription_arn = string_at(&subscription, "arn");

    // A domain with no web application is deleted within the DeleteDomain call, so its
    // ARN stops resolving immediately — and so do its subscriptions'.
    let doomed = emulator
        .create_domain(&json!({ "name": "clinic-doomed", "tags": { "env": "doomed" } }))
        .await;
    let doomed_id = string_at(&doomed, "domainId");
    let doomed_arn = string_at(&doomed, "arn");
    let reported = list_tags(&emulator, "the doomed domain before its delete", &doomed_arn).await;
    assert_eq!(
        reported,
        [("env".to_string(), "doomed".to_string())]
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
        "the doomed domain must report its tags while it is still held, got {reported:?}"
    );
    let (status, deleted) = emulator
        .send("DELETE", &format!("/domain/{doomed_id}"), None)
        .await;
    assert_eq!(status, 200, "DeleteDomain got {deleted}");

    let unresolvable: Vec<(&str, String)> = vec![
        (
            "a well-formed domain ARN whose id names no domain",
            "arn:aws:health-agent:us-west-2:000000000000:domain/dom-zzzzzzzzzzzzzzzz".to_string(),
        ),
        (
            "the live domain's ARN in another region",
            arn.replace("us-west-2", "ap-south-1"),
        ),
        (
            "the live domain's ARN under another account",
            under_another_account(&arn),
        ),
        (
            "a subscription id no subscription of the live domain carries",
            format!("{arn}/subscription/sub-000000000000000000000"),
        ),
        (
            "the live subscription's ARN under a domain that does not exist",
            subscription_arn.replace(&domain_id, "dom-zzzzzzzzzzzzzzzz"),
        ),
        ("the ARN of a deleted domain", doomed_arn),
        ("another service's ARN", "arn:aws:s3:::a-bucket".to_string()),
        ("not an ARN at all", "not-an-arn".to_string()),
        ("a bare domain id", domain_id.clone()),
    ];

    for (case, unresolved) in &unresolvable {
        let (status, body) = emulator.send("GET", &tags_path(unresolved), None).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?} ({unresolved}): expected 404 ResourceNotFoundException, \
             got {status} with {body}"
        );
        // The detail text is built from the REQUEST: it names the ARN the caller asked
        // for, which is what tells one 404 from another.
        let message = body
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: the 404 must carry a Message, got {body}"));
        assert!(
            message.contains(unresolved.as_str()),
            "case {case:?}: the message must name the ARN the request asked for, got {message:?}"
        );
    }
}

#[tokio::test]
async fn reading_tags_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new("us-east-1");
    let created = emulator
        .create_domain(&json!({ "name": "clinic", "tags": { "env": "prod", "team": "ops" } }))
        .await;
    let domain_id = string_at(&created, "domainId");
    let arn = string_at(&created, "arn");
    let (status, subscription) = emulator
        .send(
            "POST",
            &format!("/domains/{domain_id}/subscriptions"),
            Some(&json!({})),
        )
        .await;
    assert_eq!(status, 201, "CreateSubscription got {subscription}");
    let subscription_arn = string_at(&subscription, "arn");
    tag_resource(
        &emulator,
        "seeding the subscription",
        &subscription_arn,
        &json!({ "tier": "gold" }),
    )
    .await;

    // The report is a pure function of the stored set: repeating it, in any order over
    // any of the resources, answers the same entries every time.
    let first = list_tags(&emulator, "the domain, first read", &arn).await;
    let subscription_first = list_tags(&emulator, "the subscription, first read", &subscription_arn)
        .await;
    for round in 1..=4 {
        let again = list_tags(&emulator, "the domain, read again", &arn).await;
        assert_eq!(
            again, first,
            "round {round}: a read must not change the domain's set, got {again:?}"
        );
        let subscription_again =
            list_tags(&emulator, "the subscription, read again", &subscription_arn).await;
        assert_eq!(
            subscription_again, subscription_first,
            "round {round}: a read must not change the subscription's set, \
             got {subscription_again:?}"
        );
    }

    // Nor does it disturb the resource itself: every other member of the domain is still
    // the one the create reported, and the domain's own reader agrees with the ARN reader.
    let (status, fetched) = emulator
        .send("GET", &format!("/domain/{domain_id}"), None)
        .await;
    assert_eq!(status, 200, "GetDomain: got {fetched}");
    for member in ["domainId", "arn", "name", "status", "createdAt", "accountId"] {
        assert_eq!(
            fetched.get(member),
            created.get(member),
            "member {member:?} must be untouched by reading tags, got {fetched}"
        );
    }
    assert_eq!(
        domain_tags(&emulator, &domain_id).await,
        first,
        "both readers must still report the same set"
    );

    // A write after the reads still merges into the set the reads reported, so no read
    // consumed or reset anything.
    let longest_value = "v".repeat(MAX_TAG_VALUE_LEN);
    tag_resource(
        &emulator,
        "a write after the reads",
        &arn,
        &json!({ "env": longest_value.clone() }),
    )
    .await;
    let mut expected = first.clone();
    expected.insert("env".to_string(), longest_value);
    let reported = list_tags(&emulator, "after the write", &arn).await;
    assert_eq!(
        reported, expected,
        "the reads must have left the set the write merged into, got {reported:?}"
    );
}
