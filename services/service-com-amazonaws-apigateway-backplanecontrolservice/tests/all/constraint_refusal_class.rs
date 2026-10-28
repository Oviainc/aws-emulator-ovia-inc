// SPDX-License-Identifier: CC0-1.0

//! Which ERROR CLASS a refused control-plane request belongs to, asserted on the wire.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator and the request builder are this
//! crate's existing harness, reused from `rest_api_lifecycle`.
//!
//! The RULES under test:
//!
//! * a value outside an input member's MODELLED CONSTRAINT is refused by the layer in
//!   front of the operation, whose error class is its own — HTTP 400
//!   `ValidationException` — whatever member carried it and whichever operation it was
//!   sent to;
//! * a well-formed request the HANDLER refuses for a semantic reason keeps the
//!   operation's own modelled client error, HTTP 400 `BadRequestException`, so the two
//!   classes stay distinguishable: one says the value cannot be sent at all, the other
//!   that it cannot be used here;
//! * a `CreateDeployment` of an API that carries no method is such a semantic refusal:
//!   a deployment is the snapshot of the API's methods a stage serves, so there is
//!   nothing to deploy and no deployment is created;
//! * a patch VALUE that names no stored record is a refusal of the REQUEST (400), while
//!   an identifier in the URI that names no record stays the not-found (404).
//!
//! The tables span what each rule generalises over rather than one recorded call: the
//! constraint CLASS (an enum value set, a `@pattern`, an absent required member), where
//! the violated member TRAVELS (a body member, a URI label, a query parameter), how
//! DEEPLY it is nested (a top-level member, a member of a structure, an element of a
//! list inside a structure), and which operation it was sent to.

use serde_json::{json, Value};

use crate::rest_api_lifecycle::{
    assert_failure, create_api, create_resource, make_deployable, page_items, string_member,
    Gateway,
};

/// The error class the constraint layer in front of every operation refuses under.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The error class an operation's own semantic refusal carries — the client-input error
/// this service's model declares on every operation of this surface.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// The error class an identifier in the URI that names no stored record carries.
const NOT_FOUND_EXCEPTION: &str = "NotFoundException";

/// One request, as a row of the tables below: the case's name, the HTTP method, the URI
/// and the JSON body (`None` for a request that carries none).
type Request<'a> = (&'a str, &'a str, String, Option<Value>);

#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused_as_a_constraint_violation() {
    // Each row's request is well formed EXCEPT for one member that is outside the
    // constraint its model declares, so the class under test is the only thing being
    // refused. The identifiers name no stored record on purpose: the constraint layer
    // runs ahead of every lookup, so a row that got past it would be answered the
    // handler's own not-found instead — which is what the last table below checks.
    let rows: Vec<Request> = vec![
        (
            // A `@pattern` on a BODY member (`StatusCode`, `^[1-5]\d\d$`).
            "PutGatewayResponse, a status code outside its pattern",
            "PUT",
            "/restapis/abcde12345/gatewayresponses/DEFAULT_4XX".to_string(),
            Some(json!({ "statusCode": "6xx" })),
        ),
        (
            // The SAME pattern on a member bound to the URI rather than the body.
            "PutIntegrationResponse, a status code label outside its pattern",
            "PUT",
            "/restapis/abcde12345/resources/fghij67890/methods/GET/integration/responses/abc"
                .to_string(),
            Some(json!({})),
        ),
        (
            // An enum on a body member.
            "PutIntegrationResponse, a content handling outside its enum",
            "PUT",
            "/restapis/abcde12345/resources/fghij67890/methods/GET/integration/responses/200"
                .to_string(),
            Some(json!({ "contentHandling": "BOGUS" })),
        ),
        (
            // An enum on an element of a list of structures.
            "UpdateVpcLink, a patch op outside its enum",
            "PATCH",
            "/vpclinks/abcde12345".to_string(),
            Some(json!({
                "patchOperations": [{ "op": "not-a-real-op", "path": "/name", "value": "x" }],
            })),
        ),
        (
            // An enum on an element of a list inside a structure.
            "CreateRestApi, an endpoint type outside its enum",
            "POST",
            "/restapis".to_string(),
            Some(json!({
                "name": "endpoint-types",
                "endpointConfiguration": { "types": ["BOGUS"] },
            })),
        ),
        (
            // An enum on a member of a structure — a member the issue that filed this
            // rule did not cite, so the rule is shown to hold beyond its examples.
            "CreateUsagePlan, a quota period outside its enum",
            "POST",
            "/usageplans".to_string(),
            Some(json!({
                "name": "quota-period",
                "quota": { "limit": 10, "offset": 0, "period": "YEAR" },
            })),
        ),
        (
            // An enum on a member bound to the QUERY STRING.
            "GetDomainNames, a resource owner outside its enum",
            "GET",
            "/domainnames?resourceOwner=NOBODY".to_string(),
            None,
        ),
        (
            // An absent required member, which is found while READING the request
            // rather than while walking the value it built.
            "CreateRestApi, no name at all",
            "POST",
            "/restapis".to_string(),
            Some(json!({ "description": "nameless" })),
        ),
    ];

    let gateway = Gateway::new();
    for (case, method, uri, body) in rows {
        let answer = gateway.call(method, &uri, body).await;
        assert_failure(&answer, 400, VALIDATION_EXCEPTION, case);
    }

    // The recorded wording of one of those refusals, which the class does not change:
    // the offending value, the member's path rooted at the operation's input shape, and
    // the constraint it failed.
    let answer = gateway
        .call(
            "PATCH",
            "/vpclinks/abcde12345",
            Some(json!({
                "patchOperations": [{ "op": "not-a-real-op", "path": "/name", "value": "x" }],
            })),
        )
        .await;
    assert_eq!(
        answer.body["message"].as_str(),
        Some(
            "1 validation error detected: Value 'not-a-real-op' at \
             'updateVpcLinkInput.patchOperations.1.member.op' failed to satisfy constraint: \
             Member must satisfy enum value set: [add, remove, move, test, replace, copy]"
        ),
        "case UpdateVpcLink(an off-model op): expected the constraint clause, got {:?}",
        answer.body["message"]
    );
}

#[tokio::test]
async fn a_semantic_refusal_of_a_well_formed_request_keeps_the_operations_own_error() {
    // Each row satisfies every constraint its operation declares and is refused by the
    // HANDLER for a reason of its own, so none of them may carry the constraint layer's
    // class — nor its envelope, which is what says the request got PAST that layer.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "semantic-refusals").await;
    let resource = create_resource(&gateway, &api, &root, "pets").await;
    let resource_id = string_member(&resource, "id", "CreateResource(pets)");

    let rows: Vec<Request> = vec![
        (
            // `targetArns` is "The ARN of the network load balancer of the VPC targeted
            // by the VPC link": a link targets at least one.
            "CreateVpcLink with no target at all",
            "POST",
            "/vpclinks".to_string(),
            Some(json!({ "name": "no-targets", "targetArns": [] })),
        ),
        (
            "CreateVpcLink with a target that is not an ARN",
            "POST",
            "/vpclinks".to_string(),
            Some(json!({ "name": "bad-target", "targetArns": ["not-an-arn"] })),
        ),
        (
            // Nothing to deploy: the API carries a resource but no method.
            "CreateDeployment of an API with no method",
            "POST",
            format!("/restapis/{api}/deployments"),
            Some(json!({})),
        ),
        (
            // A path the resource has no updateable member for.
            "UpdateResource with an unsupported patch path",
            "PATCH",
            format!("/restapis/{api}/resources/{resource_id}"),
            Some(json!({
                "patchOperations": [{ "op": "replace", "path": "/id", "value": "zzzzzzzzzz" }],
            })),
        ),
    ];

    for (case, method, uri, body) in rows {
        let answer = gateway.call(method, &uri, body).await;
        assert_failure(&answer, 400, BAD_REQUEST_EXCEPTION, case);
        let message = answer.body["message"].as_str().unwrap_or_default();
        assert!(
            !message.contains("validation error"),
            "case {case}: expected the handler's own refusal, got the constraint \
             envelope {message:?}"
        );
    }
}

#[tokio::test]
async fn a_deployment_of_an_api_with_no_method_is_refused_and_creates_nothing() {
    let gateway = Gateway::new();

    // Rows differ along the shape of the API that has nothing to deploy: one with only
    // the root resource it was created with, and one carrying a resource tree whose
    // nodes hold no method.
    let shapes: [(&str, &[&str]); 2] = [
        ("an API with only its root resource", &[]),
        ("an API whose resources hold no method", &["pets", "orders"]),
    ];
    for (case, path_parts) in shapes {
        let (api, root) = create_api(&gateway, case).await;
        for path_part in path_parts {
            create_resource(&gateway, &api, &root, path_part).await;
        }
        let refused = gateway
            .call("POST", &format!("/restapis/{api}/deployments"), Some(json!({})))
            .await;
        assert_failure(&refused, 400, BAD_REQUEST_EXCEPTION, case);
        assert_eq!(
            refused.body["message"].as_str(),
            Some("The REST API doesn't contain any methods"),
            "case {case}: expected the refusal to say the API has no method, got {:?}",
            refused.body["message"]
        );
        // Nothing was allocated: the API holds no deployment to read back.
        let listed = gateway
            .call("GET", &format!("/restapis/{api}/deployments"), None)
            .await;
        assert!(
            page_items(&listed.body, case).is_empty(),
            "case {case}: expected no deployment to have been created, got {:?}",
            listed.body
        );

        // One method anywhere in the tree is what makes the API deployable, and the
        // deployment is then readable through `GetDeployment`.
        make_deployable(&gateway, &api).await;
        let created = gateway
            .call("POST", &format!("/restapis/{api}/deployments"), Some(json!({})))
            .await;
        assert_eq!(
            created.status, 201,
            "case {case}, with a method: expected 201, got {} with {:?}",
            created.status, created.body
        );
        let deployment = string_member(&created.body, "id", case);
        let read = gateway
            .call(
                "GET",
                &format!("/restapis/{api}/deployments/{deployment}"),
                None,
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {case}, with a method: expected GetDeployment to answer 200, got {} \
             with {:?}",
            read.status, read.body
        );
    }
}

#[tokio::test]
async fn a_patch_value_naming_no_record_is_a_bad_request_and_the_uri_identifier_a_not_found() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "stage-repoint").await;
    make_deployable(&gateway, &api).await;

    // Two deployments and a stage pointing at the second, so the first can be deleted
    // and its id then names no record while the stage itself is perfectly addressable.
    let first = created_deployment(&gateway, &api, "the first deployment").await;
    let second = created_deployment(&gateway, &api, "the second deployment").await;
    let stage = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/stages"),
            Some(json!({ "stageName": "prod", "deploymentId": second })),
        )
        .await;
    assert_eq!(
        stage.status, 201,
        "case CreateStage(prod): expected 201, got {} with {:?}",
        stage.status, stage.body
    );
    let deleted = gateway
        .call(
            "DELETE",
            &format!("/restapis/{api}/deployments/{first}"),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteDeployment(the first): expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );

    // Another API's deployment, which exists but is not this API's.
    let (other, _other_root) = create_api(&gateway, "stage-repoint-other").await;
    make_deployable(&gateway, &other).await;
    let foreign = created_deployment(&gateway, &other, "another API's deployment").await;

    // Rows differ along WHY the patch value names no deployment of this API: an id
    // nothing ever minted, the id of a deployment that was deleted, a deployment of
    // another API, and the empty string — and along which member carries it, the stage's
    // own pointer or its canary's.
    let values: [(&str, String); 4] = [
        ("an id nothing minted", "zzzzzzzzzz".to_string()),
        ("a deleted deployment's id", first.clone()),
        ("another API's deployment", foreign.clone()),
        ("the empty string", String::new()),
    ];
    for (case, value) in values {
        for member in ["/deploymentId", "/canarySettings/deploymentId"] {
            let case = format!("UpdateStage({member} = {case})");
            let answer = gateway
                .call(
                    "PATCH",
                    &format!("/restapis/{api}/stages/prod"),
                    Some(json!({
                        "patchOperations": [
                            { "op": "replace", "path": member, "value": value },
                        ],
                    })),
                )
                .await;
            assert_failure(&answer, 400, BAD_REQUEST_EXCEPTION, &case);
        }
    }

    // The stage still serves the deployment it was pointed at: every refusal above
    // changed nothing.
    let read = gateway
        .call("GET", &format!("/restapis/{api}/stages/prod"), None)
        .await;
    assert_eq!(
        read.body["deploymentId"].as_str(),
        Some(second.as_str()),
        "case GetStage after the refusals: expected the stage to still serve {second:?}, \
         got {:?}",
        read.body
    );

    // The same patch sent to a stage or an API the URI names and nothing answers to is
    // the not-found instead: that is the identifier class the value class is distinct
    // from.
    let operations = json!({
        "patchOperations": [{ "op": "replace", "path": "/deploymentId", "value": second }],
    });
    for (case, uri) in [
        (
            "a stage this API does not have",
            format!("/restapis/{api}/stages/nosuchstage"),
        ),
        (
            "an API nothing answers to",
            "/restapis/zzzzzzzzzz/stages/prod".to_string(),
        ),
    ] {
        let answer = gateway
            .call("PATCH", &uri, Some(operations.clone()))
            .await;
        assert_failure(&answer, 404, NOT_FOUND_EXCEPTION, case);
    }
}

/// `CreateDeployment` for `api`, asserting the modelled 201 and answering the id.
async fn created_deployment(gateway: &Gateway, api: &str, case: &str) -> String {
    let answer = gateway
        .call("POST", &format!("/restapis/{api}/deployments"), Some(json!({})))
        .await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected 201 from CreateDeployment, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "id", case)
}
