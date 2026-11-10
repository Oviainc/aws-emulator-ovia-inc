// SPDX-License-Identifier: CC0-1.0

//! The form `ListTagsForResource` reports a tag in, end to end over the service's own
//! wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: SimpleWorkflowService.<Operation>`), through the same
//! `MultiServiceRouter` the emulator mounts in production. The harness is the one
//! `workflow_execution_lifecycle` already builds.
//!
//! The rule every case here asserts: every structure the service returns in an awsJson
//! list carries the type discriminator naming its OWN shape, so each element of `tags`
//! reports `ResourceTag`'s shape id beside the stored key and value, and that text is
//! fixed — it is the same in every element of every response and is derived from
//! neither the tag, nor the addressed resource, nor anything the request carried.
//!
//! The rows vary what the rule generalises over: how many tags the domain carries (none,
//! one, two, four, the documented maximum), which writer stored them (`RegisterDomain`,
//! `TagResource`, both, and what survives `UntagResource`), the form of a value (text,
//! empty, absent, the published length bound), the shape of a key (plain, symbol-bearing,
//! non-ASCII, the published length bound) and whether the request that stored the tag
//! carried a discriminator of its own.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::domain_tagging::{
    checked_tags, list_tags, registered_with_tags, Tag, MAX_TAGS_PER_DOMAIN, MAX_TAG_KEY_LEN,
    MAX_TAG_VALUE_LEN, RESOURCE_TAG_SHAPE_ID,
};
use crate::workflow_execution_lifecycle::{register_domain_body, string_at, Emulator};

/// One domain whose tags are listed back: what `RegisterDomain` stored, what a later
/// `TagResource` merged in, which keys a later `UntagResource` removed, and the tag set
/// the response must then report.
struct DiscriminatorCase {
    case: &'static str,
    domain: &'static str,
    registered: Vec<Tag>,
    tagged: Vec<Tag>,
    untagged: Vec<&'static str>,
    expected: Vec<Tag>,
}

/// Drive one row and answer what the response reported, having asserted the form of
/// every element it carried.
async fn listed(emulator: &Emulator, row: &DiscriminatorCase) -> BTreeSet<Tag> {
    let arn = registered_with_tags(emulator, row.case, row.domain, &row.registered).await;
    if !row.tagged.is_empty() {
        let wire: Vec<Value> = row.tagged.iter().map(Tag::to_json).collect();
        emulator
            .ok(
                row.case,
                "TagResource",
                &json!({ "resourceArn": arn, "tags": wire }),
            )
            .await;
    }
    if !row.untagged.is_empty() {
        emulator
            .ok(
                row.case,
                "UntagResource",
                &json!({ "resourceArn": arn, "tagKeys": row.untagged }),
            )
            .await;
    }
    let body = list_tags(emulator, row.case, &arn).await;
    checked_tags(row.case, &body)
}

#[tokio::test]
async fn every_listed_tag_carries_the_resource_tag_discriminator() {
    let long_key = "k".repeat(MAX_TAG_KEY_LEN);
    let long_value = "v".repeat(MAX_TAG_VALUE_LEN);
    let full: Vec<Tag> = (0..MAX_TAGS_PER_DOMAIN)
        .map(|index| Tag::new(&format!("key{index}"), Some(&format!("value{index}"))))
        .collect();

    let rows = vec![
        // A domain with no tags at all: an empty list, so there is no element to carry a
        // discriminator — the row checks the empty answer rather than the rule, and still
        // a 200 with the member present.
        DiscriminatorCase {
            case: "no tags at all",
            domain: "no-tags-domain",
            registered: Vec::new(),
            tagged: Vec::new(),
            untagged: Vec::new(),
            expected: Vec::new(),
        },
        DiscriminatorCase {
            case: "one tag, stored by RegisterDomain",
            domain: "one-registered-tag",
            registered: vec![Tag::new("squad", Some("payments"))],
            tagged: Vec::new(),
            untagged: Vec::new(),
            expected: vec![Tag::new("squad", Some("payments"))],
        },
        DiscriminatorCase {
            case: "two tags, stored by TagResource",
            domain: "two-tagged-tags",
            registered: Vec::new(),
            tagged: vec![
                Tag::new("region-group", Some("emea")),
                Tag::new("rotation", Some("weekly")),
            ],
            untagged: Vec::new(),
            expected: vec![
                Tag::new("region-group", Some("emea")),
                Tag::new("rotation", Some("weekly")),
            ],
        },
        DiscriminatorCase {
            case: "four tags, from both writers",
            domain: "four-mixed-tags",
            registered: vec![
                Tag::new("lifecycle", Some("pilot")),
                Tag::new("squad", Some("payments")),
            ],
            tagged: vec![
                Tag::new("rotation", Some("weekly")),
                Tag::new("budget", Some("cc-9")),
            ],
            untagged: Vec::new(),
            expected: vec![
                Tag::new("budget", Some("cc-9")),
                Tag::new("lifecycle", Some("pilot")),
                Tag::new("rotation", Some("weekly")),
                Tag::new("squad", Some("payments")),
            ],
        },
        // `value` is optional (len 0..256), so "absent", "empty" and "at the bound" are
        // three different stored tags; the discriminator is the same on all three.
        DiscriminatorCase {
            case: "value forms: empty, absent, at the published bound",
            domain: "value-forms-domain",
            registered: vec![Tag::new("empty-value", Some("")), Tag::new("no-value", None)],
            tagged: vec![Tag::new("bounded-value", Some(&long_value))],
            untagged: Vec::new(),
            expected: vec![
                Tag::new("bounded-value", Some(&long_value)),
                Tag::new("empty-value", Some("")),
                Tag::new("no-value", None),
            ],
        },
        // The documented character set, from `ResourceTag`'s doc: "Tags may only contain
        // unicode letters, digits, whitespace, or these symbols: _ . : / = + - @", plus
        // `ResourceTagKey`'s published maximum length of 128.
        DiscriminatorCase {
            case: "key forms: symbols, non-ASCII, at the published bound",
            domain: "key-forms-domain",
            registered: vec![
                Tag::new("cost.centre_1", Some("eu/west = 1")),
                Tag::new("contact@squad", Some("on-call+primary")),
                Tag::new("Übersicht", Some("Grüße")),
                Tag::new("予算", Some("九月")),
            ],
            tagged: vec![Tag::new(&long_key, Some("key-at-its-bound"))],
            untagged: Vec::new(),
            expected: vec![
                Tag::new("contact@squad", Some("on-call+primary")),
                Tag::new("cost.centre_1", Some("eu/west = 1")),
                Tag::new("Übersicht", Some("Grüße")),
                Tag::new("予算", Some("九月")),
                Tag::new(&long_key, Some("key-at-its-bound")),
            ],
        },
        DiscriminatorCase {
            case: "the documented maximum of tags",
            domain: "full-domain",
            registered: Vec::new(),
            tagged: full.clone(),
            untagged: Vec::new(),
            expected: full.clone(),
        },
        DiscriminatorCase {
            case: "what survives an untagging",
            domain: "partly-untagged-domain",
            registered: vec![
                Tag::new("keep", Some("yes")),
                Tag::new("drop-one", Some("no")),
                Tag::new("drop-two", None),
            ],
            tagged: Vec::new(),
            untagged: vec!["drop-one", "drop-two"],
            expected: vec![Tag::new("keep", Some("yes"))],
        },
        // An empty-list row: it carries no element, so it checks the empty answer rather
        // than the discriminator rule.
        DiscriminatorCase {
            case: "every tag untagged again",
            domain: "fully-untagged-domain",
            registered: vec![Tag::new("gone", Some("soon")), Tag::new("also-gone", None)],
            tagged: Vec::new(),
            untagged: vec!["gone", "also-gone"],
            expected: Vec::new(),
        },
    ];

    let emulator = Emulator::new();
    for row in &rows {
        let reported = listed(&emulator, row).await;
        let expected: BTreeSet<Tag> = row.expected.iter().cloned().collect();
        assert_eq!(
            reported, expected,
            "case {:?}: the discriminator sits beside the {} stored tags, which must still \
             read back unchanged",
            row.case,
            row.expected.len()
        );
    }
}

#[tokio::test]
async fn the_discriminator_is_fixed_text_about_the_shape() {
    let emulator = Emulator::new();

    // Two domains of different accounts' worth of tags: disjoint keys, different values,
    // different counts, different ARNs. Every discriminator across both answers must be
    // the one shape id, so the text cannot be derived from the tag or the resource.
    let first = registered_with_tags(
        &emulator,
        "fixed text",
        "fixed-text-one",
        &[Tag::new("squad", Some("payments")), Tag::new("tier", None)],
    )
    .await;
    let second = registered_with_tags(
        &emulator,
        "fixed text",
        "fixed-text-two",
        &[Tag::new("rotation", Some("weekly"))],
    )
    .await;

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut elements = 0usize;
    for (case, arn) in [("first domain", &first), ("second domain", &second)] {
        let body = list_tags(&emulator, case, arn).await;
        checked_tags(case, &body);
        for element in body
            .get("tags")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected a tags array, got {body:?}"))
        {
            elements += 1;
            seen.insert(string_at(element, "/__type"));
        }
    }
    assert_eq!(
        (elements, seen.iter().cloned().collect::<Vec<String>>()),
        (3, vec![RESOURCE_TAG_SHAPE_ID.to_string()]),
        "case \"fixed text\": every element of every response carries the one shape id"
    );
}

#[tokio::test]
async fn a_discriminator_the_request_carried_is_not_echoed_back() {
    let emulator = Emulator::new();
    let domain = "echo-check-domain";

    // `ResourceTag` is also the INPUT element shape, so a client can send a `__type` of
    // its own. What comes back names the shape the element is, never what the request
    // said it was — the same for a tag stored at registration and one stored later.
    let mut body = register_domain_body(domain, "7");
    body.as_object_mut()
        .expect("the register body is an object")
        .insert(
            "tags".to_string(),
            json!([{ "__type": "com.example#NotAResourceTag", "key": "squad", "value": "payments" }]),
        );
    emulator.ok("echoed input", "RegisterDomain", &body).await;
    let described = emulator
        .ok("echoed input", "DescribeDomain", &json!({ "name": domain }))
        .await;
    let arn = string_at(&described, "/domainInfo/arn");

    emulator
        .ok(
            "echoed input",
            "TagResource",
            &json!({
                "resourceArn": arn,
                "tags": [{ "__type": "com.example#AlsoNotAResourceTag", "key": "rotation", "value": "weekly" }],
            }),
        )
        .await;

    let listed = list_tags(&emulator, "echoed input", &arn).await;
    assert_eq!(
        checked_tags("echoed input", &listed),
        BTreeSet::from([
            Tag::new("rotation", Some("weekly")),
            Tag::new("squad", Some("payments")),
        ]),
        "case \"echoed input\": the stored key and value come back unchanged beside the \
         service's own discriminator, got {listed:?}"
    );
}
