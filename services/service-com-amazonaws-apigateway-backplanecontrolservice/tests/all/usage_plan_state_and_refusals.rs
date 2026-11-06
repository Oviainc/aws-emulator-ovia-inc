// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the state a usage plan and its keys keep across the operations
//! that change them, and for the sentences this control plane refuses them with.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder, the
//! `CreateRestApi`/`CreateDeployment` helpers, `CreateApiKey`, `CreateUsagePlan`,
//! `CreateUsagePlanKey`, `GetUsagePlanKeys` and the patch and tag helpers are this crate's
//! existing harness, reused from `rest_api_lifecycle`, `stage_lifecycle`,
//! `api_key_lifecycle`, `usage_plan_lifecycle`, `usage_plan_key_lifecycle`,
//! `usage_plan_key_collection`, `patch_updates` and `tag_reads_by_arn`.
//!
//! The RULES under test, each asserted over a SET of inputs rather than one recorded call:
//!
//! * an `apiStages` entry names ONE stage OF ONE API, so the API identifier is resolved
//!   first: an entry naming no stored API is 404 `NotFoundException` stating that
//!   identifier alone, account-qualified, and an entry whose API resolves but whose stage
//!   it does not carry is 404 `NotFoundException` naming the association — whichever
//!   operation (the create or the update) was asked to associate it (the ORDER of the two
//!   lookups is `usage_plan_api_stage_resolution`'s own suite);
//! * an `apiStages` association makes the plan gate traffic on that stage, so it stands in
//!   the way of the plan's delete: the delete is 400 `BadRequestException` naming the plan
//!   and one association in the way until the associations are removed, and 202 after;
//! * a `UsagePlanKey` is addressed under the plan that meters it, so deleting one unmeters
//!   exactly that pair — the pair's read and the plan's collection stop reporting it, while
//!   the `ApiKey` and every other plan metering it are untouched — and a pair no stored
//!   association answers to is 404 `NotFoundException`, naming the API key identifier when
//!   that is what does not resolve and the plan's usage plan key otherwise;
//! * a `keyType` outside the one type the model declares is 400 `BadRequestException`
//!   naming the set the request could have chosen from;
//! * a tag call whose ARN names no stored usage plan is 404 `NotFoundException` reporting
//!   the resource type and the id the ARN carried.

use serde_json::{json, Value};

use crate::api_key_lifecycle::{create_key, get_key};
use crate::patch_updates::op;
use crate::rest_api_identifier_refusals::no_such_api;
use crate::rest_api_lifecycle::{
    assert_failure, create_api, page_items, string_member, Answer, Gateway,
};
use crate::stage_lifecycle::create_deployed_stage;
use crate::tag_reads_by_arn::{apigateway_arn, encode_label, get_tags};
use crate::usage_plan_key_collection::list_keys;
use crate::usage_plan_key_lifecycle::{create_usage_plan_key, get_usage_plan_key};
use crate::usage_plan_lifecycle::{create_usage_plan, get_usage_plan, tag_map};

/// The modelled success status of `DeleteUsagePlan` and `DeleteUsagePlanKey`: both answer
/// 202 (`smithy_query(query="operation", operation="DeleteUsagePlan,DeleteUsagePlanKey")`).
///
/// `pub(crate)` for the sibling `delete_usage_plan_key` suite; see [`meter_key`].
pub(crate) const DELETE_STATUS: u16 = 202;

/// The modelled success status of `TagResource` and `UntagResource`: both answer 204.
const TAG_STATUS: u16 = 204;

/// The one key type a `UsagePlanKey` takes: from `UsagePlanKey$type`'s own documentation,
/// "Currently, the valid key type is `API_KEY`"
/// (`smithy_query(query="doc", shape="UsagePlanKey", member="type")`).
const API_KEY_TYPE: &str = "API_KEY";

/// The refusal for a `keyType` outside that one type: the set the request could have
/// chosen from, and nothing of the value that was rejected.
const INVALID_KEY_TYPE: &str = "Invalid Usage Plan key type specified. Must be one of [API_KEY]";

/// The refusal for a `(usagePlanId, keyId)` pair whose `keyId` names a stored API key and
/// that still names no stored association: a usage plan key is addressed under the plan
/// that meters it, so a pair resolving to nothing resolves to no usage plan key of that
/// plan.
///
/// `pub(crate)` for the sibling `delete_usage_plan_key` suite; see [`meter_key`].
pub(crate) const NO_SUCH_USAGE_PLAN_KEY: &str = "Invalid Usage Plan ID specified";

/// The refusal for a `keyId` that names no API key of the account at all: the identifier
/// that does not resolve is the API key's, whichever plan the request addressed.
const NO_SUCH_API_KEY: &str = "Invalid API Key identifier specified";

/// The refusal for an `apiStages` entry whose API resolves and that names no stage OF
/// that API. An entry whose API identifier resolves to nothing is stated by the service's
/// account-qualified identifier sentence ([`no_such_api`]) instead, which is what the
/// `usage_plan_api_stage_resolution` suite asserts the ordering of.
pub(crate) fn api_stage_not_found(api_id: &str, stage: &str) -> String {
    format!("API Stage not found: {api_id}:{stage}")
}

/// The refusal for a `DeleteUsagePlan` of a plan still associated with API stages, naming
/// the plan and one of the associations in the way.
fn plan_still_associated(plan_id: &str, api_id: &str, stage: &str) -> String {
    format!(
        "Cannot delete Usage Plan {plan_id} because there are API Stages associated with \
         it, including {api_id}:{stage}"
    )
}

/// The refusal a TAG call answers for an ARN no stored usage plan answers to.
fn tagged_plan_not_found(plan_id: &str) -> String {
    format!("UsagePlan {plan_id} was not found")
}

/// Assert `answer` is the modelled failure `(status, code)` carrying exactly `message`.
fn assert_refusal(answer: &Answer, expected: (u16, &str), message: &str, case: &str) {
    assert_failure(answer, expected.0, expected.1, case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {:?}",
        answer.body["message"]
    );
}

/// `CreateUsagePlan` with `request` sent as given, whatever the service answers.
async fn create_plan(gateway: &Gateway, request: Value) -> Answer {
    gateway.call("POST", "/usageplans", Some(request)).await
}

/// `UpdateUsagePlan` on `plan` with `operations`, whatever the service answers.
async fn patch_plan(gateway: &Gateway, plan: &str, operations: Vec<Value>) -> Answer {
    gateway
        .call(
            "PATCH",
            &format!("/usageplans/{plan}"),
            Some(json!({ "patchOperations": Value::Array(operations) })),
        )
        .await
}

/// `DeleteUsagePlan` for `plan`.
async fn delete_plan(gateway: &Gateway, plan: &str) -> Answer {
    gateway
        .call("DELETE", &format!("/usageplans/{plan}"), None)
        .await
}

/// `DeleteUsagePlanKey` for one `(usagePlanId, keyId)` pair.
///
/// `pub(crate)` for the sibling `delete_usage_plan_key` suite; see [`meter_key`].
pub(crate) async fn delete_plan_key(gateway: &Gateway, plan: &str, key_id: &str) -> Answer {
    gateway
        .call("DELETE", &format!("/usageplans/{plan}/keys/{key_id}"), None)
        .await
}

/// `TagResource` for `resource_arn` with one tag.
async fn tag_resource(gateway: &Gateway, resource_arn: &str) -> Answer {
    gateway
        .call(
            "PUT",
            &format!("/tags/{}", encode_label(resource_arn)),
            Some(json!({ "tags": { "owner": "team" } })),
        )
        .await
}

/// `UntagResource` for `resource_arn`, naming one key.
async fn untag_resource(gateway: &Gateway, resource_arn: &str) -> Answer {
    gateway
        .call(
            "DELETE",
            &format!("/tags/{}?tagKeys=owner", encode_label(resource_arn)),
            None,
        )
        .await
}

/// `CreateApiKey` for a distinct minted key, returning its id.
async fn created_key_id(gateway: &Gateway, name: &str) -> String {
    let created = create_key(gateway, name, json!({ "name": name, "enabled": true })).await;
    string_member(&created, "id", name)
}

/// `CreateUsagePlanKey` for the one key type a `UsagePlanKey` takes, asserting the
/// modelled 201.
///
/// `pub(crate)` for the sibling `delete_usage_plan_key` suite, which meters a key to assert
/// what DELETING the association changes at the `execute-api` gate: one helper, so the two
/// suites cannot associate a key two ways.
pub(crate) async fn meter_key(gateway: &Gateway, plan: &str, key_id: &str) {
    let answer = create_usage_plan_key(gateway, plan, key_id, API_KEY_TYPE).await;
    assert_eq!(
        answer.status, 201,
        "case CreateUsagePlanKey({plan}, {key_id}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// The `id` of every usage plan key the plan's collection reports, in page order.
///
/// `pub(crate)` for the sibling `delete_usage_plan_key` suite; see [`meter_key`].
pub(crate) async fn metered_key_ids(gateway: &Gateway, plan: &str, case: &str) -> Vec<String> {
    let answer = list_keys(gateway, plan, "").await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from GetUsagePlanKeys, got {} with {:?}",
        answer.status, answer.body
    );
    page_items(&answer.body, case)
        .iter()
        .map(|item| string_member(item, "id", case))
        .collect()
}

/// The `apiStages` entry at `index` of a reported plan, as the `<apiId>:<stageName>` pair
/// an association is addressed by — read from the REPORTED record, so the expectation
/// comes from stored state rather than from a value written into the test.
fn reported_stage(body: &Value, index: usize, case: &str) -> (String, String) {
    let entry = &body["apiStages"][index];
    (
        string_member(entry, "apiId", case),
        string_member(entry, "stage", case),
    )
}

#[tokio::test]
async fn an_api_stage_entry_naming_no_stored_stage_is_refused_with_the_identifier_at_fault() {
    let gateway = Gateway::new();
    let (first, _) = create_api(&gateway, "api-stage-refusals-first").await;
    create_deployed_stage(&gateway, &first, "prod").await;
    create_deployed_stage(&gateway, &first, "beta").await;
    let (second, _) = create_api(&gateway, "api-stage-refusals-second").await;
    create_deployed_stage(&gateway, &second, "canary").await;

    // The plan every update row patches: it carries one resolvable association, so a
    // refused row is told apart from a plan that was never associated with anything.
    let created = create_usage_plan(
        &gateway,
        "the patched plan",
        json!({
            "name": "stage-refusals",
            "apiStages": [{ "apiId": first, "stage": "prod" }],
        }),
    )
    .await;
    let plan_id = string_member(&created, "id", "the patched plan");

    let mut near_miss: Vec<char> = first.chars().collect();
    near_miss[0] = if near_miss[0] == 'a' { 'b' } else { 'a' };
    let near_miss: String = near_miss.into_iter().collect();
    // Rows differ in WHICH half of the pair does not resolve and in the form it takes: a
    // stage name no stage of the named API has, a stage name that is a stage of ANOTHER
    // API, an API that was never minted, a near miss of a stored API's id, and neither
    // half resolving. Each carries the sentence the rule gives it: the association for a
    // row whose API resolves, and the account-qualified API identifier for a row whose
    // does not.
    let rows: Vec<(String, String, String, String)> = vec![
        (
            "a stage the named api does not have".to_string(),
            first.clone(),
            "no-such-stage".to_string(),
            api_stage_not_found(&first, "no-such-stage"),
        ),
        (
            "a stage of another api".to_string(),
            first.clone(),
            "canary".to_string(),
            api_stage_not_found(&first, "canary"),
        ),
        (
            "a stage of the first api named on the second".to_string(),
            second.clone(),
            "beta".to_string(),
            api_stage_not_found(&second, "beta"),
        ),
        (
            "an api that was never minted".to_string(),
            "zzzzzzzzzz".to_string(),
            "prod".to_string(),
            no_such_api("zzzzzzzzzz"),
        ),
        (
            "a near miss of a stored api id".to_string(),
            near_miss.clone(),
            "prod".to_string(),
            no_such_api(&near_miss),
        ),
        (
            "neither the api nor the stage".to_string(),
            "zzzzzzzzzz".to_string(),
            "no-such-stage".to_string(),
            no_such_api("zzzzzzzzzz"),
        ),
    ];

    for (case, api_id, stage, expected) in &rows {
        // The create is refused, and nothing is stored: the account's collection is the
        // one plan created above.
        let answer = create_plan(
            &gateway,
            json!({
                "name": format!("refused-{stage}"),
                "apiStages": [{ "apiId": api_id, "stage": stage }],
            }),
        )
        .await;
        assert_refusal(
            &answer,
            (404, "NotFoundException"),
            expected,
            &format!("CreateUsagePlan / {case}"),
        );

        // The update that would associate the same pair is refused the same way, so the
        // sentence is the rule's and not one operation's.
        let answer = patch_plan(
            &gateway,
            &plan_id,
            vec![op("add", "/apiStages", Some(&format!("{api_id}:{stage}")))],
        )
        .await;
        assert_refusal(
            &answer,
            (404, "NotFoundException"),
            expected,
            &format!("UpdateUsagePlan / {case}"),
        );

        // Neither refusal changed the stored plan.
        let read = get_usage_plan(&gateway, &plan_id).await;
        assert_eq!(
            read.body, created,
            "case {case}: a refused association must leave the plan as it was, got {:?} \
             after {:?}",
            read.body, created
        );
    }

    // A pair that DOES resolve is accepted by both operations, so the refusals above are
    // about the pair and not about the member.
    let accepted = create_plan(
        &gateway,
        json!({
            "name": "stage-accepted",
            "apiStages": [{ "apiId": second, "stage": "canary" }],
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 201,
        "case CreateUsagePlan(a resolvable pair): expected 201, got {} with {:?}",
        accepted.status, accepted.body
    );
    let answer = patch_plan(
        &gateway,
        &plan_id,
        vec![op("add", "/apiStages", Some(&format!("{first}:beta")))],
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case UpdateUsagePlan(a resolvable pair): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
}

#[tokio::test]
async fn a_plan_associated_with_api_stages_is_deleted_only_once_they_are_removed() {
    let gateway = Gateway::new();
    let (first, _) = create_api(&gateway, "plan-delete-stages-first").await;
    create_deployed_stage(&gateway, &first, "prod").await;
    create_deployed_stage(&gateway, &first, "beta").await;
    let (second, _) = create_api(&gateway, "plan-delete-stages-second").await;
    create_deployed_stage(&gateway, &second, "canary").await;

    // Rows differ in how many associations stand in the way (one, two, three), whether
    // they are stages of one API or of several, and what else the plan carries (a quota, a
    // throttle, tags, metered API keys) — none of which conditions the delete.
    let rows: Vec<(&str, Value)> = vec![
        (
            "one association",
            json!({
                "name": "in-use-one",
                "apiStages": [{ "apiId": first, "stage": "prod" }],
            }),
        ),
        (
            "two associations of one api",
            json!({
                "name": "in-use-two",
                "apiStages": [
                    { "apiId": first, "stage": "prod" },
                    { "apiId": first, "stage": "beta" },
                ],
            }),
        ),
        (
            "three associations across two apis",
            json!({
                "name": "in-use-three",
                "description": "gates three stages",
                "quota": { "limit": 100, "offset": 1, "period": "WEEK" },
                "throttle": { "burstLimit": 5, "rateLimit": 2.5 },
                "tags": tag_map(2),
                "apiStages": [
                    { "apiId": second, "stage": "canary" },
                    { "apiId": first, "stage": "prod" },
                    { "apiId": first, "stage": "beta" },
                ],
            }),
        ),
    ];

    for (case, request) in &rows {
        let created = create_usage_plan(&gateway, case, request.clone()).await;
        let plan_id = string_member(&created, "id", case);
        // The plan meters an API key too, so what blocks the delete is told apart from
        // what does not.
        let key = created_key_id(&gateway, &format!("in-use-key-{plan_id}")).await;
        meter_key(&gateway, &plan_id, &key).await;

        let associated = request["apiStages"]
            .as_array()
            .map_or(0, |entries| entries.len());
        for round in 0..associated {
            // Each round the delete is refused, naming the plan and one association still
            // standing in its way — read from the plan's own reported record, so the
            // expectation is stored state rather than a value written into the test.
            let read = get_usage_plan(&gateway, &plan_id).await;
            let (api_id, stage) = reported_stage(&read.body, 0, case);
            let answer = delete_plan(&gateway, &plan_id).await;
            assert_refusal(
                &answer,
                (400, "BadRequestException"),
                &plan_still_associated(&plan_id, &api_id, &stage),
                &format!("DeleteUsagePlan / {case} / round {round}"),
            );

            // The refusal removed nothing: the plan, its associations and the key it
            // meters are all as they were.
            let after = delete_plan(&gateway, &plan_id).await;
            assert_eq!(
                after.status, answer.status,
                "case {case} / round {round}: a repeated refusal must answer the same, got \
                 {} with {:?}",
                after.status, after.body
            );
            let still = get_usage_plan(&gateway, &plan_id).await;
            assert_eq!(
                still.body, read.body,
                "case {case} / round {round}: the refused delete must leave the plan as it \
                 was, got {:?} after {:?}",
                still.body, read.body
            );
            assert_eq!(
                metered_key_ids(&gateway, &plan_id, case).await,
                vec![key.clone()],
                "case {case} / round {round}: the refused delete must leave the plan's keys"
            );

            // Remove that one association, the call a client makes to unblock the delete.
            let patched = patch_plan(
                &gateway,
                &plan_id,
                vec![op("remove", &format!("/apiStages/{api_id}:{stage}"), None)],
            )
            .await;
            assert_eq!(
                patched.status, 200,
                "case {case} / round {round}: expected 200 from UpdateUsagePlan, got {} \
                 with {:?}",
                patched.status, patched.body
            );
            assert_eq!(
                patched.body["apiStages"]
                    .as_array()
                    .map_or(0, |entries| entries.len()),
                associated - round - 1,
                "case {case} / round {round}: expected one association fewer, got {:?}",
                patched.body["apiStages"]
            );
        }

        // With no association left the delete succeeds: the modelled 202 with an empty
        // body, and the reads that reported the plan stop reporting it.
        let answer = delete_plan(&gateway, &plan_id).await;
        assert_eq!(
            answer.status, DELETE_STATUS,
            "case DeleteUsagePlan / {case} / disassociated: expected {DELETE_STATUS}, got \
             {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.body.is_null(),
            "case DeleteUsagePlan / {case}: expected an empty body, got {:?}",
            answer.body
        );
        assert_failure(
            &get_usage_plan(&gateway, &plan_id).await,
            404,
            "NotFoundException",
            &format!("GetUsagePlan after the delete / {case}"),
        );
        // The API key the plan metered is the account's and outlives it.
        let read = get_key(&gateway, &key, false).await;
        assert_eq!(
            read.status, 200,
            "case {case}: the API key must outlive the plan, got {} with {:?}",
            read.status, read.body
        );
    }

    // A plan associated with NO stage is deleted straight away, so what the rows above
    // assert is the association and not the delete.
    let created = create_usage_plan(
        &gateway,
        "no associations",
        json!({ "name": "not-in-use", "tags": tag_map(1) }),
    )
    .await;
    let plan_id = string_member(&created, "id", "no associations");
    let answer = delete_plan(&gateway, &plan_id).await;
    assert_eq!(
        answer.status, DELETE_STATUS,
        "case DeleteUsagePlan(a plan associated with no stage): expected {DELETE_STATUS}, \
         got {} with {:?}",
        answer.status, answer.body
    );
}

#[tokio::test]
async fn deleting_a_usage_plan_key_unmeters_that_pair_alone() {
    let gateway = Gateway::new();
    // Two plans metering the same keys, so what the delete unmeters is told apart from
    // what it leaves.
    let plan = {
        let created = create_usage_plan(&gateway, "the plan", json!({ "name": "key-deletes" })).await;
        string_member(&created, "id", "the plan")
    };
    let other = {
        let created =
            create_usage_plan(&gateway, "the other plan", json!({ "name": "key-deletes-other" }))
                .await;
        string_member(&created, "id", "the other plan")
    };

    // Rows differ in WHICH key of the sequence is deleted — the first, a middle one and
    // the last — so the delete is not confused with "the collection's head" or "the one
    // key the plan meters".
    let mut keys: Vec<String> = Vec::new();
    for n in 0..4 {
        let key = created_key_id(&gateway, &format!("unmeter-{n}")).await;
        meter_key(&gateway, &plan, &key).await;
        meter_key(&gateway, &other, &key).await;
        keys.push(key);
    }
    for (case, index) in [
        ("the first key of the sequence", 0usize),
        ("a middle key of the sequence", 1),
        ("the last key of the sequence", 3),
    ] {
        let key = keys[index].clone();
        let answer = delete_plan_key(&gateway, &plan, &key).await;
        assert_eq!(
            answer.status, DELETE_STATUS,
            "case DeleteUsagePlanKey / {case}: expected {DELETE_STATUS}, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.body.is_null(),
            "case DeleteUsagePlanKey / {case}: expected an empty body, got {:?}",
            answer.body
        );

        // The pair's own read stops resolving, and the plan's collection stops listing it
        // while every other association keeps its place in the added order.
        assert_refusal(
            &get_usage_plan_key(&gateway, &plan, &key).await,
            (404, "NotFoundException"),
            NO_SUCH_USAGE_PLAN_KEY,
            &format!("GetUsagePlanKey after the delete / {case}"),
        );
        keys.retain(|held| held != &key);
        assert_eq!(
            metered_key_ids(&gateway, &plan, case).await,
            keys,
            "case GetUsagePlanKeys after the delete / {case}: expected the associations \
             still held"
        );

        // A second delete of the same pair names no association, so it is the modelled
        // not-found rather than a second success.
        assert_refusal(
            &delete_plan_key(&gateway, &plan, &key).await,
            (404, "NotFoundException"),
            NO_SUCH_USAGE_PLAN_KEY,
            &format!("DeleteUsagePlanKey twice / {case}"),
        );

        // The API key itself is the account's and stays readable, and the other plan goes
        // on metering it.
        let read = get_key(&gateway, &key, true).await;
        assert_eq!(
            read.status, 200,
            "case {case}: the API key must outlive the association, got {} with {:?}",
            read.status, read.body
        );
        let read = get_usage_plan_key(&gateway, &other, &key).await;
        assert_eq!(
            read.status, 200,
            "case {case}: the other plan's association is untouched, got {} with {:?}",
            read.status, read.body
        );

        // The pair can be metered again: the delete reclaimed the association rather than
        // leaving a record the create would collide with.
        meter_key(&gateway, &plan, &key).await;
        keys.push(key.clone());
        assert_eq!(
            metered_key_ids(&gateway, &plan, case).await,
            keys,
            "case GetUsagePlanKeys after re-metering / {case}: expected the re-added key \
             last in the added order"
        );
    }
}

#[tokio::test]
async fn a_usage_plan_key_pair_that_names_no_association_is_refused_without_deleting() {
    let gateway = Gateway::new();
    let plan = {
        let created =
            create_usage_plan(&gateway, "the plan", json!({ "name": "key-pair-refusals" })).await;
        string_member(&created, "id", "the plan")
    };
    let metered = created_key_id(&gateway, "pair-metered").await;
    meter_key(&gateway, &plan, &metered).await;
    // A stored key the plan does NOT meter, and a plan that was stored and then deleted:
    // both are identifiers that resolve on their own and name no association together.
    let unmetered = created_key_id(&gateway, "pair-unmetered").await;
    let gone = {
        let created =
            create_usage_plan(&gateway, "the deleted plan", json!({ "name": "key-pair-gone" }))
                .await;
        let gone = string_member(&created, "id", "the deleted plan");
        let answer = delete_plan(&gateway, &gone).await;
        assert_eq!(
            answer.status, DELETE_STATUS,
            "case DeleteUsagePlan(the deleted plan): expected {DELETE_STATUS}, got {} \
             with {:?}",
            answer.status, answer.body
        );
        gone
    };

    // Rows differ in which half of the pair names nothing and in the form it takes: a
    // plan never minted, a deleted plan, the key's id as the plan, a key never minted, a
    // stored key the plan does not meter, and the plan's own id as the key. The last
    // member of each row is the sentence the pair earns: a `keyId` that names no API key
    // of the account is refused as that identifier, and a pair whose halves resolve and
    // that names no association is refused as no usage plan key of the plan.
    let rows: Vec<(&str, String, String, &str)> = vec![
        (
            "an unknown plan",
            "zzzzzzzzzz".to_string(),
            metered.clone(),
            NO_SUCH_USAGE_PLAN_KEY,
        ),
        (
            "a deleted plan",
            gone.clone(),
            metered.clone(),
            NO_SUCH_USAGE_PLAN_KEY,
        ),
        (
            "the key's id as the plan",
            metered.clone(),
            metered.clone(),
            NO_SUCH_USAGE_PLAN_KEY,
        ),
        (
            "an unknown key",
            plan.clone(),
            "yyyyyyyyyy".to_string(),
            NO_SUCH_API_KEY,
        ),
        (
            "a stored key the plan does not meter",
            plan.clone(),
            unmetered.clone(),
            NO_SUCH_USAGE_PLAN_KEY,
        ),
        (
            "the plan's id as the key",
            plan.clone(),
            plan.clone(),
            NO_SUCH_API_KEY,
        ),
    ];
    for (case, plan_id, key_id, expected) in &rows {
        // The delete and the pair's own read answer the same refusal.
        assert_refusal(
            &delete_plan_key(&gateway, plan_id, key_id).await,
            (404, "NotFoundException"),
            expected,
            &format!("DeleteUsagePlanKey / {case}"),
        );
        assert_refusal(
            &get_usage_plan_key(&gateway, plan_id, key_id).await,
            (404, "NotFoundException"),
            expected,
            &format!("GetUsagePlanKey / {case}"),
        );
    }

    // Every refused call removed nothing: the one association the plan holds is still
    // there, and so is the key no plan meters.
    assert_eq!(
        metered_key_ids(&gateway, &plan, "after the refusals").await,
        vec![metered.clone()],
        "case after the refusals: expected the stored association"
    );
    let read = get_key(&gateway, &unmetered, false).await;
    assert_eq!(
        read.status, 200,
        "case after the refusals: the unmetered key must still be stored, got {} with {:?}",
        read.status, read.body
    );
}

#[tokio::test]
async fn a_key_type_outside_the_modelled_set_names_the_set_it_could_have_chosen_from() {
    let gateway = Gateway::new();
    let plan = {
        let created = create_usage_plan(&gateway, "key types", json!({ "name": "key-types" })).await;
        string_member(&created, "id", "key types")
    };
    let key = created_key_id(&gateway, "typed-key").await;

    // Rows differ in the FORM of the wrong value: the right word in the wrong case, with
    // punctuation or whitespace, a plural, and a value of another vocabulary.
    for key_type in [
        "api_key",
        "ApiKey",
        "APIKEY",
        "API_KEY ",
        "API-KEY",
        "API_KEYS",
        "USAGE_PLAN",
    ] {
        let answer = create_usage_plan_key(&gateway, &plan, &key, key_type).await;
        assert_refusal(
            &answer,
            (400, "BadRequestException"),
            INVALID_KEY_TYPE,
            &format!("CreateUsagePlanKey(keyType={key_type:?})"),
        );
    }

    // The one type the model declares IS accepted, so the refusals are about the value.
    meter_key(&gateway, &plan, &key).await;
}

#[tokio::test]
async fn a_tag_call_whose_arn_names_no_stored_usage_plan_names_the_type_and_the_id() {
    let gateway = Gateway::new();
    let created = create_usage_plan(
        &gateway,
        "the tagged plan",
        json!({ "name": "tagged-plan", "tags": tag_map(1) }),
    )
    .await;
    let stored = string_member(&created, "id", "the tagged plan");
    let gone = {
        let created =
            create_usage_plan(&gateway, "the deleted plan", json!({ "name": "untagged-plan" }))
                .await;
        let gone = string_member(&created, "id", "the deleted plan");
        let answer = delete_plan(&gateway, &gone).await;
        assert_eq!(
            answer.status, DELETE_STATUS,
            "case DeleteUsagePlan(the deleted plan): expected {DELETE_STATUS}, got {} \
             with {:?}",
            answer.status, answer.body
        );
        gone
    };
    let mut near_miss: Vec<char> = stored.chars().collect();
    near_miss[0] = if near_miss[0] == 'a' { 'b' } else { 'a' };
    let near_miss: String = near_miss.into_iter().collect();

    // Rows differ in the FORM of the id the ARN carries: one no plan ever had, a near
    // miss of the stored one, the stored plan's NAME rather than its id, and the id of a
    // plan that was stored and then deleted.
    for (case, id) in [
        ("an id no plan ever had", "zzzzz99999".to_string()),
        ("a near miss of the stored id", near_miss.clone()),
        ("the stored plan's name", "tagged-plan".to_string()),
        ("a deleted plan's id", gone.clone()),
    ] {
        let arn = apigateway_arn(&format!("/usageplans/{id}"));
        let expected = tagged_plan_not_found(&id);
        // Every tag call that resolves the ARN answers the same sentence: the read, the
        // write and the removal.
        assert_refusal(
            &get_tags(&gateway, &arn).await,
            (404, "NotFoundException"),
            &expected,
            &format!("GetTags / {case}"),
        );
        assert_refusal(
            &tag_resource(&gateway, &arn).await,
            (404, "NotFoundException"),
            &expected,
            &format!("TagResource / {case}"),
        );
        assert_refusal(
            &untag_resource(&gateway, &arn).await,
            (404, "NotFoundException"),
            &expected,
            &format!("UntagResource / {case}"),
        );
    }

    // The stored plan's own ARN resolves, so the refusals are about the id and not about
    // the ARN's form.
    let arn = apigateway_arn(&format!("/usageplans/{stored}"));
    let answer = tag_resource(&gateway, &arn).await;
    assert_eq!(
        answer.status, TAG_STATUS,
        "case TagResource(the stored plan): expected {TAG_STATUS}, got {} with {:?}",
        answer.status, answer.body
    );
    let answer = get_tags(&gateway, &arn).await;
    assert_eq!(
        answer.body["tags"]["owner"],
        json!("team"),
        "case GetTags(the stored plan): expected the written tag, got {:?}",
        answer.body["tags"]
    );
    let answer = untag_resource(&gateway, &arn).await;
    assert_eq!(
        answer.status, TAG_STATUS,
        "case UntagResource(the stored plan): expected {TAG_STATUS}, got {} with {:?}",
        answer.status, answer.body
    );
}
