Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-microvms-image-lifecycle
Issue-Step: teardown#20
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-microvms-image-lifecycle lambda-microvms-microvm-error-paths
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda|GetMicrovmImage|4xx->2xx|$
Issue-Key: lambda|GetMicrovmImage|4xx->2xx|$.message
Issue-Key: lambda|GetMicrovmImage|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|GetMicrovmImage|4xx->2xx|status
Issue-Key: lambda|CreateMicrovmAuthToken|4xx->5xx|$.message
Issue-Key: lambda|CreateMicrovmAuthToken|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: lambda|CreateMicrovmAuthToken|4xx->5xx|status
Issue-Key: lambda|CreateMicrovmImage|2xx->2xx|$.baseImageVersion
Issue-Key: lambda|CreateMicrovmImage|2xx->2xx|$.egressNetworkConnectors
Issue-Key: lambda|CreateMicrovmImage|2xx->2xx|$.id
Issue-Key: lambda|CreateMicrovmImage|2xx->2xx|$.imageVersion
Issue-Key: lambda|CreateMicrovmImage|2xx->2xx|$.updatedAt
Issue-Key: lambda|GetMicrovmImage|2xx->2xx|$.id
Issue-Key: lambda|GetMicrovmImage|2xx->2xx|$.updatedAt
Issue-Key: lambda|GetMicrovmImage|4xx->4xx|$.message
Issue-Key: lambda|GetMicrovmImage|4xx->4xx|$.resourceId
Issue-Key: lambda|GetMicrovmImage|4xx->4xx|$.resourceType
Issue-Key: lambda|GetMicrovm|4xx->4xx|$.message
Issue-Key: lambda|GetMicrovm|4xx->4xx|$.resourceId
Issue-Key: lambda|GetMicrovm|4xx->4xx|$.resourceType
Issue-Key: lambda|RunMicrovm|4xx->4xx|$.message
Issue-Key: lambda|RunMicrovm|4xx->4xx|$.resourceId
Issue-Key: lambda|RunMicrovm|4xx->4xx|$.resourceType
Issue-Key: lambda|TerminateMicrovm|4xx->4xx|$.message
Issue-Key: lambda|TerminateMicrovm|4xx->4xx|$.resourceId
Issue-Key: lambda|TerminateMicrovm|4xx->4xx|$.resourceType
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# lambda microvm image and instance paths: delete not durable, auth-token unrouted, response and error envelopes wrong

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMicrovmImage

## Description
Eight distinct causes reported under the 'lambda' service token on the MicroVM image and instance surface. They share one shape of fault: created state is projected incompletely and deleted state is not removed, while every refusal on these paths is built from a generic not-found template (or not routed at all) rather than the modeled envelope. All eight must be closed before this issue is submitted.

Cause 1 of 8: DeleteMicrovmImage does not delete — the image stays readable indefinitely.
AWS: DeleteMicrovmImage (Smithy: DELETE /2025-09-09/microvm-images/{imageIdentifier}, documented idempotent) removes the image; GetMicrovmImage then answers HTTP 404 ResourceNotFoundException with x-amzn-errortype 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.lambda.microvms/'.
Emulator: after the delete, GetMicrovmImage keeps answering HTTP 200 with the full image body; the fixture polled for 404 to the 60s cap over 13 attempts and never saw it.
Why: deletion is not durable, so resources leak and any cleanup-then-wait sequence blocks until the client's own timeout. A subsequent create reusing the name then collides with state that should no longer exist.
Fix: DeleteMicrovmImage handler and the image state store — actually remove the image and its versions so later reads miss.
Traces: lambda-microvms-image-lifecycle#teardown#20.

Cause 2 of 8: CreateMicrovmAuthToken is not routed and answers 501 NotImplemented.
AWS: CreateMicrovmAuthToken is bound to the service (Smithy: POST /2025-09-09/microvms/{microvmIdentifier}/auth-token, success code 200; modeled errors ResourceNotFoundException @httpError 404, ConflictException 409, ValidationException 400). An unknown microvmIdentifier yields HTTP 404 ResourceNotFoundException.
Emulator: answers HTTP 501 with x-amzn-errortype 'NotImplemented' and 'operation POST /2025-09-09/microvms/<id>/auth-token is not implemented in this emulator'.
Why: a modeled operation is absent from the router, so the documented route to access a running MicroVM does not exist and clients get a server-fault class (501) that no SDK error path for this API expects, instead of the modeled client errors.
Fix: route and implement CreateMicrovmAuthToken, including its not-found precondition.
Traces: lambda-microvms-microvm-error-paths#task#5.

Cause 3 of 8: the CreateMicrovmImage 201 body drops modeled members and mints the wrong first version number.
AWS: CreateMicrovmImageResponse models baseImageVersion, egressNetworkConnectors and updatedAt alongside the @required imageVersion; the recording shows the first version minted as '1.0' and an 'id' member in the body (recorded behavior only — 'id' is not in the on-disk model).
Emulator: returns 201 but omits baseImageVersion, egressNetworkConnectors, id and updatedAt, and reports imageVersion as '1' rather than '1.0'.
Why: the client cannot confirm the configuration the service accepted, and the version string it must feed back into version-addressed calls (GetMicrovmImageVersion, RunMicrovm, DeleteMicrovmImageVersion) is in a format the service does not name, so follow-up calls built from this response target a nonexistent version.
Fix: CreateMicrovmImage serializer and the image state record — project the full modeled aggregate and mint versions in the service's major.minor form.
Traces: lambda-microvms-image-lifecycle#task#2.

Cause 4 of 8: the GetMicrovmImage 200 body omits id and updatedAt.
AWS: GetMicrovmImageOutput models updatedAt, and the recording also carries an 'id' member (recorded behavior only for 'id').
Emulator: returns 200 without id or updatedAt.
Why: clients that reconcile drift or order events by last-modified time have no updatedAt to compare, so change detection on an image silently degrades to 'never changed'.
Fix: the GetMicrovmImage projection and the image state record — persist and emit the identity and last-updated members.
Traces: lambda-microvms-image-lifecycle#task#3.

Cause 5 of 8: the image not-found envelope uses a generic template and adds resourceType/resourceId AWS leaves absent.
AWS: ResourceNotFoundException (Smithy: @httpError 404; message @required, resourceType and resourceId optional) is returned by GetMicrovmImage for a missing image with the two optional members absent and a request-derived message, recorded template 'MicroVMImage not found for MicroVMImageID: <imageIdentifier>'.
Emulator: returns 404 (status correct) with 'no MicroVM image matches the identifier <id>' and sets resourceId=<id>, resourceType='MicrovmImage'.
Why: the wire body differs from AWS's. Clients that branch on whether resourceType/resourceId are present — they discriminate which resource a composite call failed on — read members AWS never sends, and 'MicrovmImage' is not an AWS resource-type spelling.
Fix: the ResourceNotFoundException construction on the image lookup path — use the request-derived template and leave resourceType/resourceId unset.
Traces: lambda-microvms-image-lifecycle#task#6.

Cause 6 of 8: the same wrong not-found envelope on GetMicrovm.
AWS: GetMicrovm (Smithy: GET /2025-09-09/microvms/{microvmIdentifier}, ResourceNotFoundException @httpError 404) refuses an unknown MicroVM with the request-derived template 'MicroVM not found: <microvmIdentifier>' and no resourceType/resourceId.
Emulator: 404 with 'no MicroVM matches the identifier <id>' plus resourceId and resourceType='Microvm'.
Why: same as cause 5 on the instance read path; listed separately because it is a second construction site that the image-path fix does not reach.
Fix: the ResourceNotFoundException construction in the GetMicrovm handler.
Traces: lambda-microvms-microvm-error-paths#task#2.

Cause 7 of 8: RunMicrovm reports an image-lookup miss where AWS reports an active-version miss.
AWS: RunMicrovm (Smithy: POST /2025-09-09/microvms, 200; ResourceNotFoundException @httpError 404) resolves the image's active version, and its 404 says so — recorded message 'No active version found for MicroVM image <imageArn>' — with resourceType/resourceId absent.
Emulator: 404 with 'no MicroVM image matches the identifier <arn>' plus resourceId and resourceType='MicrovmImage'.
Why: the two conditions are operationally different — 'image absent' means create the image, 'no active version' means wait for or publish a version. Collapsing them into an image-lookup miss sends a client down the wrong remediation path, and the extra envelope members are not what AWS puts on the wire.
Fix: RunMicrovm handler — resolve the active version as a distinct precondition and raise the active-version not-found envelope.
Traces: lambda-microvms-microvm-error-paths#task#7.

Cause 8 of 8: the same wrong not-found envelope on TerminateMicrovm.
AWS: TerminateMicrovm (Smithy: DELETE /2025-09-09/microvms/{microvmIdentifier}, ResourceNotFoundException @httpError 404) refuses an unknown MicroVM with 'MicroVM not found: <microvmIdentifier>' and no resourceType/resourceId.
Emulator: 404 with 'no MicroVM matches the identifier <id>' plus resourceId and resourceType='Microvm'.
Why: same as causes 5 and 6, on the teardown path; a third construction site, so it diverges independently of the other two.
Fix: the ResourceNotFoundException construction in the TerminateMicrovm handler — share the corrected envelope.
Traces: lambda-microvms-microvm-error-paths#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200). Expected values are the recorded AWS responses.

Cause 1 — lambda-microvms-image-lifecycle#teardown#20, GetMicrovmImage: expected status 404, got 200; poll capped at 60s, 'error code None not in [ResourceNotFoundException] after 13 attempt(s)'. $.message missing (expected 'MicroVMImage not found for MicroVMImageID: arn:aws:lambda:us-west-2:000000000000:microvm-image:sim2real-probe-lambda-microvms-microvmimg-9a120e37'); $headers.x-amzn-errortype missing (expected 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.lambda.microvms/'). Body still state CREATED, latestActiveImageVersion '1'.

Cause 2 — lambda-microvms-microvm-error-paths#task#5, CreateMicrovmAuthToken: expected status 404, got 501. $.message expected 'MicroVM not found: sim2real-probe-lambda-microvms-no-such-microvm-173f412d', got 'operation POST /2025-09-09/microvms/sim2real-probe-lambda-microvms-no-such-microvm-173f412d/auth-token is not implemented in this emulator'; $headers.x-amzn-errortype got 'NotImplemented'.

Cause 3 — lambda-microvms-image-lifecycle#task#2, CreateMicrovmImage: status 201 both sides. $.baseImageVersion missing (expected '1.0'); $.egressNetworkConnectors missing (expected ['arn:aws:lambda:us-west-2:aws:network-connector:aws-network-connector:INTERNET_EGRESS']); $.id missing; $.updatedAt missing; $.imageVersion expected '1.0', got '1'.

Cause 4 — lambda-microvms-image-lifecycle#task#3, GetMicrovmImage: status 200 both sides. $.id missing; $.updatedAt missing.

Cause 5 — lambda-microvms-image-lifecycle#task#6, GetMicrovmImage: status 404 both sides. $.message expected 'MicroVMImage not found for MicroVMImageID: arn:aws:lambda:us-west-2:000000000000:microvm-image:...-missing-9a120e37', got 'no MicroVM image matches the identifier arn:...-missing-9a120e37'; $.resourceId expected absent, got the ARN; $.resourceType expected absent, got 'MicrovmImage'.

Cause 6 — lambda-microvms-microvm-error-paths#task#2, GetMicrovm: status 404 both sides. $.message expected 'MicroVM not found: sim2real-probe-lambda-microvms-no-such-microvm-173f412d', got 'no MicroVM matches the identifier ...'; $.resourceId expected absent, got the id; $.resourceType expected absent, got 'Microvm'.

Cause 7 — lambda-microvms-microvm-error-paths#task#7, RunMicrovm: status 404 both sides. $.message expected 'No active version found for MicroVM image arn:aws:lambda:us-west-2:000000000000:microvm-image:sim2real-probe-lambda-microvms-no-such-image-173f412d', got 'no MicroVM image matches the identifier arn:...'; $.resourceId expected absent, got the ARN; $.resourceType expected absent, got 'MicrovmImage'.

Cause 8 — lambda-microvms-microvm-error-paths#task#8, TerminateMicrovm: status 404 both sides; same three body divergences as cause 6.

Model grounding: services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json — DeleteMicrovmImage, CreateMicrovmAuthToken (POST .../auth-token, 200, ResourceNotFoundException 404), CreateMicrovmImageResponse, GetMicrovmImageOutput, RunMicrovm, GetMicrovm, TerminateMicrovm, ResourceNotFoundException (@httpError 404; message @required, resourceType/resourceId optional).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from the checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus) and read the clusters whose service is 'lambda'.
3. Per cause, against the emulator's endpoint:
   - Cause 1: DELETE /2025-09-09/microvm-images/<id>, then poll GET /2025-09-09/microvm-images/<id> -> 200 forever instead of 404. Fixture lambda-microvms-image-lifecycle#teardown#20.
   - Cause 2: POST /2025-09-09/microvms/<unknown-id>/auth-token -> 501 NotImplemented instead of 404. Fixture lambda-microvms-microvm-error-paths#task#5.
   - Cause 3: POST /2025-09-09/microvm-images with baseImageVersion and egressNetworkConnectors set -> compare the 201 body to CreateMicrovmImageResponse and check imageVersion ('1' vs '1.0'). Fixture lambda-microvms-image-lifecycle#task#2.
   - Cause 4: GET /2025-09-09/microvm-images/<id> -> id and updatedAt absent. Fixture lambda-microvms-image-lifecycle#task#3.
   - Cause 5: GET /2025-09-09/microvm-images/<absent-arn> -> inspect the 404 message template and the extra resourceId/resourceType. Fixture lambda-microvms-image-lifecycle#task#6.
   - Cause 6: GET /2025-09-09/microvms/<unknown-id> -> same 404 envelope divergences. Fixture lambda-microvms-microvm-error-paths#task#2.
   - Cause 7: POST /2025-09-09/microvms with an imageIdentifier that has no active version -> message reports an image-lookup miss instead of an active-version miss. Fixture lambda-microvms-microvm-error-paths#task#7.
   - Cause 8: DELETE /2025-09-09/microvms/<unknown-id> -> same 404 envelope divergences. Fixture lambda-microvms-microvm-error-paths#task#8.
4. Re-replay all eight fixture#step bindings listed in trace_ids plus the representative to confirm a fix.
