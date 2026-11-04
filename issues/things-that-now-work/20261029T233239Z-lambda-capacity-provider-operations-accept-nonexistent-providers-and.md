Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-capacity-provider-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-capacity-provider-lifecycle lambda-capacity-provider-negative-paths lambda-extended-api-error-surfaces
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
Issue-Key: lambda|CreateCapacityProvider|4xx->4xx|$.message
Issue-Key: lambda|DeleteCapacityProvider|4xx->4xx|$.Message
Issue-Key: lambda|DeleteCapacityProvider|4xx->4xx|$.Type
Issue-Key: lambda|GetCapacityProvider|4xx->4xx|$.Type
Issue-Key: lambda|UpdateCapacityProvider|4xx->4xx|$.Type
Issue-Key: lambda|ListFunctionVersionsByCapacityProvider|4xx->4xx|$.Type
Issue-Commit: 6fcb2038be76f91f904ab31cfab5a8d60c9030f7

# Lambda capacity-provider operations accept nonexistent providers and misword their refusals

> _Note: this issue was identified against the code base at commit `6fcb2038be76f91f904ab31cfab5a8d60c9030f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCapacityProvider

## Description
Shared fault: the capacity-provider handlers do not resolve the named provider before answering, so operations on a provider that does not exist succeed, and the refusals that are produced carry the wrong message/body. Ten net-new clusters.

AWS: Smithy binds CreateCapacityProvider (202), GetCapacityProvider (200), UpdateCapacityProvider, DeleteCapacityProvider and ListFunctionVersionsByCapacityProvider, each with ResourceNotFoundException (httpError 404, members {Type, Message}) and InvalidParameterValueException (httpError 400, members {Type, message}). Recorded behavior: an unknown provider is refused 404 ResourceNotFoundException, x-amzn-errortype `ResourceNotFoundException`, Message `Capacity provider not found: <identifier>. Check that the ARN is correct and the capacity provider exists in this Region.` — the two sentences are fixed text and `<identifier>` is echoed from the request exactly as the caller supplied it (a bare name when a name was sent, the ARN when an ARN was sent). CreateCapacityProvider with an operator role that is invalid or under-permissioned is refused 400 InvalidParameterValueException, message `The operator role is invalid or doesn't have sufficient permissions. Verify the role and permissions and try again.` (wholly fixed text).

Emulator: unknown-provider requests are answered as if the provider existed (200/202), the operator role is never validated, the not-found Message substitutes a synthesized ARN for the caller's identifier, and `Type` is dropped from every refusal body.

Why: a client cannot distinguish a provider that exists from one that does not — Get returns a body for a nonexistent provider and Delete/Update report success, so a caller's existence checks and cleanup logic silently pass; an invalid operator role is accepted and the failure surfaces only later.

Fix: each capacity-provider handler must resolve the identifier against capacity-provider state first and raise ResourceNotFoundException otherwise; CreateCapacityProvider's validation must check the operator role; the not-found message builder must echo the request identifier rather than rebuild an ARN; the error serializer must emit `Type`.

Cause 1 of 10: CreateCapacityProvider — fixture lambda-capacity-provider-lifecycle step task#2; expected 400 InvalidParameterValueException with the operator-role message, emulator answered 202 and created the provider.
Cause 2 of 10: GetCapacityProvider — fixture lambda-capacity-provider-lifecycle step task#3; expected 404 ResourceNotFoundException for an unknown provider, emulator answered 200.
Cause 3 of 10: UpdateCapacityProvider — fixture lambda-capacity-provider-lifecycle step task#4; expected 404, emulator answered 202.
Cause 4 of 10: ListFunctionVersionsByCapacityProvider — fixture lambda-capacity-provider-lifecycle step task#5; expected 404, emulator answered 200.
Cause 5 of 10: DeleteCapacityProvider — fixture lambda-capacity-provider-lifecycle step task#6; expected 404, emulator answered 202.
Cause 6 of 10: CreateCapacityProvider, 400 — fixture lambda-capacity-provider-negative-paths step task#1; AWS reports the rejected list as a whole: `1 validation error detected: Value '[x86]' at 'instanceRequirements.architectures' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [x86_64, arm64], Member must not be null]` (fixed text; the rejected value and the member path are the variables). The emulator reports the offending element instead: `... Value 'x86' at 'instanceRequirements.architectures.1.member' failed to satisfy constraint: Member must satisfy enum value set: [x86_64, arm64]`.
Cause 7 of 10: DeleteCapacityProvider, 404 — fixture lambda-capacity-provider-negative-paths step task#4; Message must echo the provider name given in the request, emulator substituted the full `arn:aws:lambda:<region>:<account>:capacity-provider:<name>`; `Type` also missing.
Cause 8 of 10: GetCapacityProvider, 404 — fixture lambda-capacity-provider-negative-paths step task#2; `Type` missing.
Cause 9 of 10: UpdateCapacityProvider, 404 — fixture lambda-capacity-provider-negative-paths step task#3; `Type` missing.
Cause 10 of 10: ListFunctionVersionsByCapacityProvider, 404 — fixture lambda-capacity-provider-negative-paths step task#5; `Type` missing.

## Evidence
Cluster details: task#2 `status: expected 400, got 202` plus missing Type/message/x-amzn-errortype `InvalidParameterValueException`; task#3 `status: expected 404, got 200`; task#4 `status: expected 404, got 202`; task#5 `status: expected 404, got 200`; task#6 `status: expected 404, got 202` — each with the expected Message `Capacity provider not found: ...` absent. negative-paths task#4 `$.Message: value_mismatch (expected 'Capacity provider not found: sim2real-probe-lambda-capprov-ae81d595. ...', got 'Capacity provider not found: arn:aws:lambda:us-west-2:000000000000:capacity-provider:sim2real-probe-lambda-capprov-ae81d595. ...')`. negative-paths task#1 shows the list-level vs element-level validation message. Smithy: the five operations and their ResourceNotFoundException/InvalidParameterValueException bindings.

## Steps to Reproduce
Replay the corpus. Fixture lambda-capacity-provider-lifecycle step task#3: GetCapacityProvider for `sim2real-probe-nonexistent-cp` — the emulator returns 200 instead of 404 ResourceNotFoundException. Step task#2: CreateCapacityProvider with an invalid operator role — accepted 202 instead of refused 400.
