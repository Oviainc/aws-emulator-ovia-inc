// SPDX-License-Identifier: CC0-1.0

//! The `clientToken` of an ACCEPTED `DeleteVpcEndpoint`, end to end over the wire.
//!
//! Every case drives the real request path through [`crate::collection_lifecycle::call`] —
//! `MultiServiceRouter::dispatch` with `POST /`, `X-Amz-Target:
//! OpenSearchServerless.<Operation>` and an `awsJson1_0` body — so the boundary validator,
//! the protocol codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rule under test: an `@idempotencyToken` names the one request it was first spent on,
//! identified by the OPERATION that spent it together with the resource that request
//! addressed — so a token already spent on another operation does not make a
//! `DeleteVpcEndpoint` a replay, while the retry of the delete that spent it in its own
//! namespace is answered with the detail that acceptance stored; and replaying is a read, so
//! it writes nothing: it cannot postpone the purge the accepted delete made due.
//!
//! The tables span what the rule generalises over: the operation a token was first spent on
//! (both other `@idempotent` writes of a `VpcEndpointRecord`, and an accepted delete of
//! another resource kind), and the rate and number of retries a caller makes inside the
//! window the deleting endpoint is still stored in.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::collection_lifecycle::{at, call, emulator, text, CLIENT_ERROR};
use crate::encryption_indexes_and_tagging::active_collection;
use crate::vpc_endpoint::{await_active, create_endpoint, read_one};

/// The status both `CollectionStatus` and `VpcEndpointStatus` report an accepted deletion in
/// (`smithy_query(query="shape", shape="CollectionStatus,VpcEndpointStatus")`).
const DELETING: &str = "DELETING";

/// The `VpcId` every endpoint here is created in; `VpcId`'s modelled pattern is
/// `^vpc-([0-9a-f]{8}|[0-9a-f]{17})$` (`smithy_query(query="shape", shape="VpcId")`).
const VPC_ID: &str = "vpc-0123456789abcdef0";

/// The subnet every endpoint here is created with; `SubnetId` is
/// `^subnet-([0-9a-f]{8}|[0-9a-f]{17})$` (`smithy_query(query="shape", shape="SubnetId")`).
const SUBNET_ID: &str = "subnet-0123456789abcdef0";

/// A second subnet of the other modelled width, which the update row adds.
const ANOTHER_SUBNET: &str = "subnet-0000000a";

/// Long enough for the purge an accepted delete makes due to fall due and be applied
/// several times over; the emulator's span is 500 ms and nothing sleeps for it inside a
/// handler.
const PURGE_BOUND: Duration = Duration::from_millis(3000);

/// Call `operation` with `request`, asserting it was accepted, and answer the whole body.
async fn accepted(
    router: &MultiServiceRouter,
    operation: &str,
    case: &str,
    request: Value,
) -> Value {
    let (status, code, answer) = call(router, operation, request).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from {operation}, got {status} {code} with {answer}"
    );
    answer
}

/// The delete request addressing the endpoint `id`, carrying `token`.
fn delete_request(id: &str, token: &str) -> Value {
    json!({ "id": id, "clientToken": token })
}

/// Create an endpoint named `label`, settled into `ACTIVE` — the status a delete may start
/// from — and answer its server-minted id.
async fn active_endpoint(router: &MultiServiceRouter, case: &str, label: &str) -> String {
    let detail = create_endpoint(
        router,
        case,
        json!({ "name": label, "vpcId": VPC_ID, "subnetIds": [SUBNET_ID] }),
    )
    .await;
    let id = text(&detail, "id");
    await_active(router, case, &id).await;
    id
}

/// Accept the deletion of the endpoint `id` carrying `token`, and answer the
/// `deleteVpcEndpointDetail` it reports.
async fn accepted_delete(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    token: &str,
) -> Value {
    let answer = accepted(router, "DeleteVpcEndpoint", case, delete_request(id, token)).await;
    at(&answer, "deleteVpcEndpointDetail").clone()
}

/// The operation a row spends its token on BEFORE the delete under test.
#[derive(Clone, Copy, Debug)]
enum Spender {
    /// The create of an endpoint, whose token names the endpoint it minted.
    CreateVpcEndpoint,
    /// The update of an endpoint, whose token names the endpoint it changed.
    UpdateVpcEndpoint,
    /// The accepted delete of ANOTHER resource kind, whose token names that record.
    DeleteCollection,
}

impl Spender {
    /// The operation this row spends its token on.
    fn operation(self) -> &'static str {
        match self {
            Spender::CreateVpcEndpoint => "CreateVpcEndpoint",
            Spender::UpdateVpcEndpoint => "UpdateVpcEndpoint",
            Spender::DeleteCollection => "DeleteCollection",
        }
    }

    /// Spend `token` on this operation, answering the id of the endpoint it left stored
    /// when the operation is one of an endpoint's own writes.
    async fn spend(self, router: &MultiServiceRouter, case: &str, token: &str) -> Option<String> {
        match self {
            Spender::CreateVpcEndpoint => {
                let answer = accepted(
                    router,
                    "CreateVpcEndpoint",
                    case,
                    json!({
                        "name": "token-spent-on-a-create",
                        "vpcId": VPC_ID,
                        "subnetIds": [SUBNET_ID],
                        "clientToken": token,
                    }),
                )
                .await;
                Some(text(at(&answer, "createVpcEndpointDetail"), "id"))
            }
            Spender::UpdateVpcEndpoint => {
                let id = active_endpoint(router, case, "token-spent-on-an-update").await;
                accepted(
                    router,
                    "UpdateVpcEndpoint",
                    case,
                    json!({
                        "id": &id,
                        "addSubnetIds": [ANOTHER_SUBNET],
                        "clientToken": token,
                    }),
                )
                .await;
                Some(id)
            }
            Spender::DeleteCollection => {
                let collection = active_collection(router, "token-spent-on-a-delete").await;
                let answer = accepted(
                    router,
                    "DeleteCollection",
                    case,
                    delete_request(&text(&collection, "id"), token),
                )
                .await;
                assert_eq!(
                    text(at(&answer, "deleteCollectionDetail"), "status"),
                    DELETING,
                    "case {case}: expected the collection delete to be accepted, got {answer}"
                );
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// A token already spent on another operation does not make a delete a replay, and the
// delete's own retry still is one.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_vpc_endpoint_token_is_scoped_to_the_operation_that_spent_it() {
    for spender in [
        Spender::CreateVpcEndpoint,
        Spender::UpdateVpcEndpoint,
        Spender::DeleteCollection,
    ] {
        // One emulator per row, so a row is answered only out of what it stored itself.
        let router = emulator();
        let token = "one-token-spent-twice-over";
        let case = format!("the token first spent on {}", spender.operation());
        let spent_on = spender.spend(&router, &case, token).await;

        // The SAME token on `DeleteVpcEndpoint`, addressing an endpoint that operation has
        // never answered for: the token names another operation's request, so this is an
        // ordinary delete of the endpoint it addresses rather than a replay.
        let target = active_endpoint(&router, &case, "the-delete-under-test").await;
        let first = accepted_delete(&router, &case, &target, token).await;
        assert_eq!(
            (text(&first, "id"), text(&first, "status")),
            (target.clone(), DELETING.to_string()),
            "case {case}: expected the delete it asked for to be accepted, got {first}"
        );

        // And the retry of THAT request is a replay: the token now names it inside
        // `DeleteVpcEndpoint`'s own namespace, so the retry answers the detail the
        // acceptance stored instead of the conflict `DELETING` would otherwise be.
        let retried = format!("{case}: the retry of the delete");
        let replayed = accepted_delete(&router, &retried, &target, token).await;
        assert_eq!(
            replayed, first,
            "case {retried}: expected the detail the accepted delete stored {first}, got {replayed}"
        );

        // The record the token was first spent on is untouched by that delete: a token
        // spent on one operation did not delete the other operation's record.
        if let Some(id) = spent_on {
            let detail = read_one(&router, &case, &id).await;
            assert_ne!(
                text(&detail, "status"),
                DELETING,
                "case {case}: expected the endpoint the first call left to be untouched, got {detail}"
            );
            assert_ne!(
                text(&detail, "id"),
                target,
                "case {case}: expected two distinct endpoints in the row, got {detail}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Replaying is a read: however often it is retried, the purge the accepted delete made due
// still falls due.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retrying_an_accepted_delete_does_not_postpone_the_purge_it_made_due() {
    // The rows differ in how fast and how often the caller retries inside the window the
    // deleting endpoint is still stored in: a tight retry loop, and a slower one.
    let rows: Vec<(&str, Duration)> = vec![
        ("a retry every 25 ms", Duration::from_millis(25)),
        ("a retry every 100 ms", Duration::from_millis(100)),
    ];

    for (case, interval) in rows {
        // One emulator per row, so a row is answered only out of what it stored itself.
        let router = emulator();
        let token = "a-hammered-delete-token";
        let id = active_endpoint(&router, case, "hammered-endpoint").await;

        let accepted_at = Instant::now();
        let first = accepted_delete(&router, case, &id, token).await;
        assert_eq!(
            text(&first, "status"),
            DELETING,
            "case {case}: expected the delete to be accepted, got {first}"
        );

        // Retry until the purge has removed the record, which the retries must not hold
        // off: an implementation that wrote on a replay would keep pushing the purge out
        // and this loop would never see the not-found.
        let mut replays = 0_usize;
        let mut purged = false;
        let attempts = (PURGE_BOUND.as_millis() / interval.as_millis()) as usize + 10;
        for _ in 0..attempts {
            tokio::time::sleep(interval).await;
            let (status, code, answer) =
                call(&router, "DeleteVpcEndpoint", delete_request(&id, token)).await;
            if status == 200 {
                replays += 1;
                assert_eq!(
                    at(&answer, "deleteVpcEndpointDetail"),
                    &first,
                    "case {case}: expected every retry to replay {first}, got {answer}"
                );
                continue;
            }
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_ERROR, "ResourceNotFoundException"),
                "case {case}: expected a replay or the not-found of a purged endpoint, got {status} {code} with {answer}"
            );
            purged = true;
            break;
        }

        let elapsed = accepted_at.elapsed();
        assert!(
            purged,
            "case {case}: expected the purge to fall due while the delete was retried {replays} times over {elapsed:?}"
        );
        assert!(
            replays > 0,
            "case {case}: expected at least one retry inside the window the endpoint is still stored in, got {replays} after {elapsed:?}"
        );

        // The endpoint is gone from the reader too, so the purge the retries did not hold
        // off is what a later caller sees.
        let answer = accepted(&router, "BatchGetVpcEndpoint", case, json!({ "ids": [&id] })).await;
        assert!(
            at(&answer, "vpcEndpointDetails")
                .as_array()
                .is_some_and(Vec::is_empty),
            "case {case}: expected the purged endpoint to be reported by no detail, got {answer}"
        );
    }
}
