Issue-Source: trace-sweep-divergence
Issue-Trace: synthetics-group-canary-association-errors
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: synthetics-group-canary-association-errors
Issue-Shape-Id: com.amazonaws.synthetics#Synthetics
Issue-Key: synthetics|DisassociateResource|4xx->4xx|$.Message
Issue-Key: synthetics|DisassociateResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: synthetics|DisassociateResource|4xx->4xx|status
Issue-Commit: 81fd89040b63f7cd7e51a42e1e801a2421dd5e81

# synthetics: DisassociateResource answers 404 ResourceNotFoundException instead of AWS's 400 ValidationException for a canary that is not a member of the group

> _Note: this issue was identified against the code base at commit `81fd89040b63f7cd7e51a42e1e801a2421dd5e81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisassociateResource

## Description
One root cause in the emulated CloudWatch Synthetics service.

=== Cause 1 of 1: DisassociateResource does not model AWS's "canary is not in this group" rejection — it 404s on an unresolvable canary ARN and silently succeeds on a resolvable non-member ===

AWS: Smithy model (services/service-com-amazonaws-synthetics-synthetics/smithy-model/com-amazonaws-synthetics-synthetics.json) binds `DisassociateResource` (PATCH /group/{GroupIdentifier}/disassociate) to errors `ValidationException` (httpError 400, errorKind client, "A parameter could not be validated."), `ResourceNotFoundException` (404), `ConflictException` (409) and `InternalServerException` (500); input is `GroupIdentifier` (httpLabel, required, len 1..128) and `ResourceArn` (target CanaryArn, required, len 1..2048, pattern `^arn:(aws[a-zA-Z-]*)?:synthetics:[a-z]{2,4}(-[a-z]{2,4})?-[a-z]+-\d{1}:\d{12}:canary:[0-9a-z_\-]{1,255}$`). Recorded behavior: when the `ResourceArn` the request names is NOT a member of the group named by `GroupIdentifier`, AWS refuses the call with HTTP **400**, header `x-amzn-errortype: ValidationException`, and `Message` of the fixed form `Canary <resourceArn> does not exist in group <groupName>`. The fixed text is "Canary " / " does not exist in group "; the variable parts are the `ResourceArn` exactly as supplied in the request and the group's NAME resolved from `GroupIdentifier`. AWS reaches that verdict from group membership, not from a canary-registry lookup: in the recorded step the ARN resolves to no existing canary at all and AWS still answers the membership ValidationException rather than a ResourceNotFoundException, so "not a member" subsumes "no such canary" for this operation.

Emulator: the handler validates the canary as a resource first and then treats removal as a set operation. services/service-com-amazonaws-synthetics-synthetics/src/ops/disassociate_resource.rs resolves `guard.canary_by_arn(&input.resource_arn)` and, when that misses, returns `not_found(canary_not_found(&input.resource_arn))` — HTTP 404, `x-amzn-errortype: ResourceNotFoundException`, `Message` = `Canary does not exist: <resourceArn>`. That is what the replayed step got (status 404 where 400 was recorded, errortype ResourceNotFoundException where ValidationException was recorded, and the emulator-authored "Canary does not exist: <arn>" sentence where AWS's "Canary <arn> does not exist in group <group>" was recorded). The same handler also never produces this rejection for the membership itself: when the canary DOES resolve but is not in the group, `group.resources.remove(...)` returns false and the handler deliberately answers 200 with an empty body (audit outcome "no_change"), so AWS's ValidationException is unreachable on both paths into it.

Why: a client that removes a canary from a group cannot distinguish the outcomes AWS distinguishes. Error-handling keyed on `ValidationException` (a request naming a canary the group does not hold) never fires; instead the client sees either a `ResourceNotFoundException`, which it is entitled to read as "the group/canary resource is gone" and to retry or re-create against, or a flat 200 that it reads as proof the membership existed and was removed. The HTTP class also differs (400 vs 404), so retry policies and SDK error taxonomies branch the wrong way, and cleanup code that disassociates defensively cannot tell an accepted removal from a rejected one.

Fix: the `DisassociateResource` handler's validation and its membership-removal branch in services/service-com-amazonaws-synthetics-synthetics/src/ops/disassociate_resource.rs — the canary-resolution miss must stop producing the 404 `ResourceNotFoundException` for this operation, and the "removed nothing" case must return the modeled `ValidationException` (400) carrying the message above with the request's `ResourceArn` and the resolved group name; the group-identifier miss keeps its own 404. A shared message helper next to the existing `canary_not_found`/`group_not_found` in src/state.rs keeps the wording from drifting against AssociateResource's paired refusal.

Traces: synthetics-group-canary-association-errors#task#4.

## Evidence
Corpus simparity-wire-aws-v1, fixture `synthetics-group-canary-association-errors`, step `task#4`, operation DisassociateResource, verdict mismatch:
- status: expected 400, got 404
- $headers.x-amzn-errortype: expected 'ValidationException', got 'ResourceNotFoundException'
- $.Message: expected 'Canary arn:aws:synthetics:us-west-2:000000000000:canary:sim2real-probe-synthetics-assoc-canary-9c17c16d does not exist in group sim2real-probe-synthetics-assoc-group-9c17c16d', got 'Canary does not exist: arn:aws:synthetics:us-west-2:000000000000:canary:sim2real-probe-synthetics-assoc-canary-9c17c16d'

The canary ARN, the generated group name and the account id above are per-run values and are NOT part of the requirement: the requirement is the 400 status, the `ValidationException` errortype, and the message template "Canary <ResourceArn from the request> does not exist in group <group name resolved from GroupIdentifier>".

Source: src/ops/disassociate_resource.rs lines 158-162 (canary-resolution miss -> `not_found(canary_not_found(...))`, a 404 ResourceNotFoundException) and lines 164-182 (`removed == false` falls through to `Ok(DisassociateResourceOutput {})` with audit outcome "no_change"). Modeled errors for the operation include ValidationException (400), which this handler never constructs.

Related but NOT covering this: open issue 20261029T184613Z-synthetics-4-net-new-root-causes-duplicate-associateresource.md files AssociateResource's missing duplicate-membership ConflictException and the state-transition message wording; it says nothing about DisassociateResource's non-member rejection.

## Steps to Reproduce
1. CreateGroup with a fresh name, e.g. `<group>`.
2. Call DisassociateResource(GroupIdentifier=`<group>`, ResourceArn=a well-formed canary ARN that the group does not hold — in the recorded step an ARN of the form `arn:aws:synthetics:<region>:<account>:canary:<name>` naming a canary that does not exist).
3. Emulator answers HTTP 404, `x-amzn-errortype: ResourceNotFoundException`, Message `Canary does not exist: <arn>`. AWS answers HTTP 400, `x-amzn-errortype: ValidationException`, Message `Canary <arn> does not exist in group <group>`.
4. Second path into the same cause: CreateCanary and wait for READY, do NOT associate it, then DisassociateResource(GroupIdentifier=`<group>`, ResourceArn=that canary's ARN). Emulator answers HTTP 200 with an empty body; AWS answers the same 400 ValidationException "Canary <arn> does not exist in group <group>".
