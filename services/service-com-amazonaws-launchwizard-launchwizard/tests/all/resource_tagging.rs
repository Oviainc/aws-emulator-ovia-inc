// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Launch Wizard tagging APIs — `ListTagsForResource`,
//! `TagResource` and `UntagResource` — driven over HTTP through the same router the
//! production emulator serves with (the harness in `deployment_lifecycle`).
//!
//! The rule under test is an ORDER, so every assertion here is about that order rather
//! than about one recorded call:
//!
//! 1. the `resourceArn` is READ first — an ARN that cannot name a resource of this
//!    service is refused `BadRequestException` (400) before anything is looked up;
//! 2. then the resource — a parseable ARN that names no deployment is refused
//!    `NotFoundException` (404) WHATEVER tags or tag keys the request carries;
//! 3. only for a resource that exists is the tag set itself judged against the model
//!    (`ValidationException`, 400) and written.
//!
//! Each step is exercised over a table whose rows differ along what the rule
//! generalises over: the shape of the ARN, the SIZE of the tag map / key list (at each
//! modelled bound and one past it), the length and character set of a key or value,
//! and the operation the call is made through.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::{json, Value};

use crate::deployment_lifecycle::{create, ACCOUNT};
use crate::harness::{Harness, REGION};

/// A deployment id this service never minted, in the form one takes (a hyphenated
/// UUID) — so an ARN built from it parses and is merely absent.
const ABSENT_DEPLOYMENT_ID: &str = "8f1a2b3c-4d5e-6f70-8192-a3b4c5d6e7f8";

/// Modelled bounds, from `smithy_query(query="shape", shape="Tags,TagKeyList")`:
/// `Tags` / `TagKeyList` are `@length(min: 1, max: 200)`, `TagKey` is
/// `@length(min: 1, max: 128)` with `@pattern("^[a-zA-Z0-9 _.:/=+@-]+$")`, and
/// `TagValue` is `@length(min: 0, max: 256)` with
/// `@pattern("^[a-zA-Z0-9 _.:/=+@-]*$")`. Every one of them is read back out of
/// `smithy-model/` by [`the_tag_rules_the_handlers_enforce_are_the_models`], so a row
/// below cannot be asserting a bound the model no longer declares.
const MAX_TAGS: usize = 200;
/// See [`MAX_TAGS`].
const MAX_TAG_KEY_LEN: usize = 128;
/// See [`MAX_TAGS`].
const MAX_TAG_VALUE_LEN: usize = 256;
/// Every punctuation character `TagKey`'s pattern admits, as one string. Source: the
/// `TagKey` pattern above.
const TAG_PUNCTUATION: &str = " _.:/=+@-";

/// This service's Smithy model, as the tagging shapes' declared constraints are read
/// off it. Shared with the sibling suites that pin a restated bound to the model.
pub(crate) fn model() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-launchwizard-launchwizard.json"
    );
    let text = std::fs::read_to_string(path).expect("the service's Smithy model is readable");
    serde_json::from_str(&text).expect("the service's Smithy model is JSON")
}

/// The `traits` of one shape in the model.
pub(crate) fn shape_traits(model: &Value, shape: &str) -> Value {
    model["shapes"][format!("com.amazonaws.launchwizard#{shape}")]["traits"].clone()
}

#[test]
fn the_tag_rules_the_handlers_enforce_are_the_models() {
    // The handlers judge a tag set against the model, and two of those rules are
    // RESTATED in Rust rather than read off a generated schema: `TagKey`'s length and
    // pattern, because codegen captures a collection's element/value constraints but
    // not a map's KEY constraints (`state::TAG_KEY_PATTERN` and friends), and the
    // ceiling a stored tag set keeps (`state::MAX_TAGS_PER_RESOURCE`). This pins every
    // restatement to the model, so a model refresh that moved a bound fails here
    // instead of leaving the emulator judging by a stale one.
    use service_com_amazonaws_launchwizard_launchwizard::state;

    let model = model();
    let tag_key = shape_traits(&model, "TagKey");
    assert_eq!(
        tag_key["smithy.api#length"]["min"],
        json!(state::TAG_KEY_MIN_LEN),
        "TagKey's modelled length floor must be the one the handler enforces, got {tag_key}"
    );
    assert_eq!(
        tag_key["smithy.api#length"]["max"],
        json!(state::TAG_KEY_MAX_LEN),
        "TagKey's modelled length ceiling must be the one the handler enforces, got {tag_key}"
    );
    assert_eq!(
        tag_key["smithy.api#pattern"],
        json!(state::TAG_KEY_PATTERN),
        "TagKey's modelled pattern must be the one the handler enforces, got {tag_key}"
    );
    assert_eq!(
        state::TAG_KEY_MAX_LEN as usize, MAX_TAG_KEY_LEN,
        "this suite's TagKey ceiling must be the one the handler enforces"
    );

    // The bounds this suite's rows are built from, which the framework's own walk
    // enforces from the generated schema.
    let tags = shape_traits(&model, "Tags");
    assert_eq!(
        tags["smithy.api#length"]["max"],
        json!(MAX_TAGS),
        "Tags' modelled entry ceiling must be the one this suite asserts, got {tags}"
    );
    assert_eq!(
        tags["smithy.api#length"]["min"],
        json!(1),
        "Tags' modelled entry floor must be one entry, got {tags}"
    );
    assert_eq!(
        state::MAX_TAGS_PER_RESOURCE, MAX_TAGS,
        "the ceiling a stored tag set keeps must be the modelled `Tags` ceiling"
    );
    let tag_key_list = shape_traits(&model, "TagKeyList");
    assert_eq!(
        tag_key_list["smithy.api#length"]["max"],
        json!(MAX_TAGS),
        "TagKeyList's modelled ceiling must be the one this suite asserts, got {tag_key_list}"
    );
    let tag_value = shape_traits(&model, "TagValue");
    assert_eq!(
        tag_value["smithy.api#length"]["max"],
        json!(MAX_TAG_VALUE_LEN),
        "TagValue's modelled ceiling must be the one this suite asserts, got {tag_value}"
    );
    assert_eq!(
        tag_value["smithy.api#length"]["min"],
        json!(0),
        "TagValue's modelled floor must admit the empty value, got {tag_value}"
    );
    // The punctuation rows are built from the key pattern's own character class.
    for character in TAG_PUNCTUATION.chars() {
        assert!(
            state::TAG_KEY_PATTERN.contains(character),
            "case {character:?}: this suite's punctuation must come from the TagKey pattern"
        );
    }
}

/// The tags `create` gives a new deployment (`create_body`'s `tags`), which is the tag
/// set every suite below starts from.
fn created_with() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("Owner".to_string(), "emulator-tests".to_string()),
        ("Workload".to_string(), "SAP".to_string()),
    ])
}

/// Percent-encode `value` as ONE URI component, the way an AWS SDK encodes a
/// non-greedy `@httpLabel` or a query value: everything outside the unreserved set is
/// escaped, `:` and `/` included.
///
/// Written out rather than taken from a crate, for the reason the sibling
/// `service-com-amazonaws-eks-awswesleyfrontend` suite states on its own
/// `encode_label`: a new dependency means a new `Cargo.lock` entry, and this change
/// deliberately leaves that file — the one every concurrent change also edits —
/// untouched. The rule is the one an SDK applies: keep the ASCII alphanumerics (a
/// subset of RFC 3986's unreserved set, so this escapes a superset of what must be
/// escaped) and percent-encode every other byte; the server percent-decodes either way.
pub(crate) fn encode_component(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// The `/tags/{resourceArn}` path for `arn`.
fn tags_path(arn: &str) -> String {
    format!("/tags/{}", encode_component(arn))
}

/// The `/tags/{resourceArn}?tagKeys=..` path `UntagResource` is called on. `tagKeys`
/// is `@httpQuery`, so a list travels as one parameter per key.
fn untag_path(arn: &str, keys: &[String]) -> String {
    let mut path = tags_path(arn);
    for (index, key) in keys.iter().enumerate() {
        let separator = if index == 0 { '?' } else { '&' };
        let _ = write!(path, "{separator}tagKeys={}", encode_component(key));
    }
    path
}

/// A deployment ARN over `deployment_id`, in the form this service builds.
pub(crate) fn deployment_arn(deployment_id: &str) -> String {
    format!("arn:aws:launchwizard:{REGION}:{ACCOUNT}:deployment/{deployment_id}")
}

/// `count` distinct tags, named so the row they belong to is readable in a failure.
fn tags(prefix: &str, count: usize) -> Value {
    let map: BTreeMap<String, Value> = (0..count)
        .map(|index| (format!("{prefix}{index}"), json!(format!("v{index}"))))
        .collect();
    json!(map)
}

/// `count` distinct tag keys.
fn keys(prefix: &str, count: usize) -> Vec<String> {
    (0..count).map(|index| format!("{prefix}{index}")).collect()
}

/// A JSON object as a sorted map of strings.
fn as_map(case: &str, value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a map, got {value}"))
        .iter()
        .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
        .collect()
}

/// The tags `ListTagsForResource` reports for `arn`, as a sorted map.
async fn listed_tags(harness: &Harness, case: &str, arn: &str) -> BTreeMap<String, String> {
    let body = harness
        .expect_ok(case, "GET", &tags_path(arn), None)
        .await;
    let tags = body
        .get("tags")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: ListTagsForResource reported no tags: {body}"));
    as_map(case, &tags)
}

/// The tags `GetDeployment` reports for `deployment_id` — the second reader, so a
/// write is proven to have landed on the deployment record itself.
async fn deployment_tags(
    harness: &Harness,
    case: &str,
    deployment_id: &str,
) -> BTreeMap<String, String> {
    let body = harness
        .ok(case, "/getDeployment", json!({"deploymentId": deployment_id}))
        .await;
    match body["deployment"].get("tags") {
        Some(tags) => as_map(case, tags),
        None => BTreeMap::new(),
    }
}

/// Create a deployment to tag, checking its minted id is in the form the tagging APIs
/// parse out of a resource ARN.
async fn deployment_to_tag(harness: &Harness, name: &str) -> String {
    let id = create(harness, "a deployment to tag", "SAP", "SapHanaSingle", name).await;
    // A `deploymentId` is a hyphenated UUID: that is the form a deployment ARN carries
    // and the form `ListTagsForResource`/`TagResource`/`UntagResource` parse.
    assert_eq!(
        id.len(),
        36,
        "a minted deploymentId must be a hyphenated UUID, got {id:?}"
    );
    assert!(
        id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
            && id.match_indices('-').count() == 4,
        "a minted deploymentId must be a hyphenated UUID, got {id:?}"
    );
    id
}

#[tokio::test]
async fn an_arn_that_cannot_name_a_resource_is_refused_before_any_lookup() {
    let harness = Harness::new();
    // A real deployment exists, so a refusal below cannot be "the store is empty".
    let live = deployment_to_tag(&harness, "arn-parse-subject").await;

    // Each row is an ARN that cannot name a Launch Wizard deployment, and the reason.
    // Most rows carry a deployment id that EXISTS, so the only thing wrong with them is
    // the ARN around it.
    let malformed: Vec<(&str, String)> = vec![
        ("not an ARN at all", "a-deployment".to_string()),
        (
            "too few ARN fields",
            format!("arn:aws:launchwizard:{REGION}:deployment/{live}"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:deployment/{live}"),
        ),
        (
            "another resource type",
            format!("arn:aws:launchwizard:{REGION}:{ACCOUNT}:workload/{live}"),
        ),
        (
            "no resource type",
            format!("arn:aws:launchwizard:{REGION}:{ACCOUNT}:{live}"),
        ),
        (
            "an empty region",
            format!("arn:aws:launchwizard::{ACCOUNT}:deployment/{live}"),
        ),
        (
            "an empty account",
            format!("arn:aws:launchwizard:{REGION}::deployment/{live}"),
        ),
        (
            "an empty partition",
            format!("arn::launchwizard:{REGION}:{ACCOUNT}:deployment/{live}"),
        ),
        (
            "a deployment id that is not a deployment id",
            deployment_arn("not-a-uuid"),
        ),
        (
            "a deployment id with a UUID's characters but not its shape",
            deployment_arn(&ABSENT_DEPLOYMENT_ID.replace('-', "")),
        ),
        ("an empty deployment id", deployment_arn("")),
    ];

    for (reason, arn) in malformed {
        let case = format!("ListTagsForResource with {reason}");
        harness
            .expect_refused(
                &case,
                "GET",
                &tags_path(&arn),
                None,
                400,
                "BadRequestException",
            )
            .await;

        // An unparseable ARN is refused for being unparseable, whatever else the
        // request carries: a tag map far past the modelled 200-entry cap does not
        // change the answer, because the ARN is read first.
        for (size_label, size) in [("one tag", 1), ("one tag past the cap", MAX_TAGS + 1)] {
            let case = format!("TagResource with {reason} and {size_label}");
            harness
                .expect_refused(
                    &case,
                    "POST",
                    &tags_path(&arn),
                    Some(&json!({"tags": tags("k", size)})),
                    400,
                    "BadRequestException",
                )
                .await;
            let case = format!("UntagResource with {reason} and {size_label}");
            harness
                .expect_refused(
                    &case,
                    "DELETE",
                    &untag_path(&arn, &keys("k", size)),
                    None,
                    400,
                    "BadRequestException",
                )
                .await;
        }
    }

    // Nothing above touched the live deployment's tags.
    assert_eq!(
        deployment_tags(&harness, "after the malformed ARNs", &live).await,
        created_with(),
        "a refused tagging call must not write"
    );
}

#[tokio::test]
async fn a_parseable_arn_naming_no_deployment_is_not_found_whatever_the_tags() {
    let harness = Harness::new();
    // A different deployment exists: "not found" is about the resource the ARN names,
    // not about an empty store.
    deployment_to_tag(&harness, "not-found-neighbour").await;
    let absent = deployment_arn(ABSENT_DEPLOYMENT_ID);

    harness
        .expect_refused(
            "ListTagsForResource on a deployment that does not exist",
            "GET",
            &tags_path(&absent),
            None,
            404,
            "NotFoundException",
        )
        .await;

    // The resource is checked AHEAD of the tag map's own rules, so every size — inside
    // the modelled 1..200 bound, exactly at it, and past it — answers the same.
    for size in [0, 1, 2, MAX_TAGS, MAX_TAGS + 1, MAX_TAGS * 2 + 1] {
        let case = format!("TagResource on a deployment that does not exist with {size} tags");
        harness
            .expect_refused(
                &case,
                "POST",
                &tags_path(&absent),
                Some(&json!({"tags": tags("k", size)})),
                404,
                "NotFoundException",
            )
            .await;
        // `tagKeys` is `@httpQuery` and `@required`, so a request carrying none names
        // no modelled route at all and never reaches `UntagResource`: the key-list
        // sizes start at one.
        if size == 0 {
            continue;
        }
        let case = format!("UntagResource on a deployment that does not exist with {size} tagKeys");
        harness
            .expect_refused(
                &case,
                "DELETE",
                &untag_path(&absent, &keys("k", size)),
                None,
                404,
                "NotFoundException",
            )
            .await;
    }

    // Nor does a tag whose own key or value breaks the model change the answer: the
    // resource is decided first.
    let out_of_shape = [
        (
            "a key past its length bound",
            "k".repeat(MAX_TAG_KEY_LEN + 1),
            "v".to_string(),
        ),
        ("an empty key", String::new(), "v".to_string()),
        (
            "a key outside its pattern",
            "no\u{a3}good".to_string(),
            "v".to_string(),
        ),
        (
            "a value past its length bound",
            "k".to_string(),
            "v".repeat(MAX_TAG_VALUE_LEN + 1),
        ),
    ];
    for (reason, key, value) in out_of_shape {
        let case = format!("TagResource on a deployment that does not exist with {reason}");
        harness
            .expect_refused(
                &case,
                "POST",
                &tags_path(&absent),
                Some(&json!({"tags": {key.clone(): value}})),
                404,
                "NotFoundException",
            )
            .await;
        let case = format!("UntagResource on a deployment that does not exist with {reason}");
        harness
            .expect_refused(
                &case,
                "DELETE",
                &untag_path(&absent, std::slice::from_ref(&key)),
                None,
                404,
                "NotFoundException",
            )
            .await;
    }
}

#[tokio::test]
async fn tags_written_by_tagresource_are_read_back_and_removed_by_untagresource() {
    let harness = Harness::new();
    let id = deployment_to_tag(&harness, "round-trip").await;
    let arn = deployment_arn(&id);

    // What the deployment was created with is what the tagging API reports.
    let mut expected = created_with();
    assert_eq!(
        listed_tags(&harness, "the create-time tags", &arn).await,
        expected,
        "ListTagsForResource must report the tags the deployment carries"
    );

    // One row per thing the rule generalises over: a plain pair, a key at each
    // published bound, a value at each published bound (including the empty value the
    // `TagValue` length admits), every character the key pattern allows, and a key
    // already present (which takes the new value rather than being added twice).
    let longest_key = "k".repeat(MAX_TAG_KEY_LEN);
    let punctuation_key = format!("Mixed0{TAG_PUNCTUATION}");
    let rows: Vec<(&str, BTreeMap<String, String>)> = vec![
        (
            "a plain pair",
            BTreeMap::from([("Team".to_string(), "platform".to_string())]),
        ),
        (
            "a one-character key",
            BTreeMap::from([("k".to_string(), "short".to_string())]),
        ),
        (
            "a key at the length bound",
            BTreeMap::from([(longest_key.clone(), "at-the-bound".to_string())]),
        ),
        (
            "an empty value",
            BTreeMap::from([("Empty".to_string(), String::new())]),
        ),
        (
            "a value at the length bound",
            BTreeMap::from([("Long".to_string(), "v".repeat(MAX_TAG_VALUE_LEN))]),
        ),
        (
            "every character the key pattern allows",
            BTreeMap::from([(punctuation_key.clone(), "punctuation".to_string())]),
        ),
        (
            "two pairs at once",
            BTreeMap::from([
                ("Env".to_string(), "prod".to_string()),
                ("Tier".to_string(), "db".to_string()),
            ]),
        ),
        (
            "a key that is already present",
            BTreeMap::from([("Owner".to_string(), "someone-else".to_string())]),
        ),
    ];

    for (case, written) in &rows {
        let body = json!({"tags": written.clone()});
        harness
            .expect_ok(case, "POST", &tags_path(&arn), Some(&body))
            .await;
        // Tagging MERGES: the keys named take the new values, the rest stay.
        for (key, value) in written {
            expected.insert(key.clone(), value.clone());
        }
        assert_eq!(
            listed_tags(&harness, case, &arn).await,
            expected,
            "case {case:?}: ListTagsForResource must report the merged tag set"
        );
        // And the write landed on the deployment record, not beside it: the second
        // reader sees the same set.
        assert_eq!(
            deployment_tags(&harness, case, &id).await,
            expected,
            "case {case:?}: GetDeployment must report the same tags"
        );
    }

    // Removing keys: one, several, and one the resource never carried. `@idempotent`,
    // so the same request twice leaves the same set and answers the same way.
    let removals: Vec<(&str, Vec<String>)> = vec![
        ("one key", vec!["Team".to_string()]),
        (
            "several keys",
            vec!["Env".to_string(), "Tier".to_string(), "Empty".to_string()],
        ),
        ("a key that was never there", vec!["Absent".to_string()]),
        ("a key at the length bound", vec![longest_key.clone()]),
        ("a key of punctuation", vec![punctuation_key.clone()]),
    ];
    for (case, removed) in &removals {
        for attempt in 1..=2 {
            let case = format!("UntagResource {case} (attempt {attempt})");
            harness
                .expect_ok(&case, "DELETE", &untag_path(&arn, removed), None)
                .await;
            for key in removed {
                expected.remove(key);
            }
            assert_eq!(
                listed_tags(&harness, &case, &arn).await,
                expected,
                "case {case:?}: the named keys must be gone and the rest kept"
            );
        }
    }

    // Removing every remaining key leaves an empty set, which is still an answer: the
    // resource exists and carries no tags.
    let rest: Vec<String> = expected.keys().cloned().collect();
    harness
        .expect_ok(
            "remove every remaining key",
            "DELETE",
            &untag_path(&arn, &rest),
            None,
        )
        .await;
    assert!(
        listed_tags(&harness, "an untagged deployment", &arn)
            .await
            .is_empty(),
        "a deployment with no tags must report an empty tag map"
    );
}

#[tokio::test]
async fn an_existing_resources_tag_set_is_judged_against_the_model() {
    let harness = Harness::new();
    let id = deployment_to_tag(&harness, "shape-checks").await;
    let arn = deployment_arn(&id);
    let expected = created_with();

    // `TagResource`: the map's entry count and each key's and value's own shape. One
    // row per published bound, one past it; the rows inside the bounds are covered by
    // the round-trip suite above.
    let refusals: Vec<(&str, Value)> = vec![
        ("no tags at all (the map's minimum is one entry)", json!({})),
        ("one entry past the map's maximum", tags("k", MAX_TAGS + 1)),
        (
            "a key one character past its bound",
            json!({"k".repeat(MAX_TAG_KEY_LEN + 1): "v"}),
        ),
        ("an empty key", json!({"": "v"})),
        ("a key outside the key pattern", json!({"no\u{a3}good": "v"})),
        (
            "a value one character past its bound",
            json!({"k": "v".repeat(MAX_TAG_VALUE_LEN + 1)}),
        ),
        (
            "a value outside the value pattern",
            json!({"k": "no\u{a3}good"}),
        ),
    ];
    for (case, tags) in refusals {
        harness
            .expect_refused(
                case,
                "POST",
                &tags_path(&arn),
                Some(&json!({"tags": tags})),
                400,
                "ValidationException",
            )
            .await;
        assert_eq!(
            listed_tags(&harness, case, &arn).await,
            expected,
            "case {case:?}: a refused TagResource must write nothing"
        );
    }

    // A tag set at the maximum is accepted, so the refusal above is the bound and not
    // the size.
    let room = MAX_TAGS - expected.len();
    harness
        .expect_ok(
            "a tag set at the maximum",
            "POST",
            &tags_path(&arn),
            Some(&json!({"tags": tags("bulk", room)})),
        )
        .await;
    assert_eq!(
        listed_tags(&harness, "a tag set at the maximum", &arn)
            .await
            .len(),
        MAX_TAGS,
        "the resource must carry the maximum tag set"
    );
    // And one more tag would take the resource's own set past that maximum.
    harness
        .expect_refused(
            "one tag past the resource's maximum",
            "POST",
            &tags_path(&arn),
            Some(&json!({"tags": {"OneTooMany": "v"}})),
            400,
            "ValidationException",
        )
        .await;

    // `UntagResource`: the key list's own shape, judged the same way.
    let key_refusals: Vec<(&str, Vec<String>)> = vec![
        ("one key past the list's maximum", keys("k", MAX_TAGS + 1)),
        (
            "a key one character past its bound",
            vec!["k".repeat(MAX_TAG_KEY_LEN + 1)],
        ),
        ("an empty key", vec![String::new()]),
        (
            "a key outside the key pattern",
            vec!["no\u{a3}good".to_string()],
        ),
    ];
    let before = listed_tags(&harness, "before the key refusals", &arn).await;
    for (case, keys) in key_refusals {
        harness
            .expect_refused(
                case,
                "DELETE",
                &untag_path(&arn, &keys),
                None,
                400,
                "ValidationException",
            )
            .await;
        assert_eq!(
            listed_tags(&harness, case, &arn).await,
            before,
            "case {case:?}: a refused UntagResource must remove nothing"
        );
    }
}
