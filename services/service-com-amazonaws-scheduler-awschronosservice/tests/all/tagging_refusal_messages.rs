// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the two refusals its tagging surface answers before
//! anything is read or written, and which requests it refuses at all.
//!
//! Two rules are under test.
//!
//! 1. **A tag key in the reserved namespace is refused as a modification of
//!    SYSTEM TAGS.** The keys under `aws:` belong to the service, so the refusal
//!    is an authorization statement about that namespace rather than a complaint
//!    about the key's syntax: 400 `ValidationException` with one FIXED sentence
//!    that names neither the key nor the prefix, identical for every reserved
//!    key and at every entry point that takes tags (`TagResource`,
//!    `UntagResource`, `CreateScheduleGroup`).
//! 2. **The `resourceArn` constraint the tagging operations enforce — and quote
//!    in their refusal — admits every AWS partition name**, multi-segment ones
//!    included. So a schedule-group ARN in such a partition gets PAST the
//!    boundary validator and is answered by the resource rule, while an ARN of
//!    any other shape is refused with that same expression quoted.
//!
//! Every request goes through the shared [`crate::wire_harness`], the real
//! restJson1 front door, so the URI/query binding, the boundary validator, the
//! handler and the error serialization are all exercised.

use std::collections::BTreeSet;

use smithy_server_runtime::MultiServiceRouter;

use crate::wire_harness::{
    account_id, call, encode_label, error_code, error_message, group_arn, list_tags,
    scheduler_router, tags_of, REGION,
};

/// The sentence a request that names a reserved tag key is refused with, as the
/// service words it. Spelled out here rather than read from the crate, so the
/// text is asserted against a literal and not against itself.
const SYSTEM_TAG_MESSAGE: &str = "Caller is an end user and not allowed to modify system tags.";

/// The expression the tagging operations publish for `resourceArn`: the
/// constraint they enforce, without the anchors a message never carries. Its
/// partition segment is `aws[a-z-]*`, which is what admits a partition name of
/// several segments.
const PUBLISHED_ARN_PATTERN: &str =
    "arn:aws[a-z-]*:scheduler:[a-z0-9\\-]+:\\d{12}:schedule-group\\/[0-9a-zA-Z-_.]+";

/// Create a schedule group and answer its ARN.
async fn create_group(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(router, "POST", &format!("/schedule-groups/{name}"), None).await;
    assert_eq!(
        status, 200,
        "group {name:?}: CreateScheduleGroup got {status} {body}"
    );
    body.get("ScheduleGroupArn")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("group {name:?}: no ScheduleGroupArn in {body}"))
        .to_string()
}

/// `POST /tags/{ResourceArn}` with one key/value pair.
async fn tag_one(
    router: &MultiServiceRouter,
    arn: &str,
    key: &str,
    value: &str,
) -> (u16, serde_json::Value) {
    call(
        router,
        "POST",
        &format!("/tags/{}", encode_label(arn)),
        Some(serde_json::json!({ "Tags": [{ "Key": key, "Value": value }] })),
    )
    .await
}

/// `DELETE /tags/{ResourceArn}?TagKeys=...` for one key.
async fn untag_one(router: &MultiServiceRouter, arn: &str, key: &str) -> (u16, serde_json::Value) {
    let uri = format!(
        "/tags/{}?TagKeys={}",
        encode_label(arn),
        encode_label(key)
    );
    call(router, "DELETE", &uri, None).await
}

/// `POST /schedule-groups/{Name}` carrying one tag.
async fn create_group_with_tag(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    value: &str,
) -> (u16, serde_json::Value) {
    call(
        router,
        "POST",
        &format!("/schedule-groups/{name}"),
        Some(serde_json::json!({ "Tags": [{ "Key": key, "Value": value }] })),
    )
    .await
}

/// Reserved keys spanning what the rule generalises over: the bare prefix, a
/// namespaced key AWS itself writes, each case of the prefix, a remainder made
/// of punctuation, and a key at the modelled maximum length (`TagKey` is
/// `length 1..128`, `smithy_query(query="shape",shape="TagMap")`). The rule's
/// subject is the NAMESPACE, so none of these differences may reach the text.
fn reserved_keys() -> Vec<(&'static str, String)> {
    vec![
        ("the bare prefix", "aws:".to_string()),
        (
            "a namespaced key the service writes",
            "aws:cloudformation:stack-name".to_string(),
        ),
        ("an upper-case prefix", "AWS:Owner".to_string()),
        ("a mixed-case prefix", "aWs:billing".to_string()),
        (
            "a remainder of punctuation only",
            "aws:._-/+=@".to_string(),
        ),
        (
            "a key at the modelled maximum length",
            format!("aws:{}", "k".repeat(124)),
        ),
    ]
}

#[tokio::test]
async fn a_reserved_tag_key_is_refused_with_one_fixed_system_tag_sentence() {
    let router = scheduler_router();
    let arn = create_group(&router, "system-tags").await;
    let (status, body) = tag_one(&router, &arn, "Environment", "production").await;
    assert_eq!(status, 200, "seeding a tag got {status} {body}");

    // Every (entry point, key) pair must answer the SAME sentence; collecting
    // them is what proves the invariance rather than six independent equalities.
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for (index, (case, key)) in reserved_keys().into_iter().enumerate() {
        let group = format!("system-tags-{index}");
        let answers = vec![
            ("TagResource", tag_one(&router, &arn, &key, "x").await),
            ("UntagResource", untag_one(&router, &arn, &key).await),
            (
                "CreateScheduleGroup",
                create_group_with_tag(&router, &group, &key, "x").await,
            ),
        ];

        for (operation, (status, body)) in answers {
            let message = error_message(&body);
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {case:?} ({operation}): a reserved key must be refused, got {status} {body}"
            );
            assert_eq!(
                message, SYSTEM_TAG_MESSAGE,
                "case {case:?} ({operation}): expected {SYSTEM_TAG_MESSAGE:?}, got {message:?}"
            );
            assert!(
                !message.contains(key.as_str()) && !message.contains("aws:"),
                "case {case:?} ({operation}): the refusal must quote neither the key \
                 nor the prefix, got {message:?}"
            );
            seen.insert(message);
        }

        // The refused CreateScheduleGroup created nothing, so its name is still
        // free: the refusal is about the request, not about state.
        let (status, body) = call(&router, "GET", &format!("/schedule-groups/{group}"), None).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a refused create must store nothing, got {status} {body}"
        );
    }

    assert_eq!(
        seen.len(),
        1,
        "the system-tag refusal must be one fixed sentence, got {seen:?}"
    );

    // And nothing the refusals named reached the tag set.
    let (status, body) = list_tags(&router, &arn).await;
    let listed = tags_of(&body);
    assert_eq!(
        (status, listed.len(), listed.get("Environment").map(String::as_str)),
        (200, 1, Some("production")),
        "a refused request must store nothing, got {status} {listed:?}"
    );
}

/// An ARN that is not a schedule group's, spanning the ways a value can fall
/// outside the published expression: the wrong resource type, the wrong service,
/// an account that is not twelve digits, a partition segment outside the
/// character class, a nested group name, and a value that is no ARN at all.
fn arns_outside_the_pattern() -> Vec<(&'static str, String)> {
    let account = account_id();
    vec![
        (
            "a schedule ARN, not a schedule group ARN",
            format!("arn:aws:scheduler:{REGION}:{account}:schedule/default/some-schedule"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:sqs:{REGION}:{account}:schedule-group/queue"),
        ),
        (
            "an account id that is not twelve digits",
            format!("arn:aws:scheduler:{REGION}:12345:schedule-group/present"),
        ),
        (
            "a partition segment outside the character class",
            format!("arn:AWS-GOV:scheduler:{REGION}:{account}:schedule-group/present"),
        ),
        (
            "a group name carrying a second path segment",
            format!("arn:aws:scheduler:{REGION}:{account}:schedule-group/outer/inner"),
        ),
        ("not an ARN at all", "present-group".to_string()),
    ]
}

#[tokio::test]
async fn an_arn_outside_the_published_pattern_is_refused_quoting_that_pattern() {
    let router = scheduler_router();

    for (case, arn) in arns_outside_the_pattern() {
        let expected = format!(
            "1 validation error detected: Value '{arn}' at 'resourceArn' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: {PUBLISHED_ARN_PATTERN}"
        );
        for (operation, (status, body)) in [
            ("TagResource", tag_one(&router, &arn, "k", "v").await),
            ("UntagResource", untag_one(&router, &arn, "k").await),
            ("ListTagsForResource", list_tags(&router, &arn).await),
        ] {
            let message = error_message(&body);
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {case:?} ({operation}): expected a validation refusal, got {status} {body}"
            );
            assert_eq!(
                message, expected,
                "case {case:?} ({operation}): expected {expected:?}, got {message:?}"
            );
        }
    }
}

/// Partition names a schedule-group ARN can carry. Source: the partitions AWS's
/// ARN reference names (`aws`, `aws-cn`, `aws-us-gov`) together with the
/// isolated ones (`aws-iso`, `aws-iso-b`, `aws-iso-e`, `aws-iso-f`), so the rows
/// span both a single-segment and a multi-segment suffix.
const PARTITIONS: &[&str] = &[
    "aws-cn",
    "aws-us-gov",
    "aws-iso",
    "aws-iso-b",
    "aws-iso-e",
    "aws-iso-f",
];

#[tokio::test]
async fn a_group_arn_in_any_partition_gets_past_the_boundary_validator() {
    let router = scheduler_router();
    let name = "partition-group";
    let mine = create_group(&router, name).await;
    let (status, body) = tag_one(&router, &mine, "Environment", "production").await;
    assert_eq!(status, 200, "seeding a tag got {status} {body}");

    // This endpoint serves ONE partition, so an ARN naming another addresses no
    // group reachable here — but it is a well-formed `resourceArn`, so the
    // answer is the resource rule's 404, never the pattern refusal.
    for partition in PARTITIONS {
        let arn = format!(
            "arn:{partition}:scheduler:{REGION}:{}:schedule-group/{name}",
            account_id()
        );
        for (operation, (status, body)) in [
            ("TagResource", tag_one(&router, &arn, "k", "v").await),
            ("UntagResource", untag_one(&router, &arn, "k").await),
            ("ListTagsForResource", list_tags(&router, &arn).await),
        ] {
            let message = error_message(&body);
            assert_eq!(
                (status, error_code(&body).as_str()),
                (404, "ResourceNotFoundException"),
                "case {partition:?} ({operation}): a well-formed ARN in another partition \
                 must reach the resource rule, got {status} {body}"
            );
            assert!(
                !message.contains("regular expression pattern"),
                "case {partition:?} ({operation}): the ARN satisfies the enforced pattern, \
                 so no pattern clause may be reported, got {message:?}"
            );
        }
    }

    // The ARN this endpoint does mint is still served, and the refusals above
    // left its tag set alone.
    assert_eq!(
        mine,
        group_arn(REGION, name),
        "the created group's ARN must be the one this endpoint mints, got {mine:?}"
    );
    let (status, body) = list_tags(&router, &mine).await;
    let listed = tags_of(&body);
    assert_eq!(
        (status, listed.get("Environment").map(String::as_str)),
        (200, Some("production")),
        "case \"this endpoint's own partition\": expected the stored tag, got {status} {listed:?}"
    );
}
