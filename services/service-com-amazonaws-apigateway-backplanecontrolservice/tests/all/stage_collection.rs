// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetStages`: the collection of an API's stages, and the
//! `deploymentId` filter that narrows it.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi`/`CreateDeployment`/`CreateStage` helpers are this crate's existing
//! harness, reused from `rest_api_lifecycle` and `stage_lifecycle`.
//!
//! The tests assert RULES over sets of inputs, not one recorded call: the collection is
//! asserted over a table of stage shapes and counts (including none at all), the filter
//! over every deployment the API has plus one it does not, each listed entry against the
//! `GetStage` read of the same record, and the collection against the API that owns it.

use serde_json::{json, Value};

use crate::rest_api_lifecycle::{
    assert_failure, create_api, page_items, string_member, Answer, Gateway,
};
use crate::stage_lifecycle::{create_deployment, get_stage};

/// `GetStages` for `api`, with `query` appended verbatim (`""` for no filter).
async fn get_stages(gateway: &Gateway, api: &str, query: &str) -> Answer {
    gateway
        .call("GET", &format!("/restapis/{api}/stages{query}"), None)
        .await
}

/// The `stageName`s of a `GetStages` answer, sorted so the assertion is about the SET
/// the collection reports (AWS documents no order for it) rather than one ordering.
fn reported_stage_names(body: &Value, case: &str) -> Vec<String> {
    let mut names: Vec<String> = page_items(body, case)
        .iter()
        .map(|item| string_member(item, "stageName", case))
        .collect();
    names.sort();
    names
}

/// `CreateStage` of `stage_name` on `deployment`, with `extra` merged into the request.
async fn create_stage(
    gateway: &Gateway,
    api: &str,
    stage_name: &str,
    deployment: &str,
    extra: &Value,
) -> Value {
    let mut request = json!({ "stageName": stage_name, "deploymentId": deployment });
    if let (Some(target), Some(source)) = (request.as_object_mut(), extra.as_object()) {
        for (member, value) in source {
            target.insert(member.clone(), value.clone());
        }
    }
    let answer = gateway
        .call("POST", &format!("/restapis/{api}/stages"), Some(request))
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateStage({stage_name}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

#[tokio::test]
async fn get_stages_reports_every_stage_of_the_api_as_get_stage_reports_it() {
    // Rows differ along what the rule generalises over: which optional members a stage
    // carries, how many variables and tags it has, and whether a cache cluster or a
    // canary was asked for. The collection is read after each row, so the assertion
    // spans a collection of one stage up to one of five.
    let cases: Vec<(&str, &str, Value)> = vec![
        ("every optional member absent", "bare", json!({})),
        (
            "described and documented",
            "described",
            json!({ "description": "the production stage", "documentationVersion": "v1" }),
        ),
        (
            "three stage variables",
            "with_vars",
            json!({ "variables": { "table": "pets", "region": "us-east-1", "tier": "gold" } }),
        ),
        (
            "a cache cluster and tags",
            "cached-1",
            json!({ "cacheClusterEnabled": true, "cacheClusterSize": "0.5", "tags": { "team": "api" } }),
        ),
        (
            "tracing and a canary",
            "canary_stage",
            json!({
                "tracingEnabled": true,
                "canarySettings": { "percentTraffic": 10.0, "useStageCache": false }
            }),
        ),
    ];

    let gateway = Gateway::new();
    let (api, _) = create_api(&gateway, "stage-collection").await;
    let deployment = create_deployment(
        &gateway,
        &api,
        "CreateDeployment for the collection",
        json!({ "description": "the first deployment" }),
    )
    .await;

    // An API with no stages has the collection, and it is empty — not a refusal and not
    // an absent member.
    let empty = get_stages(&gateway, &api, "").await;
    assert_eq!(
        empty.status, 200,
        "case GetStages(no stages): expected 200, got {} with {:?}",
        empty.status, empty.body
    );
    assert_eq!(
        reported_stage_names(&empty.body, "GetStages(no stages)"),
        Vec::<String>::new(),
        "case GetStages(no stages): expected an empty collection, got {:?}",
        empty.body
    );

    let mut expected: Vec<String> = Vec::new();
    for (case, stage_name, extra) in &cases {
        let created = create_stage(&gateway, &api, stage_name, &deployment, extra).await;
        expected.push((*stage_name).to_string());
        expected.sort();

        let listed = get_stages(&gateway, &api, "").await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected 200 from GetStages, got {} with {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            reported_stage_names(&listed.body, case),
            expected,
            "case {case}: expected every created stage, got {:?}",
            listed.body
        );

        // Each entry is the stored record: the same body `GetStage` answers for it, and
        // the same body the create answered, member for member. `cacheClusterStatus` is
        // the one member the clock moves, so the comparison that spans a span is the one
        // against the concurrent `GetStage` read.
        let entry = page_items(&listed.body, case)
            .into_iter()
            .find(|item| item["stageName"].as_str() == Some(*stage_name))
            .unwrap_or_else(|| panic!("case {case}: the created stage must be listed"));
        let read = get_stage(&gateway, &api, stage_name).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetStage, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            entry, read.body,
            "case {case}: a listed stage must be the record GetStage reports, got {entry:?} against {:?}",
            read.body
        );
        for member in ["deploymentId", "description", "variables", "tags", "createdDate"] {
            assert_eq!(
                entry[member], created[member],
                "case {case}: the listed {member} must be the created stage's, expected {:?}, got {:?}",
                created[member], entry[member]
            );
        }
    }
}

#[tokio::test]
async fn the_deployment_id_filter_selects_the_stages_that_serve_that_deployment() {
    let gateway = Gateway::new();
    let (api, _) = create_api(&gateway, "stage-filter").await;

    // Three deployments carrying 2, 1 and 0 stages, so the filter is asserted over a
    // deployment several stages serve, one a single stage serves, one no stage serves,
    // and an id that is no deployment of this API at all.
    let first = create_deployment(&gateway, &api, "CreateDeployment(first)", json!({})).await;
    let second = create_deployment(&gateway, &api, "CreateDeployment(second)", json!({})).await;
    let third = create_deployment(&gateway, &api, "CreateDeployment(third)", json!({})).await;
    for (stage_name, deployment) in [
        ("alpha", &first),
        ("beta", &first),
        ("gamma", &second),
    ] {
        create_stage(&gateway, &api, stage_name, deployment, &json!({})).await;
    }

    let cases: Vec<(&str, String, Vec<&str>)> = vec![
        (
            "no filter",
            String::new(),
            vec!["alpha", "beta", "gamma"],
        ),
        (
            "a deployment two stages serve",
            format!("?deploymentId={first}"),
            vec!["alpha", "beta"],
        ),
        (
            "a deployment one stage serves",
            format!("?deploymentId={second}"),
            vec!["gamma"],
        ),
        (
            "a deployment no stage serves",
            format!("?deploymentId={third}"),
            vec![],
        ),
        (
            "an identifier that is no deployment of this API",
            "?deploymentId=zzzzzz".to_string(),
            vec![],
        ),
    ];

    for (case, query, expected) in &cases {
        let answer = get_stages(&gateway, &api, query).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200 from GetStages, got {} with {:?}",
            answer.status, answer.body
        );
        let mut want: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        want.sort();
        assert_eq!(
            reported_stage_names(&answer.body, case),
            want,
            "case {case}: expected the stages serving that deployment, got {:?}",
            answer.body
        );
        // Whatever the filter selects, every entry it reports serves the filtered
        // deployment.
        if let Some(deployment_id) = query.strip_prefix("?deploymentId=") {
            for item in page_items(&answer.body, case) {
                assert_eq!(
                    item["deploymentId"].as_str(),
                    Some(deployment_id),
                    "case {case}: a filtered entry must serve {deployment_id}, got {item:?}"
                );
            }
        }
    }

    // Re-pointing a stage at another deployment moves it between the two filtered
    // collections: the filter reads the stage's stored deployment, it does not remember
    // which deployment created the stage.
    create_deployment(
        &gateway,
        &api,
        "CreateDeployment(redeploy alpha)",
        json!({ "stageName": "alpha" }),
    )
    .await;
    let still_first = get_stages(&gateway, &api, &format!("?deploymentId={first}")).await;
    assert_eq!(
        reported_stage_names(&still_first.body, "after the redeploy"),
        vec!["beta".to_string()],
        "case after the redeploy: alpha no longer serves the first deployment, got {:?}",
        still_first.body
    );
}

#[tokio::test]
async fn the_collection_belongs_to_the_api_that_owns_it() {
    let gateway = Gateway::new();
    let (first_api, _) = create_api(&gateway, "stage-owner-one").await;
    let (second_api, _) = create_api(&gateway, "stage-owner-two").await;
    let deployment =
        create_deployment(&gateway, &first_api, "CreateDeployment(owner)", json!({})).await;
    create_stage(&gateway, &first_api, "prod", &deployment, &json!({})).await;

    // Rows: the API that has the stage, a sibling API that has none, and an identifier
    // no API answers to.
    assert_eq!(
        reported_stage_names(
            &get_stages(&gateway, &first_api, "").await.body,
            "the owning API"
        ),
        vec!["prod".to_string()],
        "case the owning API: expected its one stage"
    );
    assert_eq!(
        reported_stage_names(
            &get_stages(&gateway, &second_api, "").await.body,
            "a sibling API"
        ),
        Vec::<String>::new(),
        "case a sibling API: another API's stage must not be listed on it"
    );
    assert_failure(
        &get_stages(&gateway, "zzzzzzzzzz", "").await,
        404,
        "NotFoundException",
        "GetStages on an unknown restApiId",
    );

    // A second stage joins the same collection, and deleting the API takes the whole
    // collection with it.
    create_stage(&gateway, &first_api, "staging", &deployment, &json!({})).await;
    assert_eq!(
        reported_stage_names(
            &get_stages(&gateway, &first_api, "").await.body,
            "two stages"
        ),
        vec!["prod".to_string(), "staging".to_string()],
        "case two stages: expected both stages of the API"
    );
    let dropped = gateway
        .call("DELETE", &format!("/restapis/{first_api}"), None)
        .await;
    assert_eq!(
        dropped.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        dropped.status, dropped.body
    );
    assert_failure(
        &get_stages(&gateway, &first_api, "").await,
        404,
        "NotFoundException",
        "GetStages after DeleteRestApi",
    );
}

#[tokio::test]
async fn listing_the_stages_does_not_change_what_a_later_caller_sees() {
    let gateway = Gateway::new();
    let (api, _) = create_api(&gateway, "stage-collection-stable").await;
    let deployment =
        create_deployment(&gateway, &api, "CreateDeployment(stable)", json!({})).await;
    create_stage(&gateway, &api, "prod", &deployment, &json!({})).await;
    create_stage(&gateway, &api, "staging", &deployment, &json!({})).await;

    let mut seen: Vec<Value> = Vec::new();
    for round in 0..3 {
        let answer = get_stages(&gateway, &api, "").await;
        assert_eq!(
            answer.status, 200,
            "case GetStages(round {round}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        seen.push(answer.body);
    }
    assert_eq!(
        seen[0], seen[2],
        "case GetStages repeated: a read must not change the collection, got {:?} then {:?}",
        seen[0], seen[2]
    );
    // And it changed nothing a single-stage read sees either.
    let read = get_stage(&gateway, &api, "prod").await;
    assert_eq!(
        read.status, 200,
        "case GetStage after the listings: expected 200, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.body["deploymentId"].as_str(),
        Some(deployment.as_str()),
        "case GetStage after the listings: expected the stored deployment, got {:?}",
        read.body["deploymentId"]
    );
}
