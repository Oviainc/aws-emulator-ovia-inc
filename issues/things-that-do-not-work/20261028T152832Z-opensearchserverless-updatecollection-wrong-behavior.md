Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 48bb9604e330d1bf8edda7143fefbdaecc9daf20

# Scope UpdateCollection's clientToken replay to the collection the request addresses

_Filed: 2026-10-28T15:28:32Z_
_Operation: UpdateCollection | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `48bb9604e330d1bf8edda7143fefbdaecc9daf20`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateCollection` looks a replayed `clientToken` up in the shared idempotency index and returns whatever collection that token FIRST named, without checking that the collection is the one `id` addresses. Every other `@idempotent` handler in this crate filters the replay on the addressed identity, and the state helper's own documentation says every caller does.

Consequence: a caller that spends one `clientToken` on `UpdateCollection` for collection A and then (deliberately or by reusing a token) on collection B gets HTTP 200 carrying A's `updateCollectionDetail` — A's `id`, `name`, `arn` and `status` — while the update it asked for on B is silently never applied. The write is dropped, the response describes a resource the request did not address, and the handler's own header doc ("applies the members the request names ... to the `CollectionRecord` stored under the requested id") is not what happens.

## Evidence

The state helper states the rule the handler must follow: "The handler SCOPES what this answers to the resource its request addresses: a token spent again on another resource is a request for that other resource, which is still served on its own terms rather than answered with the record the token first created. Every caller in `src/ops/` therefore filters on the addressed identity." — src/state.rs:2426-2429, directly above `pub fn replayed_collection(&self, operation: &str, token: &str) -> Option<&CollectionRecord>` (src/state.rs:2430).

The handler does NOT filter: "if let Some(token) = input.client_token.as_deref() {\n            if let Some(record) = store.replayed_collection(Self::NAME, token) {\n                return Ok(UpdateCollectionOutput {\n                    update_collection_detail: Some(record.update_detail()),\n                });" — src/ops/update_collection.rs:197-203. It is the only unfiltered caller in the crate.

Every sibling filters: `.replayed_collection(Self::NAME, token).filter(|record| record.name == input.name)` — src/ops/create_collection.rs:235-238; `.replayed_collection_group(Self::NAME, token).filter(|record| record.id == input.id)` with the comment "A replayed `clientToken` is a RETRY of the write this call already performed" — src/ops/update_collection_group.rs:180-184; `.replayed_security_config(Self::NAME, token).filter(|record| record.id == input.id)` under "The replay is scoped to the id THIS request addresses." — src/ops/update_security_config.rs:214-219.

The handler's own header claims the opposite of what the replay path does: "IMPLEMENTED: applies the members the request names (`description`, `vectorOptions`, `deletionProtection`) to the `CollectionRecord` stored under the requested id" — src/ops/update_collection.rs:4-5.

Test coverage: no test exercises an `UpdateCollection` token spent on a second collection — tests/all/encryption_indexes_and_tagging.rs:1086-1299 drives `UpdateCollection` without `clientToken`, and the cross-resource token test tests/all/policy_and_group_writes.rs:689-762 (`a_client_token_spent_on_another_resource_still_creates_it`) covers only the policy create families, so nothing pins the current answer.

## Steps to Reproduce

1. CreateCollection(name="coll-a", encryptionConfig={aWSOwnedKey:true}) and CreateCollection(name="coll-b", encryptionConfig={aWSOwnedKey:true}); wait for both to settle to ACTIVE; note ids A and B.
2. UpdateCollection(id=A, description="first", clientToken="T") -> 200, detail names A, A moves to UPDATING; wait for ACTIVE.
3. UpdateCollection(id=B, description="second", clientToken="T") -> 200, but `updateCollectionDetail.id`/`name` are A's, not B's.
4. BatchGetCollection(ids=[B]) -> B's description is unchanged: the update was accepted with 200 and silently dropped.
