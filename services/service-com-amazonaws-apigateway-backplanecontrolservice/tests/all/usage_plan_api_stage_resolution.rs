// SPDX-License-Identifier: CC0-1.0

//! The ORDER in which a usage plan's `apiStages` entry is resolved, asserted on the wire.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder, the
//! `CreateRestApi`/`CreateDeployment` helpers, `CreateUsagePlan`, `GetUsagePlan` and the
//! patch helper are this crate's existing harness, reused from `rest_api_lifecycle`,
//! `stage_lifecycle`, `usage_plan_lifecycle`, `patch_updates`, `rest_api_identifier_refusals`
//! and `usage_plan_state_and_refusals`.
//!
//! The RULE under test: an entry naming a stage of a REST API names a stage OF that API, so
//! the API identifier is resolved FIRST — an identifier no stored API answers to is the
//! account-qualified identifier sentence this service states wherever it resolves an API,
//! naming that identifier ALONE, and only an API that does resolve has its stages looked at,
//! where a stage it does not carry names the association instead. Both are 404
//! `NotFoundException` (`smithy_query(query="operation",
//! operation="CreateUsagePlan,UpdateUsagePlan")` models it with `httpError` 404), and neither
//! stores anything.
//!
//! The rows span what the rule generalises over rather than repeating one recorded call:
//! which half of the entry does not resolve, the form of the unknown identifier (its length,
//! alphabet and case), the stage name beside it (unknown, a stage of another API, a stage the
//! named API would have had), the entry's POSITION in a multi-entry `apiStages` list, and both
//! operations that associate a plan with a stage (the create's `apiStages` member and the
//! update's `add /apiStages` patch).

use serde_json::{json, Value};

use crate::patch_updates::op;
use crate::rest_api_identifier_refusals::no_such_api;
use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};
use crate::stage_lifecycle::create_deployed_stage;
use crate::usage_plan_lifecycle::{create_usage_plan, get_usage_plan};
use crate::usage_plan_state_and_refusals::api_stage_not_found;

/// Identifiers no stored API answers to. The rows differ along what the rule generalises
/// over: the length (one character, the ten characters API Gateway mints, longer), the
/// alphabet (letters, digits, a mix, a hyphen) and the case — `restApiId` is a plain
/// `String` with no `@pattern` (`smithy_query(query="operation", operation="GetRestApi")`),
/// so every row reaches the lookup instead of being refused for its shape.
const UNKNOWN_APIS: &[&str] = &["q", "zzzzzzzzzz", "0123456789", "Ab3Xy9Zq1W", "no-such-api-01"];

/// Stage names to name beside an unknown API: the stage name never reaches the sentence, so
/// a row differing only in it must be refused identically.
const STAGES_BESIDE: &[&str] = &["prod", "no-such-stage", "a"];

/// `CreateUsagePlan` with `request` sent as given, whatever the service answers.
async fn create_plan(gateway: &Gateway, request: Value) -> Answer {
    gateway.call("POST", "/usageplans", Some(request)).await
}

/// `UpdateUsagePlan` on `plan` with one `add /apiStages` of `entry`, whatever the service
/// answers — the other spelling a client associates one entry with.
async fn add_api_stage(gateway: &Gateway, plan: &str, entry: &str) -> Answer {
    gateway
        .call(
            "PATCH",
            &format!("/usageplans/{plan}"),
            Some(json!({ "patchOperations": [op("add", "/apiStages", Some(entry))] })),
        )
        .await
}

/// Assert `answer` is the modelled 404 `NotFoundException` carrying exactly `message`.
fn assert_not_found(answer: &Answer, message: &str, case: &str) {
    assert_failure(answer, 404, "NotFoundException", case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {:?}",
        answer.body["message"]
    );
}

/// Assert both operations refuse the entry `(api_id, stage)` with exactly `message`, and
/// that neither call changed the plan `plan_id` reports. Answers the sentence the create
/// stated, so a caller can compare the sentences a SET of rows was refused with.
async fn assert_both_refuse(
    gateway: &Gateway,
    plan_id: &str,
    stored_plan: &Value,
    api_id: &str,
    stage: &str,
    message: &str,
    case: &str,
) -> String {
    let answer = create_plan(
        gateway,
        json!({
            "name": format!("refused-{case}"),
            "apiStages": [{ "apiId": api_id, "stage": stage }],
        }),
    )
    .await;
    assert_not_found(&answer, message, &format!("CreateUsagePlan / {case}"));
    let stated = answer.body["message"].as_str().unwrap_or_default().to_string();

    let answer = add_api_stage(gateway, plan_id, &format!("{api_id}:{stage}")).await;
    assert_not_found(&answer, message, &format!("UpdateUsagePlan / {case}"));

    let read = get_usage_plan(gateway, plan_id).await;
    assert_eq!(
        &read.body, stored_plan,
        "case {case}: a refused association must leave the plan as it was, got {:?} after \
         {:?}",
        read.body, stored_plan
    );
    stated
}

#[tokio::test]
async fn an_api_stages_entry_whose_api_does_not_resolve_states_that_identifier_alone() {
    let gateway = Gateway::new();
    let (api, _) = create_api(&gateway, "api-stage-resolution").await;
    create_deployed_stage(&gateway, &api, "prod").await;

    // The plan every update row patches: it carries one resolvable association, so a
    // refused row is told apart from a plan that was never associated with anything.
    let stored_plan = create_usage_plan(
        &gateway,
        "the patched plan",
        json!({
            "name": "api-stage-resolution",
            "apiStages": [{ "apiId": api, "stage": "prod" }],
        }),
    )
    .await;
    let plan_id = string_member(&stored_plan, "id", "the patched plan");

    for unknown in UNKNOWN_APIS {
        let mut stated = Vec::new();
        for stage in STAGES_BESIDE {
            let case = format!("apiId {unknown:?} with stage {stage:?}");
            stated.push(
                assert_both_refuse(
                    &gateway,
                    &plan_id,
                    &stored_plan,
                    unknown,
                    stage,
                    &no_such_api(unknown),
                    &case,
                )
                .await,
            );
        }
        // The requested stage is no part of the sentence, so rows differing only in it are
        // refused identically: what the caller has to fix is the API identifier.
        let first = stated.first().cloned().unwrap_or_default();
        assert!(
            stated.iter().all(|message| *message == first),
            "case apiId {unknown:?}: the stage must not change the refusal, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn an_api_stages_entry_whose_api_resolves_states_the_association() {
    let gateway = Gateway::new();
    let (first, _) = create_api(&gateway, "api-stage-resolution-first").await;
    create_deployed_stage(&gateway, &first, "prod").await;
    let (second, _) = create_api(&gateway, "api-stage-resolution-second").await;
    create_deployed_stage(&gateway, &second, "canary").await;

    let stored_plan = create_usage_plan(
        &gateway,
        "the patched plan",
        json!({
            "name": "api-stage-resolution-stages",
            "apiStages": [{ "apiId": first, "stage": "prod" }],
        }),
    )
    .await;
    let plan_id = string_member(&stored_plan, "id", "the patched plan");

    // Rows differ in the form of the stage name a STORED API does not carry: one no stage
    // of either API answers to, one that is a stage of the OTHER API, and one that differs
    // from a stage of this API only in case.
    let rows: Vec<(String, String, String)> = vec![
        (
            "a stage neither api has".to_string(),
            first.clone(),
            "no-such-stage".to_string(),
        ),
        (
            "a stage of the other api".to_string(),
            first.clone(),
            "canary".to_string(),
        ),
        (
            "a stage of the first api named on the second".to_string(),
            second.clone(),
            "prod".to_string(),
        ),
        (
            "a stage of the api in another case".to_string(),
            first.clone(),
            "PROD".to_string(),
        ),
    ];

    for (case, api_id, stage) in &rows {
        let expected = api_stage_not_found(api_id, stage);
        assert_both_refuse(
            &gateway,
            &plan_id,
            &stored_plan,
            api_id,
            stage,
            &expected,
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn the_api_identifier_of_an_api_stages_entry_is_resolved_before_its_stage() {
    let gateway = Gateway::new();
    let (api, _) = create_api(&gateway, "api-stage-resolution-order").await;
    create_deployed_stage(&gateway, &api, "prod").await;

    let stored_plan = create_usage_plan(
        &gateway,
        "the patched plan",
        json!({
            "name": "api-stage-resolution-order",
            "apiStages": [{ "apiId": api, "stage": "prod" }],
        }),
    )
    .await;
    let plan_id = string_member(&stored_plan, "id", "the patched plan");

    // An entry with NEITHER half resolvable is refused for its API identifier: that is the
    // half resolved first, so the stage is never reached.
    let unknown = "zzzzzzzzzz";
    assert_both_refuse(
        &gateway,
        &plan_id,
        &stored_plan,
        unknown,
        "no-such-stage",
        &no_such_api(unknown),
        "neither half resolves",
    )
    .await;

    // The list is resolved ENTRY BY ENTRY, so the unresolvable entry is what is stated
    // wherever it sits: a list whose first entry resolves and whose second does not is
    // refused for the second, and nothing of the first is stored.
    for (case, api_stages) in [
        (
            "the unresolvable entry first",
            json!([
                { "apiId": unknown, "stage": "prod" },
                { "apiId": api, "stage": "prod" },
            ]),
        ),
        (
            "the unresolvable entry second",
            json!([
                { "apiId": api, "stage": "prod" },
                { "apiId": unknown, "stage": "prod" },
            ]),
        ),
    ] {
        let answer = create_plan(
            &gateway,
            json!({ "name": format!("refused-{case}"), "apiStages": api_stages }),
        )
        .await;
        assert_not_found(
            &answer,
            &no_such_api(unknown),
            &format!("CreateUsagePlan / {case}"),
        );
    }

    // The entries that DO resolve are associated and read back, so the refusals above are
    // about the identifier and not about the operations.
    let created = create_usage_plan(
        &gateway,
        "a resolvable entry",
        json!({
            "name": "api-stage-resolution-accepted",
            "apiStages": [{ "apiId": api, "stage": "prod" }],
        }),
    )
    .await;
    let created_id = string_member(&created, "id", "a resolvable entry");
    let read = get_usage_plan(&gateway, &created_id).await;
    assert_eq!(
        read.body, created,
        "case a resolvable entry: GetUsagePlan must report the created plan, got {:?} after \
         {:?}",
        read.body, created
    );

    create_deployed_stage(&gateway, &api, "beta").await;
    let answer = add_api_stage(&gateway, &plan_id, &format!("{api}:beta")).await;
    assert_eq!(
        answer.status, 200,
        "case UpdateUsagePlan(a resolvable entry): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let read = get_usage_plan(&gateway, &plan_id).await;
    let associated: Vec<String> = read.body["apiStages"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|entry| {
            format!(
                "{}:{}",
                string_member(entry, "apiId", "the patched plan"),
                string_member(entry, "stage", "the patched plan")
            )
        })
        .collect();
    assert_eq!(
        associated,
        vec![format!("{api}:prod"), format!("{api}:beta")],
        "case UpdateUsagePlan(a resolvable entry): expected both associations, got {associated:?}"
    );
}
