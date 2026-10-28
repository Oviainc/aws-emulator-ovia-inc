Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5f1c460e5544105d602e680f733bf81495110ec8

# RemoveFlowOutput leaves router inputs whose configuration names a flow output the store no longer holds

_Filed: 2026-10-28T16:05:10Z_
_Operation: RemoveFlowOutput | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5f1c460e5544105d602e680f733bf81495110ec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

RemoveFlowOutput deletes the output from the flow without regard for the router inputs whose stored `MEDIACONNECT FLOW` configuration names it in `FlowOutputArn`. After the removal those records hold a `FlowOutputArn` that resolves to nothing — the exact state CreateRouterInput and UpdateRouterInput both refuse to store, through `unknown_flow_output_rejection`, because "a `FlowOutputArn` names an OUTPUT of a flow". GetRouterInput / BatchGetRouterInput / ListRouterInputs then keep reporting a configuration pointing at an output that no longer exists, and re-sending the same configuration through UpdateRouterInput is refused 400.

The crate already keeps this kind of referential integrity for the other router reference it holds: when a router input is removed, every router output's recorded route to it is cleared in the same settle pass, with the stated reason that an input the clock took away "carries content into nothing". The flow-output reference gets no such treatment — the handler's own comment asserts the removal "moves nothing else about the flow", which is true of the flow and false of the router inputs that point into it.

## Evidence

The removal writes only the flow — services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/remove_flow_output.rs:213-230:
```
        let Some(position) = flow
            .outputs
            .iter()
            .position(|output| output.output_arn == input.output_arn)
        else {
            return Err(not_found(&input.output_arn));
        };
        let removed = flow.outputs.remove(position);
        ...
        // Removing an output moves nothing else about the flow: its status is left as it
        // was, because no clock owes a change for a removed output.
```
The reference the write paths refuse to take on trust — src/state.rs:5617-5646 and :5747-5752:
```
    /// Why `arn` names no flow OUTPUT this account holds, or `None` when the store holds one
    /// for it.
    pub fn unknown_flow_output_rejection(
...
                let output_arn = flow.flow_output_arn.as_deref()?;
                self.unknown_flow_output_rejection(
                    "Configuration MediaConnectFlow FlowOutputArn",
                    flow_arn,
                    output_arn,
                )
```
reached from src/ops/create_router_input.rs:321 and src/ops/update_router_input.rs:322:
```
        if let Some(rejection) = store.router_input_configuration_rejection(&input.configuration) {
```
The sibling reference IS repaired, in the same store — src/state.rs:5432-5446:
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
Test coverage: tests/all/remove_flow_output.rs covers the flow side only; tests/all/router_input_configuration_targets.rs covers the create/update-time resolution. Nothing pins what happens to a router input after its target output is removed.

## Steps to Reproduce

1. POST /v1/flows with one output — 201; keep FlowArn and OutputArn.
2. POST /v1/router/inputs with a `mediaConnectFlow` configuration naming {"flowArn":FlowArn,"flowOutputArn":OutputArn} — 201 (the create resolves both references).
3. DELETE /v1/flows/{FlowArn}/outputs/{OutputArn} — observe 202 and the output gone from DescribeFlow.
4. GET /v1/router/inputs/{RouterInputArn} — observe the input still reporting `configuration.mediaConnectFlow.flowOutputArn == OutputArn`, an ARN the store no longer holds.
5. PUT /v1/router/inputs/{RouterInputArn} re-stating the same configuration — observe 400 BadRequestException "Configuration MediaConnectFlow FlowOutputArn … does not name a flow output this account holds." for the state step 3 produced.
