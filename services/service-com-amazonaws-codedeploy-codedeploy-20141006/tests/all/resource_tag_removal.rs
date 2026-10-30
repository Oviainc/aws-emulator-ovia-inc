// SPDX-License-Identifier: CC0-1.0

//! Removing tags from a CodeDeploy resource by its ARN, driven over the wire.
//!
//! RULE under test: `UntagResource` reads its `ResourceArn` as the resource that
//! ARN identifies — an application or a deployment group — and drops from that
//! resource's own record every tag whose key
//! the `TagKeys` list names, whatever value the record holds for it, leaving
//! every other tag in place; a key the resource does not carry names no tag of
//! it, so it removes nothing and the call still succeeds, which makes the
//! removal idempotent. `ListTagsForResource` afterwards reports exactly the tags
//! that are left (`[]` once none are). An ARN the service cannot resolve is
//! refused with the modelled error for THAT failure, and a `TagKeys` list it
//! will not act on with `InvalidTagsToAddException` (an empty key) — each before
//! anything is removed.
//!
//! Every case goes through the same front door a client uses — the shared
//! [`Emulator`] harness of `codedeploy_lifecycle` — so the boundary validator,
//! this service's refusal wording and the awsJson1_1 envelope are exercised.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, seeded_group, Emulator};

use service_com_amazonaws_codedeploy_codedeploy_20141006::state;

/// The region the harness's unsigned calls are served in, which is the one the
/// service builds an ARN with when a request carries no credential scope.
const REGION: &str = "us-east-1";

/// One row of the removal table: the case name, the `(key, value)` tags the
/// resource is given, the keys the removal call names, and the pairs the
/// resource must report afterwards.
type RemovalCase<'a> = (
    &'a str,
    &'a [(&'a str, &'a str)],
    &'a [&'a str],
    &'a [(&'a str, &'a str)],
);

/// A `Tags` list entry, as CodeDeploy's `Tag` shape names its members
/// (`smithy_query(query="shape",shape="Tag")`: `Key`, `Value`).
fn tag(key: &str, value: &str) -> Value {
    json!({"Key": key, "Value": value})
}

/// `tags` as the wire list of `Tag` structures.
fn tag_list(tags: &[(&str, &str)]) -> Value {
    Value::Array(tags.iter().map(|(key, value)| tag(key, value)).collect())
}

/// The `$.message` of a failure body: the sentence the refusal carries.
fn message(case: &str, body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: expected a string $.message, got {body}"))
        .to_string()
}

/// The `(key, value)` pairs `ListTagsForResource` reports for `resource_arn`, in
/// report order, requiring the read to succeed — the round trip every removal
/// case is checked through.
async fn tags_of(emulator: &Emulator, resource_arn: &str) -> Vec<(String, String)> {
    let listed = emulator
        .ok("ListTagsForResource", json!({"ResourceArn": resource_arn}))
        .await;
    listed["Tags"]
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

/// `pairs` as the owned `(key, value)` form `tags_of` reports.
fn owned(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Apply `tags` to the resource `resource_arn` names, requiring the call to
/// succeed: the state every removal case starts from.
async fn apply_tags(emulator: &Emulator, resource_arn: &str, tags: &[(&str, &str)]) {
    emulator
        .ok(
            "TagResource",
            json!({"ResourceArn": resource_arn, "Tags": tag_list(tags)}),
        )
        .await;
}

/// RULE: the keys a removal call names are dropped from the resource its ARN
/// identifies, whatever value the resource holds for them, and every other tag
/// is left in place — whatever the resource kind, the number of keys named, or
/// whether the resource carries the key at all.
#[tokio::test]
async fn tags_removed_by_key_read_back_from_the_resource() {
    let emulator = Emulator::new();
    // Two resources of each taggable kind, each created through the API a
    // client uses, so the ARN under test names a record the store really holds.
    emulator
        .ok("CreateApplication", json!({"applicationName": "untag-app"}))
        .await;
    let group = seeded_group(&emulator, "untag-group-app", "Server").await;
    emulator
        .ok("CreateApplication", json!({"applicationName": "untag-app-2"}))
        .await;
    let second_group = seeded_group(&emulator, "untag-group-app-2", "Server").await;

    // Rows differ along the resource KIND the ARN names, how many keys the call
    // names (one, several, all, none of the resource's own), whether a named key
    // is carried, the ORDER the keys are given in, a repeated key, a key whose
    // value is empty, and a key written outside ASCII.
    let cases: &[RemovalCase<'_>] = &[
        (
            "application, one key of three",
            &[("Env", "prod"), ("Owner", "ana"), ("Tier", "api")],
            &["Owner"],
            &[("Env", "prod"), ("Tier", "api")],
        ),
        (
            "deployment group, two keys in reverse order",
            &[("Env", "dev"), ("Owner", "ana"), ("Tier", "api")],
            &["Tier", "Env"],
            &[("Owner", "ana")],
        ),
        (
            "a second application, every key",
            &[("Env", "staging"), ("Marker", "")],
            &["Env", "Marker"],
            &[],
        ),
        (
            "a second deployment group, a key it does not carry",
            &[("Owner", "team platform/eu, on-call")],
            &["Env"],
            &[("Owner", "team platform/eu, on-call")],
        ),
        (
            "application, a repeated key and one it does not carry",
            &[("Env", "prod"), ("Tier", "api")],
            &["Env", "Env", "Absent"],
            &[("Tier", "api")],
        ),
        (
            "deployment group, a key and an empty value",
            &[("Marker", ""), ("Env", "prod")],
            &["Marker"],
            &[("Env", "prod")],
        ),
        (
            "a second application, a key outside ASCII",
            &[("Ümlaut", "ja"), ("Env", "prod")],
            &["Ümlaut"],
            &[("Env", "prod")],
        ),
    ];

    // The resource each row acts on, chosen so both taggable kinds are exercised
    // and a row never depends on the row before it: the tags are re-applied in
    // full first, and the case's expectation is the whole remaining list.
    let arns: Vec<String> = vec![
        state::application_arn(REGION, "untag-app"),
        state::deployment_group_arn(REGION, "untag-group-app", &group),
        state::application_arn(REGION, "untag-app-2"),
        state::deployment_group_arn(REGION, "untag-group-app-2", &second_group),
        state::application_arn(REGION, "untag-app"),
        state::deployment_group_arn(REGION, "untag-group-app", &group),
        state::application_arn(REGION, "untag-app-2"),
    ];

    for ((case, applied, keys, expected), resource_arn) in cases.iter().zip(&arns) {
        // Start each row from exactly the tags it names: every key the previous
        // row left behind is removed first.
        let carried = tags_of(&emulator, resource_arn).await;
        if !carried.is_empty() {
            let present: Vec<String> = carried.into_iter().map(|(key, _)| key).collect();
            emulator
                .ok(
                    "UntagResource",
                    json!({"ResourceArn": resource_arn, "TagKeys": present}),
                )
                .await;
        }
        apply_tags(&emulator, resource_arn, applied).await;

        let (status, body) = emulator
            .call(
                "UntagResource",
                json!({"ResourceArn": resource_arn, "TagKeys": keys}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UntagResource must succeed, got {body:?}"
        );

        let reported = tags_of(&emulator, resource_arn).await;
        assert_eq!(
            reported,
            owned(expected),
            "case {case:?}: the resource reports exactly the tags left, got {reported:?}"
        );
    }

    // Untagging one resource untags no other: the application still carries the
    // tag its own last row left, and the group its own.
    let application = tags_of(&emulator, &state::application_arn(REGION, "untag-app")).await;
    assert_eq!(
        application,
        owned(&[("Tier", "api")]),
        "case \"removals stay on their own resource\": expected the application's own remaining \
         tag, got {application:?}"
    );
    let grouped = tags_of(
        &emulator,
        &state::deployment_group_arn(REGION, "untag-group-app", &group),
    )
    .await;
    assert_eq!(
        grouped,
        owned(&[("Env", "prod")]),
        "case \"removals stay on their own resource\": expected the group's own remaining tag, got \
         {grouped:?}"
    );
}

/// RULE: removing a key is idempotent — the same call repeated leaves the
/// resource holding the same tags and still succeeds, and a key removed can be
/// applied again and removed again.
#[tokio::test]
async fn removing_a_key_is_idempotent() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateApplication", json!({"applicationName": "idempotent"}))
        .await;
    let resource_arn = state::application_arn(REGION, "idempotent");
    apply_tags(
        &emulator,
        &resource_arn,
        &[("Env", "prod"), ("Owner", "ana")],
    )
    .await;

    for attempt in 1..=3 {
        let (status, body) = emulator
            .call(
                "UntagResource",
                json!({"ResourceArn": resource_arn, "TagKeys": ["Env"]}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case \"attempt {attempt}\": removing a key again still succeeds, got {body:?}"
        );
        let reported = tags_of(&emulator, &resource_arn).await;
        assert_eq!(
            reported,
            owned(&[("Owner", "ana")]),
            "case \"attempt {attempt}\": the same tags are left, got {reported:?}"
        );
    }

    // A key that was removed is applied again by a tagging call, and removed
    // again by the same removal: the record is not remembering the removal.
    apply_tags(&emulator, &resource_arn, &[("Env", "dev")]).await;
    let reported = tags_of(&emulator, &resource_arn).await;
    assert_eq!(
        reported,
        owned(&[("Owner", "ana"), ("Env", "dev")]),
        "case \"re-apply a removed key\": the key is carried again, got {reported:?}"
    );
    emulator
        .ok(
            "UntagResource",
            json!({"ResourceArn": resource_arn, "TagKeys": ["Env", "Owner"]}),
        )
        .await;
    let reported = tags_of(&emulator, &resource_arn).await;
    assert!(
        reported.is_empty(),
        "case \"remove every key\": the resource carries no tags, got {reported:?}"
    );

    // The resource itself is still there once its every tag is gone.
    let application = emulator
        .ok("GetApplication", json!({"applicationName": "idempotent"}))
        .await;
    assert_eq!(
        application["application"]["applicationName"].as_str(),
        Some("idempotent"),
        "case \"the resource survives its tags\": expected the application, got {application:?}"
    );
}

/// RULE: a `ResourceArn` the service cannot resolve to a resource it tags is
/// refused with HTTP 400 and the modelled error for THAT failure — not in the
/// ARN format, naming a resource these operations do not act on, or naming a
/// resource the account does not hold — and nothing is removed.
#[tokio::test]
async fn an_unresolvable_resource_arn_removes_nothing() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateApplication", json!({"applicationName": "present"}))
        .await;
    let present_arn = state::application_arn(REGION, "present");
    apply_tags(&emulator, &present_arn, &[("Env", "prod")]).await;
    seeded_group(&emulator, "present-group-app", "Server").await;

    // Rows differ along WHAT is wrong with the ARN: its form, the service or
    // resource type it names, and whether the resource it names exists — one
    // row per modelled "does not exist" error. The last column is what the
    // refusal has to NAME, so a caller can tell from the message what to change.
    let cases: Vec<(&str, String, &str, String)> = vec![
        (
            "not an ARN at all",
            "present".to_string(),
            "InvalidArnException",
            "present".to_string(),
        ),
        (
            "too few ARN fields",
            "arn:aws:codedeploy:us-east-1:000000000000".to_string(),
            "InvalidArnException",
            "arn:aws:codedeploy:us-east-1:000000000000".to_string(),
        ),
        (
            "an application ARN with no name",
            "arn:aws:codedeploy:us-east-1:000000000000:application:".to_string(),
            "InvalidArnException",
            "arn:aws:codedeploy:us-east-1:000000000000:application:".to_string(),
        ),
        (
            "another service's ARN",
            "arn:aws:iam::000000000000:role/CodeDeployRole".to_string(),
            "ArnNotSupportedException",
            "arn:aws:iam::000000000000:role/CodeDeployRole".to_string(),
        ),
        (
            "a CodeDeploy on-premises instance ARN",
            state::instance_arn(REGION, "host-01", "registration"),
            "ArnNotSupportedException",
            state::instance_arn(REGION, "host-01", "registration"),
        ),
        (
            "an application that does not exist",
            state::application_arn(REGION, "absent-app"),
            "ApplicationDoesNotExistException",
            "absent-app".to_string(),
        ),
        (
            "a deployment group whose application does not exist",
            state::deployment_group_arn(REGION, "absent-app", "any-group"),
            "ApplicationDoesNotExistException",
            "absent-app".to_string(),
        ),
        (
            "a deployment group that does not exist",
            state::deployment_group_arn(REGION, "present-group-app", "absent-group"),
            "DeploymentGroupDoesNotExistException",
            "absent-group".to_string(),
        ),
        (
            "a deployment configuration, a type these operations do not act on",
            state::deployment_config_arn(REGION, "absent-config"),
            "ArnNotSupportedException",
            state::deployment_config_arn(REGION, "absent-config"),
        ),
    ];

    for (case, resource_arn, expected, named) in cases {
        let (status, body) = emulator
            .call(
                "UntagResource",
                json!({"ResourceArn": resource_arn, "TagKeys": ["Env"]}),
            )
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
        let message = message(case, &body);
        assert!(
            message.contains(&named),
            "case {case:?}: the refusal must name {named:?}, got {message:?}"
        );
    }

    // None of the refusals above removed the tag of the one resource that does
    // carry one.
    let reported = tags_of(&emulator, &present_arn).await;
    assert_eq!(
        reported,
        owned(&[("Env", "prod")]),
        "case \"a refused request removes nothing\": got {reported:?}"
    );
}

/// RULE: a `TagKeys` list the service will not act on is refused with the
/// modelled error for that failure — an empty key is
/// `InvalidTagsToAddException` — and the resource keeps every
/// tag it held; a list merely LONGER than what a resource may carry is not a
/// failure, because a key the resource does not hold removes nothing; and a
/// request carrying no `TagKeys` member at all names no tag, which is
/// `TagRequiredException`.
#[tokio::test]
async fn a_tag_keys_list_is_refused_by_what_is_wrong_with_it() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateApplication", json!({"applicationName": "key-gated"}))
        .await;
    let resource_arn = state::application_arn(REGION, "key-gated");
    apply_tags(
        &emulator,
        &resource_arn,
        &[("Env", "prod"), ("Owner", "ana")],
    )
    .await;

    let cases: Vec<(&str, Value, &str)> = vec![
        ("an empty key", json!([""]), "InvalidTagsToAddException"),
        (
            "an empty key beside a carried one",
            json!(["Env", ""]),
            "InvalidTagsToAddException",
        ),
    ];

    for (case, keys, expected) in cases {
        let (status, body) = emulator
            .call(
                "UntagResource",
                json!({"ResourceArn": resource_arn, "TagKeys": keys}),
            )
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
        let reported = tags_of(&emulator, &resource_arn).await;
        assert_eq!(
            reported,
            owned(&[("Env", "prod"), ("Owner", "ana")]),
            "case {case:?}: a refused call removes nothing, got {reported:?}"
        );
    }

    // The one tag refusal this operation answers is a request that carries no
    // `TagKeys` member AT ALL; a member it does carry is acted on as given, an
    // empty list included (`aws_refusal_sentences`).
    let (status, body) = emulator
        .call("UntagResource", json!({"ResourceArn": resource_arn}))
        .await;
    assert_eq!(
        status, 400,
        "case \"no TagKeys member\": a request naming no tag is a client error, got {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "TagRequiredException",
        "case \"no TagKeys member\": expected TagRequiredException, got {body:?}"
    );
    let reported = tags_of(&emulator, &resource_arn).await;
    assert_eq!(
        reported,
        owned(&[("Env", "prod"), ("Owner", "ana")]),
        "case \"no TagKeys member\": a refused call removes nothing, got {reported:?}"
    );

    // A list naming far MORE keys than a resource may carry is served: the keys
    // the resource holds go, the rest name no tag of it. The length compared
    // against is read from the service itself rather than restated here
    // (`crate::state::MAX_TAGS_PER_RESOURCE`).
    let bound = state::MAX_TAGS_PER_RESOURCE;
    let past_bound: Vec<String> = (0..bound)
        .map(|index| format!("Key{index}"))
        .chain(["Env".to_string(), "Owner".to_string()])
        .collect();
    assert!(
        past_bound.len() > bound,
        "case \"past the bound\": the list must name more keys than the bound, got {}",
        past_bound.len()
    );
    let (status, body) = emulator
        .call(
            "UntagResource",
            json!({"ResourceArn": resource_arn, "TagKeys": past_bound}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"past the bound\": a long key list is served, not refused, got {body:?}"
    );
    let reported = tags_of(&emulator, &resource_arn).await;
    assert!(
        reported.is_empty(),
        "case \"past the bound\": every carried key was removed, got {reported:?}"
    );
}
