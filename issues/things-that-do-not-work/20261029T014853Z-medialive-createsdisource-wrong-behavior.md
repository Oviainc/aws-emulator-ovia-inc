Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1211f7611505016a47ab1bf8932fdc778b7fd399

# A RequestId spent on an SdiSource the account has since deleted still replays that DELETED source instead of minting a new one

_Filed: 2026-10-29T01:48:53Z_
_Operation: CreateSdiSource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1211f7611505016a47ab1bf8932fdc778b7fd399`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteSdiSource` is a tombstone delete: `delete_sdi_source` only sets `deleted = true` and the record stays in the store so `DescribeSdiSource`/`ListSdiSources` keep reporting it in the terminal `DELETED` state. The SdiSource idempotency-token map is never cleared when that happens, and `sdi_source_for_token` resolves a token purely by looking the recorded id up in `self.sdi_sources`, which still holds the tombstone.

So a later `CreateSdiSource` carrying the `RequestId` of a source that has since been deleted takes the handler's replay branch and answers 200 with the DELETED source — a source whose `state` is `DELETED`, that `UpdateSdiSource` refuses with 404 as "already deleted", and that no input may reference (`input_configuration_error` rejects a deleted source) — while minting nothing.

That contradicts the rule this crate states for exactly this situation in the shared token helper and implements for channels, networks, clusters, signal maps and the four template/template-group slices: a token must not outlive the resource it minted, so a retry past the delete mints a new resource.

## Evidence

The rule, from the shared helper the SdiSource token map is an instance of — services/service-com-amazonaws-medialive-medialive/src/state.rs:1790-1792:
```
    /// Drop every token that created `resource_id`, so a token never outlives
    /// the resource it minted.
    fn forget(&mut self, resource_id: &str) {
```
and the harm it names, from the channel arm of `settle` — src/state.rs:2458-2464:
```
        // A token never outlives the channel it minted: once that channel has
        // reached the terminal `DELETED` state its idempotency window is over, so
        // a retried create mints a new channel rather than answering with a
        // deleted one.
        for id in just_deleted {
            self.channel_tokens.forget(&id);
        }
```

The SdiSource delete keeps the record and clears no token — src/state.rs:6119-6125:
```
/// Delete an SdiSource: it moves to the terminal `DELETED` state and stays in
/// the account, so `DescribeSdiSource` and `ListSdiSources` keep reporting it
/// there rather than losing it. Idempotent — deleting an already deleted source
/// changes nothing.
pub fn delete_sdi_source(source: &mut SdiSourceRecord) {
    source.deleted = true;
}
```
and the lookup therefore still answers for the tombstone — src/state.rs:2658-2662:
```
    /// The SdiSource a stored `RequestId` created, if that source still exists.
    pub fn sdi_source_for_token(&self, request_id: &str) -> Option<&str> {
        let id = self.sdi_source_tokens.created(request_id)?;
        self.sdi_sources.get(id).map(|source| source.id.as_str())
    }
```
(`grep -n '\.forget(' src` lists `channel_tokens`, `network_tokens`, `cluster_tokens`, `alarm_template_tokens`, `alarm_template_group_tokens`, `event_bridge_rule_template_tokens`, `event_bridge_rule_template_group_tokens`, `signal_map_tokens` — `sdi_source_tokens` is absent.)

The replay branch that answers with it — src/ops/create_sdi_source.rs:180-194:
```
        // A retried create (same `@idempotencyToken` `RequestId`) must return the
        // SdiSource the first one created, not a second source.
        if let Some(existing) = input
            .request_id
            .as_deref()
            .and_then(|token| state.sdi_source_for_token(token))
            .map(str::to_string)
        {
            let sdi_source = state
                .sdi_source_description(&existing)
                .ok_or_else(unavailable)?;
            return Ok(CreateSdiSourceOutput {
                sdi_source: Some(sdi_source),
            });
        }
```
and the sibling that proves the replayed id is unusable — src/ops/update_sdi_source.rs:169-184 answers `NotFoundException` with `already_deleted_message` for the very same record.

Test coverage: the SdiSource idempotency test covers only the four live-resource rows and never re-presents a token after a delete — tests/all/sdi_source_lifecycle.rs:318-329:
```
/// Rule: a create retried with the `RequestId` (`@idempotencyToken`) of an
/// earlier one answers with the source that one created; two creates that do not
/// share a token create two sources.
...
    let rows: [(&str, Option<&str>, Option<&str>, usize); 4] = [
        ("the same token", Some("token-a"), Some("token-a"), 1),
        ("two tokens", Some("token-a"), Some("token-b"), 2),
        ("no token at all", None, None, 2),
        ("only the second names one", None, Some("token-b"), 2),
    ];
```
while the same rule past the delete IS pinned for the resources that implement it — tests/all/signal_map_deletion.rs:481-489 (`a_deleted_signal_map_is_not_resurrected_by_its_idempotency_token`).

## Steps to Reproduce

1. POST /prod/sdiSources with {"name":"camera-a","type":"SINGLE","requestId":"token-a"} -> 200; note the id S.
2. DELETE /prod/sdiSources/S -> 200; GET /prod/sdiSources/S reports sdiSource.state == "DELETED" (the record is kept, src/state.rs:6119-6125).
3. POST /prod/sdiSources again with {"name":"camera-b","type":"QUAD","mode":"QUADRANT","requestId":"token-a"} -> the emulator answers 200 carrying source S with "state":"DELETED" and mints nothing; GET /prod/sdiSources shows no new source. Expected, per src/state.rs:1790-1792 and src/state.rs:2458-2464, a freshly minted SdiSource — the behaviour POST /prod/signal-maps and POST /prod/clusters give for a token whose resource was deleted.
4. PUT /prod/sdiSources/S with {"name":"x"} -> 404 "already deleted" (src/ops/update_sdi_source.rs:169-184), so the 200 in step 3 named a resource the caller can neither use nor change.
