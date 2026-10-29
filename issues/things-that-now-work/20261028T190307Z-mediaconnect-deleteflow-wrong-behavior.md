Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7cfd583752b909e6f589468ac42594db7887f86f

# DeleteFlow drops a flow that router inputs and outputs still name, leaving configurations the crate's own write paths refuse to store

_Filed: 2026-10-28T19:03:07Z_
_Operation: DeleteFlow | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7cfd583752b909e6f589468ac42594db7887f86f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteFlow removes the whole flow record (with its sources and outputs) without regard for the router inputs and router outputs whose STORED configuration names that flow — `MediaConnectFlowRouterInputConfiguration` names `FlowArn` / `FlowOutputArn`, and the router-output counterpart names `FlowArn` / `FlowSourceArn`. After the delete those records hold references that resolve to nothing: GetRouterInput / BatchGetRouterInput / ListRouterInputs (and the output equivalents) keep reporting a configuration pointing at a flow the store no longer holds, and re-sending that very configuration through UpdateRouterInput / UpdateRouterOutput is refused 400, because `router_input_configuration_rejection` / `router_output_configuration_rejection` resolve exactly those members against the store.

The crate keeps this referential integrity for its OTHER stored cross-reference: DeleteRouterNetworkInterface refuses to take an interface away while a router input or output still names it, with the stated reason that doing so "would therefore leave records holding a reference nothing resolves, which no later call could even restate". A flow is referenced in the same way and gets no such treatment — DeleteFlow resolves nothing but the flow's own status precondition.

## Evidence

The delete writes only the flow map — services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/delete_flow.rs:197-205:
```
        let flow_arn = flow.flow_arn.clone();
        store.flows.remove(&input.flow_arn);

        Ok(DeleteFlowOutput {
            flow_arn: Some(flow_arn),
```
The reference the write paths refuse to take on trust — src/state.rs:5689-5698 and :5774-5792:
```
    /// Why `arn` names no flow this account holds, or `None` when the store holds one for
    /// it. `member` is the request member that named it, for the refusal detail.
    pub fn unknown_flow_rejection(&self, member: &str, arn: &str) -> Option<String> {
        if self.flows.contains_key(arn) {
...
                    if let Some(rejection) =
                        self.unknown_flow_rejection("Configuration MediaConnectFlow FlowArn", arn)
```
reached from src/ops/create_router_input.rs:321 and src/ops/update_router_input.rs:322 (and the output pair at create_router_output.rs:305 / update_router_output.rs:302):
```
        if let Some(rejection) = store.router_input_configuration_rejection(&input.configuration) {
```
The sibling delete that DOES protect the same kind of reference — src/state.rs:5563-5591:
```
    /// Why the router network interface `interface_arn` cannot be removed from the store
    /// yet, or `None` when no router input and no router output is associated with it.
...
    /// Taking the interface away
    /// while an input or an output still names it would therefore leave records holding a
    /// reference nothing resolves, which no later call could even restate;
    pub fn associated_router_resource_rejection(&self, interface_arn: &str) -> Option<String> {
```
called from src/ops/delete_router_network_interface.rs:244-246:
```
        if let Some(detail) = store.associated_router_resource_rejection(&input.arn) {
            return Err(still_associated(detail));
        }
```
Test coverage: tests/all/router_input_configuration_targets.rs:517-579 deliberately deletes the router input BEFORE deleting the flow and then only asserts that a NEW configuration naming the gone flow is refused ("a configuration naming it is refused once it is gone"); nothing pins what happens to a router input that still names a flow when the flow is deleted.

## Steps to Reproduce

1. POST /v1/flows with one output — 201; keep FlowArn and the output's OutputArn.
2. POST /v1/routerInput with a `mediaConnectFlow` configuration naming {"flowArn":FlowArn,"flowOutputArn":OutputArn} — 201 (the create resolves both references through router_input_configuration_rejection).
3. DELETE /v1/flows/{FlowArn} — observe 202 and the flow gone from DescribeFlow/ListFlows.
4. GET /v1/routerInput/{RouterInputArn} — observe the input still reporting configuration.mediaConnectFlow.flowArn == FlowArn, an ARN the store no longer holds.
5. PUT /v1/routerInput/{RouterInputArn} re-stating the same configuration — observe 400 BadRequestException "Configuration MediaConnectFlow FlowArn ... does not name a flow this account holds." for the state step 3 produced.
