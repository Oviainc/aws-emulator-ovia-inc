// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteUsagePlan`, read back through the operations that report
//! the records it removes and the records it must leave alone (`GetUsagePlan`,
//! `GetUsagePlans`, `GetUsagePlanKey`, `GetApiKey` and `GetTags`).
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder, the
//! `CreateRestApi`/`CreateDeployment` helpers, `CreateApiKey`, `CreateUsagePlan`,
//! `CreateUsagePlanKey` and the tag-call helpers are this crate's existing harness, reused
//! from `rest_api_lifecycle`, `stage_lifecycle`, `api_key_lifecycle`,
//! `usage_plan_lifecycle`, `usage_plan_key_lifecycle` and `tag_reads_by_arn`.
//!
//! The RULES under test, asserted over SETS of inputs rather than one recorded call:
//!
//! * a delete removes exactly the plan its `usagePlanId` names, so the reads that
//!   reported that plan — its own read, the account's collection and its tag collection —
//!   stop reporting it while every sibling plan is untouched; nothing the plan carries
//!   conditions the removal, so the table spans the optional members a plan may carry and
//!   how many API keys it meters;
//! * the `UsagePlanKey` associations the deleted plan held go with it, while the `ApiKey`
//!   records they named are the account's and stay — readable, and still metered by every
//!   other plan that meters them;
//! * a `usagePlanId` no stored plan answers to is the modelled 404 `NotFoundException`
//!   rather than a success, which is what makes a second delete of one plan resolve to one
//!   success and one not-found rather than to a conflict, and such a refusal removes
//!   nothing.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::USAGE_PLAN_KEY_TYPE;

use crate::api_key_lifecycle::{create_key, get_key};
use crate::rest_api_lifecycle::{
    assert_failure, create_api, page_items, string_member, Answer, Gateway,
};
use crate::stage_lifecycle::create_deployed_stage;
use crate::tag_reads_by_arn::{apigateway_arn, get_tags};
use crate::usage_plan_key_lifecycle::{create_usage_plan_key, get_usage_plan_key};
use crate::usage_plan_lifecycle::{create_usage_plan, get_usage_plan, tag_map};

/// The modelled success status of `DeleteUsagePlan`: `DELETE /usageplans/{usagePlanId}`
/// answers 202 (`smithy_query(query="operation", operation="DeleteUsagePlan")`).
const DELETE_STATUS: u16 = 202;

/// `DeleteUsagePlan` for `id`.
async fn delete_usage_plan(gateway: &Gateway, id: &str) -> Answer {
    gateway
        .call("DELETE", &format!("/usageplans/{id}"), None)
        .await
}

/// `GetUsagePlans`, optionally narrowed to the plans metering `key_id`, asserting the
/// modelled 200 and returning the `id` of every plan on the page in page order.
async fn listed_plan_ids(gateway: &Gateway, key_id: Option<&str>) -> Vec<String> {
    let uri = match key_id {
        None => "/usageplans".to_string(),
        Some(key_id) => format!("/usageplans?keyId={key_id}"),
    };
    let case = format!("GET {uri}");
    let answer = gateway.call("GET", &uri, None).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    page_items(&answer.body, &case)
        .iter()
        .map(|item| string_member(item, "id", &case))
        .collect()
}

/// `CreateApiKey` for a distinct minted key, returning its id.
async fn created_key_id(gateway: &Gateway, name: &str) -> String {
    let created = create_key(gateway, name, json!({ "name": name, "enabled": true })).await;
    string_member(&created, "id", name)
}

/// `CreateUsagePlanKey` for the one key type a `UsagePlanKey` takes, asserting the
/// modelled 201. Local to this suite, as the sibling `usage_plan_collection`'s own
/// spelling of it is: both wrap the shared `create_usage_plan_key` call rather than
/// standing up a second harness.
async fn meter_key(gateway: &Gateway, plan: &str, key_id: &str) {
    let answer = create_usage_plan_key(gateway, plan, key_id, USAGE_PLAN_KEY_TYPE).await;
    assert_eq!(
        answer.status, 201,
        "case CreateUsagePlanKey({plan}, {key_id}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

#[tokio::test]
async fn deleting_a_usage_plan_removes_that_plan_alone_and_a_second_delete_is_not_found() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "usage-plan-deletes").await;
    create_deployed_stage(&gateway, &api, "prod").await;
    let metered = created_key_id(&gateway, "plan-delete-metered-key").await;

    // Rows differ along what the rule generalises over: which optional members the plan
    // carries (none, a description, a throttle, a quota, an API stage association, tags),
    // and how many API keys the plan meters — none conditions the removal — crossed with
    // the plan's position among its siblings, since each row is deleted in turn while the
    // rest stay.
    let rows: Vec<(&str, Value, usize)> = vec![
        ("only the required name", json!({ "name": "del-minimal" }), 0),
        (
            "a described plan",
            json!({ "name": "del-described", "description": "the described plan" }),
            0,
        ),
        (
            "a throttled plan",
            json!({ "name": "del-throttled", "throttle": { "burstLimit": 5, "rateLimit": 2.5 } }),
            0,
        ),
        (
            "a plan with a quota",
            json!({ "name": "del-quota", "quota": { "limit": 100, "offset": 1, "period": "WEEK" } }),
            0,
        ),
        (
            "a plan associated with an api stage",
            json!({ "name": "del-staged", "apiStages": [{ "apiId": api, "stage": "prod" }] }),
            0,
        ),
        ("a tagged plan", json!({ "name": "del-tagged", "tags": tag_map(3) }), 0),
        ("a plan metering one key", json!({ "name": "del-metered-1" }), 1),
        ("a plan metering three keys", json!({ "name": "del-metered-3" }), 3),
    ];

    // Every row is created up front, so each delete is applied to a plan standing among
    // siblings rather than to the only plan the account holds.
    let mut plans: Vec<(&str, String)> = Vec::new();
    for (case, request, metered_keys) in &rows {
        let created = create_usage_plan(&gateway, case, request.clone()).await;
        let id = string_member(&created, "id", case);
        for n in 0..*metered_keys {
            let key = created_key_id(&gateway, &format!("{}-key-{n}", string_member(&created, "name", case))).await;
            meter_key(&gateway, &id, &key).await;
        }
        // The shared key is metered by every row, so each delete is also a delete of a
        // plan whose associations overlap a sibling's.
        meter_key(&gateway, &id, &metered).await;
        plans.push((case, id));
    }

    let mut remaining: Vec<(&str, String)> = plans.clone();
    for (case, id) in &plans {
        let arn = apigateway_arn(&format!("/usageplans/{id}"));
        // The delete succeeds for a plan that exists: the modelled 202 with an empty body,
        // whatever the plan carries.
        let answer = delete_usage_plan(&gateway, id).await;
        assert_eq!(
            answer.status, DELETE_STATUS,
            "case DeleteUsagePlan / {case}: expected {DELETE_STATUS}, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.body.is_null(),
            "case DeleteUsagePlan / {case}: expected an empty body, got {:?}",
            answer.body
        );

        // The record is gone: the read that reported it answers the modelled not-found,
        // and so does the tag collection its ARN addressed.
        assert_failure(
            &get_usage_plan(&gateway, id).await,
            404,
            "NotFoundException",
            &format!("GetUsagePlan after the delete / {case}"),
        );
        assert_failure(
            &get_tags(&gateway, &arn).await,
            404,
            "NotFoundException",
            &format!("GetTags after the delete / {case}"),
        );
        // A second delete of the SAME plan names no record, so it is the modelled
        // not-found rather than a second success or a conflict — the answer one of two
        // concurrent deletes of one plan gets.
        assert_failure(
            &delete_usage_plan(&gateway, id).await,
            404,
            "NotFoundException",
            &format!("DeleteUsagePlan twice / {case}"),
        );

        // Every plan this delete did not name is untouched, in the account's collection
        // and in its own read.
        remaining.retain(|(_, other)| other != id);
        let listed = listed_plan_ids(&gateway, None).await;
        assert_eq!(
            listed,
            remaining
                .iter()
                .map(|(_, other)| other.clone())
                .collect::<Vec<String>>(),
            "case GetUsagePlans after the delete / {case}: expected the plans still \
             stored, got {listed:?}"
        );
        for (sibling, other) in &remaining {
            let read = get_usage_plan(&gateway, other).await;
            assert_eq!(
                read.status, 200,
                "case {case}: the sibling plan ({sibling}) must be untouched, got {} \
                 with {:?}",
                read.status, read.body
            );
        }
    }

    // With every plan gone the collection is empty rather than absent, and the API key
    // every one of them metered is still the account's.
    let listed = listed_plan_ids(&gateway, None).await;
    assert!(
        listed.is_empty(),
        "case GetUsagePlans after every delete: expected no plans, got {listed:?}"
    );
    let read = get_key(&gateway, &metered, false).await;
    assert_eq!(
        read.status, 200,
        "case GetApiKey after every delete: an API key outlives the plans that metered \
         it, got {} with {:?}",
        read.status, read.body
    );
}

#[tokio::test]
async fn a_deleted_plan_s_key_associations_go_with_it_while_the_api_keys_stay() {
    let gateway = Gateway::new();
    // Two plans metering overlapping key sets, so what the delete removes is told apart
    // from what it leaves: rows differ in how many plans meter the key (one, both) and in
    // which plan is the one deleted.
    let deleted = {
        let created = create_usage_plan(&gateway, "the deleted plan", json!({ "name": "cascade-gone" })).await;
        string_member(&created, "id", "the deleted plan")
    };
    let kept = {
        let created = create_usage_plan(&gateway, "the surviving plan", json!({ "name": "cascade-kept" })).await;
        string_member(&created, "id", "the surviving plan")
    };

    // (case, key id, metered by the surviving plan too)
    let mut keys: Vec<(String, String, bool)> = Vec::new();
    for (case, shared) in [
        ("a key only the deleted plan meters", false),
        ("a key both plans meter", true),
        ("a second key both plans meter", true),
        ("a second key only the deleted plan meters", false),
    ] {
        let key = created_key_id(&gateway, &format!("cascade-{}", keys.len())).await;
        meter_key(&gateway, &deleted, &key).await;
        if shared {
            meter_key(&gateway, &kept, &key).await;
        }
        keys.push((case.to_string(), key, shared));
    }

    // Before the delete every association reads back, from whichever plan holds it.
    for (case, key, shared) in &keys {
        let read = get_usage_plan_key(&gateway, &deleted, key).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetUsagePlanKey before the delete, got {} \
             with {:?}",
            read.status, read.body
        );
        if *shared {
            let read = get_usage_plan_key(&gateway, &kept, key).await;
            assert_eq!(
                read.status, 200,
                "case {case}: the surviving plan's association must read back, got {} \
                 with {:?}",
                read.status, read.body
            );
        }
    }

    let answer = delete_usage_plan(&gateway, &deleted).await;
    assert_eq!(
        answer.status, DELETE_STATUS,
        "case DeleteUsagePlan(a plan metering keys): expected {DELETE_STATUS}, got {} \
         with {:?}",
        answer.status, answer.body
    );

    for (case, key, shared) in &keys {
        // The association the deleted plan held is gone with the plan.
        assert_failure(
            &get_usage_plan_key(&gateway, &deleted, key).await,
            404,
            "NotFoundException",
            &format!("GetUsagePlanKey on the deleted plan / {case}"),
        );
        // The API key itself is the account's and stays readable.
        let read = get_key(&gateway, key, true).await;
        assert_eq!(
            read.status, 200,
            "case {case}: the API key must outlive the plan that metered it, got {} \
             with {:?}",
            read.status, read.body
        );
        // The surviving plan goes on metering what it metered, and the collection
        // narrowed by that key reports exactly the plans that still meter it.
        let narrowed = listed_plan_ids(&gateway, Some(key)).await;
        let expected: Vec<String> = if *shared { vec![kept.clone()] } else { Vec::new() };
        assert_eq!(
            narrowed, expected,
            "case GetUsagePlans(keyId) after the delete / {case}: expected {expected:?}, \
             got {narrowed:?}"
        );
        if *shared {
            let read = get_usage_plan_key(&gateway, &kept, key).await;
            assert_eq!(
                read.status, 200,
                "case {case}: the surviving plan's association is untouched, got {} \
                 with {:?}",
                read.status, read.body
            );
        }
    }

    // A new plan can meter those keys again: the delete reclaimed the associations rather
    // than leaving a record a later create would collide with.
    let replacement = {
        let created =
            create_usage_plan(&gateway, "the replacement plan", json!({ "name": "cascade-new" })).await;
        string_member(&created, "id", "the replacement plan")
    };
    for (case, key, _) in &keys {
        let answer = create_usage_plan_key(&gateway, &replacement, key, USAGE_PLAN_KEY_TYPE).await;
        assert_eq!(
            answer.status, 201,
            "case CreateUsagePlanKey on a new plan / {case}: expected 201, got {} \
             with {:?}",
            answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn a_delete_naming_no_stored_plan_is_the_modelled_not_found_and_removes_nothing() {
    let gateway = Gateway::new();
    let created = create_usage_plan(
        &gateway,
        "the stored plan",
        json!({ "name": "stays", "description": "the stored plan", "tags": tag_map(1) }),
    )
    .await;
    let id = string_member(&created, "id", "the stored plan");
    let key = created_key_id(&gateway, "not-found-key").await;
    meter_key(&gateway, &id, &key).await;

    let gone = {
        let created = create_usage_plan(&gateway, "the deleted plan", json!({ "name": "went" })).await;
        let gone = string_member(&created, "id", "the deleted plan");
        let answer = delete_usage_plan(&gateway, &gone).await;
        assert_eq!(
            answer.status, DELETE_STATUS,
            "case DeleteUsagePlan(the deleted plan): expected {DELETE_STATUS}, got {} \
             with {:?}",
            answer.status, answer.body
        );
        gone
    };

    // Rows differ in the FORM of the identifier that names nothing: one no plan ever had,
    // the stored plan's NAME — which is not the id the `usagePlanId` label takes — an
    // identifier differing from the stored one in a single character, the id of a plan
    // that was stored and then deleted, and an API key's id, which addresses another
    // collection entirely.
    let mut near_miss: Vec<char> = id.chars().collect();
    near_miss[0] = if near_miss[0] == 'a' { 'b' } else { 'a' };
    let near_miss: String = near_miss.into_iter().collect();
    // The near miss is a NON-EXISTENT identifier by construction: it differs from the
    // stored plan's id and from every other identifier this account holds.
    for (other, what) in [(&gone, "the deleted plan's id"), (&key, "the API key's id")] {
        assert_ne!(
            &near_miss, other,
            "the near-miss identifier must name nothing, but it is {what}"
        );
    }
    for identifier in [
        "zzzzzzzzzz",
        "stays",
        near_miss.as_str(),
        gone.as_str(),
        key.as_str(),
    ] {
        let case = format!("DELETE /usageplans/{identifier}");
        assert_failure(
            &delete_usage_plan(&gateway, identifier).await,
            404,
            "NotFoundException",
            &case,
        );
    }

    // Every refused call removed nothing: the stored plan, its association and the API
    // key are all still as they were.
    let read = get_usage_plan(&gateway, &id).await;
    assert_eq!(
        read.status, 200,
        "case after the refused deletes: expected 200 from GetUsagePlan, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.body, created,
        "case after the refused deletes: the stored plan must be unchanged, got {:?} \
         after {:?}",
        read.body, created
    );
    let read = get_usage_plan_key(&gateway, &id, &key).await;
    assert_eq!(
        read.status, 200,
        "case after the refused deletes: expected 200 from GetUsagePlanKey, got {} \
         with {:?}",
        read.status, read.body
    );
    let listed = listed_plan_ids(&gateway, None).await;
    assert_eq!(
        listed,
        vec![id.clone()],
        "case after the refused deletes: expected only the stored plan, got {listed:?}"
    );
}
