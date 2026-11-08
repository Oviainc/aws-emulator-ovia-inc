Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.put-resource-policy-nonexistent-resource-not-found
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control.put-resource-policy-nonexistent-resource-not-found|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: ca5e86cb5ef0fd96914240d43edae6de07731cd9

# bedrock-agentcore-control PutResourcePolicy's ResourceNotFoundException reuses the generic resource not-found sentence instead of AWS's ResourceArn wording

> _Note: this issue was identified against the code base at commit `ca5e86cb5ef0fd96914240d43edae6de07731cd9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
Cause 1 of 1: the resource-policy plane's not-found refusal is answered with the service's generic "resource doesn't exist" sentence rather than the ResourceArn-specific message AWS returns for the resource-policy operations.

AWS: Smithy PutResourcePolicy (PUT /resourcepolicy/{resourceArn}, success 201) declares ResourceNotFoundException with httpError 404 among its errors; the input member `resourceArn` targets BedrockAgentcoreResourceArn (@httpLabel, required, @length 20..1011). GetResourcePolicy (GET /resourcepolicy/{resourceArn}) and DeleteResourcePolicy declare the same error. The model does not fix the message text, so recorded behavior only: when `resourceArn` addresses a resource that does not exist in the caller's account, the 404 ResourceNotFoundException message is the fixed sentence "ResourceArn provided in the request does not exist" — the whole sentence is fixed text, with no variable part interpolated from the request (the offending ARN is NOT echoed).

Emulator: returns the correct HTTP 404 and the correct error code ResourceNotFoundException, but the message is "Failed to retrieve resource because it doesn't exist. Retry the request with a different resource identifier." — a different sentence entirely, evidently the service's shared generic resource-lookup not-found text, including retry guidance AWS does not give on this path. Only the message diverges; the code and status match. The check asserts the same AWS sentence for GetResourcePolicy on the same nonexistent ARN immediately afterwards (same file, next assertion), so the shared not-found construction used by the resource-policy read path is in scope for the fix even though the check halted on the PutResourcePolicy assertion first.

Why: a client that distinguishes "the ARN you named is not a real resource" from "the resource exists but has no policy" by matching the resource-policy operations' fixed sentence never matches, and retry logic that trusts the emulator's "Retry the request with a different resource identifier." guidance is being told something AWS never says. Operators and recorded-log diffing see a refusal sentence that does not exist in AWS for this operation.

Fix: the ResourceNotFoundException construction in the PutResourcePolicy handler's resource-resolution/validation step (and the equivalent step in the GetResourcePolicy and DeleteResourcePolicy handlers, which share the same not-found text) in the bedrock-agentcore-control service crate — these resource-policy paths must emit the resource-policy plane's own fixed sentence rather than reusing the service's generic resource not-found message.

Related open issue 20261029T212239Z-bedrock-agentcore-control-refusal-messages-diverge-from-aws.md describes the same class of emulator-authored refusal wording for other paths (agent-runtime not-found, gateway duplicate-name, gateway invalid-state, Double @range rendering). It does not name the resource-policy operations and does not carry this check's key, so it does not cover this cause.

Traces: bedrock-agentcore-control.put-resource-policy-nonexistent-resource-not-found.

## Evidence
Check bedrock-agentcore-control.put-resource-policy-nonexistent-resource-not-found (verdict: mismatch).

Assertion that failed (check_bedrock_agentcore_control_put_resource_policy_nonexistent_resource_not_found.py, line 42):
  AssertionError: expected message 'ResourceArn provided in the request does not exist', got "Failed to retrieve resource because it doesn't exist. Retry the request with a different resource identifier."

Expected (AWS): HTTP 404, code ResourceNotFoundException, message "ResourceArn provided in the request does not exist" — entirely fixed text, no request- or state-derived values.
Actual (emulator): HTTP 404, code ResourceNotFoundException (both correct), message "Failed to retrieve resource because it doesn't exist. Retry the request with a different resource identifier."

The request used a syntactically valid but nonexistent gateway ARN of the form arn:aws:bedrock-agentcore:<region>:<account>:gateway/<name> (the region, account id and generated gateway name are per-run values and are not part of the requirement; the ARN is not echoed in either message).

The same check then asserts the identical AWS sentence for GetResourcePolicy on the same nonexistent ARN; it was not reached because the PutResourcePolicy assertion aborted the check first.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (check bedrock-agentcore-control.put-resource-policy-nonexistent-resource-not-found).
2. Call PutResourcePolicy with resourceArn set to a syntactically valid bedrock-agentcore ARN for a resource that does not exist (e.g. arn:aws:bedrock-agentcore:<region>:<account>:gateway/<name-never-created>) and any valid policy document.
3. Observe the 404 response: the error code is ResourceNotFoundException as expected, but read $.message — it is "Failed to retrieve resource because it doesn't exist. Retry the request with a different resource identifier." instead of AWS's "ResourceArn provided in the request does not exist".
4. Call GetResourcePolicy with the same nonexistent resourceArn and read $.message of its 404: AWS returns the same fixed sentence "ResourceArn provided in the request does not exist".
