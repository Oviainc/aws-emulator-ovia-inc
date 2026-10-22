Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediaconnect.delete-active-flow-rejected mediaconnect.create-gateway-duplicate-name-allowed
Issue-Shape-Id: com.amazonaws.mediaconnect#MediaConnect
Issue-Key: mediaconnect.delete-active-flow-rejected|_client_error#8|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: mediaconnect|DescribeGateway|NotFoundException
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# MediaConnect: DeleteFlow accepts an ACTIVE flow instead of rejecting it, and DeleteGateway hard-deletes instead of settling in GatewayState DELETED

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteFlow

## Description
Two distinct root causes found replaying the behavioral suite against the MediaConnect emulator. Both belong to service `mediaconnect`. Both must be closed before this issue is submitted. Neither is marked ALREADY FILED nor OWNED by an existing issue: the two open MediaConnect issues (20261022T132818Z-mediaconnect-router-interface-union-payload-rejected-no-flow.md, about the router-interface union, EventBridge eventing, three scaffold stubs, native UDP ingest and the error-message envelope, and 20261022T170848Z-mediaconnect-flow-projection-omits-service-assigned-source-output.md, about missing flow-projection members) name neither DeleteFlow's state gate nor DeleteGateway's delete semantics.

=== Cause 1 of 2: DeleteFlow does not gate on flow status — an ACTIVE flow is deleted instead of refused ===
AWS: Smithy model `services/service-com-amazonaws-mediaconnect-mediaconnect/smithy-model/com-amazonaws-mediaconnect-mediaconnect.json` — DeleteFlow (DELETE /v1/flows/{FlowArn}, success 202, output {flowArn, status}) models BadRequestException (HTTP 400), documented as "thrown if the request contains a semantic error. The precise meaning depends on the API, and is documented in the error message". The operation's own documentation states the precondition explicitly: "Deletes a flow. Before you can delete a flow, you must stop the flow." Recorded behavior: DeleteFlow against a flow whose Status is ACTIVE is refused with HTTP 400, error code `BadRequestException`, message `Bad request: Flow must be in STANDBY in order for DELETE to be invoked.` (that message is built from the service's documented envelope and the named precondition, so the status, the code and the `Bad request: ` envelope are the requirement; the prose after the envelope is an example). The flow is left fully intact — still describable, still ACTIVE. After StopFlow settles the flow in STANDBY, the identical DeleteFlow succeeds with 202 and Status DELETING.
Emulator: there is no status gate at all. `services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/delete_flow.rs` `handle` goes straight to `store.flows.remove(&input.flow_arn)` and returns HTTP 202 with `Status: DELETING` for a flow in any status, including ACTIVE. The replay shows the delete of an ACTIVE flow SUCCEEDING with 202 and `{'FlowArn': 'arn:aws:mediaconnect:<region>:<account>:flow:<id>:<name>', 'Status': 'DELETING'}` where a 400 BadRequestException was required.
Why: this is an over-permissive destructive path. A client (or an operator script, or an IaC destroy loop) that relies on MediaConnect refusing to delete a running flow loses a live, ingesting flow on a call AWS would have rejected; the stop-then-delete ordering that every MediaConnect teardown is written around is unenforced, and error-handling branches keyed on BadRequestException never run. It also means the emulator cannot be used to test that ordering at all.
Fix: the DeleteFlow handler's validation — `src/ops/delete_flow.rs` must look the stored flow up, and when its status is not STANDBY (ACTIVE/STARTING/STOPPING/UPDATING) return the modeled BadRequestException instead of removing the record; only a STANDBY flow proceeds to removal. The removal path itself and its 202/DELETING response are already correct.
Traces: mediaconnect.delete-active-flow-rejected.

=== Cause 2 of 2: DeleteGateway hard-deletes the gateway, so DescribeGateway 404s instead of reporting GatewayState DELETED, and a repeated DeleteGateway raises ===
AWS: Smithy model — DeleteGateway (DELETE /v1/gateways/{GatewayArn}, success 200, output {gatewayArn}) is marked `@idempotent`; DescribeGateway (GET /v1/gateways/{GatewayArn}, success 200) returns a `Gateway` whose `GatewayState` member targets the enum `GatewayState` with values CREATING | ACTIVE | UPDATING | ERROR | DELETING | DELETED. The presence of a terminal `DELETED` enum value is the model's own statement that a deleted gateway remains a readable resource. Recorded behavior: DeleteGateway is asynchronous and a SOFT delete — the gateway moves DELETING -> DELETED and stays describable in that state; DescribeGateway keeps answering HTTP 200 with `Gateway.GatewayState == "DELETED"` and never NotFoundException, and repeating DeleteGateway against the now-DELETED gateway succeeds again (consistent with @idempotent) rather than raising. Deleting one gateway leaves an unrelated gateway sharing the same Name untouched.
Emulator: `services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/delete_gateway.rs` `handle` does `store.gateways.remove(&input.gateway_arn)`, evicting the record outright; the module doc even states the intent ("afterwards DescribeGateway answers 404 and ListGateways no longer lists it"). No DELETING/DELETED state is ever stored or projected. Consequently the first DescribeGateway after a DeleteGateway fails with HTTP 404 `NotFoundException` (`Not found: arn:aws:mediaconnect:<region>:<account>:gateway:<id>:<name>`), and a second DeleteGateway on the same ARN would likewise raise NotFoundException instead of succeeding idempotently.
Why: a client cannot observe a gateway's deletion lifecycle at all. Code that polls DescribeGateway for GatewayState to reach DELETED — the normal way to confirm an async MediaConnect gateway teardown finished — gets a 404 it reads as "this gateway never existed" or as a transient fault, and a retried/idempotent DeleteGateway (what SDK retries and apply-loops do for an @idempotent operation) turns into a hard client error. This also makes the surviving-sibling assertion untestable: the check could not even reach it.
Fix: gateway deletion state in `src/ops/delete_gateway.rs` plus the gateway record and its projection in `src/state.rs` — DeleteGateway must mark the stored gateway's state (DELETING, settling to DELETED) and keep the record readable rather than removing it from `store.gateways`; DescribeGateway must then project GatewayState DELETED with 200, and a repeat DeleteGateway on an already-DELETED gateway must succeed. Whether ListGateways continues to list a DELETED gateway should follow the same stored record.
Traces: mediaconnect.create-gateway-duplicate-name-allowed.

## Evidence
Replay report (simparity-behavioral-aws-v1): mediaconnect 6/28 passed (21%), 22 failed (9 mismatch, 13 error). All other failures were marked ALREADY FILED (unimplemented scaffold stubs: RemoveFlowOutput, AddFlowSources, CreateBridge, UpdateFlowEntitlement, UpdateFlowMediaStream, UpdateFlowOutput, RevokeFlowEntitlement) or OWNED by 20261022T132818Z-mediaconnect-router-interface-union-payload-rejected-no-flow.md; the two clusters below are the only net-new ones.

Cause 1 — [mismatch] mediaconnect.delete-active-flow-rejected:
  setup: create_flow(Name=..., Source={'Name':'source1','Protocol':'rtp','WhitelistCidr':'0.0.0.0/0'}); start_flow; poll DescribeFlow until Status == 'ACTIVE'
  request: mc.delete_flow(FlowArn=<the ACTIVE flow>)
  expected: HTTP 400, Error.Code 'BadRequestException', Error.Message 'Bad request: Flow must be in STANDBY in order for DELETE to be invoked.'; flow still describable and still ACTIVE
  actual:   HTTP 202 success — AssertionError: expected this call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 202, ...}, 'FlowArn': 'arn:aws:mediaconnect:us-west-2:000000000000:flow:1-00000001:sim2real-mc-flow-<id>', 'Status': 'DELETING'}
  code reference: services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/delete_flow.rs:162 — `store.flows.remove(&input.flow_arn)` with no status check.

Cause 2 — [error] mediaconnect.create-gateway-duplicate-name-allowed:
  setup: two create_gateway calls sharing one Name (distinct EgressCidrBlocks/Networks) both succeed with distinct GatewayArns and both appear in ListGateways; then delete_gateway(GatewayArn=<first>)
  request: mc.describe_gateway(GatewayArn=<first, just deleted>), polled for Gateway.GatewayState == 'DELETED'
  expected: HTTP 200 with Gateway.GatewayState settling to 'DELETED' (never NotFoundException), then a repeated delete_gateway on the same ARN succeeding, then describe_gateway(<second>) showing it unaffected
  actual:   botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the DescribeGateway operation: Not found: arn:aws:mediaconnect:us-west-2:000000000000:gateway:1-00000001:sim2real-mc-gw-<id> — the gateway is gone, not DELETED, so the check never reaches the idempotent-redelete or surviving-sibling assertions
  code reference: services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/delete_gateway.rs:169 — `store.gateways.remove(&input.gateway_arn)`; module doc lines 5-7 state the 404-afterwards intent.

## Steps to Reproduce
Boot the emulator and run the behavioral suite (run_behavioral_suite), or drive a botocore mediaconnect client at the emulator endpoint:

Cause 1 (DeleteFlow status gate):
1. arn = mc.create_flow(Name='f1', Source={'Name':'source1','Protocol':'rtp','WhitelistCidr':'0.0.0.0/0'})['Flow']['FlowArn']
2. mc.start_flow(FlowArn=arn); poll mc.describe_flow(FlowArn=arn)['Flow']['Status'] until 'ACTIVE'
3. mc.delete_flow(FlowArn=arn) -> observe HTTP 202 with Status 'DELETING'. Expected: HTTP 400 BadRequestException with a 'Bad request: ' message naming the STANDBY precondition, and mc.describe_flow(FlowArn=arn) still returning Status 'ACTIVE' afterwards.
4. Positive control (must keep working): mc.stop_flow(FlowArn=arn); poll until 'STANDBY'; mc.delete_flow(FlowArn=arn) -> 202/DELETING, and DescribeFlow eventually 404 NotFoundException.

Cause 2 (DeleteGateway soft delete):
1. a = mc.create_gateway(Name='gw1', EgressCidrBlocks=['10.0.0.0/24'], Networks=[{'Name':'net1','CidrBlock':'172.28.0.0/16'}])['Gateway']['GatewayArn']
2. b = mc.create_gateway(Name='gw1', EgressCidrBlocks=['10.0.1.0/24'], Networks=[{'Name':'net2','CidrBlock':'172.29.0.0/16'}])['Gateway']['GatewayArn']  (duplicate Name is accepted; a != b)
3. mc.delete_gateway(GatewayArn=a)
4. mc.describe_gateway(GatewayArn=a) -> observe HTTP 404 NotFoundException 'Not found: <a>'. Expected: HTTP 200 with Gateway.GatewayState transitioning DELETING -> 'DELETED' and remaining describable.
5. mc.delete_gateway(GatewayArn=a) again -> expected to succeed (operation is @idempotent); today it raises NotFoundException.
6. mc.describe_gateway(GatewayArn=b) -> must still return GatewayArn b, Name 'gw1', GatewayState != 'DELETED'.
