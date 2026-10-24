// SPDX-License-Identifier: CC0-1.0

//! `UpdateDeploymentStrategy` as a PATCH over the stored deployment-strategy record.
//!
//! The rule under test: a PATCH to a deployment strategy this account created writes every
//! member the request carries into the stored record and leaves every member it omits
//! alone, so the patched record is what EVERY reader of that strategy answers afterwards —
//! the update's own response, `GetDeploymentStrategy`, and the strategy's entry in
//! `ListDeploymentStrategies` alike, since the model binds all three to the same
//! `DeploymentStrategy` shape
//! (`smithy_query(query="operation",operation="UpdateDeploymentStrategy,ListDeploymentStrategies")`).
//!
//! The tables below span what that rule generalises over: WHICH members the patch carries
//! (each one alone, every one together, and none at all), WHAT values they carry (both
//! published bounds of `MinutesBetween0And24Hours` (range 0..1440) and `GrowthFactor`
//! (range 1..100), each `GrowthType` the model lists, and an empty description), and WHICH
//! strategy the path names (one this account created, each of AppConfig's predefined
//! catalogue entries, and an id nothing answers to). Every assertion names its case and
//! prints the actual value.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::state::PREDEFINED_DEPLOYMENT_STRATEGIES;

use super::appconfig_workflow::{json_call, router, Answer};

/// The collection the model binds the deployment-strategy operations to
/// (`/deploymentstrategies`, with the single resource at `/deploymentstrategies/{Id}`).
const STRATEGIES: &str = "/deploymentstrategies";

/// `CreateDeploymentStrategy`, answering the minted id.
async fn create(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let created = json_call(router, "POST", STRATEGIES, body).await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: CreateDeploymentStrategy expected 201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
        .string("Id")
        .unwrap_or_else(|| panic!("case {case:?}: CreateDeploymentStrategy answered no Id"))
}

/// `UpdateDeploymentStrategy`: `PATCH /deploymentstrategies/{DeploymentStrategyId}`.
async fn patch(router: &MultiServiceRouter, id: &str, body: &Value) -> Answer {
    json_call(router, "PATCH", &format!("{STRATEGIES}/{id}"), body).await
}

/// `GetDeploymentStrategy`.
async fn get(router: &MultiServiceRouter, id: &str) -> Answer {
    json_call(router, "GET", &format!("{STRATEGIES}/{id}"), &Value::Null).await
}

/// The entry `ListDeploymentStrategies` carries for `id`, or `None` when the catalogue
/// does not report it.
async fn listed(router: &MultiServiceRouter, case: &str, id: &str) -> Option<Value> {
    let answer = json_call(router, "GET", STRATEGIES, &Value::Null).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: ListDeploymentStrategies expected 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|item| item.get("Id").and_then(Value::as_str) == Some(id))
}

/// The record a reader must answer for one strategy: the whole `DeploymentStrategy` shape.
#[derive(Clone, Debug, PartialEq)]
struct Record {
    name: String,
    description: Option<String>,
    duration: i64,
    bake: i64,
    growth_factor: f64,
    growth_type: String,
    replicate_to: String,
}

/// Assert `document` reports `expected`, member by member, naming the reader it came from.
fn assert_record(case: &str, reader: &str, document: &Value, expected: &Record) {
    assert_eq!(
        document.get("Name").and_then(Value::as_str),
        Some(expected.name.as_str()),
        "case {case:?}: {reader} expected Name {:?}, got {document}",
        expected.name
    );
    assert_eq!(
        document.get("Description").and_then(Value::as_str),
        expected.description.as_deref(),
        "case {case:?}: {reader} expected Description {:?}, got {document}",
        expected.description
    );
    assert_eq!(
        document.get("DeploymentDurationInMinutes").and_then(Value::as_i64),
        Some(expected.duration),
        "case {case:?}: {reader} expected DeploymentDurationInMinutes {}, got {document}",
        expected.duration
    );
    assert_eq!(
        document.get("FinalBakeTimeInMinutes").and_then(Value::as_i64),
        Some(expected.bake),
        "case {case:?}: {reader} expected FinalBakeTimeInMinutes {}, got {document}",
        expected.bake
    );
    let growth_factor = document
        .get("GrowthFactor")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: {reader} reported no numeric GrowthFactor, got {document}"));
    assert!(
        (growth_factor - expected.growth_factor).abs() < 1e-6,
        "case {case:?}: {reader} expected GrowthFactor {}, got {growth_factor}",
        expected.growth_factor
    );
    assert_eq!(
        document.get("GrowthType").and_then(Value::as_str),
        Some(expected.growth_type.as_str()),
        "case {case:?}: {reader} expected GrowthType {:?}, got {document}",
        expected.growth_type
    );
    assert_eq!(
        document.get("ReplicateTo").and_then(Value::as_str),
        Some(expected.replicate_to.as_str()),
        "case {case:?}: {reader} expected ReplicateTo {:?}, got {document}",
        expected.replicate_to
    );
}

/// Assert the update's response, a Get of the strategy and the strategy's entry in the
/// listing all report `expected` — and are the same document, as one shape answered by
/// three readers of one record.
async fn assert_every_reader_reports(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    patched: &Answer,
    expected: &Record,
) {
    assert_record(case, "UpdateDeploymentStrategy", &patched.json(), expected);

    let fetched = get(router, id).await;
    assert_eq!(
        fetched.status,
        200,
        "case {case:?}: GetDeploymentStrategy expected 200, got {} / {}",
        fetched.status,
        String::from_utf8_lossy(&fetched.body)
    );
    assert_record(case, "GetDeploymentStrategy", &fetched.json(), expected);

    let entry = listed(router, case, id)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: the catalogue must carry strategy {id:?}"));
    assert_record(case, "ListDeploymentStrategies", &entry, expected);

    assert_eq!(
        entry,
        fetched.json(),
        "case {case:?}: the listed entry must be the record GetDeploymentStrategy answers {}, \
         got {entry}",
        fetched.json()
    );
    assert_eq!(
        patched.json(),
        fetched.json(),
        "case {case:?}: the update's response must be the record a later read answers {}, \
         got {}",
        fetched.json(),
        patched.json()
    );
}

/// A patch replaces the members it carries and leaves the rest of the stored record alone,
/// and every reader of the strategy — the update itself, the Get, and the listing — serves
/// the patched record from then on.
#[tokio::test]
async fn a_patch_persists_to_every_reader_of_the_strategy() {
    let router = router();
    let created = json!({
        "Name": "rollout",
        "Description": "created",
        "DeploymentDurationInMinutes": 10,
        "FinalBakeTimeInMinutes": 0,
        "GrowthFactor": 10,
        "GrowthType": "LINEAR",
        "ReplicateTo": "NONE",
    });
    let id = create(&router, "the strategy under test", &created).await;

    // Rows are applied in order to the one strategy, so each row also asserts that the
    // members it does NOT carry still report what the previous row left stored. They
    // differ along which members the patch carries (three at once, each one alone, every
    // one together, none at all) and along the values carried: the published bounds of
    // `MinutesBetween0And24Hours` (range 0..1440), of `GrowthFactor` (range 1..100) and of
    // `Description` (len 0..1024), and each `GrowthType` the model lists — bounds from
    // `smithy_query(query="operation",operation="UpdateDeploymentStrategy")`.
    let longest_description = "d".repeat(1024);
    let rows: Vec<(&str, Value, Record)> = vec![
        (
            "duration, bake time and growth factor together",
            json!({
                "DeploymentDurationInMinutes": 20,
                "FinalBakeTimeInMinutes": 5,
                "GrowthFactor": 25,
            }),
            Record {
                name: "rollout".to_string(),
                description: Some("created".to_string()),
                duration: 20,
                bake: 5,
                growth_factor: 25.0,
                growth_type: "LINEAR".to_string(),
                replicate_to: "NONE".to_string(),
            },
        ),
        (
            "the description alone",
            json!({ "Description": "updated-strategy" }),
            Record {
                name: "rollout".to_string(),
                description: Some("updated-strategy".to_string()),
                duration: 20,
                bake: 5,
                growth_factor: 25.0,
                growth_type: "LINEAR".to_string(),
                replicate_to: "NONE".to_string(),
            },
        ),
        (
            "the growth type alone, to EXPONENTIAL",
            json!({ "GrowthType": "EXPONENTIAL" }),
            Record {
                name: "rollout".to_string(),
                description: Some("updated-strategy".to_string()),
                duration: 20,
                bake: 5,
                growth_factor: 25.0,
                growth_type: "EXPONENTIAL".to_string(),
                replicate_to: "NONE".to_string(),
            },
        ),
        (
            "the growth factor alone, at its published upper bound",
            json!({ "GrowthFactor": 100 }),
            Record {
                name: "rollout".to_string(),
                description: Some("updated-strategy".to_string()),
                duration: 20,
                bake: 5,
                growth_factor: 100.0,
                growth_type: "EXPONENTIAL".to_string(),
                replicate_to: "NONE".to_string(),
            },
        ),
        (
            "the duration alone, at its published upper bound",
            json!({ "DeploymentDurationInMinutes": 1440 }),
            Record {
                name: "rollout".to_string(),
                description: Some("updated-strategy".to_string()),
                duration: 1440,
                bake: 5,
                growth_factor: 100.0,
                growth_type: "EXPONENTIAL".to_string(),
                replicate_to: "NONE".to_string(),
            },
        ),
        (
            "the bake time alone, at its published upper bound",
            json!({ "FinalBakeTimeInMinutes": 1440 }),
            Record {
                name: "rollout".to_string(),
                description: Some("updated-strategy".to_string()),
                duration: 1440,
                bake: 1440,
                growth_factor: 100.0,
                growth_type: "EXPONENTIAL".to_string(),
                replicate_to: "NONE".to_string(),
            },
        ),
        (
            "every patchable member at its published lower bound",
            json!({
                "Description": "",
                "DeploymentDurationInMinutes": 0,
                "FinalBakeTimeInMinutes": 0,
                "GrowthFactor": 1,
                "GrowthType": "LINEAR",
            }),
            Record {
                name: "rollout".to_string(),
                // The empty string is a value the caller sent, not an omission:
                // `Description` is len 0..1024.
                description: Some(String::new()),
                duration: 0,
                bake: 0,
                growth_factor: 1.0,
                growth_type: "LINEAR".to_string(),
                replicate_to: "NONE".to_string(),
            },
        ),
        (
            "a description at its published upper bound",
            json!({ "Description": longest_description.clone() }),
            Record {
                name: "rollout".to_string(),
                description: Some(longest_description.clone()),
                duration: 0,
                bake: 0,
                growth_factor: 1.0,
                growth_type: "LINEAR".to_string(),
                replicate_to: "NONE".to_string(),
            },
        ),
        (
            "a patch carrying no member at all",
            json!({}),
            Record {
                name: "rollout".to_string(),
                description: Some(longest_description.clone()),
                duration: 0,
                bake: 0,
                growth_factor: 1.0,
                growth_type: "LINEAR".to_string(),
                replicate_to: "NONE".to_string(),
            },
        ),
    ];

    for (case, body, expected) in &rows {
        let patched = patch(&router, &id, body).await;
        assert_eq!(
            patched.status,
            200,
            "case {case:?}: UpdateDeploymentStrategy expected 200, got {} / {}",
            patched.status,
            String::from_utf8_lossy(&patched.body)
        );
        assert_eq!(
            patched.json().get("Id").and_then(Value::as_str),
            Some(id.as_str()),
            "case {case:?}: the patched record keeps its id {id:?}, got {}",
            patched.json()
        );
        assert_every_reader_reports(&router, case, &id, &patched, expected).await;
    }
}

/// A patch of one strategy moves no other strategy: the account's other records and the
/// predefined catalogue report exactly what they did before.
#[tokio::test]
async fn a_patch_moves_no_other_strategy() {
    let router = router();
    let mut ids = Vec::new();
    for index in 0..3 {
        ids.push(
            create(
                &router,
                &format!("neighbour {index}"),
                &json!({
                    "Name": format!("neighbour-{index}"),
                    "DeploymentDurationInMinutes": 10 + index,
                    "FinalBakeTimeInMinutes": index,
                    "GrowthFactor": 10 + index,
                }),
            )
            .await,
        );
    }

    let before = json_call(&router, "GET", STRATEGIES, &Value::Null).await.json();
    let target = ids[1].clone();
    let patched = patch(
        &router,
        &target,
        &json!({
            "DeploymentDurationInMinutes": 45,
            "FinalBakeTimeInMinutes": 7,
            "GrowthFactor": 33,
            "GrowthType": "EXPONENTIAL",
            "Description": "retuned",
        }),
    )
    .await;
    assert_eq!(
        patched.status,
        200,
        "case \"one of several strategies\": expected 200, got {} / {}",
        patched.status,
        String::from_utf8_lossy(&patched.body)
    );

    let after = json_call(&router, "GET", STRATEGIES, &Value::Null).await.json();
    let entries = |catalogue: &Value| -> Vec<Value> {
        catalogue
            .get("Items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let before_entries = entries(&before);
    let after_entries = entries(&after);
    assert_eq!(
        after_entries.len(),
        before_entries.len(),
        "case \"one of several strategies\": a patch creates and removes nothing, expected \
         {} entries, got {after_entries:?}",
        before_entries.len()
    );
    for (old, new) in before_entries.iter().zip(after_entries.iter()) {
        let listed_id = new.get("Id").and_then(Value::as_str).unwrap_or_default();
        assert_eq!(
            old.get("Id").and_then(Value::as_str),
            new.get("Id").and_then(Value::as_str),
            "case \"one of several strategies\": the catalogue's order must not move, \
             expected {old}, got {new}"
        );
        if listed_id == target {
            assert_ne!(
                old, new,
                "case \"one of several strategies\": the patched strategy must have changed, \
                 got {new}"
            );
        } else {
            assert_eq!(
                old, new,
                "case \"one of several strategies\": strategy {listed_id:?} was not patched and \
                 must be unchanged, expected {old}, got {new}"
            );
        }
    }
}

/// AppConfig's predefined catalogue entries are not this account's records: a patch of one
/// is refused with the modelled `BadRequestException` and changes nothing.
#[tokio::test]
async fn a_predefined_strategy_is_not_patchable() {
    let router = router();

    // Rows are the whole predefined catalogue (`state::PREDEFINED_DEPLOYMENT_STRATEGIES`,
    // the AWS AppConfig User Guide's "Predefined deployment strategies" table), so every
    // entry is asserted rather than the one a caller happens to name.
    for predefined in PREDEFINED_DEPLOYMENT_STRATEGIES {
        let case = predefined.id;
        let before = get(&router, predefined.id).await;
        assert_eq!(
            before.status,
            200,
            "case {case:?}: GetDeploymentStrategy expected 200, got {} / {}",
            before.status,
            String::from_utf8_lossy(&before.body)
        );

        let refused = patch(
            &router,
            predefined.id,
            &json!({ "DeploymentDurationInMinutes": 7, "GrowthFactor": 3 }),
        )
        .await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: patching a predefined strategy expected 400, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert!(
            refused.error_code().contains("BadRequestException"),
            "case {case:?}: expected the modelled BadRequestException, got {:?}",
            refused.error_code()
        );

        let after = get(&router, predefined.id).await;
        assert_eq!(
            after.json(),
            before.json(),
            "case {case:?}: a refused patch must change nothing, expected {}, got {}",
            before.json(),
            after.json()
        );
        let entry = listed(&router, case, predefined.id)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the catalogue must still carry it"));
        assert_eq!(
            entry,
            before.json(),
            "case {case:?}: the listing must still report the published record {}, got {entry}",
            before.json()
        );
    }
}

/// An id nothing answers to is refused with the modelled `ResourceNotFoundException`, and
/// a patch never creates a record.
#[tokio::test]
async fn an_id_nothing_answers_to_is_not_found() {
    let router = router();
    let existing = create(
        &router,
        "a strategy that does exist",
        &json!({
            "Name": "exists",
            "DeploymentDurationInMinutes": 10,
            "FinalBakeTimeInMinutes": 0,
            "GrowthFactor": 10,
        }),
    )
    .await;

    // Rows differ along how the id fails to name a stored record, each one still inside
    // this member's own pattern (`^(^[a-z0-9]{4,7}$|^AppConfig\.[A-Za-z0-9]{9,40}$)$`): the
    // two forms the pattern allows, and a strategy a delete has just removed.
    let deleted = create(
        &router,
        "a strategy that is then deleted",
        &json!({
            "Name": "deleted",
            "DeploymentDurationInMinutes": 1,
            "FinalBakeTimeInMinutes": 1,
            "GrowthFactor": 1,
        }),
    )
    .await;
    // The model spells the DELETE route `/deployementstrategies/{DeploymentStrategyId}`
    // while every other strategy route is `/deploymentstrategies/...`
    // (`smithy_query(query="operation",operation="DeleteDeploymentStrategy")`), so the
    // delete is addressed by its own path rather than by `STRATEGIES`.
    let removal = json_call(
        &router,
        "DELETE",
        &format!("/deployementstrategies/{deleted}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        removal.status,
        204,
        "case \"a strategy that is then deleted\": DeleteDeploymentStrategy expected 204, \
         got {} / {}",
        removal.status,
        String::from_utf8_lossy(&removal.body)
    );

    for (case, id) in [
        ("a minted-looking id nothing holds", "zzzzzz9".to_string()),
        ("the shortest id the pattern allows", "ab12".to_string()),
        (
            "a predefined-looking id AppConfig does not publish",
            "AppConfig.NoSuchStrategy".to_string(),
        ),
        ("a strategy a delete already removed", deleted.clone()),
    ] {
        let refused = patch(&router, &id, &json!({ "GrowthFactor": 50 })).await;
        assert_eq!(
            refused.status,
            404,
            "case {case:?}: expected 404, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert!(
            refused.error_code().contains("ResourceNotFoundException"),
            "case {case:?}: expected the modelled ResourceNotFoundException, got {:?}",
            refused.error_code()
        );
        assert!(
            listed(&router, case, &id).await.is_none(),
            "case {case:?}: a refused patch must create no record, got it in the catalogue"
        );
        let fetched = get(&router, &id).await;
        assert_eq!(
            fetched.status,
            404,
            "case {case:?}: GetDeploymentStrategy expected 404, got {} / {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
    }

    // The control: the strategy that does exist is still patchable, so the refusals above
    // are about the id and not about the operation.
    let patched = patch(&router, &existing, &json!({ "GrowthFactor": 50 })).await;
    assert_eq!(
        patched.status,
        200,
        "case \"a strategy that does exist\": expected 200, got {} / {}",
        patched.status,
        String::from_utf8_lossy(&patched.body)
    );
}

/// A value outside a member's modelled bound is refused with 400 and leaves the stored
/// record exactly as it was; the bounds themselves are accepted.
#[tokio::test]
async fn a_value_outside_a_modelled_bound_is_refused_and_stores_nothing() {
    let router = router();
    let id = create(
        &router,
        "bounded patches",
        &json!({
            "Name": "bounded",
            "Description": "created",
            "DeploymentDurationInMinutes": 10,
            "FinalBakeTimeInMinutes": 0,
            "GrowthFactor": 10,
            "GrowthType": "LINEAR",
        }),
    )
    .await;
    let before = get(&router, &id).await.json();

    // One past each published bound of the patchable members, plus a value outside the
    // `GrowthType` enum: bounds and values from
    // `smithy_query(query="operation",operation="UpdateDeploymentStrategy")`.
    for (case, body) in [
        (
            "one below the duration's lower bound",
            json!({ "DeploymentDurationInMinutes": -1 }),
        ),
        (
            "one past the duration's upper bound",
            json!({ "DeploymentDurationInMinutes": 1441 }),
        ),
        (
            "one below the bake time's lower bound",
            json!({ "FinalBakeTimeInMinutes": -1 }),
        ),
        (
            "one past the bake time's upper bound",
            json!({ "FinalBakeTimeInMinutes": 1441 }),
        ),
        (
            "one below the growth factor's lower bound",
            json!({ "GrowthFactor": 0 }),
        ),
        (
            "one past the growth factor's upper bound",
            json!({ "GrowthFactor": 101 }),
        ),
        (
            "one past the description's upper length bound",
            json!({ "Description": "d".repeat(1025) }),
        ),
        (
            "a growth type the enum does not list",
            json!({ "GrowthType": "QUADRATIC" }),
        ),
    ] {
        let refused = patch(&router, &id, &body).await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert!(
            refused.error_code().contains("BadRequestException"),
            "case {case:?}: expected the modelled BadRequestException, got {:?}",
            refused.error_code()
        );
        let after = get(&router, &id).await.json();
        assert_eq!(
            after, before,
            "case {case:?}: a refused patch must store nothing, expected {before}, got {after}"
        );
    }

    // The bounds themselves are values a patch stores.
    for (case, body, expected_duration, expected_factor) in [
        (
            "both lower bounds",
            json!({ "DeploymentDurationInMinutes": 0, "FinalBakeTimeInMinutes": 0, "GrowthFactor": 1 }),
            0_i64,
            1.0_f64,
        ),
        (
            "both upper bounds",
            json!({ "DeploymentDurationInMinutes": 1440, "FinalBakeTimeInMinutes": 1440, "GrowthFactor": 100 }),
            1440,
            100.0,
        ),
    ] {
        let patched = patch(&router, &id, &body).await;
        assert_eq!(
            patched.status,
            200,
            "case {case:?}: expected 200, got {} / {}",
            patched.status,
            String::from_utf8_lossy(&patched.body)
        );
        let fetched = get(&router, &id).await.json();
        assert_eq!(
            fetched.get("DeploymentDurationInMinutes").and_then(Value::as_i64),
            Some(expected_duration),
            "case {case:?}: expected DeploymentDurationInMinutes {expected_duration}, got {fetched}"
        );
        let factor = fetched
            .get("GrowthFactor")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a numeric GrowthFactor, got {fetched}"));
        assert!(
            (factor - expected_factor).abs() < 1e-6,
            "case {case:?}: expected GrowthFactor {expected_factor}, got {factor}"
        );
    }
}
