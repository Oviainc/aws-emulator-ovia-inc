Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53-recovery-cluster.access-denied-missing-permission-rejected route53-recovery-cluster.batch-update-service-limit-exceeded-rejected route53-recovery-cluster.validation-exception-malformed-routing-control-arn-rejected
Issue-Shape-Id: com.amazonaws.route53recoverycluster#ToggleCustomerAPI
Issue-Key: route53-recovery-cluster.access-denied-missing-permission-rejected|_error#21|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: route53-recovery-cluster.batch-update-service-limit-exceeded-rejected|run#11|AssertionError|assert message == "Cannot update more than 10 controls in a single batch", message
Issue-Key: route53-recovery-cluster.validation-exception-malformed-routing-control-arn-rejected|run#10|AssertionError|assert message == "Missing or invalid resource arn", message
Issue-Commit: 482573dd66a8fdd59e0387c95d89889e67d3b0e1

# route53-recovery-cluster data plane: 3 net-new causes — GetRoutingControlState applies no IAM authorization gate (explicit Deny answered 200), and the ServiceLimitExceededException / ValidationException refusal messages are emulator-authored instead of AWS's wording

> _Note: this issue was identified against the code base at commit `482573dd66a8fdd59e0387c95d89889e67d3b0e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRoutingControlState, UpdateRoutingControlStates

## Description
Three distinct root causes in the route53-recovery-cluster (ToggleCustomerAPI, awsJson1_0) data plane. All three are refusal-fidelity defects: the emulator either does not refuse at all, or refuses with text it authored itself rather than the text AWS returns for that error class. No open issue covers this service (the existing route53-recovery-* issues are all for route53-recovery-control-config: the CreateSafetyRule / UpdateRoutingControl / TagResource stubs and the CreateCluster BadRequestException pattern issue).

Cause 1 of 3: GetRoutingControlState applies no IAM authorization gate, so an explicit Deny is ignored and the call succeeds.
AWS: GetRoutingControlState models AccessDeniedException as a client error at HTTP 403 (Smithy: GetRoutingControlState errors include AccessDeniedException, httpError 403, shape doc "You don't have sufficient permissions to perform this action."; AccessDeniedException.message is @required). When the calling principal's identity policy carries an explicit Deny for route53-recovery-cluster:GetRoutingControlState, AWS refuses with code AccessDeniedException and the standard authorization message: the fixed text is "User: ", " is not authorized to perform: ", " on resource: ", " with an explicit deny in an identity-based policy"; the variable parts are the caller's assumed-role identity ARN (account id, role name and session name from the credentials used), the fully qualified action name (service prefix plus operation, here route53-recovery-cluster:GetRoutingControlState), and the resource ARN taken verbatim from the request's RoutingControlArn. The denial must be scoped to the denied action only, and must disappear again when the Deny statement is removed from the same assumed-role identity.
Emulator: the request is served normally. The replay shows HTTP 200 with a full GetRoutingControlStateResponse body ({RoutingControlArn, RoutingControlState: 'On', RoutingControlName}) for credentials whose role has an inline Deny on exactly that action, so the check's _error() helper raised "expected a rejection, but the call SUCCEEDED". The handler explicitly discards the request context (`let _ = ctx;`) and never consults the caller's identity or any policy, so there is no authorization decision point at all on this operation.
Why: a client cannot test or rely on least-privilege behaviour against the emulator. IAM-regression tests ("this role must NOT be able to read routing control state"), break-glass runbooks and policy-boundary tests all pass against the emulator while failing against AWS, and code that treats AccessDeniedException as a signal to re-assume a different role or to fall back to another cluster endpoint never exercises that path.
Fix: belongs in the authorization gate of the data-plane operation handler (the request walk for this service's ops, i.e. the AccessDeniedException branch missing from GetRoutingControlState's handler) — it must evaluate the caller's identity-based policy for the action/resource pair before reading state, and return the modeled AccessDeniedException (403) with the message template above.
Traces: route53-recovery-cluster.access-denied-missing-permission-rejected.

Cause 2 of 3: UpdateRoutingControlStates over-the-quota refusal carries an emulator-authored ServiceLimitExceededException message.
AWS: the entry list has no modeled @length bound (Smithy: UpdateRoutingControlStates.UpdateRoutingControlStateEntries -> list UpdateRoutingControlStateEntries, @required, no length constraint), so the batch cap of 10 routing controls per call is a runtime service quota reported as ServiceLimitExceededException (client error, httpError 402). Recorded behavior: AWS returns code ServiceLimitExceededException with message "Cannot update more than 10 controls in a single batch" — the whole string is fixed text apart from the limit value (10), which is the quota; it does NOT echo the submitted entry count. None of the submitted controls change state.
Emulator: the correct code is returned but the message is its own wording: "The request can't update 11 routing control states at the same time. Try again with at most 10 routing control states." — a rephrasing of the shape's doc string that additionally interpolates the submitted entry count (11). Source: services/service-com-amazonaws-route53recoverycluster-togglecustomerapi/src/ops/update_routing_control_states.rs lines 189-207 formats that message from entries.len() and MAX_UPDATE_ROUTING_CONTROL_STATE_ENTRIES.
Why: callers that match on the quota message (log scrapers, user-facing error surfacing, tests asserting the documented refusal text) see text AWS never emits, and the entry count in the emulator's message makes the string vary per call where AWS's is constant.
Fix: the message construction in the ServiceLimitExceededException branch of the UpdateRoutingControlStates handler.
Traces: route53-recovery-cluster.batch-update-service-limit-exceeded-rejected.

Cause 3 of 3: a RoutingControlArn that violates the modeled character-set pattern is refused with the generic boundary-validator message instead of this service's ValidationException wording.
AWS: RoutingControlArn targets Arn with @length min 1 max 255 and @pattern ^[A-Za-z0-9:.\\/_-]*$ (Smithy: GetRoutingControlState.RoutingControlArn -> Arn; ValidationException is a client error at httpError 400 with @required message plus optional reason/fields). Recorded behavior: an ARN containing characters the pattern excludes (e.g. a space and '!') is refused with code ValidationException and message "Missing or invalid resource arn" — the entire string is fixed text; AWS does NOT echo the offending value, the member name or the regex. The same call with the real routing control's well-formed ARN succeeds.
Emulator: code ValidationException is right, but the message is the framework's generic constraint report: "1 validation error detected: Value '<the submitted ARN>' at 'routingControlArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9:.\\/_-]*$". The handler does no ARN-shape check of its own (services/service-com-amazonaws-route53recoverycluster-togglecustomerapi/src/ops/get_routing_control_state.rs lines 163-197 goes straight to the registry lookup), so the boundary validator's message escapes to the wire.
Why: the emulator leaks the request value and the model's internal regex into a customer-visible message, and clients asserting on or displaying the documented refusal text get a different string; it also diverges in information content (per-call value vs constant text), so the message cannot be used as a stable discriminator.
Fix: the input validation for this service's ARN-bearing data-plane operations — the pattern/length violation on RoutingControlArn must be mapped to the service's own ValidationException message rather than reported by the generic boundary validator.
Traces: route53-recovery-cluster.validation-exception-malformed-routing-control-arn-rejected.

## Evidence
Suite: 11/17 passed for route53-recovery-cluster; 3 mismatches clustered here (the 3 remaining failures are the ALREADY FILED CreateSafetyRule stub cluster and are not part of this issue).

Cause 1 — route53-recovery-cluster.access-denied-missing-permission-rejected [mismatch]
Expected (AWS): ClientError, code "AccessDeniedException" (HTTP 403), message "User: arn:aws:sts::<account>:assumed-role/<role-name>/<session-name> is not authorized to perform: route53-recovery-cluster:GetRoutingControlState on resource: <RoutingControlArn from the request> with an explicit deny in an identity-based policy".
Actual (emulator): HTTP 200 and a success body — {'RoutingControlArn': 'arn:aws:route53-recovery-control::000000000000:controlpanel/.../routingcontrol/...', 'RoutingControlState': 'On', 'RoutingControlName': 'sim2real-rc-...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}} — raising "AssertionError: expected a rejection, but the call SUCCEEDED and returned {...}" (the ids/names above are per-run values, not requirements).

Cause 2 — route53-recovery-cluster.batch-update-service-limit-exceeded-rejected [mismatch]
Call: update_routing_control_states with 11 entries (one over the quota of 10).
Expected (AWS): code "ServiceLimitExceededException" (402), message "Cannot update more than 10 controls in a single batch".
Actual (emulator): code matches; message is "The request can't update 11 routing control states at the same time. Try again with at most 10 routing control states." — AssertionError raised at `assert message == "Cannot update more than 10 controls in a single batch"`.

Cause 3 — route53-recovery-cluster.validation-exception-malformed-routing-control-arn-rejected [mismatch]
Call: get_routing_control_state with a RoutingControlArn built from the valid ARN by replacing the first ':' with a space and appending '!', so it violates ^[A-Za-z0-9:.\\/_-]*$.
Expected (AWS): code "ValidationException" (400), message "Missing or invalid resource arn".
Actual (emulator): code matches; message is "1 validation error detected: Value 'arn aws:route53-recovery-control::000000000000:controlpanel/.../routingcontrol/...!' at 'routingControlArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9:.\\/_-]*$" — AssertionError raised at `assert message == "Missing or invalid resource arn"`.

## Steps to Reproduce
Shared setup for all three causes: create (or reuse) an ARC cluster via route53-recovery-control-config CreateCluster, wait for Status DEPLOYED, read its ClusterEndpoints and build a route53-recovery-cluster client against the endpoint for the current region (there is no generic regional endpoint for this data plane). Create routing control(s) in the cluster's default control panel with CreateRoutingControl and wait for Status DEPLOYED.

Cause 1 (AccessDenied not enforced):
1. Create an IAM role trusting the account root; assume it via sts:AssumeRole.
2. Attach TWO separate inline policies to the role: one Allow for "route53-recovery-cluster:*" on "*", and one Deny for "route53-recovery-cluster:GetRoutingControlState" on "*".
3. Set the routing control to On with UpdateRoutingControlState and wait until GetRoutingControlState reports On with the unrestricted credentials.
4. Call GetRoutingControlState(RoutingControlArn=<rc arn>) with the assumed-role credentials against the cluster endpoint.
   Expected: AccessDeniedException (403) with the explicit-deny message. Observed: HTTP 200 and the routing control state.
5. Delete only the Deny inline policy and repeat step 4: the call must succeed again (confirms the denial was policy-driven, not session-driven).

Cause 2 (quota message):
1. Create 11 routing controls in the cluster and drive them all to Off.
2. Call UpdateRoutingControlStates with 11 UpdateRoutingControlStateEntries, each {RoutingControlArn, RoutingControlState: "On"}.
   Expected: ServiceLimitExceededException with message "Cannot update more than 10 controls in a single batch"; no control changes state. Observed: right code, emulator-authored message that interpolates the submitted count.
3. Repeat with exactly 10 entries: it must succeed (positive control).

Cause 3 (malformed-ARN message):
1. Take the valid RoutingControlArn and corrupt it so it violates ^[A-Za-z0-9:.\\/_-]*$ — e.g. replace the first ':' with a space and append '!'.
2. Call GetRoutingControlState with that ARN.
   Expected: ValidationException with message "Missing or invalid resource arn". Observed: the generic "1 validation error detected: Value '...' at 'routingControlArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ..." message.
3. Call GetRoutingControlState with the real ARN: it must return RoutingControlState On (positive control).
