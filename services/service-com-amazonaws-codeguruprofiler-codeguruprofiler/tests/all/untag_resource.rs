// SPDX-License-Identifier: CC0-1.0

//! `UntagResource`, end to end over HTTP through the router this service mounts in
//! production.
//!
//! The rule every case here asserts: `UntagResource` deletes, from the tag map of the
//! ONE stored resource its `resourceArn` identifies, exactly the keys the request names
//! and nothing else — a named key the resource does not carry leaves the map as it stood
//! (the operation is `@idempotent`, so removal is by key and asking for a key that is
//! already absent is a success, not an error) — and it writes no other member of that
//! record, so a later `DescribeProfilingGroup` reports the surviving tags and an
//! unchanged group otherwise. An ARN this store holds no resource for answers the
//! modelled 404 `ResourceNotFoundException`; a `resourceArn` that is not a
//! profiling-group ARN of this service answers the modelled 400 `ValidationException`.
//!
//! The harness is the one [`crate::profiling_group_lifecycle`] already builds. The table
//! varies what the rule generalises over: how many tags the resource carries, how many
//! keys one request names, whether each named key is present or absent, the ORDER and
//! multiplicity of the named keys, and the form of a key (case, separators, spaces,
//! non-ASCII, empty).

use std::collections::BTreeMap;

use percent_encoding::{percent_encode, AsciiSet, NON_ALPHANUMERIC};

use crate::profiling_group_lifecycle::{Answer, Harness};

/// RFC 3986 unreserved characters stay literal; everything else is percent-encoded.
/// That is what an SDK does to a value it puts in a single-segment `@httpLabel` (so an
/// ARN's `:` and `/` arrive as `%3A` and `%2F`) or in an `@httpQuery` value.
const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

fn encode(value: &str) -> String {
    percent_encode(value.as_bytes(), UNRESERVED).to_string()
}

/// One `UntagResource` case: the tags the group is created with, the keys one request
/// names, and the tags the group must then report.
#[derive(Debug)]
struct UntagCase {
    /// Part of the profiling-group name, so each row gets its own resource.
    label: &'static str,
    /// The tag map `CreateProfilingGroup` stores on the group.
    tags: &'static [(&'static str, &'static str)],
    /// `tagKeys`, in request order — repeats and absent keys included on purpose.
    remove: &'static [&'static str],
    /// The tags the group must carry afterwards.
    remaining: &'static [(&'static str, &'static str)],
}

/// The table. Rows differ in tag count (0, 1, 2, 4), in how many keys one request names
/// (1, 2, 3, 4), in whether those keys are present, absent or a mix, in their order and
/// multiplicity, and in their character class (lower, mixed case, dotted, dashed,
/// spaced, non-ASCII, empty). No row is privileged: the issue's recorded case is the
/// single absent key in `absent-key-only`.
const CASES: &[UntagCase] = &[
    // The recorded instance: a key the resource does not carry is a no-op.
    UntagCase {
        label: "absent-key-only",
        tags: &[("env", "dev")],
        remove: &["nope"],
        remaining: &[("env", "dev")],
    },
    // A resource with NO tags at all: still a success, still nothing stored.
    UntagCase {
        label: "untagged-resource",
        tags: &[],
        remove: &["env"],
        remaining: &[],
    },
    // The only key, removed.
    UntagCase {
        label: "only-key",
        tags: &[("only", "one")],
        remove: &["only"],
        remaining: &[],
    },
    // One of several, so the rest must survive.
    UntagCase {
        label: "one-of-four",
        tags: &[
            ("team", "payments"),
            ("tier", "prod"),
            ("cost.center", "42"),
            ("zz", ""),
        ],
        remove: &["tier"],
        remaining: &[("team", "payments"), ("cost.center", "42"), ("zz", "")],
    },
    // Present and absent keys in ONE request: the present ones go, the absent ones are
    // ignored, and the request still succeeds.
    UntagCase {
        label: "mixed-present-and-absent",
        tags: &[("a", "1"), ("b", "2"), ("c", "3")],
        remove: &["b", "missing", "c"],
        remaining: &[("a", "1")],
    },
    // Every key at once, named in an order unrelated to any stored order.
    UntagCase {
        label: "all-keys",
        tags: &[("one", "1"), ("two", "2"), ("three", "3")],
        remove: &["two", "one", "three"],
        remaining: &[],
    },
    // The same key twice in one request: the second mention is already-absent, so it is
    // the no-op the rule describes rather than a failure.
    UntagCase {
        label: "repeated-key",
        tags: &[("dup", "x"), ("keep", "y")],
        remove: &["dup", "dup"],
        remaining: &[("keep", "y")],
    },
    // Tag keys are case-sensitive, so removing `env` must not remove `Env` or `ENV`.
    UntagCase {
        label: "case-sensitive",
        tags: &[("env", "lower"), ("Env", "mixed"), ("ENV", "upper")],
        remove: &["env"],
        remaining: &[("Env", "mixed"), ("ENV", "upper")],
    },
    // Keys whose characters need percent-encoding on the wire, plus a value that does.
    UntagCase {
        label: "encoded-keys",
        tags: &[
            ("with space", "a b"),
            ("dot.ted", "v1"),
            ("dash-ed", "v2"),
            ("ünïcode", "v3"),
        ],
        remove: &["with space", "ünïcode"],
        remaining: &[("dot.ted", "v1"), ("dash-ed", "v2")],
    },
    // The empty key: not a key any resource here carries, so naming it is a no-op and
    // must not remove anything else.
    UntagCase {
        label: "empty-key",
        tags: &[("keep", "me")],
        remove: &[""],
        remaining: &[("keep", "me")],
    },
];

/// The profiling-group name this row's resource gets (within the modelled
/// `ProfilingGroupName` pattern `^[\w-]+$`).
fn group_name(case: &UntagCase) -> String {
    format!("untag-{}", case.label)
}

/// `CreateProfilingGroup`, with this row's tag map.
async fn create(harness: &Harness, case: &UntagCase) -> Answer {
    let mut body = serde_json::Map::new();
    body.insert(
        "profilingGroupName".to_string(),
        serde_json::Value::from(group_name(case)),
    );
    if !case.tags.is_empty() {
        body.insert(
            "tags".to_string(),
            serde_json::Value::Object(
                case.tags
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), serde_json::Value::from(*value)))
                    .collect(),
            ),
        );
    }
    harness
        .send(
            "POST",
            &format!("/profilingGroups?clientToken={}", case.label),
            "application/json",
            serde_json::to_vec(&serde_json::Value::Object(body)).expect("a serializable body"),
        )
        .await
}

/// `UntagResource`: `DELETE /tags/{resourceArn}?tagKeys=…`, with `resourceArn` in one
/// path segment and one `tagKeys` query pair per key, exactly as the model binds them.
async fn untag(harness: &Harness, arn: &str, keys: &[&str]) -> Answer {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", encode(key)))
        .collect::<Vec<_>>()
        .join("&");
    harness
        .send(
            "DELETE",
            &format!("/tags/{}?{query}", encode(arn)),
            "application/json",
            Vec::new(),
        )
        .await
}

/// The `tags` member of a `DescribeProfilingGroup` answer, as a comparable map.
/// `profilingGroup` is the response's `@httpPayload`, so the description IS the body.
fn described_tags(harness_answer: &Answer) -> BTreeMap<String, String> {
    harness_answer
        .json()
        .pointer("/tags")
        .and_then(serde_json::Value::as_object)
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
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

#[tokio::test]
async fn untagging_removes_exactly_the_named_keys_from_the_stored_resource() {
    let harness = Harness::new();

    for case in CASES {
        let created = create(&harness, case).await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: create must succeed, got {} {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let arn = created
            .json()
            .pointer("/arn")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: create returned no arn"))
            .to_string();

        let answer = untag(&harness, &arn, case.remove).await;
        assert_eq!(
            answer.status,
            204,
            "case {case:?}: UntagResource must answer 204, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.body.is_empty() || answer.body.as_slice() == b"{}",
            "case {case:?}: the modelled output has no members, got {:?}",
            String::from_utf8_lossy(&answer.body)
        );

        // The decisive assertion: the removal was STORED, so the reader answers with the
        // surviving tags rather than the untag being an empty success.
        let described = harness.describe(&group_name(case)).await;
        assert_eq!(
            described.status,
            200,
            "case {case:?}: describe must answer 200, got {} {}",
            described.status,
            String::from_utf8_lossy(&described.body)
        );
        assert_eq!(
            described_tags(&described),
            expected_tags(case.remaining),
            "case {case:?}: wrong surviving tags, got {}",
            String::from_utf8_lossy(&described.body)
        );
    }
}

#[tokio::test]
async fn untagging_is_idempotent_and_writes_nothing_but_the_tag_map() {
    let harness = Harness::new();

    for case in CASES {
        assert_eq!(
            create(&harness, case).await.status,
            201,
            "case {case:?}: create must succeed"
        );
        let before = harness.describe(&group_name(case)).await.json();
        let arn = before
            .pointer("/arn")
            .and_then(serde_json::Value::as_str)
            .expect("a described arn")
            .to_string();

        // Repeat the SAME request: the first may remove keys, the second and third find
        // every named key already absent, and all three answer the same way.
        for attempt in 0..3 {
            let answer = untag(&harness, &arn, case.remove).await;
            assert_eq!(
                answer.status,
                204,
                "case {case:?} (attempt {attempt}): must answer 204, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
        }

        let after = harness.describe(&group_name(case)).await.json();
        let after_group = &after;
        let before_group = &before;
        assert_eq!(
            described_tags(&harness.describe(&group_name(case)).await),
            expected_tags(case.remaining),
            "case {case:?}: repeating the request must leave the same tags, got {after:?}"
        );
        // Every other member of the record is untouched: an untag writes the tag map and
        // nothing else (`updatedAt` reports when the profiling GROUP was updated).
        for member in [
            "name",
            "arn",
            "computePlatform",
            "agentOrchestrationConfig",
            "createdAt",
            "updatedAt",
            "profilingStatus",
        ] {
            assert_eq!(
                after_group.get(member),
                before_group.get(member),
                "case {case:?}: an untag must not change {member}, got {after_group:?}"
            );
        }
    }
}

#[tokio::test]
async fn untagging_touches_only_the_resource_its_arn_names() {
    let harness = Harness::new();
    for case in CASES {
        assert_eq!(
            create(&harness, case).await.status,
            201,
            "case {case:?}: create must succeed"
        );
    }

    // Remove every key of ONE resource; every other resource keeps the tags it was
    // created with, because the ARN identifies one record.
    let target = &CASES[3];
    let arn = harness
        .describe(&group_name(target))
        .await
        .json()
        .pointer("/arn")
        .and_then(serde_json::Value::as_str)
        .expect("a described arn")
        .to_string();
    let every_key: Vec<&str> = target.tags.iter().map(|(key, _)| *key).collect();
    let answer = untag(&harness, &arn, &every_key).await;
    assert_eq!(
        answer.status,
        204,
        "case {target:?}: must answer 204, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );

    for case in CASES {
        let expected = if std::ptr::eq(case, target) {
            expected_tags(&[])
        } else {
            expected_tags(case.tags)
        };
        let described = harness.describe(&group_name(case)).await;
        assert_eq!(
            described_tags(&described),
            expected,
            "case {case:?}: only {} was untagged, got {}",
            group_name(target),
            String::from_utf8_lossy(&described.body)
        );
    }
}

#[tokio::test]
async fn untagging_an_arn_the_store_holds_no_resource_for_answers_the_modelled_404() {
    let harness = Harness::new();
    assert_eq!(
        create(&harness, &CASES[0]).await.status,
        201,
        "create must succeed"
    );
    let live = harness
        .describe(&group_name(&CASES[0]))
        .await
        .json()
        .pointer("/arn")
        .and_then(serde_json::Value::as_str)
        .expect("a described arn")
        .to_string();

    // Well-formed profiling-group ARNs of this service that name no stored resource: a
    // different group name, a different region, a different account, a different
    // partition. An ARN identifies ONE resource, so each is a miss.
    for arn in [
        "arn:aws:codeguru-profiler:us-east-1:000000000000:profilingGroup/no-such-group".to_string(),
        live.replace("us-east-1", "eu-west-1"),
        live.replace("000000000000", "123456789012"),
        live.replace("arn:aws:", "arn:aws-us-gov:"),
        format!("{live}-missing"),
    ] {
        let answer = untag(&harness, &arn, &["env"]).await;
        assert_eq!(
            answer.status,
            404,
            "arn {arn:?}: an unknown resource must answer 404, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "arn {arn:?}: expected ResourceNotFoundException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    // The refused requests stored nothing: the one real resource still carries its tags.
    assert_eq!(
        described_tags(&harness.describe(&group_name(&CASES[0])).await),
        expected_tags(CASES[0].tags),
        "a refused untag must store nothing"
    );
}

#[tokio::test]
async fn a_resource_arn_that_is_not_a_profiling_group_arn_is_refused_as_a_bad_request() {
    let harness = Harness::new();
    assert_eq!(
        create(&harness, &CASES[0]).await.status,
        201,
        "create must succeed"
    );

    // `ProfilingGroupArn` carries no modelled pattern, so the boundary validator passes
    // these to the handler. None of them names a resource of ANY kind: not an ARN, too
    // few fields, another service's namespace, another resource type, or no name.
    for arn in [
        "demo-app",
        "not-an-arn-at-all",
        "arn:aws:codeguru-profiler:us-east-1:000000000000",
        "arn::codeguru-profiler:us-east-1:000000000000:profilingGroup/demo-app",
        "arn:aws:codeguru-reviewer:us-east-1:000000000000:profilingGroup/demo-app",
        "arn:aws:codeguru-profiler:us-east-1:000000000000:profilingGroups/demo-app",
        "arn:aws:codeguru-profiler:us-east-1:000000000000:profilingGroup/",
        "arn:aws:codeguru-profiler:us-east-1:000000000000:profilingGroup/demo/app",
    ] {
        let answer = untag(&harness, arn, &["env"]).await;
        assert_eq!(
            answer.status,
            400,
            "arn {arn:?}: must be refused as a bad request, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "arn {arn:?}: expected ValidationException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    assert_eq!(
        described_tags(&harness.describe(&group_name(&CASES[0])).await),
        expected_tags(CASES[0].tags),
        "a refused untag must store nothing"
    );
}

#[tokio::test]
async fn an_untag_without_the_required_tag_keys_stores_nothing() {
    let harness = Harness::new();
    assert_eq!(
        create(&harness, &CASES[0]).await.status,
        201,
        "create must succeed"
    );
    let arn = harness
        .describe(&group_name(&CASES[0]))
        .await
        .json()
        .pointer("/arn")
        .and_then(serde_json::Value::as_str)
        .expect("a described arn")
        .to_string();

    // `tagKeys` is `required` and binds to the query string, so a request that carries
    // no `tagKeys` pair is not a legal `UntagResource` at all: the protocol layer
    // refuses it before the handler runs. Whatever it answers, it must not SUCCEED and
    // must not have written the record.
    let answer = harness
        .send(
            "DELETE",
            &format!("/tags/{}", encode(&arn)),
            "application/json",
            Vec::new(),
        )
        .await;
    assert!(
        answer.status >= 400,
        "a request binding no tagKeys must not succeed, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        described_tags(&harness.describe(&group_name(&CASES[0])).await),
        expected_tags(CASES[0].tags),
        "a refused untag must store nothing"
    );
}
