// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the usage plan key lifecycle: `CreateUsagePlanKey` and
//! `GetUsagePlanKey`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder, the
//! `CreateRestApi`/`CreateDeployment` helpers, `CreateApiKey` and `CreateUsagePlan` are
//! this crate's existing harness, reused from `rest_api_lifecycle`, `stage_lifecycle`,
//! `api_key_lifecycle` and `usage_plan_lifecycle`.
//!
//! The RULES under test, asserted over SETS of inputs rather than one recorded call:
//!
//! * a `UsagePlanKey` is the ASSOCIATION between a stored usage plan and a stored API
//!   key: the create persists it and answers from the stored records — the association's
//!   own `id` and `type`, and the API key's `name` and `value` — so a later
//!   `GetUsagePlanKey` reports the same body, over a table spanning which members the API
//!   key carries, whether its value is caller-supplied or minted, how many keys one plan
//!   meters, how many plans meter one key, and what the plan itself carries;
//! * both identifiers must resolve to stored records, else the request is refused 404
//!   `NotFoundException` — including a key that was stored and then deleted;
//! * `keyType` must be the one type the model declares valid (`API_KEY`), else the
//!   request is refused 400 `BadRequestException` and nothing is persisted;
//! * a key a plan already meters is refused 409 `ConflictException`, and the stored
//!   association is left as it was, while the per-plan association count is capped
//!   (`MAX_USAGE_PLAN_KEYS_PER_PLAN`) and reclaimed with the key it names, so a
//!   create/associate/delete cycle does not accumulate records;
//! * the body's `name` and `value` are the API key's own, so renaming that key is
//!   reported by the association, and reading an association changes nothing a later
//!   caller sees.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::service_state;

use crate::api_key_lifecycle::create_key;
use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};
use crate::stage_lifecycle::create_deployed_stage;
use crate::usage_plan_lifecycle::create_usage_plan;

/// The one key type a `UsagePlanKey` takes. Source: `UsagePlanKey$type`'s own
/// documentation, "Currently, the valid key type is `API_KEY`"
/// (`smithy_query(query="doc", shape="UsagePlanKey", member="type")`).
const API_KEY_TYPE: &str = "API_KEY";

/// AWS's published minimum length for a caller-supplied API key value, as the sibling
/// `api_key_lifecycle` suite documents it: 20 characters.
const MIN_VALUE_CHARS: usize = 20;

/// A caller-supplied API key value of the published minimum length, distinct per `n`: an
/// API key's value is unique per account, so two rows cannot supply the same one.
fn supplied_value(n: usize) -> String {
    let distinct = format!("{n}");
    format!("{}{distinct}", "v".repeat(MIN_VALUE_CHARS - distinct.len()))
}

/// One row of the create/read table: the case name, and the `CreateApiKey` request for
/// the plan at that index (see [`supplied_value`]).
type KeyRow = (&'static str, fn(usize) -> Value);

/// `CreateUsagePlanKey` on `plan` for `key_id` with `key_type`. Shared with the sibling
/// `usage_plan_collection` suite, which meters keys to narrow the plan collection.
pub(crate) async fn create_usage_plan_key(
    gateway: &Gateway,
    plan: &str,
    key_id: &str,
    key_type: &str,
) -> Answer {
    gateway
        .call(
            "POST",
            &format!("/usageplans/{plan}/keys"),
            Some(json!({ "keyId": key_id, "keyType": key_type })),
        )
        .await
}

/// `CreateUsagePlanKey` for an `API_KEY`, asserting the modelled 201 and returning the
/// body.
async fn add_key(gateway: &Gateway, case: &str, plan: &str, key_id: &str) -> Value {
    let answer = create_usage_plan_key(gateway, plan, key_id, API_KEY_TYPE).await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// `GetUsagePlanKey` for `key_id` of `plan`.
async fn get_usage_plan_key(gateway: &Gateway, plan: &str, key_id: &str) -> Answer {
    gateway
        .call("GET", &format!("/usageplans/{plan}/keys/{key_id}"), None)
        .await
}

/// Assert one `UsagePlanKey` body reports the association's own members and the stored
/// API key's name and value.
fn assert_usage_plan_key(body: &Value, case: &str, key: &Value) {
    assert_eq!(
        body["id"], key["id"],
        "case {case}: a usage plan key's id is the API key's id, got {:?}",
        body["id"]
    );
    assert_eq!(
        body["type"],
        json!(API_KEY_TYPE),
        "case {case}: expected type {API_KEY_TYPE}, got {:?}",
        body["type"]
    );
    assert_eq!(
        body["value"], key["value"],
        "case {case}: a usage plan key reports the API key's value, got {:?}",
        body["value"]
    );
    assert_eq!(
        body["name"], key["name"],
        "case {case}: a usage plan key reports the API key's name, got {:?}",
        body["name"]
    );
}

/// Whether the STORE still holds an association for `key_id` on `plan`, read from the
/// engine the emulator is mounted on.
///
/// What a client sees is the 404 the reads above assert; this is how the record itself is
/// checked to be reclaimed rather than merely unreportable, which no operation reports.
fn association_is_stored(gateway: &Gateway, plan: &str, key_id: &str) -> bool {
    let state = service_state(gateway.engine())
        .expect("the service is mounted on the shared in-memory engine");
    let store = state.lock();
    store
        .usage_plans
        .get(plan)
        .is_some_and(|plan| plan.keys.contains_key(key_id))
}

/// How many associations the STORE holds on `plan`; see [`association_is_stored`].
fn stored_association_count(gateway: &Gateway, plan: &str) -> usize {
    let state = service_state(gateway.engine())
        .expect("the service is mounted on the shared in-memory engine");
    let store = state.lock();
    store
        .usage_plans
        .get(plan)
        .map_or(0, |plan| plan.keys.len())
}

/// `UpdateApiKey` replacing `/name` on `key_id`.
async fn rename_key(gateway: &Gateway, key_id: &str, name: &str) -> Answer {
    gateway
        .call(
            "PATCH",
            &format!("/apikeys/{key_id}"),
            Some(json!({
                "patchOperations": [{ "op": "replace", "path": "/name", "value": name }],
            })),
        )
        .await
}

#[tokio::test]
async fn a_created_usage_plan_key_is_persisted_and_read_back_member_for_member() {
    let gateway = Gateway::new();
    let (api, _) = create_api(&gateway, "usage-plan-key-stages").await;
    create_deployed_stage(&gateway, &api, "prod").await;

    // The plans rows are metered against differ in what the plan itself carries: no
    // association at all, an API stage, and the throttle and quota limits the keys are
    // metered by. The rule is about the association, so none of them may change it.
    let plans: Vec<(&str, Value)> = vec![
        ("a bare plan", json!({ "name": "plan-bare" })),
        (
            "a plan with one api stage",
            json!({
                "name": "plan-staged",
                "apiStages": [{ "apiId": api, "stage": "prod" }],
            }),
        ),
        (
            "a metered plan",
            json!({
                "name": "plan-metered",
                "throttle": { "burstLimit": 10, "rateLimit": 5.0 },
                "quota": { "limit": 100, "offset": 0, "period": "DAY" },
            }),
        ),
    ];

    // The key rows differ along what the rule generalises over: which members the API key
    // carries (so `name` is present on some and absent on others), whether its value is
    // caller-supplied or minted by the service, and whether it is enabled. The row's
    // argument is the plan's index: an API key's VALUE is unique per account, so a
    // caller-supplied one differs per plan.
    let key_rows: Vec<KeyRow> = vec![
        ("an unnamed key", |_| json!({})),
        ("a named key", |_| json!({ "name": "metered-key" })),
        ("a named and described key", |_| {
            json!({ "name": "described-key", "description": "the described key" })
        }),
        ("a disabled key", |_| {
            json!({ "name": "disabled-key", "enabled": false })
        }),
        ("a key with a caller-supplied value", |n| {
            json!({ "name": "imported-key", "value": supplied_value(n) })
        }),
        ("a key with a marketplace customer", |_| {
            json!({ "customerId": "marketplace-customer-1" })
        }),
    ];

    for (plan_index, (plan_case, plan_request)) in plans.iter().enumerate() {
        let plan = create_usage_plan(&gateway, plan_case, plan_request.clone()).await;
        let plan_id = string_member(&plan, "id", plan_case);

        for (key_case, key_request) in &key_rows {
            let case = format!("{plan_case} metering {key_case}");
            let key = create_key(&gateway, &case, key_request(plan_index)).await;
            let key_id = string_member(&key, "id", &case);

            let created = add_key(&gateway, &case, &plan_id, &key_id).await;
            assert_usage_plan_key(&created, &case, &key);

            // The round trip: the create answered from the stored association, so the
            // read reports exactly the same body.
            let read = get_usage_plan_key(&gateway, &plan_id, &key_id).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected 200 from GetUsagePlanKey, got {} with {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.body, created,
                "case {case}: GetUsagePlanKey must report the stored association, got {:?} \
                 after {:?}",
                read.body, created
            );
        }
    }
}

#[tokio::test]
async fn one_key_is_metered_by_several_plans_and_one_plan_meters_several_keys() {
    let gateway = Gateway::new();
    // Rows: the counts the association generalises over — several keys in one plan, and
    // one key in several plans. Each association is its own record, so every pair reads
    // back on its own.
    let mut plans: Vec<String> = Vec::new();
    for name in ["plan-a", "plan-b", "plan-c"] {
        let plan = create_usage_plan(&gateway, name, json!({ "name": name })).await;
        plans.push(string_member(&plan, "id", name));
    }

    let mut keys: Vec<Value> = Vec::new();
    for n in 0..4 {
        let case = format!("key {n}");
        keys.push(create_key(&gateway, &case, json!({ "name": format!("key-{n}") })).await);
    }

    for plan_id in &plans {
        for key in &keys {
            let key_id = string_member(key, "id", "a stored key");
            let case = format!("plan {plan_id} metering key {key_id}");
            let created = add_key(&gateway, &case, plan_id, &key_id).await;
            assert_usage_plan_key(&created, &case, key);
        }
    }

    // Every pair is still readable: adding a key to one plan did not move it out of
    // another.
    for plan_id in &plans {
        for key in &keys {
            let key_id = string_member(key, "id", "a stored key");
            let case = format!("GetUsagePlanKey({plan_id}, {key_id})");
            let read = get_usage_plan_key(&gateway, plan_id, &key_id).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected 200, got {} with {:?}",
                read.status, read.body
            );
            assert_usage_plan_key(&read.body, &case, key);
        }
    }
}

#[tokio::test]
async fn a_key_type_outside_the_one_type_the_model_declares_is_refused() {
    let gateway = Gateway::new();
    let plan = create_usage_plan(&gateway, "type-rows", json!({ "name": "plan-types" })).await;
    let plan_id = string_member(&plan, "id", "type-rows");
    let key = create_key(&gateway, "type-rows", json!({ "name": "typed-key" })).await;
    let key_id = string_member(&key, "id", "type-rows");

    // Rows differ in the FORM of the wrong value: the right word in the wrong case, the
    // right word with punctuation or whitespace, a value of another vocabulary, a
    // plural, and the empty string.
    for key_type in [
        "api_key",
        "ApiKey",
        "APIKEY",
        "API_KEY ",
        "API_KEYS",
        "USAGE_PLAN",
        "",
    ] {
        let case = format!("CreateUsagePlanKey(keyType={key_type:?})");
        let answer = create_usage_plan_key(&gateway, &plan_id, &key_id, key_type).await;
        assert_failure(&answer, 400, "BadRequestException", &case);
        // A refused request persists nothing: the association does not exist.
        let read = get_usage_plan_key(&gateway, &plan_id, &key_id).await;
        assert_failure(&read, 404, "NotFoundException", &case);
    }

    // The one type the model declares valid IS accepted, so the refusals above are about
    // the value and not about the operation.
    let created = add_key(&gateway, "the modelled key type", &plan_id, &key_id).await;
    assert_usage_plan_key(&created, "the modelled key type", &key);
}

#[tokio::test]
async fn both_identifiers_must_name_a_stored_record() {
    let gateway = Gateway::new();
    let plan = create_usage_plan(&gateway, "identifiers", json!({ "name": "plan-ids" })).await;
    let plan_id = string_member(&plan, "id", "identifiers");
    let key = create_key(&gateway, "identifiers", json!({ "name": "stored-key" })).await;
    let key_id = string_member(&key, "id", "identifiers");
    let key_value = string_member(&key, "value", "identifiers");

    // A key stored and then DELETED: its identifier was once a real one, which is the
    // row an id-shaped check would miss.
    let doomed = create_key(&gateway, "identifiers", json!({ "name": "doomed-key" })).await;
    let doomed_id = string_member(&doomed, "id", "identifiers");
    let deleted = gateway
        .call("DELETE", &format!("/apikeys/{doomed_id}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteApiKey: expected the modelled 202, got {} with {:?}",
        deleted.status, deleted.body
    );

    // A near miss of each identifier: one character of the stored id changed.
    let near_miss = |id: &str| -> String {
        let mut chars: Vec<char> = id.chars().collect();
        chars[0] = if chars[0] == 'a' { 'b' } else { 'a' };
        chars.into_iter().collect()
    };

    // Rows differ in WHICH identifier does not resolve and in the form it takes: never
    // minted, a near miss, the other resource's identifier, the key's VALUE rather than
    // its id, and a deleted key.
    let refused: Vec<(String, String, String)> = vec![
        (
            "an unknown plan".to_string(),
            "zzzzzzzzzz".to_string(),
            key_id.clone(),
        ),
        (
            "a near miss of the plan id".to_string(),
            near_miss(&plan_id),
            key_id.clone(),
        ),
        (
            "the key's id as the plan".to_string(),
            key_id.clone(),
            key_id.clone(),
        ),
        (
            "an unknown key".to_string(),
            plan_id.clone(),
            "zzzzzzzzzz".to_string(),
        ),
        (
            "a near miss of the key id".to_string(),
            plan_id.clone(),
            near_miss(&key_id),
        ),
        (
            "the key's value as its id".to_string(),
            plan_id.clone(),
            key_value.clone(),
        ),
        (
            "the plan's id as the key".to_string(),
            plan_id.clone(),
            plan_id.clone(),
        ),
        (
            "a deleted key".to_string(),
            plan_id.clone(),
            doomed_id.clone(),
        ),
        (
            "neither identifier".to_string(),
            "zzzzzzzzzz".to_string(),
            "yyyyyyyyyy".to_string(),
        ),
    ];
    for (case, plan, key_identifier) in &refused {
        let answer = create_usage_plan_key(&gateway, plan, key_identifier, API_KEY_TYPE).await;
        assert_failure(&answer, 404, "NotFoundException", case);
        // The read of the same pair is refused the same way.
        let read = get_usage_plan_key(&gateway, plan, key_identifier).await;
        assert_failure(&read, 404, "NotFoundException", case);
    }

    // A STORED key the plan does not meter is not an association either: the read is a
    // 404 until the create is made.
    let unassociated = get_usage_plan_key(&gateway, &plan_id, &key_id).await;
    assert_failure(
        &unassociated,
        404,
        "NotFoundException",
        "a stored key the plan does not meter",
    );
    add_key(&gateway, "the resolvable pair", &plan_id, &key_id).await;
    let read = get_usage_plan_key(&gateway, &plan_id, &key_id).await;
    assert_eq!(
        read.status, 200,
        "case GetUsagePlanKey(the resolvable pair): expected 200, got {} with {:?}",
        read.status, read.body
    );
}

#[tokio::test]
async fn a_key_the_plan_already_meters_is_a_conflict() {
    let gateway = Gateway::new();
    let first = create_usage_plan(&gateway, "conflict-a", json!({ "name": "plan-first" })).await;
    let first_id = string_member(&first, "id", "conflict-a");
    let second = create_usage_plan(&gateway, "conflict-b", json!({ "name": "plan-second" })).await;
    let second_id = string_member(&second, "id", "conflict-b");

    // Rows differ in which API key is re-added, so the refusal is about the pair and not
    // about one record.
    for n in 0..3 {
        let case = format!("key {n} re-added");
        let key = create_key(&gateway, &case, json!({ "name": format!("conflict-key-{n}") })).await;
        let key_id = string_member(&key, "id", &case);
        let created = add_key(&gateway, &case, &first_id, &key_id).await;

        // Every repeat is a conflict, however many times it is tried.
        for attempt in 1..=2 {
            let case = format!("{case}, attempt {attempt}");
            let answer = create_usage_plan_key(&gateway, &first_id, &key_id, API_KEY_TYPE).await;
            assert_failure(&answer, 409, "ConflictException", &case);
        }

        // The stored association is untouched by the refusals.
        let read = get_usage_plan_key(&gateway, &first_id, &key_id).await;
        assert_eq!(
            read.body, created,
            "case {case}: a refused repeat must leave the association as it was, got {:?} \
             after {:?}",
            read.body, created
        );

        // The SAME key in a DIFFERENT plan is a different association, so it is accepted.
        let other = add_key(&gateway, &format!("{case} in the other plan"), &second_id, &key_id)
            .await;
        assert_usage_plan_key(&other, &format!("{case} in the other plan"), &key);
    }
}

#[tokio::test]
async fn an_association_reports_its_api_key_s_current_name_and_outlives_nothing() {
    let gateway = Gateway::new();
    let plan = create_usage_plan(&gateway, "one-record", json!({ "name": "plan-one-record" })).await;
    let plan_id = string_member(&plan, "id", "one-record");

    // Rows differ in the name the key is renamed to, including from no name to a name.
    let rows: Vec<(&str, Value, &str)> = vec![
        ("a named key renamed", json!({ "name": "before" }), "after"),
        ("an unnamed key named", json!({}), "newly-named"),
        (
            "a described key renamed",
            json!({ "name": "described", "description": "d" }),
            "re-described",
        ),
    ];
    for (case, key_request, renamed) in &rows {
        let key = create_key(&gateway, case, key_request.clone()).await;
        let key_id = string_member(&key, "id", case);
        add_key(&gateway, case, &plan_id, &key_id).await;

        let patched = rename_key(&gateway, &key_id, renamed).await;
        assert_eq!(
            patched.status, 200,
            "case {case}: expected 200 from UpdateApiKey, got {} with {:?}",
            patched.status, patched.body
        );
        // One record: the association reports the key's name, so renaming the key is
        // reported here rather than leaving a stale copy behind.
        let read = get_usage_plan_key(&gateway, &plan_id, &key_id).await;
        assert_eq!(
            read.body["name"],
            json!(renamed),
            "case {case}: expected the renamed key's name, got {:?}",
            read.body["name"]
        );
        assert_eq!(
            read.body["value"], key["value"],
            "case {case}: a rename must not change the key's value, got {:?}",
            read.body["value"]
        );

        // Reading the association three times reports the same body: a read changes
        // nothing a later caller sees.
        let mut seen: Vec<Value> = Vec::new();
        for round in 0..3 {
            let again = get_usage_plan_key(&gateway, &plan_id, &key_id).await;
            assert_eq!(
                again.status, 200,
                "case {case} (round {round}): expected 200, got {} with {:?}",
                again.status, again.body
            );
            seen.push(again.body);
        }
        for round in 1..seen.len() {
            assert_eq!(
                seen[round], seen[0],
                "case {case} (round {round}): a read must not change the association, got {:?} \
                 after {:?}",
                seen[round], seen[0]
            );
        }

        // Deleting the API key leaves no key for the plan to meter, so the association
        // answers to nothing.
        let deleted = gateway
            .call("DELETE", &format!("/apikeys/{key_id}"), None)
            .await;
        assert_eq!(
            deleted.status, 202,
            "case {case}: expected the modelled 202 from DeleteApiKey, got {} with {:?}",
            deleted.status, deleted.body
        );
        let after = get_usage_plan_key(&gateway, &plan_id, &key_id).await;
        assert_failure(
            &after,
            404,
            "NotFoundException",
            &format!("{case} after the key was deleted"),
        );
        // The association record itself is RECLAIMED with the key, not merely hidden: a
        // deleted key is metered by no plan, so nothing is left behind in the plan.
        assert!(
            !association_is_stored(&gateway, &plan_id, &key_id),
            "case {case}: deleting the API key must drop its association from the plan, \
             got a stored association for {key_id}"
        );
    }
}

#[tokio::test]
async fn associations_do_not_accumulate_as_keys_come_and_go() {
    let gateway = Gateway::new();
    // Two plans, so the reclamation is asserted across every plan that metered the key
    // rather than only the one the delete happened to be read through.
    let mut plans: Vec<String> = Vec::new();
    for name in ["plan-churn-a", "plan-churn-b"] {
        let plan = create_usage_plan(&gateway, name, json!({ "name": name })).await;
        plans.push(string_member(&plan, "id", name));
    }

    // Rows are rounds of the create/associate/delete cycle: each one associates a FRESH
    // key id, so nothing the earlier rounds added may still be held. The store's
    // association count is therefore bounded by the keys that exist, not by the number of
    // requests made.
    for round in 0..5 {
        let case = format!("round {round}");
        let key = create_key(&gateway, &case, json!({ "name": format!("churn-{round}") })).await;
        let key_id = string_member(&key, "id", &case);

        for plan_id in &plans {
            add_key(&gateway, &case, plan_id, &key_id).await;
            assert_eq!(
                stored_association_count(&gateway, plan_id),
                1,
                "case {case}: plan {plan_id} must hold exactly the one live association, got {}",
                stored_association_count(&gateway, plan_id)
            );
        }

        let deleted = gateway
            .call("DELETE", &format!("/apikeys/{key_id}"), None)
            .await;
        assert_eq!(
            deleted.status, 202,
            "case {case}: expected the modelled 202 from DeleteApiKey, got {} with {:?}",
            deleted.status, deleted.body
        );
        for plan_id in &plans {
            assert_eq!(
                stored_association_count(&gateway, plan_id),
                0,
                "case {case}: plan {plan_id} must hold no association once the key is gone, \
                 got {}",
                stored_association_count(&gateway, plan_id)
            );
            let read = get_usage_plan_key(&gateway, plan_id, &key_id).await;
            assert_failure(
                &read,
                404,
                "NotFoundException",
                &format!("{case}: GetUsagePlanKey({plan_id}, {key_id}) after the delete"),
            );
        }
    }
}
