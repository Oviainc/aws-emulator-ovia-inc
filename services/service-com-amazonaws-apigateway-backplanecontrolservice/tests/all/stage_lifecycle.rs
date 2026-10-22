// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the deployment and stage lifecycle: `CreateDeployment`,
//! `CreateStage`, `GetDeployment` and `GetStage`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi`/`CreateResource` helpers are this crate's existing harness, reused
//! from `rest_api_lifecycle`.
//!
//! The tests assert RULES over sets of inputs, not one recorded call: the create/read
//! round trip over a table of request shapes, `stageName` over the character set and
//! length its documentation publishes (and one past the bound), the deployment's
//! `apiSummary` over the methods the API had when it was created, and every identifier
//! these operations resolve over the records that do and do not exist.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    CACHE_CLUSTER_AVAILABLE, CACHE_CLUSTER_CREATE_IN_PROGRESS, CACHE_CLUSTER_CREATE_SPAN,
    CACHE_CLUSTER_NOT_AVAILABLE, MAX_STAGE_NAME_CHARS,
};

use crate::rest_api_lifecycle::{
    assert_failure, create_api, create_resource, string_member, Answer, Gateway,
};

/// `CreateDeployment` for `api` with `request`, asserting the modelled 201 and
/// answering the deployment id.
async fn create_deployment(gateway: &Gateway, api: &str, case: &str, request: Value) -> String {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/deployments"),
            Some(request),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected 201 from CreateDeployment, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "id", case)
}

/// Deploy `api` to a stage named `stage_name`, the one call that creates both records,
/// and answer the deployment id. Used by the `api_key_lifecycle` suite, which needs a
/// stage to associate an API key with.
pub(crate) async fn create_deployed_stage(
    gateway: &Gateway,
    api: &str,
    stage_name: &str,
) -> String {
    create_deployment(
        gateway,
        api,
        &format!("CreateDeployment(stageName={stage_name})"),
        json!({ "stageName": stage_name }),
    )
    .await
}

/// `GetStage` for `stage_name` of `api`.
async fn get_stage(gateway: &Gateway, api: &str, stage_name: &str) -> Answer {
    gateway
        .call("GET", &format!("/restapis/{api}/stages/{stage_name}"), None)
        .await
}

/// An API with one resource carrying one method, which is what a deployment snapshots.
async fn api_with_a_method(gateway: &Gateway, name: &str) -> (String, String) {
    let (api, root) = create_api(gateway, name).await;
    let resource = create_resource(gateway, &api, &root, "pets").await;
    let resource_id = string_member(&resource, "id", name);
    let answer = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource_id}/methods/GET"),
            Some(json!({ "authorizationType": "NONE", "apiKeyRequired": true })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case {name}: expected 201 from PutMethod, got {} with {:?}",
        answer.status, answer.body
    );
    (api, resource_id)
}

#[tokio::test]
async fn a_created_stage_is_persisted_and_read_back_member_for_member() {
    // Rows differ along what the rule generalises over: which optional members the
    // request carries, how many stage variables and tags it sets, and whether tracing
    // and a canary are asked for.
    let cases: Vec<(&str, &str, Value)> = vec![
        ("every member absent", "bare", json!({})),
        (
            "described and documented",
            "described",
            json!({ "description": "the production stage", "documentationVersion": "v1" }),
        ),
        (
            "two stage variables",
            "with_vars",
            json!({ "variables": { "table": "pets", "region": "us-east-1" } }),
        ),
        (
            "tracing and tags",
            "traced-1",
            json!({ "tracingEnabled": true, "tags": { "team": "api" } }),
        ),
        (
            "a canary taking a tenth of the traffic",
            "canary_stage",
            json!({
                "canarySettings": {
                    "percentTraffic": 10.0,
                    "useStageCache": false,
                    "stageVariableOverrides": { "table": "pets-canary" }
                }
            }),
        ),
    ];

    let gateway = Gateway::new();
    let (api, _) = api_with_a_method(&gateway, "stage-round-trip").await;
    let deployment = create_deployment(
        &gateway,
        &api,
        "CreateDeployment for the round trip",
        json!({ "description": "the first deployment" }),
    )
    .await;

    for (case, stage_name, extra) in &cases {
        let mut request = json!({ "stageName": stage_name, "deploymentId": deployment });
        if let (Some(target), Some(source)) = (request.as_object_mut(), extra.as_object()) {
            for (member, value) in source {
                target.insert(member.clone(), value.clone());
            }
        }
        let created = gateway
            .call("POST", &format!("/restapis/{api}/stages"), Some(request.clone()))
            .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from CreateStage, got {} with {:?}",
            created.status, created.body
        );

        // Every echoed member reports what the request set, and nothing it did not.
        assert_eq!(
            created.body["stageName"].as_str(),
            Some(*stage_name),
            "case {case}: expected the stage name back, got {:?}",
            created.body["stageName"]
        );
        assert_eq!(
            created.body["deploymentId"].as_str(),
            Some(deployment.as_str()),
            "case {case}: expected the deployment it points at, got {:?}",
            created.body["deploymentId"]
        );
        for member in ["description", "documentationVersion", "variables", "tags"] {
            assert_eq!(
                created.body[member],
                request.get(member).cloned().unwrap_or(Value::Null),
                "case {case}: {member} must report the request's value, got {:?}",
                created.body[member]
            );
        }
        assert_eq!(
            created.body["tracingEnabled"],
            request.get("tracingEnabled").cloned().unwrap_or(json!(false)),
            "case {case}: tracingEnabled must report the request's value or its default, got {:?}",
            created.body["tracingEnabled"]
        );
        assert_eq!(
            created.body["canarySettings"]["percentTraffic"],
            request["canarySettings"]["percentTraffic"],
            "case {case}: canarySettings must report the request's settings, got {:?}",
            created.body["canarySettings"]
        );
        // A stage with no cache cluster has no cluster status to report but
        // NOT_AVAILABLE, and it always has a (possibly empty) methodSettings map.
        assert_eq!(
            created.body["cacheClusterStatus"].as_str(),
            Some(CACHE_CLUSTER_NOT_AVAILABLE),
            "case {case}: expected {CACHE_CLUSTER_NOT_AVAILABLE}, got {:?}",
            created.body["cacheClusterStatus"]
        );
        assert_eq!(
            created.body["methodSettings"],
            json!({}),
            "case {case}: a new stage carries an empty methodSettings map, got {:?}",
            created.body["methodSettings"]
        );
        assert!(
            created.body["createdDate"].is_number() && created.body["lastUpdatedDate"].is_number(),
            "case {case}: expected both timestamps, got {:?}",
            created.body
        );

        // Read back through GetStage: the stored record, member for member.
        let read = get_stage(&gateway, &api, stage_name).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetStage, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, created.body,
            "case {case}: GetStage must report the stored stage, got {:?} after {:?}",
            read.body, created.body
        );

        // A stage name names one stage: creating it again is a conflict.
        let again = gateway
            .call("POST", &format!("/restapis/{api}/stages"), Some(request))
            .await;
        assert_failure(
            &again,
            409,
            "ConflictException",
            &format!("{case} / CreateStage again"),
        );
    }
}

#[tokio::test]
async fn a_stage_name_is_accepted_only_as_its_documentation_publishes_it() {
    // Source: the `stageName` member's documentation — "Stage names can only contain
    // alphanumeric characters, hyphens, and underscores. Maximum length is 128
    // characters." The bound is tested at 1, at 128 and one past it.
    let cases: Vec<(String, bool)> = vec![
        ("prod".to_string(), true),
        ("Prod-2_beta".to_string(), true),
        ("0".to_string(), true),
        ("a".repeat(MAX_STAGE_NAME_CHARS), true),
        ("a".repeat(MAX_STAGE_NAME_CHARS + 1), false),
        ("with space".to_string(), false),
        ("with.dot".to_string(), false),
        ("with/slash".to_string(), false),
        ("pets+toys".to_string(), false),
    ];

    let gateway = Gateway::new();
    let (api, _) = api_with_a_method(&gateway, "stage-names").await;
    let deployment = create_deployment(
        &gateway,
        &api,
        "CreateDeployment for the stage names",
        json!({}),
    )
    .await;

    for (stage_name, accepted) in &cases {
        let case = format!("CreateStage(stageName={stage_name:?})");
        let answer = gateway
            .call(
                "POST",
                &format!("/restapis/{api}/stages"),
                Some(json!({ "stageName": stage_name, "deploymentId": deployment })),
            )
            .await;
        if *accepted {
            assert_eq!(
                answer.status, 201,
                "case {case}: expected 201 inside the published name rule, got {} with {:?}",
                answer.status, answer.body
            );
            let read = get_stage(&gateway, &api, stage_name).await;
            assert_eq!(
                read.body["stageName"].as_str(),
                Some(stage_name.as_str()),
                "case {case}: GetStage must report the stored name, got {:?}",
                read.body["stageName"]
            );
        } else {
            assert_failure(&answer, 400, "BadRequestException", &case);
            // A refused create stores nothing, so the name is still unknown. Only a
            // name that can be addressed as one path segment is probed this way; a
            // space or a slash is not a path segment at all.
            let one_segment = stage_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.~+".contains(c));
            if one_segment {
                assert_failure(
                    &get_stage(&gateway, &api, stage_name).await,
                    404,
                    "NotFoundException",
                    &format!("{case} / GetStage"),
                );
            }
        }
    }
}

#[tokio::test]
async fn a_cache_cluster_a_stage_asked_for_comes_up_after_its_span() {
    let gateway = Gateway::new();
    let (api, _) = api_with_a_method(&gateway, "stage-cache").await;
    let deployment =
        create_deployment(&gateway, &api, "CreateDeployment for the cache", json!({})).await;

    let created = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/stages"),
            Some(json!({
                "stageName": "cached",
                "deploymentId": deployment,
                "cacheClusterEnabled": true,
                "cacheClusterSize": "0.5"
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case CreateStage(cacheClusterEnabled): expected 201, got {} with {:?}",
        created.status, created.body
    );
    // A caller reading straight after the call still sees the cluster coming up.
    assert_eq!(
        created.body["cacheClusterStatus"].as_str(),
        Some(CACHE_CLUSTER_CREATE_IN_PROGRESS),
        "case CreateStage(cacheClusterEnabled): expected {CACHE_CLUSTER_CREATE_IN_PROGRESS}, got {:?}",
        created.body["cacheClusterStatus"]
    );
    assert_eq!(
        created.body["cacheClusterSize"].as_str(),
        Some("0.5"),
        "case CreateStage(cacheClusterEnabled): expected the requested size back, got {:?}",
        created.body["cacheClusterSize"]
    );

    // The transition is one the clock owes: once the span has passed, the cluster is up
    // — for every reader, not just the first.
    tokio::time::sleep(CACHE_CLUSTER_CREATE_SPAN + Duration::from_millis(100)).await;
    for round in 0..2 {
        let read = get_stage(&gateway, &api, "cached").await;
        assert_eq!(
            read.body["cacheClusterStatus"].as_str(),
            Some(CACHE_CLUSTER_AVAILABLE),
            "case GetStage(round {round}) after the span: expected {CACHE_CLUSTER_AVAILABLE}, got {:?}",
            read.body["cacheClusterStatus"]
        );
    }
}

#[tokio::test]
async fn a_deployment_snapshots_the_methods_the_api_had_when_it_was_created() {
    let gateway = Gateway::new();
    let (api, resource_id) = api_with_a_method(&gateway, "deployment-summary").await;

    let first = create_deployment(
        &gateway,
        &api,
        "CreateDeployment(first)",
        json!({ "description": "with one method" }),
    )
    .await;

    // The summary is reported only when the caller asks for it, which is what `embed`
    // documents.
    let without = gateway
        .call("GET", &format!("/restapis/{api}/deployments/{first}"), None)
        .await;
    assert_eq!(
        without.status, 200,
        "case GetDeployment(no embed): expected 200, got {} with {:?}",
        without.status, without.body
    );
    assert!(
        without.body["apiSummary"].is_null(),
        "case GetDeployment(no embed): the summary is embedded only on request, got {:?}",
        without.body["apiSummary"]
    );
    assert_eq!(
        without.body["description"].as_str(),
        Some("with one method"),
        "case GetDeployment(no embed): expected the stored description, got {:?}",
        without.body
    );

    let embedded = gateway
        .call(
            "GET",
            &format!("/restapis/{api}/deployments/{first}?embed=apisummary"),
            None,
        )
        .await;
    assert_eq!(
        embedded.body["apiSummary"],
        json!({ "/pets": { "GET": { "authorizationType": "NONE", "apiKeyRequired": true } } }),
        "case GetDeployment(embed=apisummary): expected the methods at creation time, got {:?}",
        embedded.body["apiSummary"]
    );

    // A method added after that deployment is not in it, and is in the next one: the
    // summary is a snapshot, not a live read.
    let added = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource_id}/methods/POST"),
            Some(json!({ "authorizationType": "NONE" })),
        )
        .await;
    assert_eq!(
        added.status, 201,
        "case PutMethod(POST): expected 201, got {} with {:?}",
        added.status, added.body
    );
    let second = create_deployment(&gateway, &api, "CreateDeployment(second)", json!({})).await;

    for (case, deployment, verbs) in [
        ("the first deployment", &first, vec!["GET"]),
        ("the second deployment", &second, vec!["GET", "POST"]),
    ] {
        let answer = gateway
            .call(
                "GET",
                &format!("/restapis/{api}/deployments/{deployment}?embed=apisummary"),
                None,
            )
            .await;
        let mut reported: Vec<String> = answer.body["apiSummary"]["/pets"]
            .as_object()
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default();
        reported.sort();
        assert_eq!(
            reported, verbs,
            "case {case}: expected the verbs {verbs:?} at its creation time, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn deploying_to_an_existing_stage_points_that_stage_at_the_new_deployment() {
    let gateway = Gateway::new();
    let (api, _) = api_with_a_method(&gateway, "redeploy").await;

    // The first deployment creates the stage, carrying the stage members this request
    // sets.
    let first = create_deployment(
        &gateway,
        &api,
        "CreateDeployment(creating the stage)",
        json!({
            "stageName": "prod",
            "stageDescription": "the production stage",
            "variables": { "table": "pets" }
        }),
    )
    .await;
    let created = get_stage(&gateway, &api, "prod").await;
    assert_eq!(
        created.status, 200,
        "case CreateDeployment(stageName): the stage must exist, got {} with {:?}",
        created.status, created.body
    );
    assert_eq!(
        created.body["deploymentId"].as_str(),
        Some(first.as_str()),
        "case CreateDeployment(stageName): expected the new deployment, got {:?}",
        created.body["deploymentId"]
    );
    assert_eq!(
        created.body["description"].as_str(),
        Some("the production stage"),
        "case CreateDeployment(stageName): expected stageDescription on the stage, got {:?}",
        created.body["description"]
    );
    assert_eq!(
        created.body["variables"],
        json!({ "table": "pets" }),
        "case CreateDeployment(stageName): expected the stage variables, got {:?}",
        created.body["variables"]
    );

    // A second deployment to the same stage re-points it rather than creating a second
    // stage of that name. The stage settings a request carries configure a stage this
    // call CREATES ("the Stage resource ... to create"), so an existing stage keeps its
    // own configuration — `UpdateStage` is the call that changes it — and this row sends
    // settings that differ from the stage's to pin that.
    let second = create_deployment(
        &gateway,
        &api,
        "CreateDeployment(redeploying the stage)",
        json!({
            "stageName": "prod",
            "stageDescription": "ignored on a redeploy",
            "variables": { "table": "other" },
            "tracingEnabled": true
        }),
    )
    .await;
    assert_ne!(
        first, second,
        "case CreateDeployment(redeploy): each deployment takes its own id, got {first} twice"
    );
    let repointed = get_stage(&gateway, &api, "prod").await;
    assert_eq!(
        repointed.body["deploymentId"].as_str(),
        Some(second.as_str()),
        "case CreateDeployment(redeploy): expected the stage to serve {second}, got {:?}",
        repointed.body["deploymentId"]
    );
    assert_eq!(
        repointed.body["createdDate"], created.body["createdDate"],
        "case CreateDeployment(redeploy): the stage is the same record, got {:?}",
        repointed.body["createdDate"]
    );
    for member in ["description", "variables", "tracingEnabled"] {
        assert_eq!(
            repointed.body[member], created.body[member],
            "case CreateDeployment(redeploy): the stage keeps its own {member}, expected {:?}, got {:?}",
            created.body[member], repointed.body[member]
        );
    }

    // Both deployments are still readable: a redeploy creates one, it does not replace
    // the other.
    for (case, deployment) in [("first", &first), ("second", &second)] {
        let answer = gateway
            .call(
                "GET",
                &format!("/restapis/{api}/deployments/{deployment}"),
                None,
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case GetDeployment({case}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn an_unknown_identifier_is_a_not_found_on_every_stage_operation() {
    let gateway = Gateway::new();
    let (api, _) = api_with_a_method(&gateway, "stage-notfound").await;
    let deployment = create_deployment(&gateway, &api, "CreateDeployment", json!({})).await;
    let missing = "zzzzzzzzzz";

    // Rows span each identifier these operations resolve, one unknown at a time.
    let cases: Vec<(&str, String, Option<Value>)> = vec![
        (
            "POST",
            format!("/restapis/{missing}/deployments"),
            Some(json!({})),
        ),
        ("GET", format!("/restapis/{missing}/deployments/{deployment}"), None),
        ("GET", format!("/restapis/{api}/deployments/{missing}"), None),
        (
            "POST",
            format!("/restapis/{missing}/stages"),
            Some(json!({ "stageName": "prod", "deploymentId": deployment })),
        ),
        (
            "POST",
            format!("/restapis/{api}/stages"),
            Some(json!({ "stageName": "prod", "deploymentId": missing })),
        ),
        (
            "POST",
            format!("/restapis/{api}/stages"),
            Some(json!({
                "stageName": "canaryprobe",
                "deploymentId": deployment,
                "canarySettings": { "percentTraffic": 5.0, "deploymentId": missing }
            })),
        ),
        ("GET", format!("/restapis/{missing}/stages/prod"), None),
        ("GET", format!("/restapis/{api}/stages/prod"), None),
    ];

    for (method, uri, body) in cases {
        let case = format!("{method} {uri}");
        assert_failure(
            &gateway.call(method, &uri, body).await,
            404,
            "NotFoundException",
            &case,
        );
    }

    // The same canary member naming the deployment that DOES exist is accepted, and
    // reads back: the refusal above is about the identifier, not about the member.
    let accepted = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/stages"),
            Some(json!({
                "stageName": "canaryprobe",
                "deploymentId": deployment,
                "canarySettings": { "percentTraffic": 5.0, "deploymentId": deployment }
            })),
        )
        .await;
    assert_eq!(
        accepted.status, 201,
        "case CreateStage(canary on a stored deployment): expected 201, got {} with {:?}",
        accepted.status, accepted.body
    );
    let read = get_stage(&gateway, &api, "canaryprobe").await;
    assert_eq!(
        read.body["canarySettings"]["deploymentId"].as_str(),
        Some(deployment.as_str()),
        "case GetStage(canaryprobe): expected the stored canary deployment, got {:?}",
        read.body["canarySettings"]
    );

    // Deleting the API takes its stages and deployments with it.
    create_deployed_stage(&gateway, &api, "prod").await;
    let deleted = gateway
        .call("DELETE", &format!("/restapis/{api}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    for uri in [
        format!("/restapis/{api}/stages/prod"),
        format!("/restapis/{api}/deployments/{deployment}"),
    ] {
        assert_failure(
            &gateway.call("GET", &uri, None).await,
            404,
            "NotFoundException",
            &format!("GET {uri} after DeleteRestApi"),
        );
    }
}

#[tokio::test]
async fn reading_a_stage_does_not_change_what_a_later_caller_sees() {
    let gateway = Gateway::new();
    let (api, _) = api_with_a_method(&gateway, "stage-stable").await;
    create_deployed_stage(&gateway, &api, "prod").await;

    let mut seen: Vec<Value> = Vec::new();
    for round in 0..3 {
        let read = get_stage(&gateway, &api, "prod").await;
        assert_eq!(
            read.status, 200,
            "case GetStage(round {round}): expected 200, got {} with {:?}",
            read.status, read.body
        );
        seen.push(read.body);
    }
    assert_eq!(
        seen[0], seen[2],
        "case GetStage repeated: a read must not change the record, got {:?} then {:?}",
        seen[0], seen[2]
    );
}
