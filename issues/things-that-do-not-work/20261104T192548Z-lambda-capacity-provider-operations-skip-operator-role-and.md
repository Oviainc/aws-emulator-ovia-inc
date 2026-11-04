Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-capacity-provider-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-capacity-provider-lifecycle
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|CreateCapacityProvider|4xx->2xx|$.Type
Issue-Key: lambda|CreateCapacityProvider|4xx->2xx|$.message
Issue-Key: lambda|CreateCapacityProvider|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|CreateCapacityProvider|4xx->2xx|status
Issue-Key: lambda|GetCapacityProvider|4xx->2xx|$.Message
Issue-Key: lambda|GetCapacityProvider|4xx->2xx|$.Type
Issue-Key: lambda|GetCapacityProvider|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|GetCapacityProvider|4xx->2xx|status
Issue-Key: lambda|UpdateCapacityProvider|4xx->2xx|$.Message
Issue-Key: lambda|UpdateCapacityProvider|4xx->2xx|$.Type
Issue-Key: lambda|UpdateCapacityProvider|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|UpdateCapacityProvider|4xx->2xx|status
Issue-Key: lambda|ListFunctionVersionsByCapacityProvider|4xx->2xx|$.Message
Issue-Key: lambda|ListFunctionVersionsByCapacityProvider|4xx->2xx|$.Type
Issue-Key: lambda|ListFunctionVersionsByCapacityProvider|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|ListFunctionVersionsByCapacityProvider|4xx->2xx|status
Issue-Key: lambda|DeleteCapacityProvider|4xx->2xx|$.Message
Issue-Key: lambda|DeleteCapacityProvider|4xx->2xx|$.Type
Issue-Key: lambda|DeleteCapacityProvider|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|DeleteCapacityProvider|4xx->2xx|status
Issue-Commit: 950b4b45469069de560b6169f04ac518413bf05c

# Lambda capacity-provider operations skip operator-role and existence validation

> _Note: this issue was identified against the code base at commit `950b4b45469069de560b6169f04ac518413bf05c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCapacityProvider

## Description
The emulator's Lambda capacity-provider surface performs no input or existence checking: every capacity-provider call succeeds, whether the operator role it names is usable or the provider it addresses exists at all. Five distinct causes, all in service `lambda`.

Cause 1 of 5: CreateCapacityProvider accepts an unusable operator role.
AWS: Smithy CreateCapacityProvider requires PermissionsConfig (CapacityProviderPermissionsConfig.CapacityProviderOperatorRoleArn, required, pattern ^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$) and models error InvalidParameterValueException (errorKind client, httpError 400, members Type and message; doc "One of the parameters in the request is not valid."). When the supplied operator role is not valid or lacks the permissions the provider needs, AWS refuses with HTTP 400, header x-amzn-errortype: InvalidParameterValueException, and body {"Type":"User","message":"The operator role is invalid or doesn't have sufficient permissions. Verify the role and permissions and try again."}. That message is entirely fixed text — it carries no values from the request.
Emulator: answers HTTP 202 with a created CapacityProvider; no Type, no message, no x-amzn-errortype header.
Why: a client that validates its IAM wiring by watching for InvalidParameterValueException sees the bad-role provider created and treats a mis-permissioned provider as ready; the role fault then surfaces only later, as capacity that never scales, with nothing in the API response to attribute it to.
Fix: CreateCapacityProvider's input validation in the lambda service crate's capacity-provider handler — validate PermissionsConfig.CapacityProviderOperatorRoleArn against the known roles/trust before admitting the create, and serialize InvalidParameterValueException as a 400 with the errortype header.
Traces: lambda-capacity-provider-lifecycle#task#2.

Cause 2 of 5: GetCapacityProvider returns a provider for a name that does not exist.
AWS: Smithy GetCapacityProvider (GET /2025-11-30/capacity-providers/{CapacityProviderName}, success 200) models ResourceNotFoundException (errorKind client, httpError 404, members Type and Message; doc "The resource specified in the request does not exist."). For an unknown provider AWS answers HTTP 404, header x-amzn-errortype: ResourceNotFoundException, body {"Type":"User","Message":"Capacity provider not found: <identifier>. Check that the ARN is correct and the capacity provider exists in this Region."}. Fixed text: "Capacity provider not found: " and ". Check that the ARN is correct and the capacity provider exists in this Region."; the one variable part is <identifier>, the CapacityProviderName exactly as the request supplied it (the full capacity-provider ARN when an ARN was passed, the bare name when a name was passed).
Emulator: answers HTTP 200 with a CapacityProvider body for the unknown name.
Why: existence probes invert. A client that reads a provider to decide whether to create it never creates it, and code that treats a 200 as proof the provider is in this Region proceeds against a provider that was never created.
Fix: the shared capacity-provider name/ARN lookup behind GetCapacityProvider in the lambda crate — miss must raise ResourceNotFoundException rather than synthesize a record.
Traces: lambda-capacity-provider-lifecycle#task#3.

Cause 3 of 5: UpdateCapacityProvider reports success for a provider that does not exist.
AWS: Smithy UpdateCapacityProvider (PUT /2025-11-30/capacity-providers/{CapacityProviderName}, success 202) models ResourceNotFoundException (client, 404). For an unknown provider AWS answers HTTP 404, x-amzn-errortype: ResourceNotFoundException, with the same Type/Message pair and the same message template as Cause 2 — fixed text around the request's identifier.
Emulator: answers HTTP 202 as though the update applied.
Why: a configuration push against a deleted or mistyped provider is silently accepted, so a client believes scaling/telemetry settings are in force on a provider that does not exist and never retries or corrects the name.
Fix: UpdateCapacityProvider's handler in the lambda crate — resolve the provider in state before mutating and before answering 202.
Traces: lambda-capacity-provider-lifecycle#task#4.

Cause 4 of 5: ListFunctionVersionsByCapacityProvider lists versions for a provider that does not exist.
AWS: Smithy ListFunctionVersionsByCapacityProvider (GET /2025-11-30/capacity-providers/{CapacityProviderName}/function-versions, success 200, paginated) models ResourceNotFoundException (client, 404). An unknown provider yields HTTP 404, x-amzn-errortype: ResourceNotFoundException, Type "User" and the same not-found message template, its variable part the identifier as supplied (a bare name here).
Emulator: answers HTTP 200 with a list payload.
Why: an empty 200 is indistinguishable from "provider exists and nothing is pinned to it", so a caller draining this paginator to check whether a provider is safe to delete concludes it is unused when in fact it was never found.
Fix: ListFunctionVersionsByCapacityProvider's handler in the lambda crate — gate the listing on the provider lookup instead of returning an empty page for a miss.
Traces: lambda-capacity-provider-lifecycle#task#5.

Cause 5 of 5: DeleteCapacityProvider reports success for a provider that does not exist.
AWS: Smithy DeleteCapacityProvider (DELETE /2025-11-30/capacity-providers/{CapacityProviderName}, success 202, idempotent trait notwithstanding) models ResourceNotFoundException (client, 404). Deleting an unknown provider yields HTTP 404, x-amzn-errortype: ResourceNotFoundException, Type "User" and the same not-found message template with the request's identifier substituted.
Emulator: answers HTTP 202 as though a provider had been deleted.
Why: cleanup logic and name-reuse checks that depend on 404-vs-202 to tell "already gone" from "just removed" both read success, so a client cannot detect that it deleted the wrong name.
Fix: DeleteCapacityProvider's handler in the lambda crate — look the provider up and raise ResourceNotFoundException on a miss.
Traces: lambda-capacity-provider-lifecycle#task#6.

All five causes must be closed before this issue is submitted. Related but NOT covering: open issue 20261029T233239Z-lambda-capacity-provider-payloads-drop-telemetryconfig-and-the.md concerns TelemetryConfig in capacity-provider payloads and the wording of the duplicate-create (409) refusal only; it does not touch operator-role validation or the not-found path for Get/Update/Delete/ListFunctionVersionsByCapacityProvider.

## Evidence
Replay of fixture lambda-capacity-provider-lifecycle (trace set simparity-wire-aws-v1), all verdict=mismatch:

1) task#2 CreateCapacityProvider — expected 400, got 202. Missing $.Type (expected 'User'), missing $.message (expected "The operator role is invalid or doesn't have sufficient permissions. Verify the role and permissions and try again."), missing header x-amzn-errortype (expected 'InvalidParameterValueException').

2) task#3 GetCapacityProvider — expected 404, got 200. Missing $.Type (expected 'User'), missing $.Message (expected 'Capacity provider not found: <capacity-provider ARN as supplied>. Check that the ARN is correct and the capacity provider exists in this Region.'), missing header x-amzn-errortype (expected 'ResourceNotFoundException').

3) task#4 UpdateCapacityProvider — expected 404, got 202. Same three missing fields; expected x-amzn-errortype 'ResourceNotFoundException'.

4) task#5 ListFunctionVersionsByCapacityProvider — expected 404, got 200. Same three missing fields; expected $.Message 'Capacity provider not found: <name as supplied>. Check that the ARN is correct and the capacity provider exists in this Region.'

5) task#6 DeleteCapacityProvider — expected 404, got 202. Same three missing fields; expected x-amzn-errortype 'ResourceNotFoundException'.

Model grounding: Smithy CreateCapacityProvider errors include InvalidParameterValueException (400); GetCapacityProvider / UpdateCapacityProvider / DeleteCapacityProvider / ListFunctionVersionsByCapacityProvider each include ResourceNotFoundException (404). InvalidParameterValueException carries members {Type, message}; ResourceNotFoundException carries members {Type, Message} — matching the casing the recordings assert.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture `lambda-capacity-provider-lifecycle` from trace set simparity-wire-aws-v1.
2. Cause 1 (step task#2): POST /2025-11-30/capacity-providers with a valid CapacityProviderName and VpcConfig but a PermissionsConfig.CapacityProviderOperatorRoleArn that is not a valid/sufficiently-permissioned role. Expect 400 + x-amzn-errortype: InvalidParameterValueException + {"Type":"User","message":"The operator role is invalid or doesn't have sufficient permissions. Verify the role and permissions and try again."}; observe 202 and a created provider.
3. Cause 2 (task#3): GET /2025-11-30/capacity-providers/<capacity-provider ARN that was never created>. Expect 404 + x-amzn-errortype: ResourceNotFoundException + {"Type":"User","Message":"Capacity provider not found: <identifier as supplied>. Check that the ARN is correct and the capacity provider exists in this Region."}; observe 200 with a CapacityProvider.
4. Cause 3 (task#4): PUT the same nonexistent provider with a CapacityProviderScalingConfig. Expect 404 as above; observe 202.
5. Cause 4 (task#5): GET /2025-11-30/capacity-providers/<nonexistent name>/function-versions. Expect 404 as above; observe 200 with a list page.
6. Cause 5 (task#6): DELETE /2025-11-30/capacity-providers/<nonexistent name>. Expect 404 as above; observe 202.
7. Re-replay lambda-capacity-provider-lifecycle steps task#2 through task#6 to confirm all five.
