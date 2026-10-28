Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a19f71fb9b9d334f8efc64d442ea8394bd20139d

# Deleting a router network interface leaves router inputs and outputs whose configuration names an interface the store no longer holds

_Filed: 2026-10-28T15:07:30Z_
_Operation: DeleteRouterNetworkInterface | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a19f71fb9b9d334f8efc64d442ea8394bd20139d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteRouterNetworkInterface` accepts the delete of any interface the store holds and lets the clock remove the record, without regard for the router inputs and router outputs whose stored configuration names it in `NetworkInterfaceArn`. Once the removal settles, those records hold a `NetworkInterfaceArn` that resolves to nothing — the exact state CreateRouterInput / UpdateRouterInput / CreateRouterOutput / UpdateRouterOutput all refuse, through `unknown_router_network_interface_rejection`, as a reference "taken on trust".

The crate already maintains this kind of referential integrity in the same settle pass for the OTHER router reference it holds: when a router input is removed, every output's recorded route to it is cleared, with the stated reason that an input the clock took away "carries content into nothing". The interface reference gets no such treatment, even though the store already knows who holds it (`associated_router_input_count` / `associated_router_output_count`, which `RouterNetworkInterface$AssociatedInputCount` / `AssociatedOutputCount` are computed from).

## Evidence

The delete writes only the interface's own lifecycle — services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/delete_router_network_interface.rs:211-220:
```
        let Some(interface) = store.router_network_interfaces.get_mut(&input.arn) else {
            return Err(not_found(&input.arn));
        };
        // An interface already on its way out keeps the deletion it has ...
        if !interface.is_deleting() {
            interface.begin_deletion(now);
        }
```
The reference the write paths refuse to take on trust — src/state.rs:5567-5583:
```
    /// A `RouterNetworkInterfaceArn` names a resource THIS service mints
    /// ([`is_mintable_router_network_interface_arn`]) and holds in
    /// [`Self::router_network_interfaces`], so a request that points at one is resolved
    /// against the store rather than taken on trust: an ARN of the right shape for an
    /// interface the account never created names nothing.
    pub fn unknown_router_network_interface_rejection(
```
reached from create_router_input.rs:321, update_router_input.rs:322, create_router_output.rs:305, update_router_output.rs:302 (via `router_input_configuration_rejection` / `router_output_configuration_rejection`).

The sibling reference IS repaired in the same settle pass — src/state.rs:5432-5446:
```
        // A route names a router input the store HOLDS: an input the clock has just taken
        // away carries content into nothing, so the association TakeRouterInput recorded
        // goes with it and the outputs that named it are UNROUTED again. Owed at the same
        // instant as the removal above, whether or not anybody called.
        let router_inputs = &self.router_inputs;
        for router_output in self.router_outputs.values_mut() {
            if router_output
                .routed_input_arn
                ...
                router_output.routed_input_arn = None;
```
and the store already counts the holders — src/state.rs:5548-5560 (`associated_router_output_count`, "How many router outputs egress from the router network interface `interface_arn` — `RouterNetworkInterface$AssociatedOutputCount`").

## Steps to Reproduce

1. POST /v1/router/network-interfaces to create interface I.
2. POST /v1/router/inputs with a STANDARD configuration whose `networkInterfaceArn` is I — 201 (the create resolves I).
3. DELETE /v1/router/network-interfaces/{I} — observe 202 and, one transition span later, GET /v1/router/network-interfaces/{I} answering 404 (the record is gone).
4. GET /v1/router/inputs/{InputArn} — observe the input still reporting `configuration.standard.networkInterfaceArn == I`, an ARN the store no longer holds.
5. PUT /v1/router/inputs/{InputArn} re-stating the same configuration — observe 400 BadRequestException "... does not name a router network interface this account holds." for the state step 3 produced.
