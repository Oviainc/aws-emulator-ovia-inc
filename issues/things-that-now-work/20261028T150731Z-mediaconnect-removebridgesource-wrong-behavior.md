Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a19f71fb9b9d334f8efc64d442ea8394bd20139d

# RemoveBridgeSource removes the bridge's primary source and leaves SourceFailoverConfig naming a source the bridge no longer has

_Filed: 2026-10-28T15:07:30Z_
_Operation: RemoveBridgeSource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a19f71fb9b9d334f8efc64d442ea8394bd20139d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RemoveBridgeSource` drops the addressed source from `StoredBridge::sources` and writes nothing else. When the removed source is the one `SourcePriority.PrimarySource` names, the stored bridge keeps a failover setting naming a source it no longer holds — the state the crate's own shared resolver declares can never be stored. DescribeBridge reports the dangling name afterwards, and a later UpdateBridge that re-states the priority is refused for the state this removal created.

## Evidence

The invariant — services/service-com-amazonaws-mediaconnect-mediaconnect/src/state.rs:1361-1365:
```
/// Every operation that writes a source priority resolves the reference HERE ... So a
/// stored failover setting never names a source its resource has not got, whichever call
/// wrote it.
```
RemoveBridgeSource does not read `source_failover_config` at all — src/ops/remove_bridge_source.rs:196-203:
```
        // Nothing in the model puts a lower bound on a STORED bridge's sources ...
        let Some(position) = bridge.source_position(&input.source_name) else {
            return Err(not_found(&input.source_name));
        };
        let removed = bridge.sources.remove(position);
```
UpdateBridge owns the rule on the same record — src/ops/update_bridge.rs:311-325:
```
        // `SourcePriority$PrimarySource` is "the name of the source you choose as the
        // primary source", so it has to name a source the bridge has — resolved through
        // the one function every path that writes a source priority resolves it with
        // (CreateBridge resolves the create shape there too).
        if let Some(rejection) = crate::state::bridge_primary_source_rejection(
```
Test coverage: tests/all/failover_primary_source.rs:431 (`create_bridge_and_update_bridge_resolve_the_primary_source_against_the_bridge_s_sources`) pins the write paths only; no test exercises RemoveBridgeSource against a failover primary.

## Steps to Reproduce

1. Create a gateway, then POST /v1/bridges for an egress bridge with two flow sources "src-a" and "src-b" and `{"sourceFailoverConfig":{"state":"ENABLED","failoverMode":"MERGE","sourcePriority":{"primarySource":"src-b"}}}`.
2. DELETE /v1/bridges/{BridgeArn}/sources/src-b — observe 202.
3. GET /v1/bridges/{BridgeArn} — observe `bridge.sourceFailoverConfig.sourcePriority.primarySource == "src-b"` while `bridge.sources` holds only "src-a".
4. PUT /v1/bridges/{BridgeArn} with `{"sourceFailoverConfig":{"sourcePriority":{"primarySource":"src-b"}}}` — observe 400 BadRequestException for the state step 2 stored.
