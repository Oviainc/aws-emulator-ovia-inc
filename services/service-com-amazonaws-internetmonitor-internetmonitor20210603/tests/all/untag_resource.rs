// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UntagResource`.
//!
//! The rule under test: a tag removal takes exactly the keys the request names off the
//! stored tag map, leaves every other key as it is, removes nothing for a key the
//! resource does not carry (the operation is `@idempotent` in the model), and refuses an
//! ARN no resource of this service carries with the modelled `NotFoundException` (404).
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator binary
//! mounts, and reads the result back through `ListTagsForResource` and `GetMonitor`, so a
//! removal is only believed once a later reader reports it.
//!
//! The harness (`emulator`, `call`, `label`, `create_monitor`, `expected_arn`) lives in
//! `crate::monitor_lifecycle` and is reused here rather than copied.

use serde_json::{json, Value};

use crate::monitor_lifecycle::{call, create_monitor, emulator, expected_arn, label, REGION};

/// `DELETE /tags/{ResourceArn}?tagKeys=…`, the way an SDK writes this operation on the
/// wire: the ARN percent-encoded into the `@httpLabel`, and `TagKeys` as the repeated
/// `tagKeys` query parameter its `@httpQuery` trait names
/// (`smithy_query(query="operation",operation="UntagResource")`).
fn untag_path<K: AsRef<str>>(arn: &str, keys: &[K]) -> String {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", label(key.as_ref())))
        .collect::<Vec<_>>()
        .join("&");
    format!("/tags/{}?{query}", label(arn))
}

/// `GET /tags/{ResourceArn}`.
fn tags_path(arn: &str) -> String {
    format!("/tags/{}", label(arn))
}

/// One removal: the tags the monitor starts with, the keys the call removes, and the map
/// that must remain.
struct UntagCase {
    /// Names the row in every assertion message.
    case: &'static str,
    /// The `MonitorName` the row creates, so each row has its own monitor.
    name: &'static str,
    /// The `Tags` the monitor is created with.
    tags: Value,
    /// The `tagKeys` the removal names, in the order the request sends them.
    remove: Vec<String>,
    /// The whole tag map that must remain afterwards.
    remaining: Value,
}

/// `count` distinct tags, for the rows about the number of keys in one call.
fn tag_map(prefix: &str, count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|index| (format!("{prefix}-{index:04}"), json!("v")))
            .collect(),
    )
}

/// The rows span what the rule generalises over: how many keys one call names, whether a
/// named key is present, the order the keys arrive in, and the characters a key is spelled
/// with. Tag keys here use only characters AWS documents for a tag key (letters, numbers,
/// spaces and `+ - = . _ : / @`).
fn untag_cases() -> Vec<UntagCase> {
    let many = tag_map("key", 20);
    let many_removed: Vec<String> = (0..10).map(|index| format!("key-{index:04}")).collect();
    let many_remaining = Value::Object(
        (10..20)
            .map(|index| (format!("key-{index:04}"), json!("v")))
            .collect(),
    );

    vec![
        UntagCase {
            case: "one key of several",
            name: "untag-one",
            tags: json!({ "Env": "dev", "Team": "net" }),
            remove: vec!["Env".to_string()],
            remaining: json!({ "Team": "net" }),
        },
        UntagCase {
            case: "the only key the monitor carries",
            name: "untag-only",
            tags: json!({ "Env": "dev" }),
            remove: vec!["Env".to_string()],
            remaining: json!({}),
        },
        UntagCase {
            case: "two keys of four, in one call",
            name: "untag-two-of-four",
            tags: json!({ "Env": "dev", "Owner": "sre", "Team": "net", "Tier": "1" }),
            remove: vec!["Env".to_string(), "Owner".to_string()],
            remaining: json!({ "Team": "net", "Tier": "1" }),
        },
        UntagCase {
            case: "every key the monitor carries",
            name: "untag-all",
            tags: json!({ "Env": "dev", "Team": "net" }),
            remove: vec!["Env".to_string(), "Team".to_string()],
            remaining: json!({}),
        },
        UntagCase {
            case: "a key the monitor does not carry",
            name: "untag-absent",
            tags: json!({ "Env": "dev" }),
            remove: vec!["Ghost".to_string()],
            remaining: json!({ "Env": "dev" }),
        },
        UntagCase {
            case: "a present key beside an absent one",
            name: "untag-mixed",
            tags: json!({ "Env": "dev", "Team": "net" }),
            remove: vec!["Ghost".to_string(), "Env".to_string()],
            remaining: json!({ "Team": "net" }),
        },
        UntagCase {
            case: "the keys in the reverse of the map's order",
            name: "untag-reversed",
            tags: json!({ "a": "1", "b": "2", "c": "3" }),
            remove: vec!["c".to_string(), "a".to_string()],
            remaining: json!({ "b": "2" }),
        },
        UntagCase {
            case: "keys whose characters need percent-encoding in the query string",
            name: "untag-encoded",
            tags: json!({
                "project/sub:1": "x",
                "owner@example.com": "y",
                "two words": "z",
                "keep": "me",
            }),
            remove: vec![
                "project/sub:1".to_string(),
                "owner@example.com".to_string(),
                "two words".to_string(),
            ],
            remaining: json!({ "keep": "me" }),
        },
        UntagCase {
            case: "ten keys of twenty, in one call",
            name: "untag-many",
            tags: many,
            remove: many_removed,
            remaining: many_remaining,
        },
    ]
}

#[tokio::test]
async fn untag_resource_removes_exactly_the_keys_it_names() {
    let router = emulator();

    for case in untag_cases() {
        let arn = create_monitor(
            &router,
            json!({ "MonitorName": case.name, "Tags": case.tags }),
        )
        .await;

        let (status, response) =
            call(&router, "DELETE", &untag_path(&arn, &case.remove), None).await;
        assert_eq!(
            status, 204,
            "case {}: UntagResource expected 204, got {status} with {response}",
            case.case
        );

        let (status, listed) = call(&router, "GET", &tags_path(&arn), None).await;
        assert_eq!(
            status, 200,
            "case {}: ListTagsForResource expected 200, got {status} with {listed}",
            case.case
        );
        assert_eq!(
            listed["Tags"], case.remaining,
            "case {}: unexpected tag map after removing {:?}, got {listed}",
            case.case, case.remove
        );

        // The same map, through the monitor itself: one stored map, two readers.
        let (status, got) = call(
            &router,
            "GET",
            &format!("/v20210603/Monitors/{}", case.name),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {}: GetMonitor expected 200, got {status} with {got}",
            case.case
        );
        assert_eq!(
            got["Tags"], case.remaining,
            "case {}: GetMonitor must report the same tag map, got {got}",
            case.case
        );
    }
}

#[tokio::test]
async fn repeating_a_removal_answers_the_same_success_and_changes_nothing() {
    let router = emulator();
    let arn = create_monitor(
        &router,
        json!({ "MonitorName": "idempotent-untag", "Tags": { "Env": "dev", "Team": "net" } }),
    )
    .await;
    let path = untag_path(&arn, &["Env", "Ghost"]);

    // The model marks `UntagResource` `@idempotent`, so every repeat of one request is
    // the same 204 over the same resulting map.
    for attempt in 1..=3 {
        let (status, response) = call(&router, "DELETE", &path, None).await;
        assert_eq!(
            status, 204,
            "attempt {attempt}: expected 204, got {status} with {response}"
        );

        let (_, listed) = call(&router, "GET", &tags_path(&arn), None).await;
        assert_eq!(
            listed["Tags"],
            json!({ "Team": "net" }),
            "attempt {attempt}: unexpected tag map, got {listed}"
        );
    }
}

#[tokio::test]
async fn untag_resource_refuses_an_arn_no_monitor_carries() {
    let router = emulator();
    create_monitor(&router, json!({ "MonitorName": "real-monitor" })).await;
    let doomed = create_monitor(&router, json!({ "MonitorName": "doomed-monitor" })).await;
    let (status, deleted) = call(
        &router,
        "DELETE",
        "/v20210603/Monitors/doomed-monitor",
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteMonitor expected 200, got {status} with {deleted}"
    );

    // Each row satisfies the modelled constraints on `ResourceArn` (len 20..512, ^arn:.*)
    // but names no monitor this emulator holds.
    let cases: Vec<(&str, String)> = vec![
        (
            "a monitor name that does not exist",
            expected_arn(REGION, "no-such-monitor"),
        ),
        (
            "the right name in another region",
            expected_arn("eu-west-1", "real-monitor"),
        ),
        (
            "another service's resource",
            "arn:aws:ec2:us-east-1:000000000000:vpc/vpc-aaaa0001".to_string(),
        ),
        ("a monitor that was deleted", doomed),
    ];

    for (case, arn) in cases {
        let (status, response) = call(&router, "DELETE", &untag_path(&arn, &["Env"]), None).await;
        assert_eq!(
            status, 404,
            "case {case}: UntagResource expected 404, got {status} with {response}"
        );
        assert_eq!(
            response["message"],
            json!(format!("No resource found with the ARN {arn}.")),
            "case {case}: expected a NotFoundException naming the ARN, got {response}"
        );
    }
}

#[tokio::test]
async fn removing_a_tag_leaves_the_monitors_configuration_alone_and_a_retag_restores_it() {
    let router = emulator();
    let arn = create_monitor(
        &router,
        json!({
            "MonitorName": "configured-monitor",
            "Tags": { "Env": "dev", "Team": "net" },
            "Resources": ["arn:aws:ec2:us-east-1:000000000000:vpc/vpc-aaaa0001"],
            "MaxCityNetworksToMonitor": 100,
        }),
    )
    .await;
    let (_, before) = call(
        &router,
        "GET",
        "/v20210603/Monitors/configured-monitor",
        None,
    )
    .await;

    let (status, response) = call(&router, "DELETE", &untag_path(&arn, &["Env"]), None).await;
    assert_eq!(
        status, 204,
        "UntagResource expected 204, got {status} with {response}"
    );

    let (_, after) = call(
        &router,
        "GET",
        "/v20210603/Monitors/configured-monitor",
        None,
    )
    .await;
    for member in ["ModifiedAt", "CreatedAt", "Resources", "MaxCityNetworksToMonitor"] {
        assert_eq!(
            after[member], before[member],
            "member {member}: removing a tag must not change it, got {after}"
        );
    }

    // And the map is a map, not a one-way door: the key goes back with `TagResource`.
    let (status, tagged) = call(
        &router,
        "POST",
        &tags_path(&arn),
        Some(json!({ "Tags": { "Env": "prod" } })),
    )
    .await;
    assert_eq!(
        status, 204,
        "TagResource expected 204, got {status} with {tagged}"
    );
    let (_, listed) = call(&router, "GET", &tags_path(&arn), None).await;
    assert_eq!(
        listed["Tags"],
        json!({ "Env": "prod", "Team": "net" }),
        "a re-tagged key must be back in the map, got {listed}"
    );
}
