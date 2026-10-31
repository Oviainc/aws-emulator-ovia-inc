Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 772fb5ab5b7496e2df886721dbd75d9882bc460e

# An update that asks for no change still moves the resource to UPDATING, the no-op rule the crate's two attachment updaters state and implement

_Filed: 2026-10-28T21:37:00Z_
_Operation: UpdateCoreNetwork | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `772fb5ab5b7496e2df886721dbd75d9882bc460e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateCoreNetwork` writes `record.updating_until = Some(now + SETTLE_DELAY)` unconditionally — after the only writable member (`Description`) has been applied *or skipped*. So a `PATCH /core-networks/{id}` with an empty body (every body member of this operation is optional), or one carrying the `Description` the record already holds, stores a transition the service owes for a reconfiguration that never happened: the answer reports `State: UPDATING`, and so does every later read until the settle span elapses.

The crate states the opposite rule and implements it on the two attachment updaters. `UpdateVpcAttachment` short-circuits a request that asks for the state the attachment is already in — explicitly "it does not move the attachment to `UPDATING`, does not advance `UpdatedAt` and writes nothing, because there is no change for the service to apply" — and `UpdateDirectConnectGatewayAttachment` does the same twice over (for an absent `EdgeLocations` and for a list equal to the stored one).

The same unconditional stamp is in the other four scalar updaters of this crate — `UpdateGlobalNetwork`, `UpdateSite`, `UpdateLink`, `UpdateDevice`, `UpdateConnection` — all of which carry the identical "AWS answers the reconfigured <resource> in the non-terminal `UPDATING` state" comment while never establishing that the resource WAS reconfigured.

For a core network the damage is not confined to the reported state: `ListCoreNetworkRoutingInformation` decides whether the core network's policy is in force by comparing `state_at(now)` against `STATE_AVAILABLE`, so a no-op `UpdateCoreNetwork` makes every segment of that core network unreadable (400 `ValidationException`, "no such segment") for the whole settle span.

## Evidence

The unconditional write — src/ops/update_core_network.rs:162-178:
```
        // Only the member the request supplied is changed: an absent `Description` leaves
        // the stored one alone, and a supplied one replaces it — the empty string
        // included, which is how this service's updates document removing information. The
        ...
        if input.description.is_some() {
            record.description = input.description;
        }
        // An update is not applied instantly: AWS answers the reconfigured core network in
        // the non-terminal `UPDATING` state, so the clock owes the `UPDATING` ->
        // `AVAILABLE` transition and a caller reading straight after this call still
        // observes it.
        let now = ::std::time::SystemTime::now();
        record.updating_until = Some(now + crate::state::SETTLE_DELAY);
```
The rule the crate states, on the sibling updater — src/ops/update_vpc_attachment.rs:322-331:
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
and the same rule twice in src/ops/update_direct_connect_gateway_attachment.rs:210-218 and :236-245:
```
        // `EdgeLocations` is the only member this operation can change, and it is OPTIONAL:
        // a request that carries none asks for no change, so the attachment is reported as
        // it stands and nothing is written.
        let Some(requested) = input.edge_locations else {
            return Ok(UpdateDirectConnectGatewayAttachmentOutput { ... });
        };
...
        // A request that asks for the list the attachment already spans changes nothing: it
        // does not move the attachment to `UPDATING`, does not advance `UpdatedAt` and
        // writes nothing, because there is no change for the service to apply
```
What the stamp makes the reader report — src/state.rs:2088-2089 (`CoreNetworkRecord::state_at`):
```
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if self.updating_until.is_some_and(|due| now < due) {
```
The knock-on for a core network — src/ops/list_core_network_routing_information.rs:310-315:
```
        let nothing_in_force = crate::state::PolicyTopology::default();
        let topology = if core_network.state_at(now) == crate::state::STATE_AVAILABLE {
            &core_network.topology
        } else {
            &nothing_in_force
        };
```
The same unconditional stamp in the sibling scalar updaters: src/ops/update_global_network.rs:154-163, src/ops/update_site.rs:179-190, src/ops/update_link.rs:216-233, src/ops/update_device.rs:314-343, src/ops/update_connection.rs:257-272.

Test coverage: tests/all/resource_state_machine.rs:313-361 (`an_update_answers_the_resource_updating_and_it_settles_back_to_available`) and tests/all/update_core_network.rs:224 only ever PATCH a body that carries a CHANGED value, so no test pins the no-change case — the no-op short-circuit the two attachment updaters already have leaves every existing assertion untouched (tests/all/direct_connect_gateway_attachment_lifecycle.rs:537 is the row that pins it for the attachment: `json!(if case.changes { UPDATING } else { AVAILABLE })`).

## Steps to Reproduce

1. CreateGlobalNetwork, then CreateCoreNetwork with a policy document (the create answers the core network already AVAILABLE)
2. GET /core-networks/{coreNetworkId} → State = AVAILABLE
3. PATCH /core-networks/{coreNetworkId} with body {} (no Description at all) → 200 and State = UPDATING, although nothing about the core network changed
4. GET /core-networks/{coreNetworkId} immediately after → State = UPDATING: the no-op update WROTE the transition (src/ops/update_core_network.rs:177)
5. GET /core-networks/{coreNetworkId}/routing-information?segmentName=<a segment the live policy declares>&edgeLocation=<an edge it configures> → 400 ValidationException "no such segment", because src/ops/list_core_network_routing_information.rs:311 now reads the core network as not AVAILABLE
6. Compare: PATCH /vpc-attachments/{attachmentId} asking for the subnet list it already holds → 200 with State = AVAILABLE and nothing written (src/ops/update_vpc_attachment.rs:325-331)
