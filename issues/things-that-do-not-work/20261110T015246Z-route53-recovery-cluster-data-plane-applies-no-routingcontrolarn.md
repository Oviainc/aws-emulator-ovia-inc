Issue-Source: trace-sweep-divergence
Issue-Trace: route53-recovery-cluster-routing-control-state-lifecycle
Issue-Step: task#17
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53-recovery-cluster-routing-control-state-lifecycle
Issue-Shape-Id: com.amazonaws.route53recoverycluster#ToggleCustomerAPI
Issue-Key: route53-recovery-cluster|GetRoutingControlState|4xx->4xx|$.__type
Issue-Key: route53-recovery-cluster|GetRoutingControlState|4xx->4xx|$.message
Issue-Key: route53-recovery-cluster|GetRoutingControlState|4xx->4xx|status
Issue-Key: route53-recovery-cluster|UpdateRoutingControlStates|4xx->4xx|status
Issue-Commit: 159c2db91a4e6757ae457414a0d5109b84d38779

# route53-recovery-cluster data plane applies no RoutingControlArn format gate: a non-ARN string is answered 404 ResourceNotFoundException instead of AWS's 400 ValidationException

> _Note: this issue was identified against the code base at commit `159c2db91a4e6757ae457414a0d5109b84d38779`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRoutingControlState, UpdateRoutingControlStates

## Description
Three net-new wire divergences in the route53-recovery-cluster (ToggleCustomerAPI, awsJson1_0) data plane, all from ONE missing decision point: the ARN-bearing data-plane operations hand the request's RoutingControlArn straight to the registry lookup, so a string that is NOT a well-formed routing-control ARN (but that still satisfies the member's modeled @length/@pattern bounds, e.g. 'not-an-arn') falls out of the lookup as a not-found resource instead of being refused as invalid input. AWS decides ARN well-formedness BEFORE existence, so it answers 400 ValidationException; the emulator answers 404 ResourceNotFoundException. A fixer must close all THREE causes below.

Note on already-open issues: 20261029T203927Z-route53-recovery-cluster-data-plane-3-net-new.md (Cause 3) covers the DIFFERENT path where the submitted ARN violates the member's ^[A-Za-z0-9:.\/_-]*$ pattern and the generic boundary validator's message escapes; 20261109T160848Z-route53-recovery-cluster-data-plane-is-unreachable-the.md covers the published ClusterEndpoints host form. Neither covers the path here, where the value passes the boundary validator and the handler then mis-classifies it as not-found. Neither of those issues carries these replay keys.

Cause 1 of 3: GetRoutingControlState with an ARN-shaped-but-invalid RoutingControlArn is answered 404 instead of 400 (fixture route53-recovery-cluster-routing-control-state-lifecycle, step task#16).
AWS: Smithy (com.amazonaws.route53recoverycluster#ToggleCustomerAPI): GetRoutingControlState.RoutingControlArn -> Arn, @required, @length 1..255, @pattern ^[A-Za-z0-9:.\/_-]*$; modeled errors include ValidationException (client, httpError 400) and ResourceNotFoundException (client, httpError 404). Recorded behavior: the request is refused with HTTP 400 — the invalid-input class, not the not-found class.
Emulator: HTTP 404 (ResourceNotFoundException). The replay reports 'status: expected 400, got 404' for this step and no further body keys.
Why: a client cannot distinguish 'you sent me garbage' from 'that routing control does not exist'. Retry/failover logic keyed on ResourceNotFoundException (re-resolve the control panel, re-list routing controls, fail over to another cluster endpoint) is entered for what is really a caller bug, and code that treats ValidationException as non-retryable and ResourceNotFoundException as retryable retries forever against the emulator.
Fix: the input validation of GetRoutingControlState's handler — an ARN-format check on RoutingControlArn that runs before the registry lookup and returns the modeled ValidationException (400).
Traces: route53-recovery-cluster-routing-control-state-lifecycle#task#16.

Cause 2 of 3: the same GetRoutingControlState refusal also carries the wrong error code and the wrong message (fixture route53-recovery-cluster-routing-control-state-lifecycle, step task#17 — the representative of this issue).
AWS: ValidationException at httpError 400 with @required message. Recorded response: HTTP 400, __type 'com.amazon.coral.validate#ValidationException', message 'Missing or invalid resource arn'. The ENTIRE message is fixed text: AWS does not echo the submitted value, the member name or the pattern, so there is no variable part to fill from the request or from state.
Emulator: HTTP 404, __type 'ResourceNotFoundException', message "Routing Control 'not-an-arn' not found" — an emulator-authored string that interpolates the submitted value ('not-an-arn' is the fixture's request value, not a requirement; the defect is that a value is interpolated at all and that the class and status are wrong).
Why: on the wire the client sees a different HTTP status, a different __type and a different message for an invalid-input request. Anything that matches on the modeled refusal — SDK error-code switches, log scrapers, user-facing error surfacing — mis-handles it, and the leaked request value makes the message vary per call where AWS's is constant.
Fix: the ValidationException branch of GetRoutingControlState's input validation (serializer picks up the class/status from the modeled error once the right error is returned). File/line citation: services/service-com-amazonaws-route53recoverycluster-togglecustomerapi/src/ops/get_routing_control_state.rs lines 182-197 go straight from the input to plane.routing_control(&input.routing_control_arn) and return ResourceNotFoundException built by crate::state::routing_control_not_found_message.
Traces: route53-recovery-cluster-routing-control-state-lifecycle#task#17.

Cause 3 of 3: UpdateRoutingControlStates has the same gap on each batch entry's RoutingControlArn (fixture route53-recovery-cluster-routing-control-state-lifecycle, step task#18).
AWS: Smithy: UpdateRoutingControlStates.UpdateRoutingControlStateEntries -> list UpdateRoutingControlStateEntries, @required; each entry's RoutingControlArn targets the same Arn shape; modeled errors include ValidationException (400) and ResourceNotFoundException (404). Recorded behavior: a batch naming a non-ARN RoutingControlArn is refused with HTTP 400 (the invalid-input class), and no routing control changes state.
Emulator: HTTP 404. The replay reports 'status: expected 400, got 404' for this step.
Why: batch callers get the retryable not-found class for a non-retryable caller error, exactly as in Cause 1, and cannot tell a malformed entry from a deleted routing control when deciding whether to re-resolve the cluster.
Fix: the per-entry input validation in UpdateRoutingControlStates' handler — the ARN-format check must run in the pre-commit validation pass over the entries, before plane.cluster_of(...) is consulted. File/line citation: services/service-com-amazonaws-route53recoverycluster-togglecustomerapi/src/ops/update_routing_control_states.rs lines 231-270 (the entry loop validates the state enum and duplicates, then goes directly to cluster_of and returns ResourceNotFoundException).
Traces: route53-recovery-cluster-routing-control-state-lifecycle#task#18.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture route53-recovery-cluster-routing-control-state-lifecycle (22 steps, 18 passed, 4 mismatched):

Cause 1 — step task#16, GetRoutingControlState, verdict mismatch
  Expected: HTTP 400 (ValidationException class)
  Actual:   HTTP 404
  detail: "status: expected 400, got 404"
  divergence_key: service=route53-recovery-cluster|GetRoutingControlState|status=400->404|keys=status

Cause 2 — step task#17, GetRoutingControlState, verdict mismatch (representative)
  Expected: HTTP 400, body {"__type": "com.amazon.coral.validate#ValidationException", "message": "Missing or invalid resource arn"}
  Actual:   HTTP 404, body {"__type": "ResourceNotFoundException", "message": "Routing Control 'not-an-arn' not found"}
  detail: "status: expected 400, got 404\n$.__type: value_mismatch (expected 'com.amazon.coral.validate#ValidationException', got 'ResourceNotFoundException')\n$.message: value_mismatch (expected 'Missing or invalid resource arn', got \"Routing Control 'not-an-arn' not found\")"
  divergence_key: service=route53-recovery-cluster|GetRoutingControlState|status=400->404|keys=$.__type,$.message,status
  Message anatomy: AWS's 'Missing or invalid resource arn' is entirely fixed text (no variable part). The emulator's string interpolates the request's RoutingControlArn ('not-an-arn' here) into a template AWS does not use.

Cause 3 — step task#18, UpdateRoutingControlStates, verdict mismatch
  Expected: HTTP 400 (ValidationException class)
  Actual:   HTTP 404
  detail: "status: expected 400, got 404"
  divergence_key: service=route53-recovery-cluster|UpdateRoutingControlStates|status=400->404|keys=status

Code path common to all three (no ARN-format gate before the registry lookup):
  services/.../route53recoverycluster-togglecustomerapi/src/ops/get_routing_control_state.rs:182-197
  services/.../route53recoverycluster-togglecustomerapi/src/ops/update_routing_control_states.rs:231-270

## Steps to Reproduce
1. Stand up a cluster and a routing control: route53-recovery-control-config CreateCluster, poll DescribeCluster until Cluster.Status == DEPLOYED, then CreateRoutingControl in the cluster's default control panel and poll until DEPLOYED. Build a route53-recovery-cluster client against the cluster endpoint for the run region.
2. Positive control: GetRoutingControlState(RoutingControlArn=<the real routing control ARN>) -> HTTP 200 with RoutingControlState.
3. Cause 1/2: call GetRoutingControlState with a RoutingControlArn that is not an ARN at all but still satisfies @length 1..255 and ^[A-Za-z0-9:.\/_-]*$ — e.g. the literal string 'not-an-arn'.
   Expected (AWS): HTTP 400, __type 'com.amazon.coral.validate#ValidationException', message 'Missing or invalid resource arn'.
   Observed: HTTP 404, __type 'ResourceNotFoundException', message "Routing Control 'not-an-arn' not found".
4. Cause 3: call UpdateRoutingControlStates with UpdateRoutingControlStateEntries = [{RoutingControlArn: 'not-an-arn', RoutingControlState: 'On'}].
   Expected (AWS): HTTP 400 (ValidationException, message 'Missing or invalid resource arn'); no control changes state.
   Observed: HTTP 404 (ResourceNotFoundException).
5. Re-run the fixture to confirm: replay_wire_corpus on fixture route53-recovery-cluster-routing-control-state-lifecycle, steps task#16, task#17 and task#18 must all pass.
