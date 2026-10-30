// SPDX-License-Identifier: CC0-1.0

//! The `clientToken` replay of a delete this service only ACCEPTS, end to end over the
//! wire.
//!
//! Every case drives the real request path through [`crate::collection_lifecycle::call`] —
//! `MultiServiceRouter::dispatch` with `POST /`, `X-Amz-Target:
//! OpenSearchServerless.<Operation>` and an `awsJson1_0` body — so the boundary validator,
//! the protocol codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rule under test: a delete that is ACCEPTED rather than completed — the resource stays
//! readable in its non-terminal `DELETING` status until the clock makes its purge due —
//! answers a retry carrying the `clientToken` that acceptance already spent with the detail
//! that call stored, instead of refusing it from the status precondition the first call's own
//! `DELETING` now fails; the replay is scoped to the resource the request addresses and to
//! the operation that spent the token, and once the purge has removed the record there is
//! nothing left to replay.
//!
//! The tables span what that rule generalises over: both operations this service accepts a
//! delete for (`DeleteCollection` and `DeleteVpcEndpoint`, the two `@idempotent` deletes
//! whose record survives the call), the `ClientToken` forms at both published length bounds,
//! the number of retries, and the three ways a token can fail to identify a retry — a
//! request that carries none, a request addressing another resource, and a token spent on
//! another operation.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::collection_lifecycle::{at, call, emulator, text, CLIENT_ERROR};
use crate::encryption_indexes_and_tagging::active_collection;
use crate::vpc_endpoint::{await_active, create_endpoint, SETTLE_WAIT};

/// The status both `CollectionStatus` and `VpcEndpointStatus` report an accepted deletion in
/// (`smithy_query(query="shape", shape="CollectionStatus,VpcEndpointStatus")`).
const DELETING: &str = "DELETING";

/// The `VpcId` every endpoint here is created in; `VpcId`'s modelled pattern is
/// `^vpc-([0-9a-f]{8}|[0-9a-f]{17})$` (`smithy_query(query="shape", shape="VpcId")`).
const VPC_ID: &str = "vpc-0123456789abcdef0";

/// The subnet every endpoint here is created with; `SubnetId` is
/// `^subnet-([0-9a-f]{8}|[0-9a-f]{17})$` (`smithy_query(query="shape", shape="SubnetId")`).
const SUBNET_ID: &str = "subnet-0123456789abcdef0";

/// The delete under test: the two operations whose acceptance leaves the resource readable
/// in `DELETING` (`smithy_query(query="operation",
/// operation="DeleteCollection,DeleteVpcEndpoint")` — both `@idempotent`, both with an
/// `@idempotencyToken` `clientToken`).
#[derive(Clone, Copy, Debug)]
enum Delete {
    Collection,
    VpcEndpoint,
}

impl Delete {
    /// The operation a row calls.
    fn operation(self) -> &'static str {
        match self {
            Delete::Collection => "DeleteCollection",
            Delete::VpcEndpoint => "DeleteVpcEndpoint",
        }
    }

    /// The output member the operation reports its deleted record in.
    fn detail_member(self) -> &'static str {
        match self {
            Delete::Collection => "deleteCollectionDetail",
            Delete::VpcEndpoint => "deleteVpcEndpointDetail",
        }
    }

    /// The batch reader a later read of the resource goes through.
    fn reader(self) -> &'static str {
        match self {
            Delete::Collection => "BatchGetCollection",
            Delete::VpcEndpoint => "BatchGetVpcEndpoint",
        }
    }

    /// The member that reader reports a still-stored resource in.
    fn reader_details(self) -> &'static str {
        match self {
            Delete::Collection => "collectionDetails",
            Delete::VpcEndpoint => "vpcEndpointDetails",
        }
    }

    /// Create the resource `label` names, settled into the status a delete may start from,
    /// and answer its server-minted id.
    async fn create(self, router: &MultiServiceRouter, case: &str, label: &str) -> String {
        match self {
            Delete::Collection => text(&active_collection(router, label).await, "id"),
            Delete::VpcEndpoint => {
                let detail = create_endpoint(
                    router,
                    case,
                    json!({ "name": label, "vpcId": VPC_ID, "subnetIds": [SUBNET_ID] }),
                )
                .await;
                let id = text(&detail, "id");
                // An endpoint may only be deleted once it has settled, so the row starts
                // from the status the clock leaves it in rather than from the create's own.
                await_active(router, case, &id).await;
                id
            }
        }
    }

    /// Create a resource that is still in its non-terminal CREATE status, which is a status
    /// no delete may start from — answered as its id, without waiting for it to settle.
    async fn create_transient(self, router: &MultiServiceRouter, case: &str, label: &str) -> String {
        match self {
            Delete::Collection => {
                let (status, code, answer) = call(
                    router,
                    "CreateCollection",
                    json!({ "name": label, "encryptionConfig": { "aWSOwnedKey": true } }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case}: expected 200 from CreateCollection, got {status} {code} with {answer}"
                );
                text(at(&answer, "createCollectionDetail"), "id")
            }
            Delete::VpcEndpoint => {
                let detail = create_endpoint(
                    router,
                    case,
                    json!({ "name": label, "vpcId": VPC_ID, "subnetIds": [SUBNET_ID] }),
                )
                .await;
                text(&detail, "id")
            }
        }
    }

    /// The details this operation's reader reports for `id` — empty once the purge has
    /// removed the record.
    async fn read(self, router: &MultiServiceRouter, case: &str, id: &str) -> Vec<Value> {
        let (status, code, answer) = call(router, self.reader(), json!({ "ids": [id] })).await;
        assert_eq!(
            status,
            200,
            "case {case}: expected 200 from {}, got {status} {code} with {answer}",
            self.reader()
        );
        at(&answer, self.reader_details())
            .as_array()
            .cloned()
            .unwrap_or_default()
    }
}

/// The delete request addressing `id`, carrying `token` when the row has one.
fn delete_request(id: &str, token: Option<&str>) -> Value {
    let mut request = json!({ "id": id });
    if let Some(token) = token {
        request["clientToken"] = json!(token);
    }
    request
}

/// Call `delete`'s operation with `request`, asserting it was accepted, and answer the detail
/// it reports.
async fn accepted(
    router: &MultiServiceRouter,
    delete: Delete,
    case: &str,
    request: Value,
) -> Value {
    let operation = delete.operation();
    let (status, code, answer) = call(router, operation, request).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from {operation}, got {status} {code} with {answer}"
    );
    at(&answer, delete.detail_member()).clone()
}

/// Call `delete`'s operation with `request`, asserting it was refused with `expected_code`.
async fn refused(
    router: &MultiServiceRouter,
    delete: Delete,
    case: &str,
    request: Value,
    expected_code: &str,
) {
    let operation = delete.operation();
    let (status, code, answer) = call(router, operation, request).await;
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_ERROR, expected_code),
        "case {case}: expected {CLIENT_ERROR} {expected_code} from {operation}, got {status} {code} with {answer}"
    );
}

/// Poll `delete`'s reader until `id` is no longer reported at all.
///
/// The purge is owed by the clock, so this waits for the wall clock rather than for a call:
/// no handler sleeps for it.
async fn await_purged(router: &MultiServiceRouter, delete: Delete, case: &str, id: &str) {
    for _ in 0..10 {
        if delete.read(router, case, id).await.is_empty() {
            return;
        }
        tokio::time::sleep(SETTLE_WAIT / 5).await;
    }
    panic!("case {case}: {id} was never purged after its deletion was accepted");
}

// ---------------------------------------------------------------------------
// A retry carrying the token the accepted delete spent replays that answer.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_retry_of_an_accepted_delete_replays_the_detail_it_stored() {
    // `ClientToken` is `@length(min: 1, max: 512)` (`smithy_query(query="shape",
    // shape="ClientToken")`), so the rows sit on both published bounds beside a token of the
    // shape an SDK generates, and each is retried a different number of times.
    let shortest = "t".to_string();
    let longest = "l".repeat(512);
    let token_rows: Vec<(&str, String, usize)> = vec![
        ("a token at the minimum length of one", shortest, 1),
        (
            "a token of the shape an SDK generates",
            "3f1b8a52-0c4d-4e2a-9b77-1a2b3c4d5e6f".to_string(),
            2,
        ),
        ("a token at the maximum length of 512", longest, 3),
    ];

    for delete in [Delete::Collection, Delete::VpcEndpoint] {
        for (token_case, token, retries) in &token_rows {
            // One emulator per row, so a row is answered only out of what it stored itself.
            let router = emulator();
            let case = format!("{}: {token_case}", delete.operation());
            let id = delete.create(&router, &case, "replayed-delete").await;

            // The call that ACCEPTS the deletion, which is what spends the token.
            let first = accepted(
                &router,
                delete,
                &case,
                delete_request(&id, Some(token.as_str())),
            )
            .await;
            assert_eq!(
                text(&first, "id"),
                id,
                "case {case}: expected the delete to report the resource it addressed, got {first}"
            );
            assert_eq!(
                text(&first, "status"),
                DELETING,
                "case {case}: expected an accepted delete to report {DELETING}, got {first}"
            );

            // Every retry carrying the SAME token answers the SAME detail, however many
            // times it is retried.
            for attempt in 1..=*retries {
                let what = format!("{case}: retry {attempt} of {retries}");
                let replayed = accepted(
                    &router,
                    delete,
                    &what,
                    delete_request(&id, Some(token.as_str())),
                )
                .await;
                assert_eq!(
                    replayed, first,
                    "case {what}: expected the retry to replay the stored detail {first}, got {replayed}"
                );
            }

            // The replay wrote nothing: the resource is still the one record the first call
            // left, readable in `DELETING`.
            let details = delete.read(&router, &case, &id).await;
            assert_eq!(
                details.len(),
                1,
                "case {case}: expected the resource to stay readable until its purge, got {details:?}"
            );
            let stored = details.into_iter().next().unwrap_or(Value::Null);
            assert_eq!(
                text(&stored, "status"),
                DELETING,
                "case {case}: expected the stored resource to report {DELETING}, got {stored}"
            );

            // A request carrying NO token is not identified as a retry, so it is still
            // refused by the status precondition: `DELETING` is not a status a delete may
            // start from.
            refused(
                &router,
                delete,
                &format!("{case}: a second delete carrying no token"),
                delete_request(&id, None),
                "ConflictException",
            )
            .await;

            // Once the purge has removed the record there is nothing to replay, so the same
            // token is answered as the not-found it now is.
            await_purged(&router, delete, &case, &id).await;
            refused(
                &router,
                delete,
                &format!("{case}: the same token once the resource is purged"),
                delete_request(&id, Some(token.as_str())),
                "ResourceNotFoundException",
            )
            .await;
        }
    }
}

// ---------------------------------------------------------------------------
// A spent token answers only for the resource its request addresses.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_spent_delete_token_cannot_answer_for_a_resource_it_does_not_address() {
    for delete in [Delete::Collection, Delete::VpcEndpoint] {
        let router = emulator();
        let operation = delete.operation();
        let token = "one-spent-delete-token";

        // Both resources are stored before anything is deleted, so every call below is made
        // inside the window the first delete's record is still readable in.
        let case = format!("{operation}: the first delete");
        let first_id = delete.create(&router, &case, "scoped-delete-first").await;
        let second_id = delete.create(&router, &case, "scoped-delete-second").await;
        let first = accepted(&router, delete, &case, delete_request(&first_id, Some(token))).await;

        // The SAME token against ANOTHER resource that may be deleted: a request for that
        // other resource, so its own deletion is accepted and its own record answered.
        let case = format!("{operation}: the token spent again on another resource");
        let second = accepted(
            &router,
            delete,
            &case,
            delete_request(&second_id, Some(token)),
        )
        .await;
        assert_eq!(
            text(&second, "id"),
            second_id,
            "case {case}: expected the answer to describe the resource it addressed, got {second}"
        );
        assert_ne!(
            text(&second, "id"),
            text(&first, "id"),
            "case {case}: expected the spent token not to answer with the first record, got {second}"
        );
        assert_eq!(
            text(&second, "status"),
            DELETING,
            "case {case}: expected its own deletion to be accepted, got {second}"
        );

        // A retry of the token's FIRST request still replays the record that call left:
        // scoping the replay has not turned a retry into a refusal.
        let case = format!("{operation}: the retry of the first request");
        let replayed = accepted(
            &router,
            delete,
            &case,
            delete_request(&first_id, Some(token)),
        )
        .await;
        assert_eq!(
            replayed, first,
            "case {case}: expected the detail the first call stored {first}, got {replayed}"
        );

        // And against a resource that may NOT be deleted yet, the spent token does not turn
        // the state conflict into a replayed success.
        let case = format!("{operation}: the token spent again on a resource mid-create");
        let transient_id = delete
            .create_transient(&router, &case, "scoped-delete-transient")
            .await;
        refused(
            &router,
            delete,
            &case,
            delete_request(&transient_id, Some(token)),
            "ConflictException",
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// A delete token's replay is scoped to the operation that spent it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_token_is_scoped_to_the_operation_that_spent_it() {
    let router = emulator();
    let token = "a-token-first-spent-on-a-create";

    // `CreateCollection` spends the token first, minting its own collection.
    let (status, code, answer) = call(
        &router,
        "CreateCollection",
        json!({
            "name": "delete-operation-scope",
            "encryptionConfig": { "aWSOwnedKey": true },
            "clientToken": token,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateCollection(carrying the token): expected 200, got {status} {code} with {answer}"
    );
    let created_id = text(at(&answer, "createCollectionDetail"), "id");

    // The SAME token on `DeleteCollection`: the token names the request it was first spent
    // on, which was another operation's, so this is an ordinary delete of the collection it
    // addresses rather than a replay of the create.
    let case = "the delete carrying the create's token";
    let settled = Delete::Collection
        .create(&router, case, "delete-operation-scope-other")
        .await;
    let detail = accepted(
        &router,
        Delete::Collection,
        case,
        delete_request(&settled, Some(token)),
    )
    .await;
    assert_eq!(
        (text(&detail, "id"), text(&detail, "status")),
        (settled.clone(), DELETING.to_string()),
        "case {case}: expected the delete it asked for to be accepted, got {detail}"
    );

    // The collection the create minted is untouched by that delete: a token spent on one
    // operation did not delete the other operation's record.
    let details = Delete::Collection
        .read(&router, "the created collection", &created_id)
        .await;
    let created = details.into_iter().next().unwrap_or(Value::Null);
    assert_ne!(
        text(&created, "status"),
        DELETING,
        "case {case}: expected the collection the create minted to be untouched, got {created}"
    );
}
