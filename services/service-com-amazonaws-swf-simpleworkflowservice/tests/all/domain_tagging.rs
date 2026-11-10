// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource`, end to end over the service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: SimpleWorkflowService.<OperationName>`),
//! through the same `MultiServiceRouter` the emulator mounts in production. The harness
//! is the one `workflow_execution_lifecycle` already builds.
//!
//! The rule every case here asserts: `ListTagsForResource` reports the tags stored on
//! the ONE domain record whose own ARN is the requested `resourceArn`, and an ARN that
//! resolves to no stored domain is answered with HTTP 400 `UnknownResourceFault`
//! ("Returned when the named resource cannot be found with in the scope of this
//! operation (region or domain)"). The tables below vary what that rule generalises
//! over: how many tags a domain carries (none, one, the documented maximum), the form
//! of a tag (value present, present-and-empty, absent, symbols and non-ASCII letters,
//! both modelled length bounds), which domain of several is addressed, and the way an
//! ARN fails to name a stored domain (unknown name, other region, other account, other
//! service, other resource type, not an ARN at all).

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::workflow_execution_lifecycle::{
    error_code, merge, register_domain_body, string_at, Emulator, ACCOUNT, REGION,
};

/// Tags one SWF domain may carry. The crate's `MAX_TAGS_PER_DOMAIN`, from
/// `TagResource`'s own doc: "Amazon SWF supports a maximum of 50 tags per resource."
/// `pub(crate)` so a sibling suite fills a domain to the same documented bound rather
/// than keeping a second copy of it.
pub(crate) const MAX_TAGS_PER_DOMAIN: usize = 50;

/// The modelled maximum length of `ResourceTag$key`
/// (`smithy_query(query="shape",shape="ResourceTag")`: key `ResourceTagKey`, len
/// 1..128). `pub(crate)` so a sibling suite checks the same published bound.
pub(crate) const MAX_TAG_KEY_LEN: usize = 128;

/// The modelled maximum length of `ResourceTag$value` (same query: value
/// `ResourceTagValue`, len 0..256). `pub(crate)` for the same reason.
pub(crate) const MAX_TAG_VALUE_LEN: usize = 256;

/// One tag exactly as the wire carries it: a required key, and a `value` that may be
/// present, present and empty, or absent entirely (the member is optional).
///
/// `pub(crate)` so a sibling suite compares tags with the same exact-presence
/// semantics rather than keeping a second notion of what a tag is.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Tag {
    pub(crate) key: String,
    pub(crate) value: Option<String>,
}

impl Tag {
    pub(crate) fn new(key: &str, value: Option<&str>) -> Self {
        Self {
            key: key.to_string(),
            value: value.map(str::to_string),
        }
    }

    /// The JSON member for a `ResourceTagList` entry, omitting `value` when absent.
    pub(crate) fn to_json(&self) -> Value {
        match &self.value {
            Some(value) => json!({ "key": self.key, "value": value }),
            None => json!({ "key": self.key }),
        }
    }
}

/// The tags a response reports, as a set: AWS documents no ordering for a tag list, so
/// the assertion is on membership and count, never on position.
pub(crate) fn reported_tags(case: &str, body: &Value) -> BTreeSet<Tag> {
    let reported = body
        .get("tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a tags array, got {body:?}"));
    let tags: BTreeSet<Tag> = reported
        .iter()
        .map(|tag| Tag {
            key: string_at(tag, "/key"),
            value: tag.get("value").map(|value| {
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: a tag value is a string, got {tag:?}"))
                    .to_string()
            }),
        })
        .collect();
    assert_eq!(
        tags.len(),
        reported.len(),
        "case {case:?}: every reported tag must be distinct, got {body:?}"
    );
    tags
}

/// The awsJson type discriminator every returned `ResourceTag` carries: the id of the
/// shape the element IS, as the service reports it. Fixed text naming the shape,
/// identical for every tag of every response. `pub(crate)` so every suite that reads a
/// tag list asserts the one shape id rather than keeping a copy of it.
pub(crate) const RESOURCE_TAG_SHAPE_ID: &str = "com.amazonaws.swf.base.model#ResourceTag";

/// Every member a returned `ResourceTag` may carry, from
/// `smithy_query(query="shape",shape="ResourceTag")`: `key` (required), `value`, and the
/// `__type` discriminator. A response that invented another member would be reporting a
/// shape AWS never emits just as surely as one that omits `__type`.
pub(crate) const RESOURCE_TAG_MEMBERS: &[&str] = &["__type", "key", "value"];

/// The tags a response reports, having asserted the FORM of every element it carried:
/// each is a `ResourceTag` object naming its own shape in `__type` and carrying no member
/// outside the modelled three. `pub(crate)` so every suite that reads a tag list applies
/// the one check rather than a copy of it, and so a case also pins that the discriminator
/// sits BESIDE the stored data rather than in place of it.
pub(crate) fn checked_tags(case: &str, body: &Value) -> BTreeSet<Tag> {
    let elements = body
        .get("tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a tags array, got {body:?}"));
    for element in elements {
        let members = element
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: a tag is an object, got {element:?}"));
        let unexpected: Vec<&String> = members
            .keys()
            .filter(|member| !RESOURCE_TAG_MEMBERS.contains(&member.as_str()))
            .collect();
        assert!(
            unexpected.is_empty(),
            "case {case:?}: a reported tag carries only the modelled members \
             {RESOURCE_TAG_MEMBERS:?}, got {unexpected:?} in {element:?}"
        );
        assert_eq!(
            members.get("__type").and_then(Value::as_str),
            Some(RESOURCE_TAG_SHAPE_ID),
            "case {case:?}: tag {:?} must carry the ResourceTag type discriminator, \
             got {element:?}",
            members.get("key"),
        );
    }
    reported_tags(case, body)
}

/// Register `domain` carrying `tags` and return the ARN the service minted for it,
/// read back through `DescribeDomain` — the way a client learns a resource ARN.
pub(crate) async fn registered_with_tags(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    tags: &[Tag],
) -> String {
    let mut body = register_domain_body(domain, "7");
    if !tags.is_empty() {
        let wire: Vec<Value> = tags.iter().map(Tag::to_json).collect();
        merge(&mut body, json!({ "tags": wire }));
    }
    emulator.ok(case, "RegisterDomain", &body).await;
    let described = emulator
        .ok(case, "DescribeDomain", &json!({ "name": domain }))
        .await;
    let arn = string_at(&described, "/domainInfo/arn");
    assert_eq!(
        arn,
        format!("arn:aws:swf:{REGION}:{ACCOUNT}:/domain/{domain}"),
        "case {case:?}: the ARN a tagging call addresses is the stored domain's own, \
         got {described:?}"
    );
    arn
}

/// Call `ListTagsForResource` and return the parsed body, requiring a 200.
pub(crate) async fn list_tags(emulator: &Emulator, case: &str, resource_arn: &str) -> Value {
    emulator
        .ok(
            case,
            "ListTagsForResource",
            &json!({ "resourceArn": resource_arn }),
        )
        .await
}

/// One domain whose tags are listed back. Rows differ in tag count and tag form.
struct TagCase {
    domain: &'static str,
    tags: Vec<Tag>,
}

#[tokio::test]
async fn tags_are_listed_back_from_the_stored_domain_record() {
    let long_key = "k".repeat(MAX_TAG_KEY_LEN);
    let long_value = "v".repeat(MAX_TAG_VALUE_LEN);
    let cases = vec![
        // A domain registered with no tags at all: an empty list, not a 400 and not an
        // absent member.
        TagCase {
            domain: "untagged-domain",
            tags: Vec::new(),
        },
        TagCase {
            domain: "one-tag-domain",
            tags: vec![Tag::new("owner", Some("platform"))],
        },
        // `value` is optional (len 0..256), so "absent" and "present but empty" are two
        // different stored tags and must come back as they went in.
        TagCase {
            domain: "value-shapes-domain",
            tags: vec![
                Tag::new("empty-value", Some("")),
                Tag::new("absent-value", None),
                Tag::new("both-bounds", Some(&long_value)),
                Tag::new(&long_key, Some("key-at-its-bound")),
            ],
        },
        // The documented character set, from `ResourceTag`'s doc: "Tags may only
        // contain unicode letters, digits, whitespace, or these symbols: _ . : / = + -
        // @" — a class the issue's own case never exercised.
        TagCase {
            domain: "symbol-tags-domain",
            tags: vec![
                Tag::new("cost_centre.1", Some("eu/west = 1")),
                Tag::new("contact@team", Some("on-call+primary")),
                Tag::new("scope:tier", Some("2")),
                Tag::new("Ubersicht", Some("Grusse")),
                Tag::new("白板", Some("キー")),
            ],
        },
        // The per-resource maximum.
        TagCase {
            domain: "fifty-tags-domain",
            tags: (0..MAX_TAGS_PER_DOMAIN)
                .map(|index| Tag::new(&format!("k{index}"), Some(&format!("v{index}"))))
                .collect(),
        },
    ];

    let emulator = Emulator::new();
    let mut arns: Vec<(&str, String)> = Vec::new();
    for case in &cases {
        let arn = registered_with_tags(&emulator, case.domain, case.domain, &case.tags).await;
        let listed = list_tags(&emulator, case.domain, &arn).await;
        let expected: BTreeSet<Tag> = case.tags.iter().cloned().collect();
        let reported = reported_tags(case.domain, &listed);
        assert_eq!(
            reported, expected,
            "case {:?}: ListTagsForResource must report the {} stored tags, got {listed:?}",
            case.domain,
            case.tags.len()
        );
        arns.push((case.domain, arn));
    }

    // Each ARN addresses exactly ONE record: no domain's tags leak into another's
    // answer, so the response cannot be a shared or canned list.
    for (domain, arn) in &arns {
        let listed = list_tags(&emulator, domain, arn).await;
        let reported = reported_tags(domain, &listed);
        for other in &cases {
            if other.domain == *domain {
                continue;
            }
            let foreign: BTreeSet<Tag> = other.tags.iter().cloned().collect();
            let shared: Vec<&Tag> = reported.intersection(&foreign).collect();
            assert!(
                shared.is_empty(),
                "case {domain:?}: tags of {:?} must not appear in this domain's answer, \
                 got {shared:?}",
                other.domain
            );
        }
    }
}

/// One way a `resourceArn` can fail to name a stored domain. `expected_message` is
/// `Some` only where the message is request-derived and therefore a requirement: a
/// well-formed SWF domain ARN names a domain, and the fault reports that name the same
/// way `DescribeDomain` does for a name that was never registered.
struct UnknownCase {
    case: &'static str,
    arn: String,
    expected_message: Option<String>,
}

#[tokio::test]
async fn an_arn_naming_no_stored_domain_is_an_unknown_resource() {
    let emulator = Emulator::new();
    let stored = "tagged-domain";
    let arn = registered_with_tags(
        &emulator,
        "seed",
        stored,
        &[Tag::new("owner", Some("platform"))],
    )
    .await;

    let cases = vec![
        // The same ARN form, for a name that was never registered.
        UnknownCase {
            case: "never registered",
            arn: format!("arn:aws:swf:{REGION}:{ACCOUNT}:/domain/no-such-domain"),
            expected_message: Some("Unknown domain: no-such-domain".to_string()),
        },
        // A stored NAME, but another region's resource: the fault is documented for a
        // resource outside the scope of this operation, "region or domain".
        UnknownCase {
            case: "other region",
            arn: format!("arn:aws:swf:eu-west-1:{ACCOUNT}:/domain/{stored}"),
            expected_message: Some(format!("Unknown domain: {stored}")),
        },
        UnknownCase {
            case: "other account",
            arn: format!("arn:aws:swf:{REGION}:210987654321:/domain/{stored}"),
            expected_message: Some(format!("Unknown domain: {stored}")),
        },
        // Not a domain ARN of this service, so it names no domain at all. Only the
        // status and the error code are the requirement for these.
        UnknownCase {
            case: "other service",
            arn: format!("arn:aws:sqs:{REGION}:{ACCOUNT}:some-queue"),
            expected_message: None,
        },
        UnknownCase {
            case: "other resource type",
            arn: format!("arn:aws:swf:{REGION}:{ACCOUNT}:/workflow/{stored}"),
            expected_message: None,
        },
        UnknownCase {
            case: "empty domain segment",
            arn: format!("arn:aws:swf:{REGION}:{ACCOUNT}:/domain/"),
            expected_message: None,
        },
        UnknownCase {
            case: "a bare domain name, not an ARN",
            arn: stored.to_string(),
            expected_message: None,
        },
        UnknownCase {
            case: "too few ARN segments",
            arn: format!("arn:aws:swf:{REGION}:/domain/{stored}"),
            expected_message: None,
        },
    ];

    for case in &cases {
        let (status, body) = emulator
            .call("ListTagsForResource", &json!({ "resourceArn": case.arn }))
            .await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "UnknownResourceFault"),
            "case {:?}: expected 400 UnknownResourceFault for {:?}, got {status} {code} \
             with {body:?}",
            case.case,
            case.arn
        );
        if let Some(expected) = &case.expected_message {
            let message = body
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {:?}: expected a message, got {body:?}", case.case));
            assert_eq!(
                message, expected,
                "case {:?}: the fault message is built from the requested resource, got {message:?}",
                case.case
            );
        }
    }

    // None of the refusals touched the store: the stored domain still lists exactly
    // what it was registered with.
    let listed = list_tags(&emulator, "after refusals", &arn).await;
    assert_eq!(
        reported_tags("after refusals", &listed),
        BTreeSet::from([Tag::new("owner", Some("platform"))]),
        "case \"after refusals\": a refused call must not change what a later caller \
         sees, got {listed:?}"
    );

    // And the unknown name is unknown to the domain reader too, so the classification
    // is one fact about the store rather than two.
    emulator
        .refused(
            "never registered",
            "DescribeDomain",
            &json!({ "name": "no-such-domain" }),
            "UnknownResourceFault",
        )
        .await;
}

#[tokio::test]
async fn listing_tags_repeatedly_does_not_change_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    let tags = vec![
        Tag::new("owner", Some("platform")),
        Tag::new("stage", Some("")),
        Tag::new("audit", None),
    ];
    let arn = registered_with_tags(&emulator, "idempotent read", "read-twice-domain", &tags).await;

    let expected: BTreeSet<Tag> = tags.iter().cloned().collect();
    for round in 1..=3 {
        let case = format!("idempotent read #{round}");
        let listed = list_tags(&emulator, &case, &arn).await;
        assert_eq!(
            reported_tags(&case, &listed),
            expected,
            "case {case:?}: a read must report the same stored tags every time, \
             got {listed:?}"
        );
    }

    // The domain record itself is untouched by those reads.
    let described = emulator
        .ok(
            "idempotent read",
            "DescribeDomain",
            &json!({ "name": "read-twice-domain" }),
        )
        .await;
    assert_eq!(
        string_at(&described, "/domainInfo/status"),
        "REGISTERED",
        "case \"idempotent read\": listing tags must not move the domain's status, \
         got {described:?}"
    );
}
