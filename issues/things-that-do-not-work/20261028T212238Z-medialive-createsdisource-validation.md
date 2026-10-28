Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 355761efac79959eac560a8efce6608069de81f3

# CreateSdiSource stores an SdiSource name the account already uses, skipping the documented name-uniqueness rule every sibling create in this crate applies

_Filed: 2026-10-28T21:22:38Z_
_Operation: CreateSdiSource | Class: validation_

> _Note: this issue was identified against the code base at commit `355761efac79959eac560a8efce6608069de81f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateSdiSourceRequest$Name` carries the same documented uniqueness sentence as `CreateClusterRequest$Name` and `CreateNetworkRequest$Name` — "Specify a name that is unique in the AWS account" — and this crate reads that exact sentence as a service-side rule for the other two, refusing a duplicate with the operation's modelled `ConflictException`.

`CreateSdiSource` applies no such check. The handler validates the reserved tag namespace, replays the idempotency token, checks the store cap, mints the id, and then writes `source.name = input.name` verbatim. The crate has no `sdi_source_named` lookup at all (`src/state.rs` has `cluster_named`, `network_named`, `signal_map_named`, `node_named`, `alarm_template_named`, `channel_placement_group_named`, `event_bridge_rule_template_group_named` — and nothing for SDI sources), so two sources under one name are accepted and both are reported by `ListSdiSources`/`DescribeSdiSource`. `UpdateSdiSource` has the same gap: `UpdateSdiSourceRequest$Name` repeats the uniqueness sentence and `src/ops/update_sdi_source.rs:190-192` assigns the new name with no collision check.

The consequence is that `CreateSdiSource`'s modelled `ConflictException` (409) — the error it declares and the one every sibling uses for exactly this class of refusal — is unreachable, and an account can hold several SDI sources that a caller cannot tell apart by the name the model says identifies them.

## Evidence

The model's rule — `smithy_query(query="doc", shape="CreateSdiSourceRequest", member="Name")`:
```
"Specify a name that is unique in the AWS account. We recommend you assign a name that describes the source, for example curling-cameraA. Names are case-sensitive."
```
and `smithy_query(query="doc", shape="UpdateSdiSourceRequest", member="Name")`: "Include this parameter only if you want to change the name of the SdiSource. Specify a name that is unique in the AWS account. ..."

The create stores it unchecked — `services/service-com-amazonaws-medialive-medialive/src/ops/create_sdi_source.rs:218-226`:
```
        // Everything the request supplied is stored verbatim; the response is then
        // read back out of the stored record, never from `input`. `Mode` is kept
        ...
        source.name = input.name;
        source.mode = input.mode;
        source.r#type = input.r#type;
```
The only refusals the handler makes are the reserved-tag guard (`create_sdi_source.rs:170-175`) and the store cap (`create_sdi_source.rs:199-209`). The update likewise — `src/ops/update_sdi_source.rs:187-192`:
```
        // "Change some of the settings in an SdiSource": a member the request
        // carries replaces the stored one, and a member it omits leaves that
        // setting as it was.
        if let Some(name) = input.name {
            source.name = Some(name);
        }
```

The siblings in THIS crate that read the identical model sentence as a rule — `src/ops/create_network.rs:232-248`:
```
        // `Name` is documented "Specify a name that is unique in the AWS
        // account… Names are case-sensitive", so a second network under a name
        // the account already holds is the modelled conflict rather than another
        // network.
        if let Some(name) = input.name.as_deref() {
            if let Some(existing) = state.network_named(name).map(str::to_string) {
                return Err(CreateNetworkErrorModeled::ConflictException(
```
and `src/state/clusters.rs:571-575`:
```
        if let Some(name) = new.name.as_deref() {
            if self.cluster_named(name).is_some() {
                return Err(ClusterRefusal::NameAlreadyInUse);
            }
        }
```
plus `src/ops/create_signal_map.rs:286-295` (`if state.signal_map_named(&input.name).is_some() { ... ConflictException ... }`) and `src/state/channel_placement_groups.rs:471` (`ChannelPlacementGroupRefusal::NameTaken`).

`CreateSdiSource` declares the error that refusal would use and never constructs it — `smithy_query(query="operation", operation="CreateSdiSource")` lists `{"shape": "ConflictException", "errorKind": "client", "httpError": 409}`, and `grep` for `ConflictException` in `src/ops/create_sdi_source.rs` and `src/ops/update_sdi_source.rs` finds only the enum variant declaration.

Test coverage: `tests/all/sdi_source_lifecycle.rs` covers the create, the partial update, idempotency, the `Inputs` derivation and the delete, and every source it creates carries a distinct name (lines 150/157/165 `format!("camera-{type}")` / `"curling-cameraA"`, lines 333/337 `"first"`/`"second"`, line 384 `format!("camera-{index}")`, line 446 `format!("other-{other}")`). No test asserts that a duplicate name is accepted, so nothing pins the current behaviour.

## Steps to Reproduce

1. POST /prod/sdiSources with {"name":"curling-cameraA","type":"SINGLE"} — 200, note the id
2. POST /prod/sdiSources with {"name":"curling-cameraA","type":"QUAD","mode":"QUADRANT"} (no requestId, or a different one) — the emulator answers 200 and mints a SECOND source under the same name; expected the 409 ConflictException this operation models, which CreateNetwork/CreateCluster/CreateSignalMap all answer for the identically documented uniqueness rule
3. GET /prod/sdiSources — both sources are listed with name "curling-cameraA"
4. PUT /prod/sdiSources/{secondId} with {"name":"curling-cameraA"} after creating a differently named second source — likewise answers 200 and stores the duplicate name, although UpdateSdiSourceRequest$Name repeats "Specify a name that is unique in the AWS account"
