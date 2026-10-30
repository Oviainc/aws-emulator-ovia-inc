// SPDX-License-Identifier: CC0-1.0

//! Tagging a resource of this service by ARN, driven over the wire.
//!
//! RULE: every operation that takes a `ResourceArn` resolves it against the resources
//! this service holds, and an ARN that resolves to nothing is refused with the modelled
//! `NotFoundException` — HTTP 404, `x-amzn-errortype: NotFoundException`, the sentence
//! Amazon MQ states that failure with — before anything is read or written. An ARN that
//! resolves is served, whichever kind of resource it names: the tags are recorded on
//! (or removed from) that resource, so the next read reports exactly what the writes
//! left behind.
//!
//! The tables span what the rule generalises over: the forms an ARN that names nothing
//! can take (a resource type this service holds no record of, an id that was never
//! minted, the right resource in another Region or account, a string that is not an ARN
//! at all), crossed with every operation that resolves one; and the tag sets a write
//! can carry (one tag, several, an empty value, a key that replaces one already on the
//! resource, a key that is removed, a key removed that was never there), crossed with
//! every kind of resource this service lets a caller create, up to the documented
//! per-resource quota and one past it.
//!
//! The wire harness is shared with [`crate::broker_lifecycle`], so every call goes
//! through `MultiServiceRouter::dispatch`, and the ARN is sent both verbatim and
//! percent-encoded the way an AWS SDK sends a path label.

use serde_json::{json, Value};
use std::collections::BTreeMap;

use service_com_amazonaws_mq_mq::state::MAX_TAGS;

use crate::broker_lifecycle::{create_body, string_at, Mq};
use crate::configuration_name::configuration_body;

/// The sentence Amazon MQ answers a `ResourceArn` that names no resource with. It
/// interpolates nothing from the request, so every row expects it unchanged.
const NOT_FOUND: &str = "Resource was not found";

/// A tag map as the wire carries it.
fn tag_object(pairs: &[(&str, &str)]) -> Value {
    json!(pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), json!(value)))
        .collect::<serde_json::Map<String, Value>>())
}

/// The tags an answer reports, as a comparable map; an absent member reads as none.
fn tags_of(answer: &Value) -> BTreeMap<String, String> {
    answer
        .pointer("/tags")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn expected_tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

impl Mq {
    pub(crate) async fn create_tags(&self, resource_arn: &str, tags: Value) -> (u16, String, Value) {
        self.call_with_code(
            "POST",
            &format!("/v1/tags/{resource_arn}"),
            Some(json!({"tags": tags})),
        )
        .await
    }

    pub(crate) async fn list_tags(&self, resource_arn: &str) -> (u16, String, Value) {
        self.call_with_code("GET", &format!("/v1/tags/{resource_arn}"), None)
            .await
    }

    pub(crate) async fn delete_tags(
        &self,
        resource_arn: &str,
        keys: &[&str],
    ) -> (u16, String, Value) {
        let query = keys
            .iter()
            .map(|key| format!("tagKeys={key}"))
            .collect::<Vec<String>>()
            .join("&");
        self.call_with_code("DELETE", &format!("/v1/tags/{resource_arn}?{query}"), None)
            .await
    }
}

/// Every kind of resource of this service a caller can create and tag today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Taggable {
    Broker,
    Configuration,
}

/// The ARN of a freshly created resource of this kind, and the path its own describe
/// reads it back from.
async fn create_taggable(
    mq: &Mq,
    kind: Taggable,
    name: &str,
    tags: &[(&str, &str)],
) -> (String, String) {
    let (path, mut body, arn_pointer) = match kind {
        Taggable::Broker => ("/v1/brokers", create_body(name), "/brokerArn"),
        Taggable::Configuration => ("/v1/configurations", configuration_body(name), "/arn"),
    };
    if !tags.is_empty() {
        body["tags"] = tag_object(tags);
    }
    let (status, created) = mq.call("POST", path, Some(body)).await;
    assert_eq!(
        status, 200,
        "setup {kind:?} {name:?}: expected the resource to be created, got {status} {created:?}"
    );
    let arn = string_at(&created, arn_pointer);
    let id = match kind {
        Taggable::Broker => string_at(&created, "/brokerId"),
        Taggable::Configuration => string_at(&created, "/id"),
    };
    let describe_path = match kind {
        Taggable::Broker => format!("/v1/brokers/{id}"),
        Taggable::Configuration => format!("/v1/configurations/{id}"),
    };
    (arn, describe_path)
}

/// RULE, refusal half: on every operation that resolves a `ResourceArn`, an ARN that
/// names no resource of this service is answered `NotFoundException` with Amazon MQ's
/// sentence for that failure — and the resource that DOES exist is left alone.
#[tokio::test]
async fn an_arn_naming_no_resource_is_refused_on_every_tag_operation() {
    let mq = Mq::new("us-east-1");
    let (arn, _) = create_taggable(&mq, Taggable::Broker, "tags-absent-arn", &[("kept", "yes")])
        .await;

    // The forms an ARN that resolves to nothing takes, each built FROM the ARN that
    // does resolve rather than copied from one recorded request: a resource type this
    // service holds no record of, an id that was never minted, the same resource in
    // another Region and in another account, and a string that is no ARN at all.
    let unknown_configuration =
        "arn:aws:mq:us-east-1:000000000000:configuration:c-00000000-0000-0000-0000-000000000000";
    let unknown_broker = "arn:aws:mq:us-east-1:000000000000:broker:never-created:\
                          b-00000000-0000-0000-0000-000000000000";
    let other_region = arn.replace("us-east-1", "eu-west-1");
    let other_account = arn.replace("000000000000", "123456789012");
    let arns: Vec<&str> = vec![
        unknown_configuration,
        unknown_broker,
        &other_region,
        &other_account,
        "not-an-arn",
    ];

    for resource_arn in arns {
        let calls: Vec<(&str, (u16, String, Value))> = vec![
            (
                "CreateTags",
                mq.create_tags(resource_arn, tag_object(&[("k", "v")])).await,
            ),
            ("ListTags", mq.list_tags(resource_arn).await),
            ("DeleteTags", mq.delete_tags(resource_arn, &["k"]).await),
        ];
        for (operation, (status, code, answer)) in calls {
            let case = format!("{operation} on {resource_arn:?}");
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status} {answer:?}"
            );
            assert_eq!(
                code, "NotFoundException",
                "case {case:?}: expected NotFoundException, got {code:?} {answer:?}"
            );
            assert_eq!(
                string_at(&answer, "/message"),
                NOT_FOUND,
                "case {case:?}: expected Amazon MQ's sentence for this failure, got {answer:?}"
            );
        }
    }

    // A refused tag operation changed nothing on the resource that does exist.
    let (status, code, listed) = mq.list_tags(&arn).await;
    assert_eq!(
        status, 200,
        "the existing resource must still be served, got {status} {code:?} {listed:?}"
    );
    assert_eq!(
        tags_of(&listed),
        expected_tags(&[("kept", "yes")]),
        "a refused tag operation must leave the existing resource's tags alone, got {listed:?}"
    );
}

/// One row of the write-then-read table: what the resource was created with, what a
/// `CreateTags` adds, what a `DeleteTags` removes, and what must then be reported.
struct TagCase {
    name: &'static str,
    created_with: &'static [(&'static str, &'static str)],
    added: &'static [(&'static str, &'static str)],
    removed: &'static [&'static str],
    expected: &'static [(&'static str, &'static str)],
}

/// Rows differing along tag count, value form, and whether a key is replaced, added,
/// removed, or removed while never present.
const TAG_CASES: &[TagCase] = &[
    TagCase {
        name: "tags-one",
        created_with: &[],
        added: &[("team", "payments")],
        removed: &[],
        expected: &[("team", "payments")],
    },
    TagCase {
        name: "tags-several",
        created_with: &[],
        added: &[("team", "payments"), ("env", "prod"), ("owner", "ops")],
        removed: &[],
        expected: &[("env", "prod"), ("owner", "ops"), ("team", "payments")],
    },
    TagCase {
        name: "tags-empty-value",
        created_with: &[],
        added: &[("marker", "")],
        removed: &[],
        expected: &[("marker", "")],
    },
    TagCase {
        name: "tags-replaced-key",
        created_with: &[("env", "prod")],
        added: &[("env", "staging")],
        removed: &[],
        expected: &[("env", "staging")],
    },
    TagCase {
        name: "tags-removed-key",
        created_with: &[("env", "prod"), ("team", "payments")],
        added: &[("owner", "ops")],
        removed: &["env"],
        expected: &[("owner", "ops"), ("team", "payments")],
    },
    TagCase {
        name: "tags-removed-absent-key",
        created_with: &[("team", "payments")],
        added: &[],
        removed: &["never-set"],
        expected: &[("team", "payments")],
    },
    TagCase {
        name: "tags-all-removed",
        created_with: &[("team", "payments")],
        added: &[("env", "prod")],
        removed: &["team", "env"],
        expected: &[],
    },
];

/// RULE, served half: the tags a write leaves on a resource are what the next read
/// reports, through `ListTags` and through the resource's own describe — for every kind
/// of resource this service holds.
#[tokio::test]
async fn tags_written_on_a_resource_are_what_the_next_read_reports() {
    let mq = Mq::new("eu-west-2");

    for kind in [Taggable::Broker, Taggable::Configuration] {
        for case in TAG_CASES {
            let label = format!("{kind:?} {}", case.name);
            let name = format!("{}-{}", case.name, if kind == Taggable::Broker { "b" } else { "c" });
            let (arn, describe_path) = create_taggable(&mq, kind, &name, case.created_with).await;

            if !case.added.is_empty() {
                let (status, code, answer) = mq.create_tags(&arn, tag_object(case.added)).await;
                assert_eq!(
                    status, 204,
                    "case {label:?}: CreateTags must answer the modelled 204, got {status} \
                     {code:?} {answer:?}"
                );
            }
            if !case.removed.is_empty() {
                let (status, code, answer) = mq.delete_tags(&arn, case.removed).await;
                assert_eq!(
                    status, 204,
                    "case {label:?}: DeleteTags must answer the modelled 204, got {status} \
                     {code:?} {answer:?}"
                );
            }

            let (status, code, listed) = mq.list_tags(&arn).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200, got {status} {code:?} {listed:?}"
            );
            assert_eq!(
                tags_of(&listed),
                expected_tags(case.expected),
                "case {label:?}: ListTags must report what the writes left, got {listed:?}"
            );

            // The tag store is the resource's own, so the resource's describe agrees.
            let (status, described) = mq.call("GET", &describe_path, None).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected the resource to read back, got {status} {described:?}"
            );
            assert_eq!(
                tags_of(&described),
                expected_tags(case.expected),
                "case {label:?}: the resource's own read must report the same tags, \
                 got {described:?}"
            );
        }
    }
}

/// An ARN percent-encoded the way an AWS SDK sends a path label resolves to the same
/// resource as the verbatim one.
#[tokio::test]
async fn a_percent_encoded_arn_resolves_to_the_same_resource() {
    let mq = Mq::new("us-east-1");

    for kind in [Taggable::Broker, Taggable::Configuration] {
        let (arn, _) = create_taggable(&mq, kind, "tags-encoded-arn", &[]).await;
        let encoded = arn.replace(':', "%3A");

        let (status, code, answer) = mq
            .create_tags(&encoded, tag_object(&[("team", "payments")]))
            .await;
        assert_eq!(
            status, 204,
            "case {kind:?}: encoded CreateTags must answer 204, got {status} {code:?} {answer:?}"
        );

        let (status, _code, listed) = mq.list_tags(&arn).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: expected 200, got {status} {listed:?}"
        );
        assert_eq!(
            tags_of(&listed),
            expected_tags(&[("team", "payments")]),
            "case {kind:?}: the encoded ARN must name the same resource as the verbatim one, \
             got {listed:?}"
        );
    }
}

/// The documented per-resource tag quota bounds what a write may leave behind: the
/// quota itself is served, one past it is refused naming the member, and the refusal
/// stores nothing.
#[tokio::test]
async fn the_tag_quota_bounds_what_a_write_may_leave_behind() {
    let mq = Mq::new("us-east-1");
    let (arn, _) = create_taggable(&mq, Taggable::Broker, "tags-quota", &[]).await;

    let pairs: Vec<(String, String)> = (0..=MAX_TAGS)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    let object = |count: usize| -> Value {
        json!(pairs[..count]
            .iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect::<serde_json::Map<String, Value>>())
    };

    // Up to the quota is served.
    let (status, code, answer) = mq.create_tags(&arn, object(MAX_TAGS)).await;
    assert_eq!(
        status, 204,
        "at the quota: expected 204, got {status} {code:?} {answer:?}"
    );

    // One past it is refused naming the member at fault, and nothing is stored.
    let (status, code, answer) = mq.create_tags(&arn, object(MAX_TAGS + 1)).await;
    assert_eq!(
        status, 400,
        "one past the quota: expected 400, got {status} {answer:?}"
    );
    assert_eq!(
        code, "BadRequestException",
        "one past the quota: expected BadRequestException, got {code:?} {answer:?}"
    );
    assert_eq!(
        answer.pointer("/errorAttribute").and_then(Value::as_str),
        Some("tags"),
        "one past the quota: expected errorAttribute \"tags\", got {answer:?}"
    );

    let (status, _code, listed) = mq.list_tags(&arn).await;
    assert_eq!(status, 200, "expected 200, got {status} {listed:?}");
    assert_eq!(
        tags_of(&listed).len(),
        MAX_TAGS,
        "a refused write must leave the resource at what the served write left, got {listed:?}"
    );
}
