Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 355761efac79959eac560a8efce6608069de81f3

# TakeRouterInput associates a REGIONAL router input with a router output in another Region

_Filed: 2026-10-28T21:23:31Z_
_Operation: TakeRouterInput | Class: validation_

> _Note: this issue was identified against the code base at commit `355761efac79959eac560a8efce6608069de81f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

TakeRouterInput records the association between the router input the request names and the router output its `@httpLabel` addresses. It resolves both records, refuses an input whose delete is accepted and an output with a state change in flight — and then writes the association without ever comparing the two records' `RoutingScope` or `RegionName`.

The model states the rule on both sides of the association: a REGIONAL input "connects only to outputs in same Region" and a REGIONAL output "can only take inputs from same Region". The emulator stores exactly the two members that rule is stated over (`routing_scope` and `region_name`, on both record types, and ListRouterInputs/ListRouterOutputs already filter on them), so a REGIONAL router input created in us-east-1 is happily routed to a router output whose `regionName` is eu-west-1, and GetRouterInput then reports `RoutedOutputs: 1` for a cross-Region association the model says cannot exist. `BadRequestException` ("thrown if the request contains a semantic error") and `ConflictException` are both modelled on this operation, so the refusal has a modelled channel.

## Evidence

Model, `CreateRouterInputRequest$RoutingScope` (smithy_query query="shape" shape="CreateRouterInputRequest"): "Specifies whether the router input can be assigned to outputs in different Regions. REGIONAL (default) - connects only to outputs in same Region. GLOBAL - connects to outputs in any Region."

Model, `CreateRouterOutputRequest$RoutingScope` (smithy_query query="doc" shape="CreateRouterOutputRequest" member="RoutingScope"): "Specifies whether the router output can take inputs that are in different Regions. REGIONAL (default) - can only take inputs from same Region. GLOBAL - can take inputs from any Region."

The handler's whole verdict set — services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/take_router_input.rs:203-290 — resolves existence and lifecycle only; neither `routing_scope` nor `region_name` is read anywhere in the file:
```
        if !store.router_outputs.contains_key(&input.router_output_arn) {
...
                if router_input.is_deleting() {
...
        if router_output.is_transitioning() {
...
        if router_output.routed_input_arn != requested {
            router_output.begin_route(requested, now);
```
The two members the rule is stated over ARE stored and are reported on both records — src/state.rs:3876-3877 (`StoredRouterInput`) and src/state.rs:4347-4349 (`StoredRouterOutput`):
```
    pub routing_scope: String,
    pub region_name: String,
```
and the crate already selects on them in src/ops/list_router_inputs.rs:166-168 (`RouterInputFilter::RoutingScopes`).

Test coverage: nothing asserts the rule, and the matrix test pins the current acceptance without giving a reason about Regions — tests/all/router_output_take_input.rs:199-220 creates an output with `"routingScope": "GLOBAL", "regionName": "eu-west-1"`, every input is created `"routingScope": "REGIONAL"` with no `regionName` (tests/all/router_input_restart.rs:303-311, REGION = us-east-1), and tests/all/router_output_take_input.rs:370-408 crosses input row 2 with that eu-west-1 output and asserts `(202, None)`.

## Steps to Reproduce

1. POST /v1/routerNetworkInterface to mint an interface ARN (Region us-east-1).
2. POST /v1/routerInput with {"name":"in-1","configuration":{"Standard":{"networkInterfaceArn":IFACE,"protocolConfiguration":{"Rtp":{"port":5000}}}},"maximumBitrate":20000000,"routingScope":"REGIONAL","tier":"INPUT_20"} — 201; the input's regionName is us-east-1.
3. POST /v1/routerOutput with {"name":"out-1",...,"routingScope":"REGIONAL","regionName":"eu-west-1","availabilityZone":"eu-west-1b"} — 201.
4. Wait out the transition span so both records are settled.
5. PUT /v1/routerOutput/takeRouterInput/{RouterOutputArn} with {"routerInputArn":<the us-east-1 REGIONAL input>} — observe 202 and routedState ROUTING, although both sides are REGIONAL and live in different Regions ("REGIONAL (default) - connects only to outputs in same Region").
6. GET /v1/routerInput/{RouterInputArn} — observe routedOutputs == 1 for the cross-Region association.
