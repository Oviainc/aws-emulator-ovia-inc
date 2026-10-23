// SPDX-License-Identifier: CC0-1.0

//! The tag set of an export definition, end to end over the awsJson1_1 wire:
//! `TagResource` -> `ListTagsForResource` -> `UntagResource`.
//!
//! The rule under test: an export's tags are ONE persisted set. `TagResource` adds the
//! submitted pairs to it (a key already there takes the submitted value, every other tag
//! stays), `UntagResource` removes exactly the keys it names and no others, and
//! `ListTagsForResource` reports the set in force at that moment, ordered by key.
//!
//! It is asserted as an INVARIANT over a walk of tagging operations: the test keeps its
//! own model of the set — a plain map it applies each step to — and compares the service
//! against it after every step. The walk's steps differ along what the rule generalises
//! over: how many pairs one call carries (none, one, several), whether a key is new or
//! already present, whether an untagged key is present or absent, key and value shapes
//! at the published bounds of `ResourceTagKey` (`@length 1..128`) and `ResourceTagValue`
//! (`@length 0..256`), and repetition of the same call. The two recorded steps (tag two
//! keys, then untag one of them) are the first two rows of that walk.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::MAX_RESOURCE_TAGS;

use crate::export_lifecycle::{cases, error_code, Client};

/// One step of the walk: the call to make.
#[derive(Debug, Clone)]
enum Step {
    /// `TagResource` with these `(key, value)` pairs.
    Tag(Vec<(String, String)>),
    /// `UntagResource` with these keys.
    Untag(Vec<String>),
}

impl Step {
    /// The operation and request body this step sends.
    fn request(&self, arn: &str) -> (&'static str, Value) {
        match self {
            Self::Tag(pairs) => (
                "TagResource",
                json!({
                    "ResourceArn": arn,
                    "ResourceTags": pairs
                        .iter()
                        .map(|(key, value)| json!({ "Key": key, "Value": value }))
                        .collect::<Vec<Value>>(),
                }),
            ),
            Self::Untag(keys) => (
                "UntagResource",
                json!({ "ResourceArn": arn, "ResourceTagKeys": keys }),
            ),
        }
    }

    /// Apply this step to the test's own model of the tag set.
    fn apply(&self, model: &mut BTreeMap<String, String>) {
        match self {
            Self::Tag(pairs) => {
                for (key, value) in pairs {
                    model.insert(key.clone(), value.clone());
                }
            }
            Self::Untag(keys) => {
                for key in keys {
                    model.remove(key);
                }
            }
        }
    }
}

/// A tagging step from `(key, value)` literals.
fn tag(pairs: &[(&str, &str)]) -> Step {
    Step::Tag(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
    )
}

/// An untagging step from key literals.
fn untag(keys: &[&str]) -> Step {
    Step::Untag(keys.iter().map(|key| (*key).to_string()).collect())
}

/// The walk: each labelled step, in order, against one export.
fn walk() -> Vec<(String, Step)> {
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);
    vec![
        (
            // The recorded step: two keys onto an export created without tags.
            "two tags onto an untagged export".to_string(),
            tag(&[("env", "test"), ("team", "billing")]),
        ),
        (
            // The recorded step: one key removed, the other must survive.
            "one key removed".to_string(),
            untag(&["env"]),
        ),
        (
            "a key the export does not carry".to_string(),
            untag(&["env"]),
        ),
        (
            "a third and fourth key in one call".to_string(),
            tag(&[("owner", "finops"), ("cost-centre", "4711")]),
        ),
        (
            "a key already present takes the new value".to_string(),
            tag(&[("team", "platform")]),
        ),
        (
            "an empty value, which the model allows".to_string(),
            tag(&[("note", "")]),
        ),
        (
            "the published key and value ceilings".to_string(),
            tag(&[(longest_key.as_str(), longest_value.as_str())]),
        ),
        (
            "the shortest key the model allows".to_string(),
            tag(&[("k", "short")]),
        ),
        (
            "no pairs at all, which changes nothing".to_string(),
            tag(&[]),
        ),
        (
            "no keys at all, which changes nothing".to_string(),
            untag(&[]),
        ),
        (
            "the same tag again, which is the same set".to_string(),
            tag(&[("team", "platform")]),
        ),
        (
            "several keys at once, one of them absent".to_string(),
            untag(&["owner", "cost-centre", "never-set"]),
        ),
        (
            "the long key removed by name".to_string(),
            untag(&[longest_key.as_str()]),
        ),
        (
            "every remaining key".to_string(),
            untag(&["team", "note", "k"]),
        ),
        (
            "tagging an export whose tags were all removed".to_string(),
            tag(&[("env", "prod")]),
        ),
    ]
}

/// The tag set `ListTagsForResource` reports for `arn`, as a map.
async fn listed_tags(client: &Client, case: &str, arn: &str) -> BTreeMap<String, String> {
    let body = client
        .ok("ListTagsForResource", &json!({ "ResourceArn": arn }))
        .await;
    let tags = body["ResourceTags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ResourceTags list, got {body}"))
        .clone();
    assert!(
        tags.len() <= MAX_RESOURCE_TAGS,
        "case {case:?}: ResourceTags must stay within the modelled 0..{MAX_RESOURCE_TAGS} bound, got {}",
        tags.len()
    );
    // The listing is ordered by key, so a paginating client walks it in one stable order.
    let keys: Vec<&str> = tags
        .iter()
        .map(|entry| entry["Key"].as_str().unwrap_or_default())
        .collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(
        keys, sorted,
        "case {case:?}: expected the tags ordered by key, got {keys:?}"
    );

    tags.iter()
        .map(|entry| {
            let key = entry["Key"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a tag without a Key: {entry}"))
                .to_string();
            let value = entry["Value"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a tag without a Value: {entry}"))
                .to_string();
            (key, value)
        })
        .collect()
}

/// Create one export without tags and return its ARN.
async fn untagged_export(client: &Client, name: &str) -> String {
    let mut case = cases().remove(0);
    case.set_name(name);
    let created = client.ok("CreateExport", &case.create_request()).await;
    created["ExportArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateExport returned no ExportArn: {created}"))
        .to_string()
}

#[tokio::test]
async fn the_tag_set_follows_every_tag_and_untag_call() {
    let client = Client::new();
    let arn = untagged_export(&client, "tagged_export").await;
    let mut model: BTreeMap<String, String> = BTreeMap::new();

    assert_eq!(
        listed_tags(&client, "created without tags", &arn).await,
        model,
        "case \"created without tags\": an export created with no ResourceTags has no tags"
    );

    for (case, step) in walk() {
        let (operation, request) = step.request(&arn);
        let response = client.ok(operation, &request).await;
        assert!(
            response.is_null() || response.as_object().is_some_and(serde_json::Map::is_empty),
            "case {case:?}: {operation} models an empty response, got {response}"
        );
        step.apply(&mut model);

        let listed = listed_tags(&client, &case, &arn).await;
        assert_eq!(
            listed, model,
            "case {case:?}: expected the tag set {model:?}, got {listed:?}"
        );

        // A listing is a report: reading it again answers the same set.
        assert_eq!(
            listed_tags(&client, &case, &arn).await,
            listed,
            "case {case:?}: ListTagsForResource must answer identically on a repeat read"
        );
    }
}

#[tokio::test]
async fn tags_belong_to_the_export_they_were_set_on() {
    let client = Client::new();
    let first = untagged_export(&client, "tag_isolation_one").await;
    let second = untagged_export(&client, "tag_isolation_two").await;

    client
        .ok(
            "TagResource",
            &json!({
                "ResourceArn": first,
                "ResourceTags": [{ "Key": "env", "Value": "test" }],
            }),
        )
        .await;
    assert_eq!(
        listed_tags(&client, "the other export", &second).await,
        BTreeMap::new(),
        "tagging one export must not tag another"
    );

    client
        .ok(
            "TagResource",
            &json!({
                "ResourceArn": second,
                "ResourceTags": [{ "Key": "env", "Value": "prod" }],
            }),
        )
        .await;
    client
        .ok(
            "UntagResource",
            &json!({ "ResourceArn": second, "ResourceTagKeys": ["env"] }),
        )
        .await;
    assert_eq!(
        listed_tags(&client, "the first export", &first).await,
        BTreeMap::from([("env".to_string(), "test".to_string())]),
        "untagging one export must not untag another"
    );

    // A delete takes the export's tags with it: the ARN names nothing afterwards.
    client
        .ok("DeleteExport", &json!({ "ExportArn": first }))
        .await;
    let recreated = untagged_export(&client, "tag_isolation_one").await;
    assert_eq!(
        recreated, first,
        "recreating a deleted export mints the same name-derived ARN"
    );
    assert_eq!(
        listed_tags(&client, "recreated export", &recreated).await,
        BTreeMap::new(),
        "a recreated export must not inherit the deleted export's tags"
    );
}

#[tokio::test]
async fn tags_set_at_create_time_are_the_same_set_tagging_adds_to() {
    let client = Client::new();
    // A row of the lifecycle table that submits tags with the create.
    let case = cases()
        .into_iter()
        .find(|case| case.tag_count() > 1)
        .expect("the lifecycle table has a row with several tags");
    let created = client.ok("CreateExport", &case.create_request()).await;
    let arn = created["ExportArn"].as_str().expect("an ExportArn").to_string();

    let mut model = listed_tags(&client, "created with tags", &arn).await;
    assert_eq!(
        model.len(),
        case.tag_count(),
        "a create's ResourceTags must all be readable, got {model:?}"
    );
    let first_key = model
        .keys()
        .next()
        .cloned()
        .expect("the export carries tags");

    for (case_label, step) in [
        ("add to the create's tags".to_string(), tag(&[("added", "1")])),
        (
            "remove one of the create's tags".to_string(),
            untag(&[first_key.as_str()]),
        ),
    ] {
        let (operation, request) = step.request(&arn);
        client.ok(operation, &request).await;
        step.apply(&mut model);
        let listed = listed_tags(&client, &case_label, &arn).await;
        assert_eq!(
            listed, model,
            "case {case_label:?}: expected {model:?}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_request_outside_the_rules_is_refused() {
    let client = Client::new();
    let arn = untagged_export(&client, "refused_tags").await;

    // One key twice names two values for one tag, which cannot be satisfied: "each key
    // must be unique for the resource" (the modelled doc for `ResourceTags`).
    let (status, body) = client
        .call(
            "TagResource",
            &json!({
                "ResourceArn": arn,
                "ResourceTags": [
                    { "Key": "env", "Value": "test" },
                    { "Key": "env", "Value": "prod" },
                ],
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "a repeated tag key must be refused, got {body}"
    );
    assert_eq!(
        listed_tags(&client, "after the refusal", &arn).await,
        BTreeMap::new(),
        "a refused TagResource must not have stored anything"
    );

    // The set is bounded by what `ListTagsForResource` can report (`ResourceTags`,
    // `@length 0..200`): the request that would exceed it is refused, and the one that
    // fills it exactly is not.
    let full: Vec<Value> = (0..MAX_RESOURCE_TAGS)
        .map(|index| json!({ "Key": format!("key-{index:04}"), "Value": index.to_string() }))
        .collect();
    client
        .ok(
            "TagResource",
            &json!({ "ResourceArn": arn, "ResourceTags": full }),
        )
        .await;
    assert_eq!(
        listed_tags(&client, "at the bound", &arn).await.len(),
        MAX_RESOURCE_TAGS,
        "a request filling the tag set exactly must be accepted"
    );

    let (status, body) = client
        .call(
            "TagResource",
            &json!({
                "ResourceArn": arn,
                "ResourceTags": [{ "Key": "one-too-many", "Value": "x" }],
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "a tag past the bound must be refused, got {body}"
    );
    let listed = listed_tags(&client, "past the bound", &arn).await;
    assert_eq!(
        listed.len(),
        MAX_RESOURCE_TAGS,
        "the refused TagResource must not have changed the set"
    );
    assert!(
        !listed.contains_key("one-too-many"),
        "the refused tag must not be readable, got {listed:?}"
    );

    // Re-tagging a key already in the full set is not growth, so it is accepted.
    client
        .ok(
            "TagResource",
            &json!({
                "ResourceArn": arn,
                "ResourceTags": [{ "Key": "key-0000", "Value": "replaced" }],
            }),
        )
        .await;
    assert_eq!(
        listed_tags(&client, "replacing a value at the bound", &arn)
            .await
            .get("key-0000")
            .map(String::as_str),
        Some("replaced"),
        "replacing a value in a full tag set must be accepted"
    );
}
