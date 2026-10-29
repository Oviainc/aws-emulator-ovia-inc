Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1211f7611505016a47ab1bf8932fdc778b7fd399

# A RequestId spent on a multiplex the account has since deleted still replays that DELETED multiplex, so the retry creates nothing

_Filed: 2026-10-29T01:48:53Z_
_Operation: CreateMultiplex | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1211f7611505016a47ab1bf8932fdc778b7fd399`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states, in the shared idempotency-token helper and again in three of the slices that use it, that a `RequestId` must NOT outlive the resource it minted: once the delete of that resource has come due the token's idempotency window is over, so a later create carrying the same token mints a NEW resource instead of answering with the tombstone. `settle()` implements that for channels and networks, `settle_clusters` for clusters, and the template / template-group / signal-map deletes do it at the point of removal.

`settle_multiplexes` does not. The multiplex token map is never `forget`-ed, and a multiplex delete KEEPS the record (`begin_delete` moves it to `DELETING` and the clock settles it into the terminal `DELETED` the record keeps). `multiplex_for_token` resolves a token by looking the id up in `self.multiplexes`, which still holds the tombstone, so it answers. `CreateMultiplex`'s replay branch then returns 201 with the DELETED multiplex and mints nothing: the caller gets a `multiplex` whose `state` is `DELETED`, cannot start it, cannot update it (the update answers 409 `ConflictException` for exactly that record), and no multiplex ever exists for that request.

The same omission applies to every other token map of this crate whose resource keeps its record after a delete: `sdi_source_tokens` (`delete_sdi_source` only sets `deleted = true`), `reservation_tokens` (`delete_reservation` only sets `record.deleted = true`), `node_tokens` (a deleted node ends in the terminal `DEREGISTERED` it keeps) and `channel_placement_group_tokens`. Fixing the rule in one place for the multiplex slice should be done the same way for those.

## Evidence

The rule, stated by the shared helper itself — services/service-com-amazonaws-medialive-medialive/src/state.rs:1790-1792:
```
    /// Drop every token that created `resource_id`, so a token never outlives
    /// the resource it minted.
    fn forget(&mut self, resource_id: &str) {
```
and restated with the harm it prevents — src/state.rs:2458-2464 (inside `settle`):
```
        // A token never outlives the channel it minted: once that channel has
        // reached the terminal `DELETED` state its idempotency window is over, so
        // a retried create mints a new channel rather than answering with a
        // deleted one.
        for id in just_deleted {
            self.channel_tokens.forget(&id);
        }
```
and again for the cluster — src/state/clusters.rs:473-479:
```
        // A token never outlives the cluster it minted: once that cluster has
        // reached the terminal `DELETED` state its idempotency window is over,
        // so a retried create mints a new cluster rather than answering with a
        // deleted one — the rule the channel and network tokens follow too.
        for id in just_deleted {
            self.cluster_tokens.forget(&id);
        }
```

The multiplex slice skips it — src/state/multiplexes.rs:1092-1095 (`settle_multiplexes`, the whole per-record pass):
```
    pub(super) fn settle_multiplexes(&mut self, now: SystemTime) {
        for multiplex in self.multiplexes.values_mut() {
            multiplex.settle(now);
        }
```
(`grep -n '\.forget(' src` finds `channel_tokens`, `network_tokens`, `cluster_tokens`, `alarm_template_tokens`, `alarm_template_group_tokens`, `event_bridge_rule_template_tokens`, `event_bridge_rule_template_group_tokens` and `signal_map_tokens` — and no `multiplex_tokens`, `input_tokens`, `sdi_source_tokens`, `reservation_tokens`, `node_tokens` or `channel_placement_group_tokens`.)

The record the token goes on resolving to — src/state/multiplexes.rs:922-929:
```
    /// Move this multiplex into `DELETING`, owing the terminal `DELETED` state
    /// it then keeps once [`TRANSIENT_STATE_SPAN`] has passed: the record stays
    /// in the account, so a reader goes on reporting the multiplex in that state
    /// (which is what the SDK's `MultiplexDeleted` waiter polls
    /// `DescribeMultiplex` for) instead of losing it.
    pub fn begin_delete(&mut self, now: SystemTime) {
```
and the lookup that therefore still answers — src/state/multiplexes.rs:1268-1275:
```
    /// The multiplex a stored `RequestId` created, if that multiplex still
    /// exists.
    pub fn multiplex_for_token(&self, request_id: &str) -> Option<&str> {
        let id = self.multiplex_tokens.created(request_id)?;
        self.multiplexes
            .get(id)
            .map(|multiplex| multiplex.id.as_str())
    }
```

The handler branch that answers 201 with the tombstone — src/ops/create_multiplex.rs:177-189:
```
        // A retried create (same `@idempotencyToken` `RequestId`) must answer
        // with the multiplex the first one created, not a second multiplex.
        if let Some(existing) = state
            .multiplex_for_token(&input.request_id)
            .map(str::to_string)
        {
            let multiplex = state
                .multiplex_description(&existing)
                .ok_or_else(unavailable)?;
            return Ok(CreateMultiplexOutput {
                multiplex: Some(multiplex),
            });
        }
```

Test coverage: the rule IS pinned for the resources that implement it — tests/all/signal_map_deletion.rs:481-489:
```
/// Rule: a `RequestId` (`@idempotencyToken`) never outlives the signal map it
/// created, so once that signal map is deleted the same token creates a NEW
/// signal map rather than answering with the deleted one.
...
async fn a_deleted_signal_map_is_not_resurrected_by_its_idempotency_token() {
```
and tests/all/cluster_deletion.rs:756-771:
```
        // The token's idempotency window is over exactly when the deletion has
        // come due: before that a retry still answers with the cluster it
        // created, and afterwards it mints a new one.
```
Nothing pins the multiplex side: the multiplex idempotency test has no post-delete row — tests/all/multiplex_lifecycle.rs:355-365 enumerates only `("the same token", "token-a", "token-a", true)`, `("two tokens", ...)` and `("the second token is a prefix of the first", ...)`, and tests/all/multiplex_deletion.rs never re-presents a create token after the delete.

## Steps to Reproduce

1. POST /prod/multiplexes with {"name":"first","requestId":"token-a","availabilityZones":["us-west-2a","us-west-2b"],"multiplexSettings":{"transportStreamBitrate":2000000,"transportStreamId":1}} -> 201; note the id M.
2. Wait for the transient span so M settles from CREATING into IDLE (GET /prod/multiplexes/M reports IDLE).
3. DELETE /prod/multiplexes/M -> 202 with state DELETING; wait for the transient span and confirm GET /prod/multiplexes/M reports state DELETED (the record is kept, src/state/multiplexes.rs:922-929).
4. POST /prod/multiplexes again with the SAME "requestId":"token-a" and a different name -> the emulator answers 201 carrying multiplex M with "state":"DELETED" and creates nothing. GET /prod/multiplexes shows no new multiplex. Expected, per src/state.rs:1790-1792 and src/state/clusters.rs:473-479, a freshly minted multiplex — which is exactly what POST /prod/clusters and POST /prod/signal-maps do with a token whose resource has been deleted (pinned by tests/all/cluster_deletion.rs:756-771 and tests/all/signal_map_deletion.rs:488-524).
5. POST /prod/multiplexes/M/start on the replayed id -> 409, so the caller is left with a 201 that named an unusable resource.
