Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-harness-control-plane-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-harness-control-plane-errors bedrock-agentcore-control-harness-not-found-errors
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|CreateHarness|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|CreateHarness|4xx->4xx|$headers.content-type
Issue-Key: bedrock-agentcore-control|CreateHarness|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore-control|CreateHarness|4xx->4xx|status
Issue-Key: bedrock-agentcore-control|GetHarness|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|GetHarness|4xx->4xx|$headers.content-type
Issue-Key: bedrock-agentcore-control|GetHarness|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# Harness operations are unreachable: CreateHarness and GetHarness answer 404 UnknownService for an unregistered bedrock-agentcore service

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateHarness

## Description
The whole harness surface of bedrock-agentcore-control never reaches a handler: requests are dispatched to a service token 'bedrock-agentcore' that the router has no registration for, and the emulator answers a generic 404 UnknownService in the wrong content type. Every other operation of this service answers normally in the same replay, so this is a routing/dispatch defect confined to the harness operations, not a missing bootstrap. 2 causes.

Related but not covering this: open issue 20261022T135641Z-bedrock-agentcore-control-is-unreachable-router-keys-on.md describes the router keying on the shared 'bedrock-agentcore' sigv4 name and returning 500 AmbiguousService, and 20261022T135153Z-bedrock-agentcore-is-entirely-unreachable-the-router-refuses.md describes the same for the data-plane service. Neither carries these replay keys and the symptom here is a 404 UnknownService, so these steps are filed separately.

Cause 1 of 2: CreateHarness with an invalid harnessName returns 404 UnknownService instead of 400 ValidationException.
AWS: Smithy CreateHarness is bound to this service as POST /harnesses with harnessName @pattern ^[a-zA-Z][a-zA-Z0-9_]{0,39}$ and models ValidationException (httpError 400); a name starting with a digit is refused 400 with x-amzn-errortype ValidationException and content-type application/json (recorded message example: "1 validation error detected: Value '1sim2realprobeharness5f40be90' at 'harnessName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z][a-zA-Z0-9_]{0,39}").
Emulator: 404 with x-amzn-errortype 'UnknownService', content-type application/x-amz-json-1.1 and body message "no emulated service is registered for 'bedrock-agentcore'" - the request never reaches CreateHarness.
Why: the operation is entirely unusable, and the error a client receives is neither a modelled error of the operation nor even a restJson1-shaped response (wrong content type), so SDK error parsing fails too.
Fix: the request router / service resolution for this service - the harness operations must resolve to the bedrock-agentcore-control dispatcher instead of an unregistered 'bedrock-agentcore' token; then CreateHarness input validation can raise ValidationException.
Traces: bedrock-agentcore-control-harness-control-plane-errors#task#1 (representative).

Cause 2 of 2: GetHarness for a missing harness returns 404 UnknownService instead of 404 ResourceNotFoundException.
AWS: Smithy GetHarness is GET /harnesses/{harnessId} and models ResourceNotFoundException (httpError 404); the recording shows 404 with x-amzn-errortype ResourceNotFoundException and content-type application/json (recorded message example: 'Agent with name sim2realprobemissing-1234567890 not found.'). The status matches by accident here; the error code and content type do not.
Emulator: 404 with x-amzn-errortype 'UnknownService', content-type application/x-amz-json-1.1, body "no emulated service is registered for 'bedrock-agentcore'".
Why: a client cannot distinguish 'this harness does not exist' from 'this service does not exist'; retry/branching on ResourceNotFoundException never fires.
Fix: same routing fix as cause 1, then the GetHarness not-found path.
Traces: bedrock-agentcore-control-harness-not-found-errors#task#1.

## Evidence
Cause 1 (bedrock-agentcore-control-harness-control-plane-errors#task#1): expected status 400, $headers.x-amzn-errortype 'ValidationException', $headers.content-type 'application/json'; actual status 404, x-amzn-errortype 'UnknownService', content-type 'application/x-amz-json-1.1', $.message "no emulated service is registered for 'bedrock-agentcore'".
Cause 2 (bedrock-agentcore-control-harness-not-found-errors#task#1): expected x-amzn-errortype 'ResourceNotFoundException' with content-type 'application/json' and a not-found message; actual x-amzn-errortype 'UnknownService', content-type 'application/x-amz-json-1.1', same "no emulated service is registered for 'bedrock-agentcore'" body.
Model: CreateHarness -> POST /harnesses, harnessName pattern ^[a-zA-Z][a-zA-Z0-9_]{0,39}$, errors include ValidationException(400); GetHarness -> GET /harnesses/{harnessId}, errors include ResourceNotFoundException(404). Both are bound to com.amazonaws.bedrockagentcorecontrol, which the same replay reaches successfully for gateways, memories and runtimes.

## Steps to Reproduce
1. Replay bedrock-agentcore-control-harness-control-plane-errors task#1: POST /harnesses with harnessName '1sim2realprobeharness5f40be90' - the emulator answers 404 UnknownService ('no emulated service is registered for bedrock-agentcore') instead of 400 ValidationException.
2. Replay bedrock-agentcore-control-harness-not-found-errors task#1: GET /harnesses/sim2realprobemissing-1234567890 - same UnknownService refusal instead of 404 ResourceNotFoundException.
3. Compare with any gateway or memory step in the same replay, which is routed and handled normally, confirming the defect is in harness-operation dispatch.
