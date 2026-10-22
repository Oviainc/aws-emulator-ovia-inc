// SPDX-License-Identifier: CC0-1.0

//! Tagging on-premises instances, driven over the wire.
//!
//! RULE under test: `AddTagsToOnPremisesInstances` resolves every name in the
//! request against the on-premises registry and stores the request's key/value
//! tags on each resolved registration — a key the instance already carries has
//! its value REPLACED rather than duplicated — so a later
//! `GetOnPremisesInstance` reports exactly those tags; a request whose shape or
//! whose names the service does not accept is refused with HTTP 400 and the
//! modelled error for that member, BEFORE anything is written.
//!
//! Every case goes through the same front door a client uses — the shared
//! [`Emulator`] harness of `codedeploy_lifecycle` — so the boundary validator,
//! this service's refusal wording and the awsJson1_1 envelope are exercised.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, Emulator};

/// One row of the tagging table: the case name, the instance names it registers
/// and tags, and the `(key, value)` tags it adds to each of them.
type TagCase<'a> = (&'a str, &'a [&'a str], &'a [(&'a str, &'a str)]);

/// One row of the re-tagging table: the case name, the `(key, value)` tags the
/// call adds, and the tags the instance must report afterwards.
type RetagCase<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a [(&'a str, &'a str)]);

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

/// `GetOnPremisesInstance`'s `instanceInfo` for `name`, requiring the read to
/// succeed — the round trip every tagging case is checked through.
async fn instance_info(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok("GetOnPremisesInstance", json!({"instanceName": name}))
        .await["instanceInfo"]
        .clone()
}

/// A `tags` list entry, as CodeDeploy's `Tag` shape names its members
/// (`smithy_query(query="shape",shape="Tag")`: `Key`, `Value`).
fn tag(key: &str, value: &str) -> Value {
    json!({"Key": key, "Value": value})
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

/// RULE: the tags a call adds are stored on every registration it names, and
/// `GetOnPremisesInstance` reports exactly those tags back — whatever the number
/// of tags, the number of instances named, or the tag's value.
#[tokio::test]
async fn tags_added_to_a_registration_read_back_from_it() {
    let emulator = Emulator::new();
    // Rows differ along the tag count, the instance count in one call, and the
    // tag values (ordinary, empty — the model forbids an empty KEY only — and
    // one carrying punctuation and spaces).
    let cases: &[TagCase<'_>] = &[
        ("one tag", &["single-tagged"], &[("Name", "web-01")]),
        (
            "three tags",
            &["triple-tagged"],
            &[("Name", "web-02"), ("Env", "prod"), ("Tier", "frontend")],
        ),
        (
            "empty tag value",
            &["empty-valued"],
            &[("Marker", ""), ("Env", "staging")],
        ),
        (
            "two instances in one call",
            &["fleet-a", "fleet-b"],
            &[("Fleet", "blue")],
        ),
        (
            "value with punctuation and spaces",
            &["punctuated"],
            &[("Owner", "team platform/eu, on-call")],
        ),
    ];

    for (case, names, tags) in cases {
        for name in *names {
            register(&emulator, name).await;
        }
        let request = json!({
            "instanceNames": names,
            "tags": tags.iter().map(|(k, v)| tag(k, v)).collect::<Vec<_>>(),
        });
        let (status, body) = emulator
            .call("AddTagsToOnPremisesInstances", request)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: AddTagsToOnPremisesInstances must succeed, got {body:?}"
        );

        let expected: Vec<(String, String)> = tags
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        for name in *names {
            let info = instance_info(&emulator, name).await;
            assert_eq!(
                tags_of(&info),
                expected,
                "case {case:?}: instance {name:?} must report the tags it was given, got {info:?}"
            );
            assert_eq!(
                info["instanceName"].as_str(),
                Some(*name),
                "case {case:?}: the instance reports its own name, got {info:?}"
            );
            assert_eq!(
                info["iamUserArn"].as_str(),
                Some(user_arn(name).as_str()),
                "case {case:?}: tagging leaves the registration's identity alone, got {info:?}"
            );
        }
    }
}

/// RULE: a tag whose key an instance already carries REPLACES that key's value;
/// it does not add a second entry for the key.
#[tokio::test]
async fn re_tagging_a_key_replaces_its_value() {
    let emulator = Emulator::new();
    register(&emulator, "retagged").await;
    // Rows are successive calls on ONE instance: a first tagging, a replacement
    // of one key, an added key, and a replacement by a tag carrying an empty
    // value.
    let cases: &[RetagCase<'_>] = &[
        (
            "first tagging",
            &[("Env", "dev"), ("Owner", "ana")],
            &[("Env", "dev"), ("Owner", "ana")],
        ),
        (
            "replace one key",
            &[("Env", "prod")],
            &[("Env", "prod"), ("Owner", "ana")],
        ),
        (
            "add a key",
            &[("Tier", "api")],
            &[("Env", "prod"), ("Owner", "ana"), ("Tier", "api")],
        ),
        (
            "replace with an empty value",
            &[("Owner", "")],
            &[("Env", "prod"), ("Owner", ""), ("Tier", "api")],
        ),
    ];

    for (case, added, expected) in cases {
        emulator
            .ok(
                "AddTagsToOnPremisesInstances",
                json!({
                    "instanceNames": ["retagged"],
                    "tags": added.iter().map(|(k, v)| tag(k, v)).collect::<Vec<_>>(),
                }),
            )
            .await;
        let info = instance_info(&emulator, "retagged").await;
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        assert_eq!(
            tags_of(&info),
            expected,
            "case {case:?}: a key is replaced, never duplicated, got {info:?}"
        );
    }
}

/// RULE: tagging a name the registry does not hold as a REGISTERED instance is
/// refused with 400 `InstanceNotRegisteredException`, and nothing in the call is
/// written — so a call naming one registered and one unknown instance leaves the
/// registered one untagged.
#[tokio::test]
async fn tagging_an_unregistered_instance_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    register(&emulator, "already-there").await;
    // Rows differ along WHERE the unknown name sits in the request: alone, and
    // before or after a registered name.
    let cases: &[(&str, &[&str])] = &[
        ("only an unknown name", &["never-registered-name"]),
        ("unknown name first", &["unknown-first", "already-there"]),
        ("unknown name last", &["already-there", "unknown-last"]),
        ("two unknown names", &["unknown-one", "unknown-two"]),
    ];

    for (case, names) in cases {
        let (status, body) = emulator
            .call(
                "AddTagsToOnPremisesInstances",
                json!({"instanceNames": names, "tags": [tag("Env", "prod")]}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an unregistered instance is a client error, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InstanceNotRegisteredException",
            "case {case:?}: expected InstanceNotRegisteredException, got {body:?}"
        );
        let info = instance_info(&emulator, "already-there").await;
        assert!(
            tags_of(&info).is_empty(),
            "case {case:?}: a refused call tags no instance, got {info:?}"
        );
    }
}

/// RULE: each member of a tagging request is refused with the code CodeDeploy
/// models for THAT member — a missing name, a malformed name, too many names, a
/// missing tag, a malformed tag and too many tags each have their own code — and
/// a request that clears every shape gate reaches the registry (so a well-formed
/// but unknown name reports `InstanceNotRegisteredException`).
#[tokio::test]
async fn a_tagging_request_is_refused_per_member() {
    let emulator = Emulator::new();
    register(&emulator, "tagme").await;

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
    // bound and one past it.
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
            "a blank instance name",
            json!({"instanceNames": ["   "], "tags": good_tags}),
            "InstanceNameRequiredException",
        ),
        (
            "an instance name with a space",
            json!({"instanceNames": ["not a name"], "tags": good_tags}),
            "InvalidInstanceNameException",
        ),
        (
            "an instance name with a reserved character",
            json!({"instanceNames": ["name:with:colons"], "tags": good_tags}),
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
            "no tags",
            json!({"instanceNames": ["tagme"], "tags": []}),
            "TagRequiredException",
        ),
        (
            "a tag with neither key nor value",
            json!({"instanceNames": ["tagme"], "tags": [{}]}),
            "InvalidTagException",
        ),
        (
            "a value-only tag",
            json!({"instanceNames": ["tagme"], "tags": [{"Value": "prod"}]}),
            "InvalidTagException",
        ),
        (
            "a tag with an empty key",
            json!({"instanceNames": ["tagme"], "tags": [{"Key": "", "Value": "prod"}]}),
            "InvalidTagException",
        ),
        (
            "a key-only tag",
            json!({"instanceNames": ["tagme"], "tags": [{"Key": "Env"}]}),
            "InvalidTagException",
        ),
        (
            "more tags in one call than an instance may carry",
            json!({"instanceNames": ["tagme"], "tags": tags_past_bound}),
            "TagLimitExceededException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, body) = emulator
            .call("AddTagsToOnPremisesInstances", request)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: every one of these is a 400 client error, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body:?}"
        );
    }

    // None of the refusals above tagged the one registered instance.
    let info = instance_info(&emulator, "tagme").await;
    assert!(
        tags_of(&info).is_empty(),
        "a refused request writes nothing, got {info:?}"
    );
}

/// RULE: an instance carries at most the service's tag bound; a call that would
/// take it past the bound is refused with `TagLimitExceededException` and
/// changes nothing, while replacing the value of a key it already carries is
/// always allowed because it adds no key.
#[tokio::test]
async fn the_tag_count_of_an_instance_is_bounded() {
    let emulator = Emulator::new();
    register(&emulator, "bounded-tags").await;

    // The bound this service enforces, read from the service itself rather than
    // restated here (`crate::state::MAX_TAGS_PER_ON_PREMISES_INSTANCE`).
    let bound =
        service_com_amazonaws_codedeploy_codedeploy_20141006::state::MAX_TAGS_PER_ON_PREMISES_INSTANCE;
    let at_bound: Vec<Value> = (0..bound)
        .map(|index| tag(&format!("Key{index}"), &format!("value-{index}")))
        .collect();
    emulator
        .ok(
            "AddTagsToOnPremisesInstances",
            json!({"instanceNames": ["bounded-tags"], "tags": at_bound}),
        )
        .await;
    let info = instance_info(&emulator, "bounded-tags").await;
    assert_eq!(
        tags_of(&info).len(),
        bound,
        "case \"at the bound\": the instance carries every tag, got {info:?}"
    );

    // Replacing an existing key at the bound adds no key, so it is accepted.
    emulator
        .ok(
            "AddTagsToOnPremisesInstances",
            json!({"instanceNames": ["bounded-tags"], "tags": [tag("Key0", "replaced")]}),
        )
        .await;
    let info = instance_info(&emulator, "bounded-tags").await;
    assert_eq!(
        tags_of(&info).len(),
        bound,
        "case \"replace at the bound\": no key was added, got {info:?}"
    );
    assert_eq!(
        tags_of(&info).first().map(|(_, value)| value.as_str()),
        Some("replaced"),
        "case \"replace at the bound\": the value was replaced, got {info:?}"
    );

    // One key past the bound is refused, and the instance is unchanged.
    let (status, body) = emulator
        .call(
            "AddTagsToOnPremisesInstances",
            json!({"instanceNames": ["bounded-tags"], "tags": [tag("OneTooMany", "x")]}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"past the bound\": the tag bound is a client error, got {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "TagLimitExceededException",
        "case \"past the bound\": expected TagLimitExceededException, got {body:?}"
    );
    let info = instance_info(&emulator, "bounded-tags").await;
    assert_eq!(
        tags_of(&info).len(),
        bound,
        "case \"past the bound\": the refused call changed nothing, got {info:?}"
    );
}

/// RULE: reading a tagged instance changes nothing — the same record, tags
/// included, is reported however many times it is read.
#[tokio::test]
async fn reading_a_tagged_instance_changes_nothing() {
    let emulator = Emulator::new();
    register(&emulator, "stable").await;
    emulator
        .ok(
            "AddTagsToOnPremisesInstances",
            json!({"instanceNames": ["stable"], "tags": [tag("Env", "prod")]}),
        )
        .await;

    let first = instance_info(&emulator, "stable").await;
    for read in 1..=3 {
        let again = instance_info(&emulator, "stable").await;
        assert_eq!(
            again, first,
            "case \"read {read}\": a read reports the same record, got {again:?}"
        );
    }
}
