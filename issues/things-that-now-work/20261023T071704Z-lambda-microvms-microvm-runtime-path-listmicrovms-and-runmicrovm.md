Issue-Source: trace-sweep-divergence
Issue-Trace: lambdamicrovms-absent-microvm-envelopes
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambdamicrovms-absent-microvm-envelopes lambdamicrovms-image-tagging-and-run-refusals
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms|ListMicrovms|2xx->4xx|$.items
Issue-Key: lambda-microvms|ListMicrovms|2xx->4xx|status
Issue-Key: lambda-microvms|RunMicrovm|4xx->2xx|$.message
Issue-Key: lambda-microvms|RunMicrovm|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda-microvms|RunMicrovm|4xx->2xx|status
Issue-Key: lambda-microvms|CreateMicrovmAuthToken|4xx->5xx|$.message
Issue-Key: lambda-microvms|CreateMicrovmAuthToken|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: lambda-microvms|CreateMicrovmAuthToken|4xx->5xx|status
Issue-Key: lambda-microvms|GetMicrovm|4xx->4xx|$.message
Issue-Key: lambda-microvms|GetMicrovm|4xx->4xx|$.resourceId
Issue-Key: lambda-microvms|GetMicrovm|4xx->4xx|$.resourceType
Issue-Key: lambda-microvms|TerminateMicrovm|4xx->4xx|$.message
Issue-Key: lambda-microvms|TerminateMicrovm|4xx->4xx|$.resourceId
Issue-Key: lambda-microvms|TerminateMicrovm|4xx->4xx|$.resourceType
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# lambda-microvms microvm runtime path: ListMicrovms and RunMicrovm wrong status, auth-token unrouted, not-found envelope wrong

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListMicrovms

## Description
Five distinct causes on the lambda-microvms MicroVM runtime surface (ListMicrovms, RunMicrovm, CreateMicrovmAuthToken, GetMicrovm, TerminateMicrovm). They share one shape of fault: absent or unlaunchable resources are mapped to the wrong HTTP outcome — a filtered list is turned into an error, a launch that must be refused succeeds, an operation is not routed at all, and the not-found envelope carries the wrong message template plus members AWS leaves absent. All five must be closed before this issue is submitted.

Cause 1 of 5: ListMicrovms answers 404 instead of an empty page when the imageIdentifier filter matches nothing.
AWS: ListMicrovms (Smithy: GET /2025-09-09/microvms, success code 200, @readonly, @paginated with items=items; ListMicrovmsResponse.items is @required) is a list operation — filtering by an imageIdentifier/imageVersion that selects no MicroVMs yields HTTP 200 with items as an empty list, not a refusal.
Emulator: answers HTTP 404 and emits no items member at all.
Why: 'no results' is indistinguishable from 'bad request'. A paginating client is handed an error where AWS hands it a terminal empty page, so enumeration aborts instead of completing; and because items is @required, a client that reads response.items unconditionally gets a null dereference rather than an empty collection.
Fix: ListMicrovms handler — treat an unmatched filter as an empty result set and always project items (empty list) with status 200.
Traces: lambdamicrovms-absent-microvm-envelopes#task#6.

Cause 2 of 5: RunMicrovm launches from an image that has no active version instead of refusing.
AWS: RunMicrovm (Smithy: POST /2025-09-09/microvms, success code 200; imageIdentifier @required; modeled error ResourceNotFoundException @httpError 404) resolves the image's active version before launching. An image with no active version cannot be launched and is refused with HTTP 404 ResourceNotFoundException (x-amzn-errortype 'ResourceNotFoundException'); the recorded message example is 'No active version found for MicroVM image arn:aws:lambda:us-west-2:000000000000:microvm-image:sim2real-probe-mvmtag-8167e0e4'.
Emulator: answers HTTP 200 with no message and no x-amzn-errortype — the launch is accepted.
Why: this is the most consequential divergence in the bundle. A client is told it has a running MicroVM backed by an image version that does not exist, so it proceeds to mint auth tokens against and connect to a VM that cannot be there, and it is billed-for/tracked as running. Every negative-path guard that depends on the active-version precondition is silently absent.
Fix: RunMicrovm handler — resolve the image's active version (honouring an explicit imageVersion) as a precondition and raise ResourceNotFoundException (404) when there is none.
Traces: lambdamicrovms-image-tagging-and-run-refusals#task#7.

Cause 3 of 5: CreateMicrovmAuthToken is not routed and answers 501 NotImplemented.
AWS: CreateMicrovmAuthToken is bound to the service (Smithy: POST /2025-09-09/microvms/{microvmIdentifier}/auth-token, success code 200, modeled errors ResourceNotFoundException @httpError 404, ConflictException 409, ValidationException 400). For a microvmIdentifier that does not exist it returns HTTP 404 with x-amzn-errortype 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.lambda.microvms/'.
Emulator: answers HTTP 501 with x-amzn-errortype 'NotImplemented' and the message 'operation POST /2025-09-09/microvms/<id>/auth-token is not implemented in this emulator'.
Why: a modeled operation is missing from the router, so the documented way to obtain access to a running MicroVM does not exist. 501 is a server-fault class no SDK retry or error-handling path expects from this API, so clients surface an infrastructure failure rather than the modeled 404/409/400, and nothing downstream of token issuance can be exercised at all.
Fix: route and implement CreateMicrovmAuthToken, including its not-found precondition on microvmIdentifier.
Traces: lambdamicrovms-absent-microvm-envelopes#task#5.

Cause 4 of 5: the ResourceNotFoundException envelope on GetMicrovm uses a generic template and adds resourceType/resourceId that AWS leaves absent.
AWS: GetMicrovm (Smithy: GET /2025-09-09/microvms/{microvmIdentifier}, modeled error ResourceNotFoundException @httpError 404). ResourceNotFoundException models message (@required) plus optional resourceType and resourceId, and AWS does not populate the two optional members on this path — the recording shows them absent. The message is built from the requested identifier, so its template is a known rule: 'MicroVM not found: <microvmIdentifier>'.
Emulator: returns 404 (status correct) but with the message 'no MicroVM matches the identifier <id>' and additionally sets resourceId=<id> and resourceType='Microvm'.
Why: the body does not match what AWS puts on the wire. Clients that parse the not-found message for the identifier, or that branch on whether resourceType/resourceId are present (they are the discriminator for which resource a composite call failed on), read fields AWS never sends and a template AWS never emits. 'Microvm' is also not an AWS resource-type spelling.
Fix: the ResourceNotFoundException construction for the microvm lookup path — use the request-derived not-found template and leave resourceType/resourceId unset.
Traces: lambdamicrovms-absent-microvm-envelopes#task#1.

Cause 5 of 5: the same wrong not-found envelope on TerminateMicrovm.
AWS: TerminateMicrovm (Smithy: DELETE /2025-09-09/microvms/{microvmIdentifier}, modeled error ResourceNotFoundException @httpError 404) refuses an unknown MicroVM with 404 and the same request-derived message template 'MicroVM not found: <microvmIdentifier>', with resourceType and resourceId absent.
Emulator: returns 404 with 'no MicroVM matches the identifier <id>' and populates resourceId and resourceType='Microvm'.
Why: same as cause 4, on the teardown path — a client reconciling a terminate against AWS's envelope sees extra members and a different message, so cleanup logic keyed on the envelope misclassifies the outcome. It is listed separately because it is a second construction site: fixing only the GetMicrovm path leaves this one diverging.
Fix: the ResourceNotFoundException construction in the TerminateMicrovm handler — share the corrected not-found envelope.
Traces: lambdamicrovms-absent-microvm-envelopes#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200). Expected values are the recorded AWS responses.

Cause 1 — lambdamicrovms-absent-microvm-envelopes#task#6, ListMicrovms: expected status 200, got 404; $.items missing (expected []).

Cause 2 — lambdamicrovms-image-tagging-and-run-refusals#task#7, RunMicrovm: expected status 404, got 200. $.message missing (expected 'No active version found for MicroVM image arn:aws:lambda:us-west-2:000000000000:microvm-image:sim2real-probe-mvmtag-8167e0e4'); $headers.x-amzn-errortype missing (expected 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.lambda.microvms/').

Cause 3 — lambdamicrovms-absent-microvm-envelopes#task#5, CreateMicrovmAuthToken: expected status 404, got 501. $.message expected 'MicroVM not found: microvm-00000000-0000-0000-0000-000000000000', got 'operation POST /2025-09-09/microvms/microvm-00000000-0000-0000-0000-000000000000/auth-token is not implemented in this emulator'; $headers.x-amzn-errortype expected 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.lambda.microvms/', got 'NotImplemented'.

Cause 4 — lambdamicrovms-absent-microvm-envelopes#task#1, GetMicrovm: status 404 both sides. $.message expected 'MicroVM not found: microvm-00000000-0000-0000-0000-000000000000', got 'no MicroVM matches the identifier microvm-00000000-0000-0000-0000-000000000000'; $.resourceId expected absent, got 'microvm-00000000-0000-0000-0000-000000000000'; $.resourceType expected absent, got 'Microvm'.

Cause 5 — lambdamicrovms-absent-microvm-envelopes#task#4, TerminateMicrovm: status 404 both sides, same three body divergences as cause 4 ($.message, $.resourceId, $.resourceType).

Model grounding: services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json — ListMicrovms (GET /2025-09-09/microvms, 200, @paginated, ListMicrovmsResponse.items @required), RunMicrovm (POST /2025-09-09/microvms, 200, ResourceNotFoundException 404), CreateMicrovmAuthToken (POST /2025-09-09/microvms/{microvmIdentifier}/auth-token, 200, ResourceNotFoundException 404), ResourceNotFoundException (@httpError 404; message @required, resourceType/resourceId optional).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from the checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus) and read the lambda-microvms clusters.
3. Per cause, against the emulator's lambda-microvms endpoint:
   - Cause 1: GET /2025-09-09/microvms?imageIdentifier=<image-with-no-microvms> -> observe 404 with no items instead of 200 {"items": []}. Fixture lambdamicrovms-absent-microvm-envelopes#task#6.
   - Cause 2: create an image whose only version is not active, then POST /2025-09-09/microvms with that imageIdentifier -> observe 200 instead of 404 ResourceNotFoundException. Fixture lambdamicrovms-image-tagging-and-run-refusals#task#7.
   - Cause 3: POST /2025-09-09/microvms/microvm-00000000-0000-0000-0000-000000000000/auth-token -> observe 501 NotImplemented instead of 404 ResourceNotFoundException. Fixture lambdamicrovms-absent-microvm-envelopes#task#5.
   - Cause 4: GET /2025-09-09/microvms/microvm-00000000-0000-0000-0000-000000000000 -> inspect the 404 body for the generic message template and the extra resourceId/resourceType. Fixture lambdamicrovms-absent-microvm-envelopes#task#1.
   - Cause 5: DELETE /2025-09-09/microvms/microvm-00000000-0000-0000-0000-000000000000 -> same 404 body divergences. Fixture lambdamicrovms-absent-microvm-envelopes#task#4.
4. Re-replay all five fixture#step bindings listed in trace_ids plus the representative to confirm a fix.
