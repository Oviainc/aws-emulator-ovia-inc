Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: d6480ecf165ed6d60533a02ba72b18c66da5a76b

# DeleteVpcEndpoint never reads its clientToken, so a retry of an accepted delete is refused 409 instead of replaying it

_Filed: 2026-10-29T12:53:26Z_
_Operation: DeleteVpcEndpoint | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `d6480ecf165ed6d60533a02ba72b18c66da5a76b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteVpcEndpoint` is `@idempotent` and models `clientToken` as an `@idempotencyToken`, but the handler never reads `input.client_token` at all — `grep client_token src/ops/delete_vpc_endpoint.rs` returns "No matches." The member is silently dropped: nothing is remembered through `store.remember_token(...)` and nothing is looked up through `store.replayed_vpc_endpoint(...)`, the replay machinery this crate already has and that BOTH other writes against a `VpcEndpointRecord` (`CreateVpcEndpoint`, `UpdateVpcEndpoint`) call.

The handler drops the token on a premise its own code immediately breaks. Its comment says the token is unnecessary because "the second delete of an endpoint already gone is the 404 below whatever token it carries" — but this handler does NOT remove the endpoint. It calls `record.begin_deletion()`, which leaves the record stored in the non-terminal `DELETING` status until the clock makes its purge due, exactly as the handler's own comment four lines later states. A retry of the SAME delete with the SAME `clientToken` inside that window therefore does not reach the 404 the comment promises: it falls through to the status precondition and is answered `ConflictException` (409), because `DELETING` is not one of `VPC_ENDPOINT_DELETABLE_STATUSES`. The retry reports a failure for a delete the service already accepted, and the caller cannot tell it from a genuine conflict even though the token says it is the very same request.

This is the identical shape of gap already recognised for `DeleteCollection` in this crate (open issue `20261029T122004Z-opensearchserverless-deletecollection-parameter-ignored`), whose evidence section cites `src/ops/delete_vpc_endpoint.rs:161-163` as merely carrying the same comment — the premise is false here too, because `VpcEndpointRecord::begin_deletion` keeps the record just as `CollectionRecord::begin_deletion` does (state.rs documents the second as "Mirrors" the first). The siblings that DO remove their record outright (`DeleteCollectionGroup`, `DeleteSecurityConfig`, the policy deletes) are correct to drop the token; this one is not. The fix is the one `UpdateVpcEndpoint`'s sibling path already shows: scope the replay to the `id` the request addresses and answer the stored `delete_detail()`.

## Evidence

The member is modelled: `DeleteVpcEndpointRequest` -> `"clientToken": {"target": "ClientToken", "idempotencyToken": true, ... "length": {"min": 1, "max": 512}}` with the operation `"idempotent": true` (smithy_query(query="operation", operation="DeleteVpcEndpoint")), restated in the handler's own contract header: "* `clientToken` (ClientToken, len 1..512, @idempotencyToken) — Unique, case-sensitive identifier to ensure idempotency of the request." — src/ops/delete_vpc_endpoint.rs:52-53.

The handler never reads it: `grep -n client_token src/ops/delete_vpc_endpoint.rs` -> "No matches." The whole handle body is the store lookup, the status check and `record.begin_deletion()` — src/ops/delete_vpc_endpoint.rs:143-210.

The premise it is dropped on: "// A `clientToken` on a delete is not a replay index: the endpoint this request\n        // addresses is named by `id`, and the second delete of an endpoint already gone is\n        // the 404 below whatever token it carries." — src/ops/delete_vpc_endpoint.rs:161-163.

The same handler then says the endpoint is NOT gone: "// The delete is ACCEPTED here, not completed: the endpoint is reported in\n        // `DELETING` ... and stays readable in that status\n        // until the clock makes its purge due, so a caller polling the deletion observes\n        // the state it goes through instead of the resource vanishing in one step." — src/ops/delete_vpc_endpoint.rs:190-194, then `record.begin_deletion();` — src/ops/delete_vpc_endpoint.rs:203. That method keeps the record: "pub fn begin_deletion(&mut self) {\n        self.status = VPC_ENDPOINT_DELETING.to_string();\n        ...\n        self.purge_at = Some(Instant::now() + SETTLE_AFTER);" — src/state.rs:2333-2339, documented as "Mirrors [`CollectionRecord::begin_deletion`]" (src/state.rs:2332).

So the retry reaches the status refusal instead: "if !crate::state::VPC_ENDPOINT_DELETABLE_STATUSES.contains(&record.status.as_str()) { return Err(DeleteVpcEndpointErrorModeled::ConflictException(" — src/ops/delete_vpc_endpoint.rs:176-188, with `pub const VPC_ENDPOINT_DELETABLE_STATUSES: [&str; 2] = [VPC_ENDPOINT_ACTIVE, VPC_ENDPOINT_FAILED];` — src/state.rs:127 (no `DELETING`).

The crate has the helper, and the sibling writes on the same record call it: "/// The VPC endpoint a replayed `clientToken` on `operation` already created, if any;\n    /// see [`Self::replayed_collection`] for why a stale token answers `None`.\n    pub fn replayed_vpc_endpoint(" — src/state.rs:2964-2970, beside `pub fn remember_token(&mut self, operation: &str, token: &str, key: &str)` (src/state.rs:2788). Used by `store.replayed_vpc_endpoint(Self::NAME, token).filter(|record| record.name == input.name)` + `store.remember_token(Self::NAME, token, &id)` — src/ops/create_vpc_endpoint.rs:190-199 and 261-263; and by "// A replayed `clientToken` is a retry of the call that already applied this update,\n        // so it answers the record that call left rather than applying the change twice." + `store.remember_token(Self::NAME, token, &input.id)` — src/ops/update_vpc_endpoint.rs:244-252 and 362-364.

Test coverage: nothing pins the retry-with-token case for an ACCEPTED delete. tests/all/vpc_endpoint_delete.rs:195 issues the second delete as `json!({ "id": &id })` with NO `clientToken` and asserts the 409 ("// A second delete of an endpoint already being deleted is the conflict\n        // `ConflictException` describes, since `DELETING` is not a status it may be deleted\n        // from." — tests/all/vpc_endpoint_delete.rs:192-200), and tests/all/vpc_endpoint_delete.rs:233-293 drives deletes WITH a token only against a `PENDING` endpoint whose delete was REFUSED ("// The rows differ in what the delete request carries beside the id: `clientToken` is an\n    // `@idempotencyToken` on this operation, and a delete is addressed by `id`, so the\n    // refusal is the same with and without one." — tests/all/vpc_endpoint_delete.rs:237-239). Since no token is ever remembered by a REFUSED delete, remembering the token on an ACCEPTED delete and replaying it leaves both of those assertions, and every other existing test, unchanged.

## Steps to Reproduce

1. CreateVpcEndpoint(name="retryable-endpoint", vpcId="vpc-1a2b3c4d", subnetIds=["subnet-1a2b3c4d"]) -> 200 with createVpcEndpointDetail.status == "PENDING"; note its id.
2. Poll BatchGetVpcEndpoint(ids=[<id>]) until the endpoint reports ACTIVE (it settles on its own after SETTLE_AFTER, src/state.rs:239).
3. DeleteVpcEndpoint(id=<id>, clientToken="T") -> 200 with deleteVpcEndpointDetail.status == "DELETING".
4. Immediately (within the 500 ms purge window) replay the identical call: DeleteVpcEndpoint(id=<id>, clientToken="T").
5. Observed: 409 ConflictException "VPC endpoint cannot be deleted because it is not in 'ACTIVE' or 'FAILED' status." — the retry of an already-accepted delete reports a conflict and the clientToken changed nothing. Expected (the shape both other @idempotent writes of a VpcEndpointRecord in this crate take): the replay answers the deleteVpcEndpointDetail the first call stored.
