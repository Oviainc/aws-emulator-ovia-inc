Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-harness-control-plane-errors
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-harness-control-plane-errors bedrock-agentcore-control-harness-not-found-errors
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|GetHarnessEndpoint|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|DeleteHarnessEndpoint|4xx->4xx|$.message
Issue-Commit: 0572fd9e58e5409879334759fdd06d92cac8af54

# Harness endpoint not-found errors report the harness instead of the endpoint

> _Note: this issue was identified against the code base at commit `0572fd9e58e5409879334759fdd06d92cac8af54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetHarnessEndpoint

## Description
The harness-endpoint operations (GetHarnessEndpoint, DeleteHarnessEndpoint) answer a not-found with the harness-granularity message instead of the endpoint-granularity one AWS returns. Both resolve the {harnessId} path label first and raise the harness not-found error, so the {endpointName} the caller actually asked about never appears in the message. The HTTP status (404) and the error shape are correct in both cases; only $.message diverges. 2 causes must all be fixed before this issue is closed.

Cause 1 of 2: GetHarnessEndpoint reports the harness, not the endpoint.
AWS: GetHarnessEndpoint is GET /harnesses/{harnessId}/endpoints/{endpointName} and models ResourceNotFoundException (Smithy: GetHarnessEndpoint errors include ResourceNotFoundException, httpError 404; input members harnessId and endpointName are both @required @httpLabel). Recorded behavior: the 404 body message is "Endpoint <endpointName> not found for harness <harnessId>." Fixed text: "Endpoint ", " not found for harness ", "." — the two variable parts are the endpointName and harnessId taken verbatim from the request path. AWS words the failure at endpoint granularity even when the named harness itself does not exist, i.e. the endpoint lookup is what is reported, not the parent harness lookup.
Emulator: returns 404 with "Agent with name <harnessId> not found." — the parent-harness not-found sentence, with the requested endpoint name dropped entirely.
Why: a client cannot tell from the message whether the harness or just one of its endpoints is missing, and tooling that parses the endpoint name out of the not-found message (or asserts on the "Endpoint ... not found for harness ..." text) gets a sentence about a different resource. Code paths that retry endpoint creation versus re-creating the whole harness branch on exactly this distinction.
Fix: the GetHarnessEndpoint handler's lookup/error construction — the endpoint resolution must own the not-found error rather than delegating to the harness resolver's error.
Traces: bedrock-agentcore-control-harness-control-plane-errors#task#2.

Cause 2 of 2: DeleteHarnessEndpoint reports the harness, not the endpoint.
AWS: DeleteHarnessEndpoint is DELETE /harnesses/{harnessId}/endpoints/{endpointName} and models ResourceNotFoundException (Smithy: DeleteHarnessEndpoint errors include ResourceNotFoundException, httpError 404; harnessId and endpointName are @required @httpLabel). Recorded behavior: the 404 message uses the same endpoint-granularity template, "Endpoint <endpointName> not found for harness <harnessId>." — the same fixed text as cause 1, with the endpointName and harnessId from the request path. The verb of the calling operation does not appear, so the delete path must emit the identical sentence as the read path.
Emulator: returns 404 with "Agent with name <harnessId> not found.", again the harness-level sentence with no endpoint name.
Why: confirms the short-circuit is systematic across the harness-endpoint handlers rather than one handler's slip; a delete that is idempotent-by-message-inspection cannot distinguish "endpoint already gone" from "harness gone".
Fix: the DeleteHarnessEndpoint handler's lookup/error construction, sharing one endpoint not-found error constructor with GetHarnessEndpoint.
Traces: bedrock-agentcore-control-harness-not-found-errors#task#3.

Related but distinct: open issue 20261029T170911Z-bedrock-agentcore-control-not-found-and-invalid-state-error-messages-are covers GetHarness (and gateway/runtime/memory) not-found wording. That issue's harness cause is about the harness-level message itself; this issue is about the endpoint-level operations emitting a harness-level message at all, and is not covered by it.

## Evidence
Both causes matched status (404) and error shape; only $.message diverged.
Cause 1 — fixture bedrock-agentcore-control-harness-control-plane-errors, step task#2, GetHarnessEndpoint, expected 404 / got 404: $.message value_mismatch — expected 'Endpoint <endpointName> not found for harness <harnessId>.' (recorded run: endpointName missingendpoint, harnessId sim2realprobemissingharness-1234567890), got 'Agent with name sim2realprobemissingharness-1234567890 not found.'
Cause 2 — fixture bedrock-agentcore-control-harness-not-found-errors, step task#3, DeleteHarnessEndpoint, expected 404 / got 404: $.message value_mismatch — expected 'Endpoint <endpointName> not found for harness <harnessId>.' (recorded run: endpointName missingendpoint, harnessId sim2realprobemissing-1234567890), got 'Agent with name sim2realprobemissing-1234567890 not found.'
The identifiers shown are the recorded runs' values filling the template's variable slots (endpoint name and harness id from the request path); the fixed text "Endpoint ... not found for harness ..." is the requirement.

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus).
2. Cause 1: fixture bedrock-agentcore-control-harness-control-plane-errors, step task#2 — call GetHarnessEndpoint (GET /harnesses/{harnessId}/endpoints/{endpointName}) with a harnessId and endpointName that do not exist; read $.message of the 404 and observe it names the harness, not the endpoint.
3. Cause 2: fixture bedrock-agentcore-control-harness-not-found-errors, step task#3 — call DeleteHarnessEndpoint (DELETE /harnesses/{harnessId}/endpoints/{endpointName}) with an unknown harnessId/endpointName; read $.message of the 404 and observe the same harness-level sentence.
4. In both cases the expected message is 'Endpoint <endpointName> not found for harness <harnessId>.' with the two values taken from the request path.
