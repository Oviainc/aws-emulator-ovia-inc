Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 75491f6dad17b9da11cd051dade27675220440ff

# CreateFlow stores a SourceFailoverConfig whose PrimarySource names no source of the flow, the request UpdateFlow refuses

_Filed: 2026-10-27T07:01:03Z_
_Operation: CreateFlow | Class: validation_

> _Note: this issue was identified against the code base at commit `75491f6dad17b9da11cd051dade27675220440ff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateFlow copies the request's `SourceFailoverConfig` straight onto the stored flow (only filling in the service-assigned recovery window) without resolving `SourcePriority.PrimarySource` against the sources the request creates. UpdateFlow implements exactly that resolution and refuses a `PrimarySource` the flow does not have with `BadRequestException`. So a create can leave a flow holding a failover configuration that names a source it has not got — a stored state the update path would never permit, and which UpdateFlow will keep refusing afterwards for any later failover change.

This is the one request-local reference CreateFlow does not resolve: the same handler already resolves a source's / output's media-stream names and VPC-interface names against the request's OWN `MediaStreams` / `VpcInterfaces` lists, for the same reason ("a flow being created holds exactly what THIS request creates").

## Evidence

UpdateFlow owns the rule — services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/update_flow.rs:372-384:
```
        // `SourcePriority$PrimarySource` is "the name of the source you choose as the
        // primary source for this flow", so it has to name a source the flow has.
        if let Some(primary) = requested_failover
            .and_then(|config| config.source_priority.as_ref())
            .and_then(|priority| priority.primary_source.as_deref())
        {
            if !flow.sources.iter().any(|source| source.name == primary) {
                return Err(bad_request(format!(
                    "Source {primary:?} is not a source of this flow, so it cannot be its \
                     primary source."
                )));
            }
        }
```
CreateFlow stores the config unchecked — src/ops/create_flow.rs:606-609:
```
            source_failover_config: input
                .source_failover_config
                .clone()
                .map(crate::state::resolved_failover_config),
```
and `resolved_failover_config` only fills the recovery window (src/state.rs:1272-1281: `FailoverConfig { recovery_window: Some(...unwrap_or(ASSIGNED_RECOVERY_WINDOW_MS)), ..config }`). `primary_source` is read nowhere else in the crate: the only hit outside the shared types is update_flow.rs:376.

The same handler DOES resolve its other request-local references — src/ops/create_flow.rs:374-379:
```
        // A source and an output reference the flow's own media streams and VPC interfaces
        // BY NAME, and a flow being created holds exactly what THIS request creates — so
        // each such name is resolved against the request's own `MediaStreams` /
        // `VpcInterfaces` ... Checked before anything is stored
```
Test coverage: the UpdateFlow refusal is pinned by tests/all/update_flow.rs:481 (`case: "a primary source the flow does not have"`); every CreateFlow failover fixture uses real source names (tests/all/flow_projection.rs:766-769 creates sources named "primary"/"backup"), so no test pins the create side accepting a bogus name.

## Steps to Reproduce

1. POST /v1/flows with `{"name":"f","sources":[{"name":"primary","protocol":"rtp","whitelistCidr":"10.1.0.0/16"}],"sourceFailoverConfig":{"state":"ENABLED","sourcePriority":{"primarySource":"not-a-source"}}}`.
2. Observe HTTP 201 and the stored flow reporting `sourceFailoverConfig.sourcePriority.primarySource == "not-a-source"` (DescribeFlow confirms it).
3. Send the equivalent through the update path instead — PUT /v1/flows/{FlowArn} with `{"sourceFailoverConfig":{"sourcePriority":{"primarySource":"not-a-source"}}}` — and observe 400 BadRequestException "Source \"not-a-source\" is not a source of this flow, so it cannot be its primary source." for the state the create already stored.
