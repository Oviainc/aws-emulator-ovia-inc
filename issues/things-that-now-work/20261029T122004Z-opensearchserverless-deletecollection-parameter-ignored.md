Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 45e969ed3c6a96399c623276452bb212f0ee3327

# DeleteCollection never reads its clientToken, so a retry of an accepted delete is refused 409 instead of replaying it

_Filed: 2026-10-29T12:20:04Z_
_Operation: DeleteCollection | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `45e969ed3c6a96399c623276452bb212f0ee3327`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteCollection` is `@idempotent` and models `clientToken` as an `@idempotencyToken`, but the handler never reads `input.client_token` at all — `grep client_token src/ops/delete_collection.rs` returns nothing. The member is dropped: nothing is remembered through `store.remember_token(...)` and nothing is looked up through `store.replayed_collection(...)`, the replay machinery this same crate already has and that both other writes against a `CollectionRecord` (`CreateCollection`, `UpdateCollection`) call.

Because this handler leaves the deleted collection readable in `DELETING` until the clock makes its purge due (its own comment says so), a retry of the SAME delete request with the SAME `clientToken`, inside that window, falls through to the status precondition and is answered `ConflictException` (409) — the collection is in `DELETING`, which is not one of `DELETABLE_STATUSES`. The retry therefore reports a failure for a delete the service already accepted, and the caller cannot tell it from a genuine conflict, even though the token says this is the very same request.

This is not the crate's stated position on deletes. The three sibling deletes that deliberately ignore a `clientToken` justify it on a premise that does not hold here: that the second delete of an id "already gone" is a 404 whatever token it carries. For `DeleteCollection` the record is NOT gone during the purge window, so the second call is a 409, not the 404 the justification assumes — and unlike those siblings, `delete_collection.rs` carries no comment about `clientToken` at all. The fix is the one `UpdateCollection`'s sibling path already shows: scope the replay to the `id` the request addresses and answer the stored `delete_detail()`.

## Evidence

The member is modelled and documented: `DeleteCollectionRequest$clientToken` — "A unique, case-sensitive identifier to ensure idempotency of the request." (smithy_query doc, DeleteCollectionRequest/clientToken), restated in the handler's own contract header: "* `clientToken` (ClientToken, len 1..512, @idempotencyToken) — A unique, case-sensitive identifier to ensure idempotency of the request." — src/ops/delete_collection.rs:55-56. The operation is `@idempotent` (OPERATIONS.md:171).

The handler never reads it: `grep -n client_token src/ops/delete_collection.rs` -> "No matches." The whole of the handle body is the store lookup, the deletion-protection check, the status check and `record.begin_deletion(deleted_at)` — src/ops/delete_collection.rs:153-231.

The record survives the accepted delete, by this handler's own statement: "The delete is ACCEPTED here, not completed: the collection is reported in `DELETING` and stays readable in that status until the clock makes its purge due, so a caller polling the deletion observes the state it goes through instead of the resource vanishing in one step." — src/ops/delete_collection.rs:213-216. So the retry reaches: "if !crate::state::DELETABLE_STATUSES.contains(&record.status.as_str()) { return Err(DeleteCollectionErrorModeled::ConflictException(..." — src/ops/delete_collection.rs:201-211, with `DELETABLE_STATUSES = [COLLECTION_ACTIVE, COLLECTION_FAILED, COLLECTION_UPDATE_FAILED]` — src/state.rs:102-103 (no `DELETING`).

The crate has the helper and says every operation that needs it uses it: "The record a replayed `clientToken` on `operation` already created, if any. ... The handler SCOPES what this answers to the resource its request addresses ... Every caller in `src/ops/` therefore filters on the addressed identity." — src/state.rs:2719-2728, above `pub fn replayed_collection(&self, operation: &str, token: &str) -> Option<&CollectionRecord>` (src/state.rs:2729), beside `pub fn remember_token(&mut self, operation: &str, token: &str, key: &str)` (src/state.rs:2788).

Siblings on the same record call it: `.replayed_collection(Self::NAME, token).filter(|record| record.name == input.name)` + `store.remember_token(Self::NAME, token, &id)` — src/ops/create_collection.rs:247-257 and 380-382; `store.replayed_collection(Self::NAME, token)` under "A replayed `clientToken` is a retry of the call that already applied this update, so it answers the record that call left rather than applying it twice." + `store.remember_token(Self::NAME, token, &id)` — src/ops/update_collection.rs:195-203 and 257-260.

The premise the other deletes drop the token on does not hold here: "A `clientToken` on a delete is not a replay index — the second delete of an id that is already gone is this 404, whatever token it carries." — src/ops/delete_collection_group.rs:166-168 (true there: `store.remove_collection_group(&input.id)` removes the record outright, src/ops/delete_collection_group.rs:205), and src/ops/delete_vpc_endpoint.rs:161-163.

Test coverage: nothing pins the token case for this operation. tests/all/collection_lifecycle.rs:773-781 issues the second delete as `json!({ "id": id.clone() })` with NO `clientToken` and asserts the 409 ("A second delete while it is DELETING is the state conflict, not a not-found: the collection is still there."), and tests/all/encryption_indexes_and_tagging.rs:1310-1311 drives DeleteCollection without a token, so the retry-with-token path is untested.

## Steps to Reproduce

1. CreateSecurityPolicy(type=encryption, name="enc", policy={"Rules":[{"ResourceType":"collection","Resource":["collection/*"]}],"AWSOwnedKey":true}) so a create is admitted.
2. CreateCollection(name="retryable-collection"); poll BatchGetCollection until it reports ACTIVE; note its id.
3. DeleteCollection(id=<id>, clientToken="T") -> 200 with deleteCollectionDetail.status == "DELETING".
4. Immediately (within the 500 ms purge window, SETTLE_AFTER at src/state.rs:239) replay the identical call: DeleteCollection(id=<id>, clientToken="T").
5. Observed: 409 ConflictException "Collection cannot be deleted because it is not in 'ACTIVE', 'FAILED', or 'UPDATE_FAILED' status." — the retry of an already-accepted delete reports a conflict and the clientToken changed nothing. Expected (the shape every other @idempotent write of a CollectionRecord in this crate takes): the replay answers the deleteCollectionDetail the first call stored.
