// SPDX-License-Identifier: CC0-1.0

//! What `CreateUsagePlan` ACCEPTS at the edge of each bound it enforces, and that a
//! refused create persists nothing.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder, the
//! `CreateRestApi`/`CreateDeployment` helpers and the `CreateUsagePlan`/`GetUsagePlan`
//! calls are this crate's existing harness, reused from `rest_api_lifecycle`,
//! `stage_lifecycle` and `usage_plan_lifecycle`, which asserts the create/read round trip
//! itself.
//!
//! The RULES under test, asserted over SETS of inputs rather than one recorded call:
//!
//! * the modelled `@required` `name` must be present, else the request is refused before
//!   the handler runs (400 `ValidationException`, the refusal this service states every
//!   modelled-constraint violation with — see `validation_message_rules`);
//! * every caller-sized member of the request is accepted at this service's bound for it
//!   and refused one past it with 400 `BadRequestException`, whether the member is a
//!   string, a map of tags, the list of API stage associations or the per-method throttle
//!   map an association carries;
//! * a tag riding on the create is held to the same rule `TagResource` applies, so a tag
//!   this service refuses on one path cannot be stored through the other;
//! * a refused create persists NOTHING: the store holds exactly the plans the accepted
//!   creates made, and each of those is reportable by `GetUsagePlan`.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    service_state, MAX_COLLECTION_ENTRIES, MAX_STAGES_PER_API, MAX_STRING_CHARS, MAX_TAGS,
    MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS,
};

use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Gateway};
use crate::stage_lifecycle::create_deployed_stage;
use crate::usage_plan_lifecycle::{create_usage_plan, get_usage_plan, tag_map};

/// A caller-supplied string of `count` characters.
fn chars(count: usize) -> String {
    "a".repeat(count)
}

/// A per-method throttle map of `count` distinct methods, each carrying limits API Gateway
/// can meter.
fn method_throttle(count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|n| {
                (
                    format!("/r{n}/GET"),
                    json!({ "burstLimit": 1, "rateLimit": 1.0 }),
                )
            })
            .collect(),
    )
}

/// How many usage plans the STORE holds, read from the engine the emulator is mounted on.
///
/// What a client sees is the create's own answer; this is how a REFUSED create is checked
/// to have written nothing, which no reporting operation of this service could show (a
/// plan it refused to create has no identifier to read back).
fn stored_plan_count(gateway: &Gateway) -> usize {
    let state = service_state(gateway.engine())
        .expect("the service is mounted on the shared in-memory engine");
    let store = state.lock();
    store.usage_plans.len()
}

/// The names of every stored usage plan; see [`stored_plan_count`].
fn stored_plan_names(gateway: &Gateway) -> Vec<String> {
    let state = service_state(gateway.engine())
        .expect("the service is mounted on the shared in-memory engine");
    let store = state.lock();
    store
        .usage_plans
        .values()
        .map(|plan| plan.name.clone())
        .collect()
}

/// Assert `request` is refused with `status`/`code` and that nothing was stored for it.
async fn assert_refused_and_nothing_stored(
    gateway: &Gateway,
    case: &str,
    request: Value,
    status: u16,
    code: &str,
) {
    let before = stored_plan_count(gateway);
    let name = request
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let answer = gateway.call("POST", "/usageplans", Some(request)).await;
    assert_failure(&answer, status, code, case);
    assert_eq!(
        stored_plan_count(gateway),
        before,
        "case {case}: a refused create must store no plan, got {:?}",
        stored_plan_names(gateway)
    );
    if !name.is_empty() {
        assert!(
            !stored_plan_names(gateway).contains(&name),
            "case {case}: the refused plan {name:?} must not be stored, got {:?}",
            stored_plan_names(gateway)
        );
    }
}

/// `count` stage associations, each naming a stored stage of a stored API, drawn from
/// `stages` in order.
fn api_stages(stages: &[(String, String)], count: usize) -> Value {
    Value::Array(
        stages
            .iter()
            .take(count)
            .map(|(api, stage)| json!({ "apiId": api, "stage": stage }))
            .collect(),
    )
}

/// Deploy `count` distinct stages, spread over as many APIs as the per-API stage quota
/// needs, and answer each `(apiId, stageName)` pair.
async fn deploy_stages(gateway: &Gateway, count: usize) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut api: Option<String> = None;
    for n in 0..count {
        if n % MAX_STAGES_PER_API == 0 {
            let (id, _) = create_api(gateway, &format!("usage-plan-bounds-{n}")).await;
            api = Some(id);
        }
        let id = api.clone().expect("an API was created for this block of stages");
        let stage = format!("s{n}");
        create_deployed_stage(gateway, &id, &stage).await;
        pairs.push((id, stage));
    }
    pairs
}

#[tokio::test]
async fn the_required_name_must_be_present() {
    let gateway = Gateway::new();
    // Rows differ in what the request DOES carry beside the missing name: nothing, an
    // optional string, an optional structure, and an empty collection — so the refusal is
    // shown to be about the required member and not about an empty body.
    let rows: Vec<(&str, Value)> = vec![
        ("an empty request", json!({})),
        ("a description only", json!({ "description": "the plan" })),
        (
            "a quota only",
            json!({ "quota": { "limit": 10, "offset": 0, "period": "DAY" } }),
        ),
        ("an empty tag map", json!({ "tags": {} })),
    ];
    for (case, request) in rows {
        assert_refused_and_nothing_stored(
            &gateway,
            case,
            request,
            400,
            "ValidationException",
        )
        .await;
    }
}

#[tokio::test]
async fn a_caller_sized_member_is_accepted_at_its_bound_and_refused_one_past_it() {
    let gateway = Gateway::new();
    // One more stage than the API stage list may carry, so both ends of that bound can be
    // exercised with entries that each name a real stage.
    let stages = deploy_stages(&gateway, MAX_COLLECTION_ENTRIES + 1).await;

    // Rows span every caller-sized member of the request and both kinds of bound it has:
    // a string's character count (`name`, `description`, a tag key, a tag value) and a
    // collection's entry count (the tag map, the API stage list, a stage's per-method
    // throttle map). Each member appears twice, AT its bound and one past it.
    let accepted: Vec<(String, Value)> = vec![
        (
            format!("a name of {MAX_STRING_CHARS} characters"),
            json!({ "name": chars(MAX_STRING_CHARS) }),
        ),
        (
            format!("a description of {MAX_STRING_CHARS} characters"),
            json!({ "name": "p-description-at-bound", "description": chars(MAX_STRING_CHARS) }),
        ),
        (
            format!("{MAX_TAGS} tags"),
            json!({ "name": "p-tags-at-bound", "tags": tag_map(MAX_TAGS) }),
        ),
        (
            format!("a tag key of {MAX_TAG_KEY_CHARS} characters"),
            json!({ "name": "p-tag-key-at-bound", "tags": { chars(MAX_TAG_KEY_CHARS): "v" } }),
        ),
        (
            format!("a tag value of {MAX_TAG_VALUE_CHARS} characters"),
            json!({ "name": "p-tag-value-at-bound", "tags": { "k": chars(MAX_TAG_VALUE_CHARS) } }),
        ),
        (
            format!("{MAX_COLLECTION_ENTRIES} api stages"),
            json!({
                "name": "p-stages-at-bound",
                "apiStages": api_stages(&stages, MAX_COLLECTION_ENTRIES),
            }),
        ),
        (
            format!("{MAX_COLLECTION_ENTRIES} per-method throttle entries"),
            json!({
                "name": "p-method-throttle-at-bound",
                "apiStages": [{
                    "apiId": stages[0].0,
                    "stage": stages[0].1,
                    "throttle": method_throttle(MAX_COLLECTION_ENTRIES),
                }],
            }),
        ),
    ];
    for (case, request) in &accepted {
        let created = create_usage_plan(&gateway, case, request.clone()).await;
        let id = string_member(&created, "id", case);
        // The accepted request was STORED, not merely answered: the plan reads back.
        let read = get_usage_plan(&gateway, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetUsagePlan, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, created,
            "case {case}: GetUsagePlan must report the stored plan, got {:?} after {:?}",
            read.body, created
        );
    }

    let refused: Vec<(String, Value)> = vec![
        (
            format!("a name of {} characters", MAX_STRING_CHARS + 1),
            json!({ "name": chars(MAX_STRING_CHARS + 1) }),
        ),
        (
            format!("a description of {} characters", MAX_STRING_CHARS + 1),
            json!({
                "name": "p-description-past-bound",
                "description": chars(MAX_STRING_CHARS + 1),
            }),
        ),
        (
            format!("{} tags", MAX_TAGS + 1),
            json!({ "name": "p-tags-past-bound", "tags": tag_map(MAX_TAGS + 1) }),
        ),
        (
            format!("a tag key of {} characters", MAX_TAG_KEY_CHARS + 1),
            json!({
                "name": "p-tag-key-past-bound",
                "tags": { chars(MAX_TAG_KEY_CHARS + 1): "v" },
            }),
        ),
        (
            format!("a tag value of {} characters", MAX_TAG_VALUE_CHARS + 1),
            json!({
                "name": "p-tag-value-past-bound",
                "tags": { "k": chars(MAX_TAG_VALUE_CHARS + 1) },
            }),
        ),
        (
            format!("{} api stages", MAX_COLLECTION_ENTRIES + 1),
            json!({
                "name": "p-stages-past-bound",
                "apiStages": api_stages(&stages, MAX_COLLECTION_ENTRIES + 1),
            }),
        ),
        (
            format!("{} per-method throttle entries", MAX_COLLECTION_ENTRIES + 1),
            json!({
                "name": "p-method-throttle-past-bound",
                "apiStages": [{
                    "apiId": stages[0].0,
                    "stage": stages[0].1,
                    "throttle": method_throttle(MAX_COLLECTION_ENTRIES + 1),
                }],
            }),
        ),
    ];
    for (case, request) in &refused {
        assert_refused_and_nothing_stored(
            &gateway,
            case,
            request.clone(),
            400,
            "BadRequestException",
        )
        .await;
    }

    // The store holds exactly the accepted plans: nothing a refusal carried reached it.
    assert_eq!(
        stored_plan_count(&gateway),
        accepted.len(),
        "expected only the accepted plans to be stored, got {:?}",
        stored_plan_names(&gateway)
    );
}

#[tokio::test]
async fn a_tag_the_service_will_not_store_is_refused_on_the_create_path_too() {
    let gateway = Gateway::new();
    // The same rule `TagResource` applies: a key is non-empty and outside AWS's reserved
    // namespace, which is a prefix whatever its case. Rows span the empty key, the
    // reserved prefix in two casings, and a reserved key beside an acceptable one.
    let refused: Vec<(&str, Value)> = vec![
        ("an empty tag key", json!({ "": "v" })),
        ("the reserved prefix", json!({ "aws:owner": "v" })),
        ("the reserved prefix upper-cased", json!({ "AWS:owner": "v" })),
        (
            "a reserved key beside an acceptable one",
            json!({ "team": "api", "aws:billing": "v" }),
        ),
    ];
    for (case, tags) in &refused {
        assert_refused_and_nothing_stored(
            &gateway,
            case,
            json!({ "name": format!("p-{case}"), "tags": tags }),
            400,
            "BadRequestException",
        )
        .await;
    }

    // A key that merely CONTAINS the reserved text is not reserved, so it is stored and
    // reported: the refusals above are about the prefix and not about the substring.
    let case = "a key containing the reserved text";
    let created = create_usage_plan(
        &gateway,
        case,
        json!({ "name": "p-not-reserved", "tags": { "team:aws:region": "us-east-1" } }),
    )
    .await;
    let id = string_member(&created, "id", case);
    let read = get_usage_plan(&gateway, &id).await;
    assert_eq!(
        read.body["tags"]["team:aws:region"],
        json!("us-east-1"),
        "case {case}: expected the stored tag, got {:?}",
        read.body["tags"]
    );
}
