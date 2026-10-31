Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f3551fb51efeca87d6f65db01e533e63f3785794

# An update that asks for no change still writes UPDATING onto the resource, against the no-op rule the crate's two attachment updaters state and implement

_Filed: 2026-10-29T02:12:32Z_
_Operation: UpdateGlobalNetwork | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f3551fb51efeca87d6f65db01e533e63f3785794`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateGlobalNetwork` writes `record.updating_until = Some(now + SETTLE_DELAY)` unconditionally — even for a request that carries no member at all (every member of `UpdateGlobalNetworkRequest` except the `@httpLabel` id is optional, so `PATCH /global-networks/{id}` with body `{}` is a legal request), and even for one whose `Description` is the value already stored. The call therefore WRITES the store and flips the reported `State` to `UPDATING` for a request that changed nothing, which any reader (`DescribeGlobalNetworks`) observes until the settle span elapses.

The crate states and implements the opposite rule for the two attachment updaters: a change that asks for what is already stored is no change, so it does not move the resource to `UPDATING`, does not advance `UpdatedAt`, and writes nothing. The same unconditional write is in four sibling handlers of this crate (`UpdateSite`, `UpdateLink`, `UpdateDevice`, `UpdateConnection` — see evidence), so the fix belongs in all five; `UpdateCoreNetwork` is already covered by an existing open issue and is excluded here.

A second, sharper consequence of the unconditional write: `GlobalNetworkRecord::state_at` gives `updating_until` precedence over `available_at`, so a no-op update issued against a global network whose CREATE has not settled yet silently replaces the reported `PENDING` with `UPDATING` — a state transition no request asked for.

## Evidence

The handler, unconditionally (services/service-com-amazonaws-networkmanager-networkmanager/src/ops/update_global_network.rs:152-164):
```
        // Only the members the request supplied are changed; an absent `Description`
        // leaves the stored one alone. Ids, ARN, tags and `CreatedAt` are never rewritten.
        if input.description.is_some() {
            record.description = input.description;
        }
        // An update is not applied instantly: AWS answers the reconfigured global network
        // in the non-terminal `UPDATING` state, ...
        let now = ::std::time::SystemTime::now();
        record.updating_until = Some(now + crate::state::SETTLE_DELAY);
        let global_network = record.to_shape(now);
```
Note the `if input.description.is_some()` guard: when it is false NOTHING about the resource changed, yet `updating_until` is still written.

The rule the crate's own attachment updaters state and implement (src/ops/update_vpc_attachment.rs:322-331):
```
        // A request that asks for the state the attachment is already in changes nothing:
        // it does not move the attachment to `UPDATING`, does not advance `UpdatedAt` and
        // writes nothing, because there is no change for the service to apply.
        if subnet_arns == held_subnets
            && crate::state::same_vpc_options(options.as_ref(), held_options.as_ref())
        {
            return Ok(UpdateVpcAttachmentOutput {
                vpc_attachment: record.to_vpc_attachment(now),
            });
        }
```
and src/ops/update_direct_connect_gateway_attachment.rs:210-218:
```
        // `EdgeLocations` is the only member this operation can change, and it is OPTIONAL:
        // a request that carries none asks for no change, so the attachment is reported as
        // it stands and nothing is written.
        let Some(requested) = input.edge_locations else {
            return Ok(UpdateDirectConnectGatewayAttachmentOutput { ... });
        };
```
and the same rule in the state module (src/state.rs:2788-2791): "Answers `false`, writing NOTHING, for an empty list, ... and for a list the record already holds — a change that asks for what is stored is no change, so no transition is owed on it."

Why the write is observable (src/state.rs:1122-1130):
```
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if self.updating_until.is_some_and(|due| now < due) {
            STATE_UPDATING
        } else if now < self.available_at {
            STATE_PENDING
```

The identical unconditional write in this crate's sibling updaters: src/ops/update_site.rs:190, src/ops/update_link.rs:233, src/ops/update_device.rs:343, src/ops/update_connection.rs:272.

Test coverage: tests/all/resource_state_machine.rs:197-291 exercises these five updates ONLY with bodies that really change a member ("a global network, one member" -> `{"Description": "updated-global-network"}`), so no existing test pins the no-change case and a fix changes no pinned response.

## Steps to Reproduce

1. CreateGlobalNetwork with `{"Description": "d"}` and note the returned GlobalNetworkId
2. Wait for the create to settle, then DescribeGlobalNetworks: State is AVAILABLE
3. Call UpdateGlobalNetwork with an empty body: `PATCH /global-networks/{GlobalNetworkId}` `{}` (or with `{"Description": "d"}`, the value already stored)
4. Observe the 200 answer reports `State: UPDATING`, and an immediate DescribeGlobalNetworks also reports UPDATING — although nothing about the global network changed. Expected (per the rule update_vpc_attachment.rs:322-331 states): the resource is reported as it stands and nothing is written
5. Repeat against a global network whose create has not settled: the reported state flips from PENDING to UPDATING
6. The same reproduction applies to UpdateSite, UpdateLink, UpdateDevice and UpdateConnection with empty bodies
