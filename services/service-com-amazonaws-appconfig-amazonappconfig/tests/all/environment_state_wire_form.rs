// SPDX-License-Identifier: CC0-1.0

//! `Environment$State` on the wire, pinned against the value AWS publishes for each
//! `EnvironmentState` member.
//!
//! The rule under test: an enum member goes on the wire as the value the service
//! publishes for it, which is not always the member's Smithy name — for
//! `EnvironmentState` the published values are the CamelCase spellings
//! `ReadyForDeployment | Deploying | RollingBack | RolledBack | Reverted` — and every
//! operation that serves an Environment document serves that same spelling, byte for
//! byte, out of the one catalog the crate spells it in.
//!
//! The tables below span what that rule generalises over: WHICH member of the enum (all
//! five, including the three no operation reaches yet), which OPERATION serves the
//! document (`CreateEnvironment`, `GetEnvironment`, `ListEnvironments`,
//! `UpdateEnvironment`), and WHAT the environment carries (name only, a description,
//! monitors, a name at the published upper bound) — none of which the state depends on,
//! which is the point. The literal strings here are deliberately literal: a regression
//! that renamed the crate's constants back to the member names would still satisfy a
//! test written in terms of those constants.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::ops::types::ENVIRONMENT_STATE_VALUES;
use service_com_amazonaws_appconfig_amazonappconfig::state;

use super::appconfig_workflow::{
    create_application, create_environment, create_profile, json_call, router, send, Answer,
};

/// Every `EnvironmentState` member paired with the value AppConfig puts on the wire for
/// it.
///
/// Source of the member names: the model's own enum, as
/// `smithy_query(query="shape", shape="EnvironmentState")` reports it and as the
/// generated `ENVIRONMENT_STATE_VALUES` carries it (`DEPLOYING`,
/// `READY_FOR_DEPLOYMENT`, `REVERTED`, `ROLLED_BACK`, `ROLLING_BACK`). Source of the
/// wire values: the AppConfig API Reference's valid-value list for `Environment$State`
/// (`ReadyForDeployment | Deploying | RollingBack | RolledBack | Reverted`), which is
/// also the form every recorded AppConfig response carries. The model JSON in
/// `smithy-model/` ships no `@enumValue` trait on these members, so the member names are
/// NOT the wire values here.
const PUBLISHED_ENVIRONMENT_STATES: &[(&str, &str)] = &[
    ("READY_FOR_DEPLOYMENT", "ReadyForDeployment"),
    ("DEPLOYING", "Deploying"),
    ("ROLLING_BACK", "RollingBack"),
    ("ROLLED_BACK", "RolledBack"),
    ("REVERTED", "Reverted"),
];

/// The value the crate serves for one modelled member — the single place it spells the
/// wire form, which is what every handler reads.
fn served_for(member: &str) -> Option<&'static str> {
    match member {
        "READY_FOR_DEPLOYMENT" => Some(state::ENVIRONMENT_READY_FOR_DEPLOYMENT),
        "DEPLOYING" => Some(state::ENVIRONMENT_DEPLOYING),
        "ROLLING_BACK" => Some(state::ENVIRONMENT_ROLLING_BACK),
        "ROLLED_BACK" => Some(state::ENVIRONMENT_ROLLED_BACK),
        "REVERTED" => Some(state::ENVIRONMENT_REVERTED),
        _ => None,
    }
}

/// A CamelCase wire value read back as the SCREAMING_SNAKE member name it belongs to, so
/// the pairing can be checked as a round trip rather than taken on trust.
fn member_name_of(wire_value: &str) -> String {
    let mut name = String::new();
    for (position, character) in wire_value.char_indices() {
        if character.is_uppercase() && position > 0 {
            name.push('_');
        }
        name.extend(character.to_uppercase());
    }
    name
}

/// The catalog the crate serves covers every member the model declares, each paired with
/// the value AppConfig publishes for it and never with the member name.
#[test]
fn every_modelled_environment_state_member_is_served_as_its_published_wire_value() {
    assert_eq!(
        PUBLISHED_ENVIRONMENT_STATES.len(),
        ENVIRONMENT_STATE_VALUES.len(),
        "the published catalog must cover every modelled EnvironmentState member, got {:?} \
         against {:?}",
        PUBLISHED_ENVIRONMENT_STATES,
        ENVIRONMENT_STATE_VALUES
    );

    for (member, wire_value) in PUBLISHED_ENVIRONMENT_STATES {
        assert!(
            ENVIRONMENT_STATE_VALUES.contains(member),
            "case {member:?}: expected a member the model declares, which is not among {:?}",
            ENVIRONMENT_STATE_VALUES
        );
        // The round trip is what makes the pairing checkable: the wire value, read as a
        // member name, must be the member it is paired with.
        let derived = member_name_of(wire_value);
        assert_eq!(
            derived.as_str(),
            *member,
            "case {member:?}: the wire value {wire_value:?} names the member {derived:?}, \
             expected {member:?}"
        );
        assert_ne!(
            wire_value, member,
            "case {member:?}: the wire value is the published spelling, not the member name, \
             got {wire_value:?}"
        );
        assert_eq!(
            served_for(member),
            Some(*wire_value),
            "case {member:?}: the crate must serve {wire_value:?}, got {:?}",
            served_for(member)
        );
    }

    // And nothing the model declares is left without a served value.
    for member in ENVIRONMENT_STATE_VALUES {
        assert!(
            PUBLISHED_ENVIRONMENT_STATES
                .iter()
                .any(|(known, _)| known == member),
            "case {member:?}: a modelled member with no published wire value in {:?}",
            PUBLISHED_ENVIRONMENT_STATES
        );
    }
}

/// The `Items` of a `ListEnvironments` answer.
fn items(answer: &Answer) -> Vec<Value> {
    answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// What `ListEnvironments` reports as `State` for the entry with `id`, or `None` when the
/// listing does not carry that entry.
async fn listed_state(
    router: &MultiServiceRouter,
    application: &str,
    id: &str,
) -> Option<Option<String>> {
    let listing = json_call(
        router,
        "GET",
        &format!("/applications/{application}/environments"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        listing.status,
        200,
        "ListEnvironments must answer 200, got {} / {}",
        listing.status,
        String::from_utf8_lossy(&listing.body)
    );
    items(&listing)
        .iter()
        .find(|item| item.get("Id").and_then(Value::as_str) == Some(id))
        .map(|item| {
            item.get("State")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

/// Assert that `GetEnvironment`, `ListEnvironments` and `UpdateEnvironment` all report
/// `expected` as the environment's `State` — the same value, out of the same catalog.
///
/// The `UpdateEnvironment` request carries no writable member, so it is a read-back of
/// the stored record through the third serializer rather than a change to it: `State` is
/// never a member the caller writes.
async fn assert_state_on_every_reader(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    id: &str,
    expected: &str,
) {
    let fetched = json_call(
        router,
        "GET",
        &format!("/applications/{application}/environments/{id}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        fetched.status,
        200,
        "case {case:?}: GetEnvironment must answer 200, got {} / {}",
        fetched.status,
        String::from_utf8_lossy(&fetched.body)
    );
    assert_eq!(
        fetched.string("State").as_deref(),
        Some(expected),
        "case {case:?}: GetEnvironment must report State {expected:?}, got {:?}",
        fetched.string("State")
    );

    let listed = listed_state(router, application, id).await;
    assert_eq!(
        listed,
        Some(Some(expected.to_string())),
        "case {case:?}: ListEnvironments must report State {expected:?} for {id}, got {listed:?}"
    );

    let updated = json_call(
        router,
        "PATCH",
        &format!("/applications/{application}/environments/{id}"),
        &json!({}),
    )
    .await;
    assert_eq!(
        updated.status,
        200,
        "case {case:?}: UpdateEnvironment must answer 200, got {} / {}",
        updated.status,
        String::from_utf8_lossy(&updated.body)
    );
    assert_eq!(
        updated.string("State").as_deref(),
        Some(expected),
        "case {case:?}: UpdateEnvironment must report State {expected:?}, got {:?}",
        updated.string("State")
    );
}

/// A fresh environment is `ReadyForDeployment` on the wire — in the 201 that minted it
/// and on every later read — whatever the environment carries.
#[tokio::test]
async fn an_environment_with_no_deployment_reports_ready_for_deployment_on_every_operation() {
    let router = router();
    let application = create_application(&router, "state-wire-form").await;

    // The published value for the member a fresh environment sits in, spelled out rather
    // than taken from the crate under test.
    let ready = "ReadyForDeployment";
    // `Name` is len 1..64 in the model, so the last row sits on the upper bound.
    let longest_name = "n".repeat(64);

    // Rows differ along what the environment document carries; the state does not depend
    // on any of it.
    let rows: Vec<(&str, Value)> = vec![
        ("name only", json!({ "Name": "bare" })),
        (
            "with a description",
            json!({ "Name": "described", "Description": "a deployment group" }),
        ),
        (
            "with two monitors",
            json!({
                "Name": "monitored",
                "Monitors": [
                    { "AlarmArn": "arn:aws:cloudwatch:us-east-1:000000000000:alarm:one" },
                    {
                        "AlarmArn": "arn:aws:cloudwatch:us-east-1:000000000000:alarm:two",
                        "AlarmRoleArn": "arn:aws:iam::000000000000:role/appconfig-monitor",
                    },
                ],
            }),
        ),
        (
            "name at the published upper bound",
            json!({ "Name": longest_name }),
        ),
    ];

    for (case, body) in &rows {
        let created = json_call(
            &router,
            "POST",
            &format!("/applications/{application}/environments"),
            body,
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: CreateEnvironment must answer 201, got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        assert_eq!(
            created.string("State").as_deref(),
            Some(ready),
            "case {case:?}: CreateEnvironment must report State {ready:?}, got {:?}",
            created.string("State")
        );
        let id = created
            .string("Id")
            .unwrap_or_else(|| panic!("case {case:?}: CreateEnvironment answered no Id"));
        assert_state_on_every_reader(&router, case, &application, &id, ready).await;
    }
}

/// An environment with a deployment in flight reports `Deploying` — the published
/// spelling of the other member this service moves an environment into — on every
/// operation that serves it, while a sibling environment in the same application is
/// still `ReadyForDeployment`.
#[tokio::test]
async fn an_environment_with_a_deployment_in_flight_reports_deploying_on_every_operation() {
    let router = router();
    let application = create_application(&router, "state-wire-form-deploying").await;
    let profile = create_profile(&router, &application, "deployed-config").await;
    let hosted = send(
        &router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        br#"{"enabled":true}"#.to_vec(),
        &[],
    )
    .await;
    assert_eq!(
        hosted.status,
        201,
        "the configuration to deploy must be hosted first, got {} / {}",
        hosted.status,
        String::from_utf8_lossy(&hosted.body)
    );

    let deploying_environment = create_environment(&router, &application, "deployed-into").await;
    let idle_environment = create_environment(&router, &application, "left-alone").await;

    // The longest-running predefined strategy, so the rollout is still in flight when the
    // next call reads the environment.
    let slowest = state::PREDEFINED_DEPLOYMENT_STRATEGIES
        .iter()
        .max_by_key(|strategy| {
            strategy.deployment_duration_in_minutes + strategy.final_bake_time_in_minutes
        })
        .expect("AppConfig predefines at least one deployment strategy");
    let started = json_call(
        &router,
        "POST",
        &format!("/applications/{application}/environments/{deploying_environment}/deployments"),
        &json!({
            "DeploymentStrategyId": slowest.id,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": "1",
        }),
    )
    .await;
    assert_eq!(
        started.status,
        201,
        "StartDeployment must answer 201, got {} / {}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );

    assert_state_on_every_reader(
        &router,
        "deployment in flight",
        &application,
        &deploying_environment,
        "Deploying",
    )
    .await;
    assert_state_on_every_reader(
        &router,
        "sibling with no deployment",
        &application,
        &idle_environment,
        "ReadyForDeployment",
    )
    .await;

    // An environment created while another one is deploying is still ready for one, and
    // the 201 that mints it says so in the same spelling.
    let fresh = json_call(
        &router,
        "POST",
        &format!("/applications/{application}/environments"),
        &json!({ "Name": "created-mid-deployment" }),
    )
    .await;
    assert_eq!(
        fresh.string("State").as_deref(),
        Some("ReadyForDeployment"),
        "case \"created mid deployment\": CreateEnvironment must report \"ReadyForDeployment\", \
         got {:?}",
        fresh.string("State")
    );
}
