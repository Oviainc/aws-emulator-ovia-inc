// SPDX-License-Identifier: CC0-1.0

//! Removing tags from on-premises instances, and what the single-instance read
//! answers once a registration is gone — driven over the wire.
//!
//! RULES under test:
//!
//! * a mutation is observable through the reader: the tag pairs a
//!   `RemoveTagsFromOnPremisesInstances` call names are dropped from every
//!   registration it resolves, whatever character set the key or the value is
//!   written in, and `GetOnPremisesInstance` afterwards reports exactly the
//!   remaining pairs (`[]` once none are left);
//! * `GetOnPremisesInstance` reports an instance only while it holds a LIVE
//!   registration: a name whose registration is gone is refused with HTTP 400
//!   `InstanceNotRegisteredException`, while the retired record is still
//!   reported by the batch read.
//!
//! Every case goes through the same front door a client uses — the shared
//! [`Emulator`] harness of `codedeploy_lifecycle` — so the boundary validator,
//! this service's refusal wording and the awsJson1_1 envelope are exercised.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, Emulator};

/// One row of the removal table: the case name, the `(key, value)` tags the
/// instance is given, the pairs the removal call names, and the pairs the
/// instance must report afterwards.
type RemovalCase<'a> = (
    &'a str,
    &'a [(&'a str, &'a str)],
    &'a [(&'a str, &'a str)],
    &'a [(&'a str, &'a str)],
);

/// An IAM user ARN for `name`: `arn:aws:iam::<account>:user/<name>`.
fn user_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:user/{name}", engine::emulator_account_id())
}

/// Register `name` against an IAM user ARN built from it.
async fn register(emulator: &Emulator, name: &str) {
    emulator
        .ok(
            "RegisterOnPremisesInstance",
            json!({"instanceName": name, "iamUserArn": user_arn(name)}),
        )
        .await;
}

/// A `tags` list entry, as CodeDeploy's `Tag` shape names its members
/// (`smithy_query(query="shape",shape="Tag")`: `Key`, `Value`).
fn tag(key: &str, value: &str) -> Value {
    json!({"Key": key, "Value": value})
}

/// `tags` as the wire list of `Tag` structures.
fn tag_list(tags: &[(&str, &str)]) -> Value {
    Value::Array(tags.iter().map(|(key, value)| tag(key, value)).collect())
}

/// `GetOnPremisesInstance`'s `instanceInfo` for `name`, requiring the read to
/// succeed — the round trip every removal case is checked through.
async fn instance_info(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok("GetOnPremisesInstance", json!({"instanceName": name}))
        .await["instanceInfo"]
        .clone()
}

/// The `(key, value)` pairs of an `instanceInfo`'s tags, in report order.
fn tags_of(info: &Value) -> Vec<(String, String)> {
    info["tags"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                entry["Key"].as_str().unwrap_or_default().to_string(),
                entry["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// `tags` as owned pairs, for comparison against what a read reported.
fn owned(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// RULE: a removal call drops exactly the key-value pairs it names from the
/// instance, and the read afterwards reports the pairs that are left.
///
/// Rows differ along what the rule generalises over: how many tags the instance
/// holds and how many of them are removed, the character set the key and the
/// value are written in (ASCII, Latin-1 accents, CJK, an emoji), an empty value,
/// and a pair that names no tag of the instance (an unheld key, and a held key
/// with another value) — which removes nothing.
#[tokio::test]
async fn removing_a_tag_pair_drops_it_from_the_stored_instance() {
    let emulator = Emulator::new();
    let cases: &[RemovalCase<'_>] = &[
        (
            "the only tag",
            &[("Team", "probe")],
            &[("Team", "probe")],
            &[],
        ),
        (
            "one of three",
            &[("Name", "web"), ("Env", "prod"), ("Tier", "api")],
            &[("Env", "prod")],
            &[("Name", "web"), ("Tier", "api")],
        ),
        (
            "two of three in one call",
            &[("Name", "web"), ("Env", "prod"), ("Tier", "api")],
            &[("Name", "web"), ("Tier", "api")],
            &[("Env", "prod")],
        ),
        (
            "a CJK key and value",
            &[("\u{94a5}unicode-key", "\u{503c}value-value")],
            &[("\u{94a5}unicode-key", "\u{503c}value-value")],
            &[],
        ),
        (
            "accented and CJK pairs beside an ASCII one",
            &[
                ("caf\u{e9}-key", "na\u{ef}ve-\u{30c6}\u{30b9}\u{30c8}"),
                ("ascii-key", "ascii-value"),
            ],
            &[("caf\u{e9}-key", "na\u{ef}ve-\u{30c6}\u{30b9}\u{30c8}")],
            &[("ascii-key", "ascii-value")],
        ),
        (
            "an emoji key",
            &[("\u{1f680}deploy", "\u{1f7e2}green"), ("Env", "prod")],
            &[("\u{1f680}deploy", "\u{1f7e2}green")],
            &[("Env", "prod")],
        ),
        (
            "an empty value",
            &[("Marker", ""), ("Env", "prod")],
            &[("Marker", "")],
            &[("Env", "prod")],
        ),
        (
            "a key the instance does not carry",
            &[("Env", "prod")],
            &[("Absent", "x")],
            &[("Env", "prod")],
        ),
        (
            "a held key with another value",
            &[("Env", "prod")],
            &[("Env", "dev")],
            &[("Env", "prod")],
        ),
        (
            "every tag at once",
            &[("Name", "web"), ("Env", "prod")],
            &[("Name", "web"), ("Env", "prod")],
            &[],
        ),
    ];

    for (index, (case, given, removed, expected)) in cases.iter().enumerate() {
        let name = format!("removal-{index:02}");
        register(&emulator, &name).await;
        emulator
            .ok(
                "AddTagsToOnPremisesInstances",
                json!({"instanceNames": [&name], "tags": tag_list(given)}),
            )
            .await;

        let (status, body) = emulator
            .call(
                "RemoveTagsFromOnPremisesInstances",
                json!({"instanceNames": [&name], "tags": tag_list(removed)}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: RemoveTagsFromOnPremisesInstances must succeed, got {status} {body:?}"
        );

        let info = instance_info(&emulator, &name).await;
        let actual = tags_of(&info);
        assert_eq!(
            actual,
            owned(expected),
            "case {case:?}: expected the remaining tags {:?}, got {actual:?}",
            owned(expected)
        );
        // The member is always there, empty included, so a client can index it.
        assert!(
            info["tags"].is_array(),
            "case {case:?}: expected a tags list, got {info:?}"
        );
        // Removing the same pairs again is a no-op, not a failure: the instance
        // is already in the state the caller asked for.
        emulator
            .ok(
                "RemoveTagsFromOnPremisesInstances",
                json!({"instanceNames": [&name], "tags": tag_list(removed)}),
            )
            .await;
        let again = tags_of(&instance_info(&emulator, &name).await);
        assert_eq!(
            again, actual,
            "case {case:?}: a repeated removal changes nothing, got {again:?}"
        );
    }
}

/// RULE: one call removes the pairs it names from EVERY registration it names,
/// and only from those.
#[tokio::test]
async fn one_call_removes_from_every_instance_it_names() {
    let emulator = Emulator::new();
    let named = ["fleet-a", "fleet-b"];
    for name in named.iter().chain(["fleet-c"].iter()) {
        register(&emulator, name).await;
        emulator
            .ok(
                "AddTagsToOnPremisesInstances",
                json!({
                    "instanceNames": [name],
                    "tags": tag_list(&[("Fleet", "blue"), ("Env", "prod")]),
                }),
            )
            .await;
    }

    emulator
        .ok(
            "RemoveTagsFromOnPremisesInstances",
            json!({"instanceNames": named, "tags": tag_list(&[("Fleet", "blue")])}),
        )
        .await;

    for name in named {
        let actual = tags_of(&instance_info(&emulator, name).await);
        assert_eq!(
            actual,
            owned(&[("Env", "prod")]),
            "case {name:?}: the named pair is gone from every named instance, got {actual:?}"
        );
    }
    let untouched = tags_of(&instance_info(&emulator, "fleet-c").await);
    assert_eq!(
        untouched,
        owned(&[("Fleet", "blue"), ("Env", "prod")]),
        "case \"not named\": an instance the call did not name keeps its tags, got {untouched:?}"
    );
}

/// RULE: each member of a removal request is refused with the code CodeDeploy
/// models for THAT member, and a refused call removes nothing — so the two
/// tagging operations refuse the same request the same way.
#[tokio::test]
async fn a_tag_removal_request_is_refused_per_member() {
    let emulator = Emulator::new();
    register(&emulator, "tagged").await;
    emulator
        .ok(
            "AddTagsToOnPremisesInstances",
            json!({"instanceNames": ["tagged"], "tags": tag_list(&[("Env", "prod")])}),
        )
        .await;

    let at_bound: Vec<String> = (0..100).map(|index| format!("bulk-{index}")).collect();
    let past_bound: Vec<String> = (0..101).map(|index| format!("bulk-{index}")).collect();
    let name_at_bound = "n".repeat(100);
    let name_past_bound = "n".repeat(101);
    let tags_past_bound: Vec<Value> = (0..51)
        .map(|index| tag(&format!("Key{index}"), "v"))
        .collect();
    let good_tags = json!([tag("Env", "prod")]);

    // Rows differ along which member is wrong and how. The name length, the
    // names-per-call count and the tags-per-call count are each exercised AT the
    // bound this service enforces and one past it; a well-formed name that holds
    // no registration reaches the registry and is refused there.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "no instance names",
            json!({"instanceNames": [], "tags": good_tags}),
            "InstanceNameRequiredException",
        ),
        (
            "an empty instance name",
            json!({"instanceNames": [""], "tags": good_tags}),
            "InstanceNameRequiredException",
        ),
        (
            "an instance name with a space",
            json!({"instanceNames": ["not a name"], "tags": good_tags}),
            "InvalidInstanceNameException",
        ),
        (
            "an instance name at the length bound",
            json!({"instanceNames": [name_at_bound], "tags": good_tags}),
            "InstanceNotRegisteredException",
        ),
        (
            "an instance name past the length bound",
            json!({"instanceNames": [name_past_bound], "tags": good_tags}),
            "InvalidInstanceNameException",
        ),
        (
            "as many names as one call allows",
            json!({"instanceNames": at_bound, "tags": good_tags}),
            "InstanceNotRegisteredException",
        ),
        (
            "one name more than a call allows",
            json!({"instanceNames": past_bound, "tags": good_tags}),
            "InstanceLimitExceededException",
        ),
        (
            "an unregistered name beside a registered one",
            json!({"instanceNames": ["tagged", "never-registered"], "tags": good_tags}),
            "InstanceNotRegisteredException",
        ),
        (
            "no tags",
            json!({"instanceNames": ["tagged"], "tags": []}),
            "TagRequiredException",
        ),
        (
            "a tag with neither key nor value",
            json!({"instanceNames": ["tagged"], "tags": [{}]}),
            "InvalidTagException",
        ),
        (
            "a value-only tag",
            json!({"instanceNames": ["tagged"], "tags": [{"Value": "prod"}]}),
            "InvalidTagException",
        ),
        (
            "a key-only tag",
            json!({"instanceNames": ["tagged"], "tags": [{"Key": "Env"}]}),
            "InvalidTagException",
        ),
        (
            "a tag with an empty key",
            json!({"instanceNames": ["tagged"], "tags": [{"Key": "", "Value": "prod"}]}),
            "InvalidTagException",
        ),
        (
            "more tags in one call than an instance may carry",
            json!({"instanceNames": ["tagged"], "tags": tags_past_bound}),
            "TagLimitExceededException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, body) = emulator
            .call("RemoveTagsFromOnPremisesInstances", request)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: every one of these is a 400 client error, got {status} {body:?}"
        );
        let actual = error_code(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected}, got {actual} ({body:?})"
        );
    }

    // None of the refusals above removed the one tag the registered instance
    // carries.
    let actual = tags_of(&instance_info(&emulator, "tagged").await);
    assert_eq!(
        actual,
        owned(&[("Env", "prod")]),
        "a refused request removes nothing, got {actual:?}"
    );
}

/// RULE: `GetOnPremisesInstance` reports an instance only while its
/// registration is live — a name with no live registration is refused with HTTP
/// 400 `InstanceNotRegisteredException` — while the retired RECORD is still
/// reported by the batch read, and registering the name again makes it readable.
#[tokio::test]
async fn a_name_with_no_live_registration_is_refused_by_the_single_instance_read() {
    let emulator = Emulator::new();
    register(&emulator, "retired-once").await;
    register(&emulator, "retired-twice").await;
    register(&emulator, "readopted").await;
    for name in ["retired-once", "retired-twice", "readopted"] {
        emulator
            .ok(
                "DeregisterOnPremisesInstance",
                json!({"instanceName": name}),
            )
            .await;
    }
    // Deregistering an already retired name stays a success and leaves it
    // retired.
    emulator
        .ok(
            "DeregisterOnPremisesInstance",
            json!({"instanceName": "retired-twice"}),
        )
        .await;
    // A name can be taken up again, which makes it readable once more.
    register(&emulator, "readopted").await;

    // Rows differ along HOW the name came to hold (or not hold) a registration:
    // never registered, deregistered once, deregistered twice, deregistered and
    // registered again.
    let cases: &[(&str, &str, bool)] = &[
        ("never registered", "no-such-instance", false),
        ("deregistered", "retired-once", false),
        ("deregistered twice", "retired-twice", false),
        ("registered again", "readopted", true),
    ];

    for (case, name, readable) in cases {
        let (status, body) = emulator
            .call("GetOnPremisesInstance", json!({"instanceName": name}))
            .await;
        if *readable {
            assert_eq!(
                status, 200,
                "case {case:?}: a live registration is reported, got {status} {body:?}"
            );
            assert!(
                body["instanceInfo"]["deregisterTime"].is_null(),
                "case {case:?}: a live registration carries no deregisterTime, got {body:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a name with no live registration is refused, got {status} {body:?}"
            );
            let actual = error_code(&body);
            assert_eq!(
                actual, "InstanceNotRegisteredException",
                "case {case:?}: expected InstanceNotRegisteredException, got {actual} ({body:?})"
            );
            assert!(
                body["instanceInfo"].is_null(),
                "case {case:?}: a refusal carries no instanceInfo, got {body:?}"
            );
        }
    }

    // The retired record itself is kept: the batch read reports it, with the
    // instant it was retired at.
    let batched = emulator
        .ok(
            "BatchGetOnPremisesInstances",
            json!({"instanceNames": ["retired-once"]}),
        )
        .await;
    let info = &batched["instanceInfos"][0];
    assert!(
        info["deregisterTime"].as_f64().unwrap_or_default() > 0.0,
        "case \"the retired record\": expected a deregisterTime, got {info:?}"
    );

    // Tagging either way needs a live registration, so both tagging operations
    // refuse a retired name with the same code.
    for operation in [
        "AddTagsToOnPremisesInstances",
        "RemoveTagsFromOnPremisesInstances",
    ] {
        let (status, body) = emulator
            .call(
                operation,
                json!({"instanceNames": ["retired-once"], "tags": tag_list(&[("Env", "prod")])}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {operation:?}: a retired name cannot be tagged, got {status} {body:?}"
        );
        let actual = error_code(&body);
        assert_eq!(
            actual, "InstanceNotRegisteredException",
            "case {operation:?}: expected InstanceNotRegisteredException, got {actual} ({body:?})"
        );
    }
}
