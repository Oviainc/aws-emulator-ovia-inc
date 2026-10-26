// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource` against the published bounds of its one input member.
//!
//! The rule under test: `ResourceArn` is modelled `TagResourceArn`,
//! `length 1..1011` with pattern
//! `^arn:aws[a-z-]*:scheduler:[a-z0-9\-]+:\d{12}:schedule-group\/[0-9a-zA-Z-_.]+$`
//! (`smithy_query(query="operation", operation="ListTagsForResource")`), so an
//! ARN WITHIN the bound is a well-formed request answered by the resource rule —
//! 200 with the group's stored tags when it names a group of this account and
//! Region, 404 `ResourceNotFoundException` when it names none — and an ARN past
//! the bound is refused 400 `ValidationException` before the handler runs.
//! `tests/all/resource_tags.rs` spans the tag-set semantics and the ARN PATTERN;
//! this suite spans its LENGTH, each published bound and one character past it,
//! and pins that repeating the read reports the same answer.
//!
//! Every request goes through the shared [`crate::wire_harness`], the real
//! restJson1 front door.

use std::collections::BTreeMap;

use crate::wire_harness::{
    call, error_code, group_arn, list_tags, scheduler_router, tags_of, REGION,
};

/// A `TagResourceArn` of exactly `length` characters: the ARN this account and
/// Region mint, with the group name padded out to the requested total. Padding
/// uses `n`, which `[0-9a-zA-Z-_.]+` admits, so only the LENGTH varies from a
/// well-formed ARN.
fn group_arn_of_length(length: usize) -> String {
    let prefix = group_arn(REGION, "");
    let padding = length
        .checked_sub(prefix.chars().count())
        .expect("the requested length must leave room for the ARN prefix");
    format!("{prefix}{}", "n".repeat(padding))
}

#[tokio::test]
async fn an_arn_within_the_modelled_length_bound_is_answered_by_the_resource_rule() {
    let router = scheduler_router();

    // `(case, resource arn)` — every row satisfies the pattern and sits at or
    // inside the modelled maximum of 1011 characters, naming no group, so each
    // is a not-found rather than a bad request. The shortest row is the shortest
    // ARN the pattern admits at all (a one-character group name).
    let within: Vec<(&str, String)> = vec![
        ("the shortest ARN the pattern admits", group_arn(REGION, "g")),
        ("64 characters of group name", group_arn(REGION, &"n".repeat(64))),
        ("1010 characters, one inside the bound", group_arn_of_length(1010)),
        ("1011 characters, exactly the bound", group_arn_of_length(1011)),
    ];

    for (case, arn) in within {
        assert!(
            arn.chars().count() <= 1011,
            "case {case:?}: the test's own ARN is past the modelled bound at {} characters",
            arn.chars().count()
        );
        let (status, body) = list_tags(&router, &arn).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: an accepted ARN naming no group must be a not-found, \
             got {status} {body}"
        );
    }

    // The other outcome of the same rule: an ARN inside the bound that DOES name
    // a group is answered 200 with that group's stored tags. The name is the
    // longest `ScheduleGroupName` allows (length 1..64), so this row is the
    // longest ARN a group can actually have.
    let longest_name = "n".repeat(64);
    let (status, body) = call(
        &router,
        "POST",
        &format!("/schedule-groups/{longest_name}"),
        Some(serde_json::json!({ "Tags": [{ "Key": "env", "Value": "prod" }] })),
    )
    .await;
    assert_eq!(status, 200, "CreateScheduleGroup got {status} {body}");
    let arn = group_arn(REGION, &longest_name);
    let (status, body) = list_tags(&router, &arn).await;
    assert_eq!(
        status, 200,
        "case \"an in-bound ARN that names a group\": got {status} {body}"
    );
    assert_eq!(
        tags_of(&body),
        BTreeMap::from([("env".to_string(), "prod".to_string())]),
        "case \"an in-bound ARN that names a group\": expected the stored tags, got {body}"
    );
}

#[tokio::test]
async fn an_arn_past_the_modelled_length_bound_is_refused() {
    let router = scheduler_router();

    // One character past the published maximum, and far past it: the boundary
    // validator refuses both before the handler decides anything about groups.
    let past: Vec<(&str, String)> = vec![
        ("1012 characters, one past the bound", group_arn_of_length(1012)),
        ("2000 characters, far past the bound", group_arn_of_length(2000)),
    ];

    for (case, arn) in past {
        assert!(
            arn.chars().count() > 1011,
            "case {case:?}: the test's own ARN is not past the modelled bound"
        );
        let (status, body) = list_tags(&router, &arn).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: an ARN past the length bound must be a bad request, \
             got {status} {body}"
        );
    }
}

#[tokio::test]
async fn repeating_the_read_reports_the_same_tags() {
    let router = scheduler_router();

    // `(case, group name, tags the create call carried)` — the member omitted, a
    // single tag, and a tag whose key and value sit at their published upper
    // bounds (`Tag$Key` 1..128, `Tag$Value` 1..256, from
    // `smithy_query(query="shape", shape="Tag")`).
    let long_key = "k".repeat(128);
    let long_value = "v".repeat(256);
    let cases: Vec<(&str, &str, Option<serde_json::Value>)> = vec![
        ("no Tags member", "untagged.group_1", None),
        (
            "one tag",
            "tagged-group",
            Some(serde_json::json!([{ "Key": "env", "Value": "prod" }])),
        ),
        (
            "key at 128 and value at 256, their upper bounds",
            "bounded-tag-group",
            Some(serde_json::json!([{ "Key": long_key, "Value": long_value }])),
        ),
    ];

    for (case, group, tags) in cases {
        let body = tags
            .clone()
            .map(|tags| serde_json::json!({ "Tags": tags }));
        let (status, created) =
            call(&router, "POST", &format!("/schedule-groups/{group}"), body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateScheduleGroup got {status} {created}"
        );
        let arn = group_arn(REGION, group);

        let expected: BTreeMap<String, String> = tags
            .as_ref()
            .and_then(serde_json::Value::as_array)
            .map(|tags| {
                tags.iter()
                    .map(|tag| {
                        (
                            tag["Key"].as_str().unwrap_or_default().to_string(),
                            tag["Value"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Three reads in a row: a report never consumes, mints or moves state, so
        // every one answers the same thing.
        let mut seen = Vec::new();
        for _ in 0..3 {
            let (status, body) = list_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: ListTagsForResource got {status} {body}"
            );
            assert_eq!(
                tags_of(&body),
                expected,
                "case {case:?}: expected the stored tags {expected:?}, got {body}"
            );
            seen.push(body);
        }
        assert!(
            seen.windows(2).all(|pair| pair[0] == pair[1]),
            "case {case:?}: repeated reads must be byte-identical, got {seen:?}"
        );
    }
}
