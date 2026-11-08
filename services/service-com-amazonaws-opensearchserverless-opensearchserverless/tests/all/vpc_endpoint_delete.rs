// SPDX-License-Identifier: CC0-1.0

//! Deleting an OpenSearch Serverless-managed interface VPC endpoint, end to end over the
//! wire.
//!
//! Every case drives the real request path through [`crate::collection_lifecycle::call`] —
//! `MultiServiceRouter::dispatch` with `POST /`, `X-Amz-Target:
//! OpenSearchServerless.<Operation>` and an `awsJson1_0` body — so the boundary validator,
//! the protocol codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rule under test: `DeleteVpcEndpoint` ACCEPTS the deletion of the stored endpoint its
//! `id` names, reporting that record in the non-terminal `DELETING` status it stays readable
//! in until the clock makes its removal due, after which the endpoint and its name are gone;
//! an id naming no stored endpoint is refused as not found, and an endpoint in a status it
//! may not be deleted from is refused as a conflict. The tables span what the rule
//! generalises over: the endpoint's shape (subnet count and width, the presence and count of
//! security groups, the `VpcEndpointName` length at both published bounds), the status the
//! delete is attempted from, and the forms a well-formed `VpcEndpointId` that names nothing
//! takes.

use serde_json::{json, Value};

use crate::collection_lifecycle::{at, call, emulator, text, CLIENT_ERROR};
use crate::vpc_endpoint::{
    await_active, create_endpoint, is_vpc_endpoint_id, read_endpoints, read_one, ACTIVE, PENDING,
    SETTLE_WAIT,
};

/// `VpcEndpointStatus$DELETING` — "Deleting VPCEndpoint resource"
/// (`smithy_query(query="shape", shape="VpcEndpointStatus")`).
const DELETING: &str = "DELETING";

/// How `DeleteVpcEndpoint` words the id it was given when no endpoint carries it.
fn delete_not_found_message(id: &str) -> String {
    format!("VpcEndpoint with ID '{id}' cannot be found.")
}

/// The `errorCode` this service's batch readers report for an identifier that names
/// nothing.
const NOT_FOUND_CODE: &str = "NOT_FOUND";

/// Call `DeleteVpcEndpoint` with `request`, answering its status, error code and body.
async fn delete_endpoint(
    router: &smithy_server_runtime::MultiServiceRouter,
    request: Value,
) -> (u16, String, Value) {
    call(router, "DeleteVpcEndpoint", request).await
}

/// Delete the endpoint `id` names, asserting it was accepted, and answer its
/// `deleteVpcEndpointDetail`.
async fn accepted_delete(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    id: &str,
) -> Value {
    let (status, code, answer) = delete_endpoint(router, json!({ "id": id })).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from DeleteVpcEndpoint({id}), got {status} {code} with {answer}"
    );
    at(&answer, "deleteVpcEndpointDetail").clone()
}

/// The `vpcEndpointErrorDetails` entry `BatchGetVpcEndpoint` reports for `id`, or
/// `Value::Null` while the endpoint is still reported as a detail.
async fn read_error(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    id: &str,
) -> Value {
    let answer = read_endpoints(router, case, &[id]).await;
    at(&answer, "vpcEndpointErrorDetails")
        .as_array()
        .and_then(|errors| errors.first().cloned())
        .unwrap_or(Value::Null)
}

/// Poll `BatchGetVpcEndpoint` until `id` is no longer reported at all, answering the
/// not-found error detail it reports instead.
///
/// The removal is owed by the clock, so this waits for the wall clock rather than for a
/// call: no handler sleeps for it.
async fn await_gone(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    id: &str,
) -> Value {
    for _ in 0..10 {
        let error = read_error(router, case, id).await;
        if !error.is_null() {
            return error;
        }
        tokio::time::sleep(SETTLE_WAIT / 5).await;
    }
    panic!("case {case}: the endpoint {id} was never removed after its deletion was accepted");
}

// ---------------------------------------------------------------------------
// The accepted delete, over a table that spans the endpoint's shape.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_accepted_delete_reports_deleting_and_then_removes_the_endpoint() {
    let router = emulator();

    // `SubnetId` is `^subnet-([0-9a-f]{8}|[0-9a-f]{17})$` and `SecurityGroupIds` is
    // `@length(min: 1, max: 5)`; `VpcEndpointName` is `@length(min: 3, max: 32)` with
    // `^[a-z][a-z0-9-]+$` (smithy_query shapes `SubnetIds` / `SecurityGroupIds` /
    // `VpcEndpointName`). Row 1 names one short-width subnet and no security groups, row 2
    // three long-width subnets and the published maximum of five groups, rows 3 and 4 sit on
    // the name bounds of 3 and 32 characters.
    let longest_name = format!("d{}", "e".repeat(31));
    let cases: Vec<(String, Value)> = vec![
        (
            "one short-width subnet, no security groups".to_string(),
            json!({
                "name": "delete-first",
                "vpcId": "vpc-1a2b3c4d",
                "subnetIds": ["subnet-1a2b3c4d"],
            }),
        ),
        (
            "three long-width subnets, five security groups".to_string(),
            json!({
                "name": "delete-second",
                "vpcId": "vpc-0123456789abcdef0",
                "subnetIds": [
                    "subnet-0123456789abcdef0",
                    "subnet-abcdef01234567890",
                    "subnet-00000000000000001",
                ],
                "securityGroupIds": ["sg-1", "sg-2", "sg-3", "sg-4", "sg+5"],
            }),
        ),
        (
            "a name at the minimum length of three".to_string(),
            json!({
                "name": "del",
                "vpcId": "vpc-",
                "subnetIds": ["subnet-deadbeef"],
                "securityGroupIds": ["sg-deadbeef"],
            }),
        ),
        (
            "a name at the maximum length of 32".to_string(),
            json!({
                "name": longest_name,
                "vpcId": "vpc-9f8e7d6c",
                "subnetIds": ["subnet-cafebabe", "subnet-cafebabecafebabe0"],
            }),
        ),
    ];

    for (case, request) in &cases {
        let created = create_endpoint(&router, case, request.clone()).await;
        let id = text(&created, "id");
        let name = text(request, "name");
        await_active(&router, case, &id).await;

        let detail = accepted_delete(&router, case, &id).await;
        assert_eq!(
            text(&detail, "id"),
            id,
            "case {case}: expected the delete to report the endpoint it addressed, got {detail}"
        );
        assert_eq!(
            text(&detail, "name"),
            name,
            "case {case}: expected the delete to report the stored name, got {detail}"
        );
        assert_eq!(
            text(&detail, "status"),
            DELETING,
            "case {case}: expected the delete to report {DELETING}, got {detail}"
        );

        // The read straight after the delete still observes the transient status, so the
        // span the deletion is observable over is never zero, and the endpoint's own
        // members are unchanged by it.
        let deleting = read_one(&router, case, &id).await;
        assert_eq!(
            text(&deleting, "status"),
            DELETING,
            "case {case}: expected the read straight after the delete to report {DELETING}, got {deleting}"
        );
        assert_eq!(
            at(&deleting, "subnetIds"),
            at(request, "subnetIds"),
            "case {case}: expected the delete to leave the stored subnetIds alone, got {deleting}"
        );

        // A second delete of an endpoint already being deleted is the conflict
        // `ConflictException` describes, since `DELETING` is not a status it may be deleted
        // from.
        let (status, code, answer) = delete_endpoint(&router, json!({ "id": &id })).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ConflictException"),
            "case {case}: expected {CLIENT_ERROR} ConflictException for a second delete, got {status} {code} with {answer}"
        );

        // The endpoint stops being reported once the clock makes its removal due.
        let error = await_gone(&router, case, &id).await;
        assert_eq!(
            text(&error, "errorCode"),
            NOT_FOUND_CODE,
            "case {case}: expected the removed endpoint to be reported as not found, got {error}"
        );

        // A delete of the endpoint once it is gone is refused as not found rather than
        // accepted a second time.
        let (status, code, answer) = delete_endpoint(&router, json!({ "id": &id })).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ResourceNotFoundException"),
            "case {case}: expected {CLIENT_ERROR} ResourceNotFoundException after the removal, got {status} {code} with {answer}"
        );

        // The removal freed the name: the same name is created again, as a new endpoint.
        let recreated = create_endpoint(&router, case, request.clone()).await;
        let new_id = text(&recreated, "id");
        assert!(
            is_vpc_endpoint_id(&new_id) && new_id != id,
            "case {case}: expected the freed name to be created again under a fresh id, got {recreated}"
        );
    }
}

// ---------------------------------------------------------------------------
// The statuses a delete is refused from, and the `clientToken` that changes none of it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_of_an_endpoint_not_in_a_deletable_status_is_refused() {
    let router = emulator();

    // The rows differ in what the delete request carries beside the id: `clientToken` is an
    // `@idempotencyToken` on this operation, and a delete is addressed by `id`, so the
    // refusal is the same with and without one.
    let cases: Vec<(&str, &str, Option<&str>)> = vec![
        ("no clientToken", "pending-none", None),
        ("a clientToken", "pending-token", Some("a-delete-token")),
        (
            "another clientToken",
            "pending-other-token",
            Some("another-delete-token"),
        ),
    ];

    for (case, name, token) in cases {
        let request = json!({
            "name": name,
            "vpcId": "vpc-1a2b3c4d",
            "subnetIds": ["subnet-1a2b3c4d"],
        });
        let created = create_endpoint(&router, case, request.clone()).await;
        let id = text(&created, "id");
        assert_eq!(
            text(&created, "status"),
            PENDING,
            "case {case}: expected the create to report {PENDING}, got {created}"
        );

        let mut delete = json!({ "id": &id });
        if let Some(token) = token {
            delete["clientToken"] = json!(token);
        }
        let (status, code, answer) = delete_endpoint(&router, delete.clone()).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ConflictException"),
            "case {case}: expected {CLIENT_ERROR} ConflictException while {PENDING}, got {status} {code} with {answer}"
        );
        assert!(
            text(&answer, "message").contains(ACTIVE),
            "case {case}: expected the refusal to state the statuses an endpoint may be deleted from, got {answer}"
        );

        // The refused delete stored nothing: the endpoint still settles into `ACTIVE` on
        // its own, and is deleted from there.
        let active = await_active(&router, case, &id).await;
        assert_eq!(
            text(&active, "status"),
            ACTIVE,
            "case {case}: expected the refused delete to leave the endpoint settling, got {active}"
        );
        let detail = accepted_delete(&router, case, &id).await;
        assert_eq!(
            text(&detail, "status"),
            DELETING,
            "case {case}: expected the delete from {ACTIVE} to be accepted, got {detail}"
        );
    }
}

// ---------------------------------------------------------------------------
// Well-formed ids that name no stored endpoint.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_of_an_id_that_names_no_endpoint_is_refused() {
    let router = emulator();

    // One endpoint is stored throughout, so the refusals below are about the id and not
    // about an empty account.
    let kept = create_endpoint(
        &router,
        "the endpoint kept throughout",
        json!({
            "name": "kept-endpoint",
            "vpcId": "vpc-1a2b3c4d",
            "subnetIds": ["subnet-1a2b3c4d"],
        }),
    )
    .await;
    let kept_id = text(&kept, "id");

    // Every row satisfies `VpcEndpointId`'s own `^vpce-[0-9a-z]*$` and
    // `@length(min: 1, max: 255)`, so each reaches the handler rather than the boundary
    // validator: a never-minted id of the width AWS uses, one with an empty suffix, a
    // one-character suffix and a suffix at the modelled maximum length.
    let longest_id = format!("vpce-{}", "a".repeat(250));
    let cases: Vec<(String, String)> = vec![
        (
            "a never-minted id".to_string(),
            "vpce-00000000000000000".to_string(),
        ),
        ("an empty suffix".to_string(), "vpce-".to_string()),
        ("a one-character suffix".to_string(), "vpce-1".to_string()),
        ("a suffix at the maximum length".to_string(), longest_id),
    ];

    for (case, id) in &cases {
        let (status, code, answer) = delete_endpoint(&router, json!({ "id": id })).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ResourceNotFoundException"),
            "case {case}: expected {CLIENT_ERROR} ResourceNotFoundException for {id}, got {status} {code} with {answer}"
        );
        assert_eq!(
            text(&answer, "message"),
            delete_not_found_message(id),
            "case {case}: expected the service's not-found wording naming {id}, got {answer}"
        );
    }

    // Nothing above touched the stored endpoint.
    let detail = read_one(&router, "after every refusal", &kept_id).await;
    assert_eq!(
        text(&detail, "name"),
        "kept-endpoint",
        "case after every refusal: expected the stored endpoint untouched, got {detail}"
    );
    assert_ne!(
        text(&detail, "status"),
        DELETING,
        "case after every refusal: expected the stored endpoint not to be deleting, got {detail}"
    );
}
