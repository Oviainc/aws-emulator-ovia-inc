// SPDX-License-Identifier: CC0-1.0

//! Tagging a Region switch plan, over the wire.
//!
//! The rule under test: the tags a plan carries are STATE, so `ListTagsForResource`
//! reports exactly the key/value set the calls that wrote them left on that plan —
//! `CreatePlan`'s `tags`, then every `TagResource` merged in and every `UntagResource`
//! key removed — and an ARN no plan is stored under is refused with the modelled
//! `ResourceNotFoundException` instead of answered with an empty set.
//!
//! Every case here drives the emulator through the same `MultiServiceRouter::dispatch`
//! the router serves on a socket, so what these tests observe is what an SDK observes:
//! `awsJson1_0`, `POST /`, `X-Amz-Target: ArcRegionSwitch.<Operation>`.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_arcregionswitch_arcregionswitch::state::{
    MAX_TAGS_PER_PLAN, TAG_KEY_MAX_LENGTH, TAG_VALUE_MAX_LENGTH,
};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// The canonical account id every ARN a generated emulator mints carries
/// (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// `(key, value)` pairs, the way a case row writes a tag set.
type Pairs<'a> = Vec<(&'a str, &'a str)>;

/// One `TagResource` merge case: its name, the tags the plan starts with, the request's
/// tags, and the tags the plan must carry afterwards.
type MergeCase<'a> = (&'a str, Pairs<'a>, Pairs<'a>, Pairs<'a>);

/// One `UntagResource` case: its name, the tags the plan starts with, the keys removed,
/// and the tags the plan must carry afterwards.
type UntagCase<'a> = (&'a str, Pairs<'a>, Vec<&'a str>, Pairs<'a>);

/// A router carrying this one service on a fresh shared engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // This suite exercises operation behaviour, not request signing.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_arcregionswitch_arcregionswitch::EmulatedService::new(engine),
    );
    router
}

/// Call one operation and return its HTTP status and parsed body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("ArcRegionSwitch.{operation}"))
        // The router keys on the signing name in the SigV4 credential scope.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDTEST/20260101/us-east-1/arc-region-switch/aws4_request, \
             SignedHeaders=host, Signature=00",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of an error response: the `__type` member's shape name.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// A `CreatePlan` request carrying the required members, plus `tags` when given.
fn create_request(name: &str, tags: Option<&BTreeMap<String, String>>) -> Value {
    let mut request = json!({
        "name": name,
        "regions": ["us-east-1", "us-west-2"],
        "recoveryApproach": "activePassive",
        "executionRole": format!("arn:aws:iam::{ACCOUNT}:role/ArcRegionSwitchRole"),
        "workflows": [{
            "workflowTargetAction": "activate",
            "workflowDescription": "the activate workflow",
            "steps": [{
                "name": "approve",
                "executionBlockType": "ManualApproval",
                "executionBlockConfiguration": {
                    "executionApprovalConfig": {
                        "approvalRole": format!("arn:aws:iam::{ACCOUNT}:role/Approver"),
                        "timeoutMinutes": 60,
                    }
                }
            }],
        }],
    });
    if let Some(tags) = tags {
        request["tags"] = tags_value(tags);
    }
    request
}

/// Create a plan and return its ARN.
async fn create_plan(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    create_tagged_plan(router, case, name, None).await
}

/// Create a plan carrying `tags` and return its ARN.
async fn create_tagged_plan(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    tags: Option<&BTreeMap<String, String>>,
) -> String {
    let (status, created) = call(router, "CreatePlan", &create_request(name, tags)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreatePlan expected 200, got {status} with {created}"
    );
    created["plan"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A tag set as the wire carries it.
fn tags_value(tags: &BTreeMap<String, String>) -> Value {
    let mut map = Map::new();
    for (key, value) in tags {
        map.insert(key.clone(), Value::String(value.clone()));
    }
    Value::Object(map)
}

/// A tag set from `(key, value)` pairs.
fn tag_set(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The tags `ListTagsForResource` reports for `arn`, as a comparable map.
async fn list_tags(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> BTreeMap<String, String> {
    let (status, listed) = call(router, "ListTagsForResource", &json!({"arn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource expected 200, got {status} with {listed}"
    );
    let reported = listed.get("resourceTags").unwrap_or(&Value::Null);
    let Some(object) = reported.as_object() else {
        panic!("case {case:?}: expected a resourceTags map, got {listed}");
    };
    object
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value.as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// `TagResource` and assert it was accepted.
async fn tag_resource(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    tags: &BTreeMap<String, String>,
) {
    let request = json!({"arn": arn, "tags": tags_value(tags)});
    let (status, body) = call(router, "TagResource", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: TagResource expected 200, got {status} with {body}"
    );
}

/// `UntagResource` and assert it was accepted.
async fn untag_resource(router: &MultiServiceRouter, case: &str, arn: &str, keys: &[&str]) {
    let request = json!({"arn": arn, "resourceTagKeys": keys});
    let (status, body) = call(router, "UntagResource", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: UntagResource expected 200, got {status} with {body}"
    );
}

/// `n` distinct tags, for the cases about how many a plan may carry.
fn bulk_tags(n: usize, prefix: &str) -> BTreeMap<String, String> {
    (0..n)
        .map(|i| (format!("{prefix}-{i:03}"), format!("value-{i:03}")))
        .collect()
}

/// Whatever `TagResource` writes, `ListTagsForResource` reports back key for key.
///
/// The rows differ along what the rule generalises over: how many tags, the length and
/// character class of the keys and values, and the empty set.
#[tokio::test]
async fn tag_sets_written_by_tag_resource_read_back_through_list_tags_for_resource() {
    let router = router();
    // `TagKey` is len 1..128 and `TagValue` len 0..256
    // (`smithy_query(query="shape", shape="Tags,TagKey,TagValue",
    //  model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`),
    // so both published maxima appear below; one past either bound is refused, which
    // `a_tag_key_or_value_outside_its_modelled_bounds_is_refused` pins.
    let longest_key = "k".repeat(TAG_KEY_MAX_LENGTH);
    let longest_value = "v".repeat(TAG_VALUE_MAX_LENGTH);
    let cases: Vec<(&str, BTreeMap<String, String>)> = vec![
        ("no tags at all", BTreeMap::new()),
        ("one tag", tag_set(&[("env", "test")])),
        (
            "several tags",
            tag_set(&[
                ("env", "prod"),
                ("team", "resilience"),
                ("cost-center", "4711"),
            ]),
        ),
        ("an empty value, the shortest one", tag_set(&[("flag", "")])),
        (
            "the longest key and the longest value",
            tag_set(&[(longest_key.as_str(), longest_value.as_str())]),
        ),
        (
            "keys and values outside ASCII",
            tag_set(&[("Umgebung", "Produktion"), ("ステージ", "本番")]),
        ),
        (
            "the documented maximum number of tags",
            bulk_tags(MAX_TAGS_PER_PLAN, "bulk"),
        ),
    ];

    for (index, (case, tags)) in cases.iter().enumerate() {
        let arn = create_plan(&router, case, &format!("tagged-{index}")).await;
        // A plan created without tags starts out carrying none.
        let initial = list_tags(&router, case, &arn).await;
        assert!(
            initial.is_empty(),
            "case {case:?}: expected a new plan to carry no tags, got {initial:?}"
        );
        tag_resource(&router, case, &arn, tags).await;
        let listed = list_tags(&router, case, &arn).await;
        assert_eq!(
            &listed, tags,
            "case {case:?}: expected ListTagsForResource to report {tags:?}, got {listed:?}"
        );
        // A read changes nothing a later caller sees.
        let again = list_tags(&router, case, &arn).await;
        assert_eq!(
            again, listed,
            "case {case:?}: expected a second ListTagsForResource to report {listed:?}, \
             got {again:?}"
        );
        // Tags belong to the plan they were written to, not to the service.
        let other = create_plan(&router, case, &format!("untagged-{index}")).await;
        let other_tags = list_tags(&router, case, &other).await;
        assert!(
            other_tags.is_empty(),
            "case {case:?}: expected another plan to carry no tags, got {other_tags:?}"
        );
    }
}

/// The tags `CreatePlan` was given are the tags the plan starts out carrying.
#[tokio::test]
async fn tags_supplied_at_create_plan_are_reported_by_list_tags_for_resource() {
    let router = router();
    let cases: Vec<(&str, BTreeMap<String, String>)> = vec![
        ("no tags member at all", BTreeMap::new()),
        ("an explicitly empty tag map", BTreeMap::new()),
        ("one tag", tag_set(&[("env", "test")])),
        (
            "two tags",
            tag_set(&[("env", "test"), ("team", "resilience")]),
        ),
        (
            "the documented maximum number of tags",
            bulk_tags(MAX_TAGS_PER_PLAN, "created"),
        ),
    ];

    for (index, (case, tags)) in cases.iter().enumerate() {
        // The first row omits the member; the rest send it.
        let supplied = if index == 0 { None } else { Some(tags) };
        let arn = create_tagged_plan(&router, case, &format!("created-{index}"), supplied).await;
        let listed = list_tags(&router, case, &arn).await;
        assert_eq!(
            &listed, tags,
            "case {case:?}: expected ListTagsForResource to report {tags:?}, got {listed:?}"
        );
    }
}

/// `TagResource` MERGES: a new key is added, a known key takes the new value, and a key
/// the request does not mention keeps the value it had.
#[tokio::test]
async fn tag_resource_merges_into_the_tags_a_plan_already_carries() {
    let router = router();
    // (case, tags the plan starts with, the request's tags, what the plan must carry)
    let cases: Vec<MergeCase> = vec![
        (
            "a disjoint key is added",
            vec![("env", "test")],
            vec![("team", "resilience")],
            vec![("env", "test"), ("team", "resilience")],
        ),
        (
            "a known key is overwritten",
            vec![("env", "test")],
            vec![("env", "prod")],
            vec![("env", "prod")],
        ),
        (
            "some keys overwritten, some added, some untouched",
            vec![("env", "test"), ("team", "resilience"), ("keep", "me")],
            vec![("env", "prod"), ("owner", "sre")],
            vec![
                ("env", "prod"),
                ("keep", "me"),
                ("owner", "sre"),
                ("team", "resilience"),
            ],
        ),
        (
            "an empty request leaves every tag alone",
            vec![("env", "test"), ("team", "resilience")],
            vec![],
            vec![("env", "test"), ("team", "resilience")],
        ),
        (
            "re-sending the same tags is idempotent",
            vec![("env", "test")],
            vec![("env", "test")],
            vec![("env", "test")],
        ),
        (
            "a value is replaced by the empty value",
            vec![("env", "test")],
            vec![("env", "")],
            vec![("env", "")],
        ),
    ];

    for (index, (case, initial, patch, expected)) in cases.iter().enumerate() {
        let arn = create_tagged_plan(
            &router,
            case,
            &format!("merge-{index}"),
            Some(&tag_set(initial)),
        )
        .await;
        tag_resource(&router, case, &arn, &tag_set(patch)).await;
        let listed = list_tags(&router, case, &arn).await;
        let expected = tag_set(expected);
        assert_eq!(
            listed, expected,
            "case {case:?}: expected ListTagsForResource to report {expected:?}, got {listed:?}"
        );
    }
}

/// `UntagResource` removes exactly the keys it names, and naming a key the plan does not
/// carry is not an error.
#[tokio::test]
async fn untag_resource_removes_only_the_keys_it_names() {
    let router = router();
    // (case, tags the plan starts with, the keys removed, what the plan must carry)
    let cases: Vec<UntagCase> = vec![
        (
            "one of several keys",
            vec![("env", "test"), ("team", "resilience")],
            vec!["env"],
            vec![("team", "resilience")],
        ),
        (
            "every key",
            vec![("env", "test"), ("team", "resilience")],
            vec!["env", "team"],
            vec![],
        ),
        (
            "no keys at all",
            vec![("env", "test")],
            vec![],
            vec![("env", "test")],
        ),
        (
            "a key the plan never carried",
            vec![("env", "test")],
            vec!["absent"],
            vec![("env", "test")],
        ),
        (
            "one present key and one absent key",
            vec![("env", "test"), ("team", "resilience")],
            vec!["env", "absent"],
            vec![("team", "resilience")],
        ),
        (
            "a key whose value is empty",
            vec![("flag", "")],
            vec!["flag"],
            vec![],
        ),
        (
            "every key of a plan carrying none",
            vec![],
            vec!["env"],
            vec![],
        ),
    ];

    for (index, (case, initial, removed, expected)) in cases.iter().enumerate() {
        let arn = create_tagged_plan(
            &router,
            case,
            &format!("untag-{index}"),
            Some(&tag_set(initial)),
        )
        .await;
        untag_resource(&router, case, &arn, removed).await;
        let listed = list_tags(&router, case, &arn).await;
        let expected = tag_set(expected);
        assert_eq!(
            listed, expected,
            "case {case:?}: expected ListTagsForResource to report {expected:?}, got {listed:?}"
        );
        // Removing the same keys again lands on the same state: removal is idempotent.
        untag_resource(&router, case, &arn, removed).await;
        let again = list_tags(&router, case, &arn).await;
        assert_eq!(
            again, expected,
            "case {case:?}: expected a repeated UntagResource to leave {expected:?}, \
             got {again:?}"
        );
    }
}

/// Tag writes and removals accumulate on the record, so a sequence of them is observable
/// as the set the last call left.
#[tokio::test]
async fn a_sequence_of_tag_and_untag_calls_is_reported_as_its_net_effect() {
    let router = router();
    let case = "tag, untag, tag again";
    let arn = create_plan(&router, case, "sequence").await;

    tag_resource(&router, case, &arn, &tag_set(&[("a", "1"), ("b", "2")])).await;
    untag_resource(&router, case, &arn, &["a"]).await;
    tag_resource(&router, case, &arn, &tag_set(&[("b", "3"), ("c", "4")])).await;
    untag_resource(&router, case, &arn, &["zzz"]).await;

    let listed = list_tags(&router, case, &arn).await;
    let expected = tag_set(&[("b", "3"), ("c", "4")]);
    assert_eq!(
        listed, expected,
        "case {case:?}: expected ListTagsForResource to report {expected:?}, got {listed:?}"
    );
}

/// Every tagging operation refuses an ARN no plan is stored under with the modelled
/// `ResourceNotFoundException` (HTTP 404) — including one whose plan was deleted.
#[tokio::test]
async fn tagging_an_arn_no_plan_is_stored_under_is_refused_with_404() {
    let router = router();
    let live = create_tagged_plan(
        &router,
        "a live plan",
        "still-here",
        Some(&tag_set(&[("env", "test")])),
    )
    .await;

    // A plan created and then deleted: its ARN matches `PlanArn` and once named a plan.
    let deleted = create_plan(&router, "a deleted plan", "gone-soon").await;
    let (status, body) = call(&router, "DeletePlan", &json!({"arn": &deleted})).await;
    assert_eq!(
        status, 200,
        "case \"a deleted plan\": DeletePlan expected 200, got {status} with {body}"
    );

    let cases: Vec<(&str, String)> = vec![
        (
            "a plan name nothing was created under",
            format!("arn:aws:arc-region-switch::{ACCOUNT}:plan/never-created:a1b2c3"),
        ),
        (
            "the shortest plan name the pattern allows",
            format!("arn:aws:arc-region-switch::{ACCOUNT}:plan/a:000000"),
        ),
        (
            "another account's plan",
            "arn:aws:arc-region-switch::123456789012:plan/elsewhere:zzzzzz".to_string(),
        ),
        (
            "a non-default partition",
            format!("arn:aws-us-gov:arc-region-switch::{ACCOUNT}:plan/never-created:a1b2c3"),
        ),
        ("a plan that was deleted", deleted),
        (
            "the live plan's name with a different suffix",
            {
                let (name, suffix) = live.rsplit_once(':').expect("a plan ARN ends in :suffix");
                let flipped: String = suffix
                    .chars()
                    .map(|c| if c == 'a' { 'b' } else { 'a' })
                    .collect();
                format!("{name}:{flipped}")
            },
        ),
    ];

    for (case, arn) in &cases {
        for operation in ["ListTagsForResource", "TagResource", "UntagResource"] {
            let request = match operation {
                "TagResource" => json!({"arn": arn, "tags": {"env": "test"}}),
                "UntagResource" => json!({"arn": arn, "resourceTagKeys": ["env"]}),
                _ => json!({"arn": arn}),
            };
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 404,
                "case {case:?}: {operation} on {arn} expected 404, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: {operation} on {arn} expected ResourceNotFoundException, \
                 got {body}"
            );
        }
    }

    // The refusals left the live plan's tags alone.
    let listed = list_tags(&router, "a live plan", &live).await;
    let expected = tag_set(&[("env", "test")]);
    assert_eq!(
        listed, expected,
        "case \"a live plan\": expected {expected:?} after the refusals, got {listed:?}"
    );
}

/// A plan carries at most the documented maximum of tags, however they arrive: AWS
/// documents "You can add a maximum of 50 tags in Region switch" on
/// `TagResourceRequest$tags`, which `state::MAX_TAGS_PER_PLAN` holds.
#[tokio::test]
async fn a_plan_is_refused_more_than_the_documented_maximum_of_tags() {
    let router = router();

    // At the bound: the maximum itself is accepted, by `CreatePlan` and by `TagResource`.
    let at_bound = bulk_tags(MAX_TAGS_PER_PLAN, "at");
    let created = create_tagged_plan(&router, "at the maximum", "at-bound", Some(&at_bound)).await;
    let listed = list_tags(&router, "at the maximum", &created).await;
    assert_eq!(
        listed.len(),
        MAX_TAGS_PER_PLAN,
        "case \"at the maximum\": expected {MAX_TAGS_PER_PLAN} tags, got {}",
        listed.len()
    );

    // Overwriting a key a plan at the maximum already carries does not push it past the
    // maximum, so it is still accepted.
    let overwrite = tag_set(&[("at-000", "rewritten")]);
    tag_resource(&router, "overwrite at the maximum", &created, &overwrite).await;
    let listed = list_tags(&router, "overwrite at the maximum", &created).await;
    assert_eq!(
        listed.get("at-000").map(String::as_str),
        Some("rewritten"),
        "case \"overwrite at the maximum\": expected the overwritten value, got {listed:?}"
    );
    assert_eq!(
        listed.len(),
        MAX_TAGS_PER_PLAN,
        "case \"overwrite at the maximum\": expected {MAX_TAGS_PER_PLAN} tags, got {}",
        listed.len()
    );

    // One past the bound, by three routes. Each must be refused with the modelled
    // client error, and must leave the stored tags exactly as they were.
    let past_bound = bulk_tags(MAX_TAGS_PER_PLAN + 1, "past");
    let fresh = create_plan(&router, "one past the maximum, in one call", "past-bound").await;
    let seed = bulk_tags(MAX_TAGS_PER_PLAN, "seed");
    let seeded = create_tagged_plan(
        &router,
        "one past the maximum, by accumulation",
        "seeded",
        Some(&seed),
    )
    .await;

    let cases: Vec<(&str, &str, Value, BTreeMap<String, String>)> = vec![
        (
            "CreatePlan with one tag past the maximum",
            "CreatePlan",
            create_request("rejected", Some(&past_bound)),
            BTreeMap::new(),
        ),
        (
            "TagResource with one tag past the maximum in one call",
            "TagResource",
            json!({"arn": &fresh, "tags": tags_value(&past_bound)}),
            BTreeMap::new(),
        ),
        (
            "TagResource pushing one tag past the maximum by accumulation",
            "TagResource",
            json!({"arn": &seeded, "tags": {"one-more": "value"}}),
            seed.clone(),
        ),
    ];

    for (case, operation, request, unchanged) in &cases {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {operation} expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "IllegalArgumentException",
            "case {case:?}: {operation} expected IllegalArgumentException, got {body}"
        );
        if *operation == "TagResource" {
            let arn = request["arn"].as_str().unwrap_or_default();
            let listed = list_tags(&router, case, arn).await;
            assert_eq!(
                &listed, unchanged,
                "case {case:?}: expected the refusal to leave {unchanged:?}, got {listed:?}"
            );
        }
    }

    // The refused CreatePlan stored nothing: no plan carries the rejected name.
    let (status, plans) = call(&router, "ListPlans", &json!({})).await;
    assert_eq!(
        status, 200,
        "case \"the refused CreatePlan stored nothing\": ListPlans expected 200, got {status}"
    );
    let rejected = plans["plans"]
        .as_array()
        .map(|plans| {
            plans
                .iter()
                .any(|plan| plan["name"].as_str() == Some("rejected"))
        })
        .unwrap_or(false);
    assert!(
        !rejected,
        "case \"the refused CreatePlan stored nothing\": expected no plan named \"rejected\", \
         got {plans}"
    );
}

/// A tag key or value outside the model's own bounds is refused with HTTP 400, and
/// nothing is stored.
///
/// `TagKey` is `@length(1..128)` and `TagValue` is `@length(0..256)`. The boundary
/// validator reaches a map's VALUE target, so an over-long value is refused there as
/// `ValidationException`; it does not reach a map's KEY target, so a key outside its
/// bounds is refused by the handler with the service's modelled
/// `IllegalArgumentException`. Either way the request is a 400 and the stored tags are
/// untouched, which is what a caller observes.
#[tokio::test]
async fn a_tag_key_or_value_outside_its_modelled_bounds_is_refused() {
    let router = router();
    let arn = create_plan(&router, "bounds", "bounds").await;

    // The published bounds themselves are accepted (pinned by the round-trip case), so
    // these rows are one past each of them: 129 characters of key, 257 of value, and the
    // empty key, one short of `TagKey`'s minimum of 1.
    let too_long_key = "k".repeat(TAG_KEY_MAX_LENGTH + 1);
    let too_long_value = "v".repeat(TAG_VALUE_MAX_LENGTH + 1);
    // (case, operation, request, the error code the refusal carries)
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "TagResource with a key one character too long",
            "TagResource",
            json!({"arn": &arn, "tags": {too_long_key.clone(): "value"}}),
            "IllegalArgumentException",
        ),
        (
            "TagResource with a value one character too long",
            "TagResource",
            json!({"arn": &arn, "tags": {"key": too_long_value.clone()}}),
            "ValidationException",
        ),
        (
            "TagResource with an empty key",
            "TagResource",
            json!({"arn": &arn, "tags": {"": "value"}}),
            "IllegalArgumentException",
        ),
        (
            "CreatePlan with a key one character too long",
            "CreatePlan",
            {
                let mut request = create_request("bad-tag-key", None);
                request["tags"] = json!({too_long_key.clone(): "value"});
                request
            },
            "IllegalArgumentException",
        ),
        (
            "CreatePlan with a value one character too long",
            "CreatePlan",
            {
                let mut request = create_request("bad-tag-value", None);
                request["tags"] = json!({"key": too_long_value.clone()});
                request
            },
            "ValidationException",
        ),
        (
            "UntagResource with a key one character too long",
            "UntagResource",
            json!({"arn": &arn, "resourceTagKeys": [too_long_key.clone()]}),
            "ValidationException",
        ),
        (
            "UntagResource with an empty key",
            "UntagResource",
            json!({"arn": &arn, "resourceTagKeys": [""]}),
            "ValidationException",
        ),
    ];

    for (case, operation, request, expected_code) in &cases {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {operation} expected 400, got {status} with {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            &code, expected_code,
            "case {case:?}: {operation} expected {expected_code}, got {body}"
        );
        let listed = list_tags(&router, case, &arn).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: expected the refusal to store nothing, got {listed:?}"
        );
    }
}
