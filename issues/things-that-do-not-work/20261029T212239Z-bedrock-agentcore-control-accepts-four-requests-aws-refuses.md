Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.delete-gateway-with-active-target-rejected bedrock-agentcore-control.delete-gateway-with-active-targets-conflict bedrock-agentcore-control.gateway-delete-with-target-conflict-rejected bedrock-agentcore-control.create-memory-invalid-dependency-not-found bedrock-agentcore-control.create-agent-runtime-without-iam-permission-denied bedrock-agentcore-control.delete-gateway-without-iam-permission-denied bedrock-agentcore-control.update-agent-runtime-invalid-state-transition-rejected bedrock-agentcore-control.update-harness-endpoint-concurrent-update-conflict bedrock-agentcore-control.harness-and-endpoint-lifecycle-transitions
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control.delete-gateway-with-active-target-rejected|_error_detail#12|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: bedrock-agentcore-control.delete-gateway-with-active-targets-conflict|_error_detail#12|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: bedrock-agentcore-control.gateway-delete-with-target-conflict-rejected|_error_code#12|AssertionError|raise AssertionError("expected the call to raise a ClientError, but it succeeded")
Issue-Key: bedrock-agentcore-control.create-memory-invalid-dependency-not-found|_client_error#18|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: bedrock-agentcore-control.create-agent-runtime-without-iam-permission-denied|_error_detail#8|AssertionError|raise AssertionError(
Issue-Key: bedrock-agentcore-control.delete-gateway-without-iam-permission-denied|run#12|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Key: bedrock-agentcore-control.update-agent-runtime-invalid-state-transition-rejected|run#14|AssertionError|assert code == "ConflictException", f"expected ConflictException, got {code!r}: {message!r}"
Issue-Key: bedrock-agentcore-control|CreateHarness|ValidationException
Issue-Key: bedrock-agentcore-control.harness-and-endpoint-lifecycle-transitions|_retry_until_role_usable#4|Timeout|time.sleep(interval_s)
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# bedrock-agentcore-control accepts four requests AWS refuses (gateway delete with targets, CreateMemory on a missing Kinesis stream, any call from an unauthorized principal, UpdateAgentRuntime mid-delete) and invents a required CreateHarness field (5 causes)

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteGateway

## Description
The emulator's accept/refuse decisions diverge from AWS on five paths: four requests AWS refuses are carried out, and one request AWS accepts is refused for a field AWS does not require. 5 causes must all be fixed before this issue is closed.

Cause 1 of 5: DeleteGateway destroys a gateway that still has targets attached.
AWS: Smithy DeleteGateway errors include ValidationException (httpError 400) and ConflictException (409); GatewayTarget is a child of the gateway. Recorded behavior: with at least one GatewayTarget attached, DeleteGateway is refused 400 ValidationException with the message "Gateway with ID: <gatewayId> has targets associated with it. Delete all targets before deleting the gateway." — fixed text as shown, the single variable part being the gateway id from the request path — and neither the gateway nor the target changes status; after every target is deleted the identical call is accepted.
Emulator: answers 202 with {gatewayId, status: "DELETING"} and starts deleting the gateway while the target is still attached, so the refusal never happens and the child is orphaned/cascaded.
Why: a client relying on AWS's ordering guarantee (delete targets, then the gateway) gets no protection — a mistaken DeleteGateway silently destroys configuration AWS would have preserved, and code that catches the ValidationException to drive a cleanup loop never runs it.
Fix: a dependent-children guard in the DeleteGateway handler's validation, checked against gateway-target state before the delete is accepted.
Traces: bedrock-agentcore-control.delete-gateway-with-active-target-rejected, bedrock-agentcore-control.delete-gateway-with-active-targets-conflict, bedrock-agentcore-control.gateway-delete-with-target-conflict-rejected.

Cause 2 of 5: CreateMemory accepts streamDeliveryResources naming a Kinesis stream that does not exist.
AWS: Smithy CreateMemory errors include ValidationException (httpError 400). Recorded behavior: a CreateMemory whose streamDeliveryResources.resources[].kinesis.dataStreamArn is a well-formed ARN for a stream that does not exist (with an execution role already proven to work against a real stream) is refused 400 ValidationException with the fixed message "Memory execution role does not have access to provided Kinesis stream. If the stream is KMS-encrypted, ensure the role has kms:GenerateDataKey and kms:Decrypt permissions on the KMS key." (no variable parts), and no memory is created.
Emulator: answers 202 and materialises the memory in CREATING with the bogus dataStreamArn echoed back in streamDeliveryResources, so the dependency is never checked.
Why: a caller that typos a stream ARN gets a memory that will never deliver events instead of an immediate, actionable refusal, and a test that asserts the dependency pre-condition cannot distinguish a valid from an invalid configuration.
Fix: the CreateMemory handler's validation of streamDeliveryResources — resolve the referenced Kinesis stream (and the execution role's access to it) before accepting the create.
Traces: bedrock-agentcore-control.create-memory-invalid-dependency-not-found.

Cause 3 of 5: no identity-based authorization gate — a principal with no bedrock-agentcore permissions is not refused.
AWS: every operation in the service models AccessDeniedException (httpError 403; Smithy CreateAgentRuntime and DeleteGateway both list it). Recorded behavior: a caller whose assumed role grants no bedrock-agentcore actions is refused 403 AccessDeniedException with the message "User: <callerArn> is not authorized to perform: bedrock-agentcore:<Action> on resource: <resourceArn> because no identity-based policy allows the bedrock-agentcore:<Action> action" — fixed text as shown, with the caller's assumed-role ARN, the action name and the target resource ARN (the resource pattern, e.g. runtime/*, for a create) as the variable parts — and no state changes.
Emulator: CreateAgentRuntime from the denied principal answers 202 and provisions the runtime (agentRuntimeArn/agentRuntimeId minted, status CREATING); DeleteGateway from the denied principal answers 400 ValidationException instead of 403 AccessDeniedException. Neither path consults the caller's policy.
Why: any test of least-privilege behavior passes against the emulator and fails against AWS, and a denied caller mutates account state the real service would have protected.
Fix: an authorization check in the service's request dispatch/handler entry for bedrock-agentcore-control, evaluating the caller's identity-based policy before the operation runs and emitting the modeled AccessDeniedException.
Traces: bedrock-agentcore-control.create-agent-runtime-without-iam-permission-denied, bedrock-agentcore-control.delete-gateway-without-iam-permission-denied.

Cause 4 of 5: DeleteAgentRuntime removes the runtime synchronously, so there is no DELETING window to conflict with.
AWS: Smithy DeleteAgentRuntime is a 202 whose output carries status (AgentRuntimeStatus), i.e. the delete is asynchronous; UpdateAgentRuntime models ConflictException (httpError 409) alongside ResourceNotFoundException (404). Recorded behavior: immediately after an accepted DeleteAgentRuntime the runtime is still present in DELETING, and an UpdateAgentRuntime issued in that window is refused 409 ConflictException with "Cannot update agent <agentRuntimeArn> while it's DELETING. Please try again when the agent is ready." — fixed text as shown, the variable part being the runtime ARN; only after the delete completes does a read return ResourceNotFoundException.
Emulator: the runtime is gone the instant DeleteAgentRuntime returns, so the very next UpdateAgentRuntime answers 404 ResourceNotFoundException instead of 409 ConflictException.
Why: a client that distinguishes "busy, retry later" (409) from "gone, stop" (404) takes the wrong branch, and no caller can observe the DELETING state the API models and returns.
Fix: the DeleteAgentRuntime handler's state transition — move the runtime to DELETING and keep it readable until the asynchronous delete completes — plus the state guard in the UpdateAgentRuntime handler.
Traces: bedrock-agentcore-control.update-agent-runtime-invalid-state-transition-rejected.

Cause 5 of 5: CreateHarness refuses every request that omits `model`, which AWS does not require.
AWS: Smithy CreateHarnessRequest marks only harnessName (@pattern ^[a-zA-Z][a-zA-Z0-9_]{0,39}$) and executionRoleArn as @required; `model` (HarnessModelConfiguration) is optional, as are environment, tools, skills, memory and the rest. A CreateHarness carrying just harnessName, executionRoleArn and tags is accepted (201) and the harness converges to READY.
Emulator: refuses it 400 ValidationException with 'model: must be specified: a harness reports the model it runs with, and this service has no default model to run one with' — an emulator-authored required-field rule and an emulator-authored sentence, for a member the model leaves optional.
Why: no harness can be created the way AWS allows, so every harness-dependent flow is unreachable: one check dies on the refusal and another burns its whole budget retrying it (its role-usability retry loop treats the ValidationException as not-yet-propagated IAM). The invented message also has no counterpart in AWS, so nothing can match it.
Fix: the CreateHarness handler's input validation — drop the invented `model` requirement and derive required members from the Smithy @required set.
Traces: bedrock-agentcore-control.update-harness-endpoint-concurrent-update-conflict, bedrock-agentcore-control.harness-and-endpoint-lifecycle-transitions.

## Evidence
Cause 1 — bedrock-agentcore-control.delete-gateway-with-active-target-rejected: expected a refusal, DeleteGateway succeeded with HTTP 202 and body {'gatewayId': 'sim2real-gwrej-61fdb9829a56-1ynrbyi3hm', 'status': 'DELETING'} while target status was READY; expected 400 ValidationException "Gateway with ID: <gatewayId> has targets associated with it. Delete all targets before deleting the gateway.". delete-gateway-with-active-targets-conflict: same, 202 {'gatewayId': 'sim2real-gw-target-6614b40c2d68-p3tly3q9y9', 'status': 'DELETING'}. gateway-delete-with-target-conflict-rejected: "expected the call to raise a ClientError, but it succeeded".
Cause 2 — bedrock-agentcore-control.create-memory-invalid-dependency-not-found: expected 400 ValidationException 'Memory execution role does not have access to provided Kinesis stream. ...', got HTTP 202 with memory status CREATING and streamDeliveryResources echoing the nonexistent stream ARN (recorded run: .../stream/sim2real-missing-stream-xyz).
Cause 3 — bedrock-agentcore-control.create-agent-runtime-without-iam-permission-denied: expected 403 AccessDeniedException 'User: <assumed-role arn> is not authorized to perform: bedrock-agentcore:CreateAgentRuntime on resource: arn:aws:bedrock-agentcore:<region>:<account>:runtime/* because no identity-based policy allows the bedrock-agentcore:CreateAgentRuntime action'; got HTTP 202 with agentRuntimeId/agentRuntimeArn minted and status CREATING. delete-gateway-without-iam-permission-denied: expected AccessDeniedException, got ValidationException.
Cause 4 — bedrock-agentcore-control.update-agent-runtime-invalid-state-transition-rejected: DeleteAgentRuntime returned status DELETING, then UpdateAgentRuntime gave 'ResourceNotFoundException': 'no agent runtime <id> in this account' where 409 ConflictException 'Cannot update agent <arn> while it's DELETING. Please try again when the agent is ready.' was expected.
Cause 5 — bedrock-agentcore-control.update-harness-endpoint-concurrent-update-conflict: CreateHarness(harnessName, executionRoleArn, tags) answered ValidationException 'model: must be specified: a harness reports the model it runs with, and this service has no default model to run one with'; Smithy marks only harnessName and executionRoleArn @required. harness-and-endpoint-lifecycle-transitions issues the same CreateHarness and timed out after 60s retrying that refusal.
Generated names, ids, ARNs and account numbers above are per-run values filling the message templates.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Cause 1: CreateGateway, wait for READY, CreateGatewayTarget, wait for the target to leave CREATING/SYNCHRONIZING, then DeleteGateway — observe 202/DELETING instead of a 400 refusal (checks delete-gateway-with-active-target-rejected, delete-gateway-with-active-targets-conflict, gateway-delete-with-target-conflict-rejected).
3. Cause 2: CreateMemory with streamDeliveryResources.resources[0].kinesis.dataStreamArn pointing at a stream that was never created (check create-memory-invalid-dependency-not-found).
4. Cause 3: assume a role whose policy grants no bedrock-agentcore actions and call CreateAgentRuntime, then DeleteGateway, with those credentials (checks create-agent-runtime-without-iam-permission-denied, delete-gateway-without-iam-permission-denied).
5. Cause 4: create a runtime, wait for READY, call DeleteAgentRuntime and immediately call UpdateAgentRuntime on the same id (check update-agent-runtime-invalid-state-transition-rejected).
6. Cause 5: CreateHarness with only harnessName, executionRoleArn and tags (checks update-harness-endpoint-concurrent-update-conflict, harness-and-endpoint-lifecycle-transitions).
