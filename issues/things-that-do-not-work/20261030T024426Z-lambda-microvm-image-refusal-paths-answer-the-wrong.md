Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-microvms-image-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-microvms-image-lifecycle
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda|GetMicrovmImage|4xx->4xx|$.message
Issue-Key: lambda|GetMicrovmImage|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda|GetMicrovmImage|4xx->4xx|status
Issue-Key: lambda|GetMicrovmImageBuild|4xx->4xx|$.message
Issue-Key: lambda|ListMicrovmImageVersions|2xx->4xx|$.items
Issue-Key: lambda|ListMicrovmImageVersions|2xx->4xx|status
Issue-Key: lambda|ListMicrovmImageBuilds|2xx->4xx|$.items
Issue-Key: lambda|ListMicrovmImageBuilds|2xx->4xx|status
Issue-Key: lambda|UpdateMicrovmImage|4xx->2xx|$.message
Issue-Key: lambda|UpdateMicrovmImage|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|UpdateMicrovmImage|4xx->2xx|status
Issue-Commit: 3cbee5fc05633571f18b42c799709a3e75b70289

# lambda MicroVM image refusal paths answer the wrong status, error class and message

> _Note: this issue was identified against the code base at commit `3cbee5fc05633571f18b42c799709a3e75b70289`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMicrovmImage

## Description
This issue bundles 5 distinct root causes observed on replay of the wire corpus against the lambda service token, all in the REFUSAL plane of the MicroVM image API: two refusals carry the wrong class or the wrong sentence, two successful enumerations are refused outright, and one request AWS refuses is accepted. A fixer must close all five (Cause 1 of 5 .. Cause 5 of 5) before submitting.

=== Cause 1 of 5: GetMicrovmImage denies an unknown CUSTOMER-account identifier with 403 AccessDeniedException instead of 404 ResourceNotFoundException ===
Fixture lambda-microvms-image-lifecycle, step task#6.
AWS: GetMicrovmImage models ResourceNotFoundException (errorKind client, httpError 404, message @required, 'The specified resource does not exist.'). For an identifier in the CALLER'S OWN account that names no image, the recorded response is HTTP 404 with x-amzn-errortype 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.lambda.microvms/' and message 'MicroVMImage not found for MicroVMImageID: <imageIdentifier>' - 'MicroVMImage not found for MicroVMImageID: ' is fixed text and <imageIdentifier> is the identifier string the request carried, echoed verbatim. AccessDeniedException (service-common, httpError 403, 'You do not have sufficient access to perform this action.') is the answer for an AWS-OWNED managed identifier, not for a dead identifier in the caller's own account.
Emulator: HTTP 403 with x-amzn-errortype 'AccessDeniedException' and the generic message 'You do not have sufficient access to perform this action.' - the deny branch is applied to an identifier that merely does not exist.
Why: 403 and 404 mean different things. A client that reads 404 as 'not created yet, create it' and 403 as 'permission boundary, escalate' takes the wrong branch: it escalates to an operator instead of creating the image, and existence checks built on catching ResourceNotFoundException break outright. Note also the errortype header: AWS sends the class name with its Coral namespace suffix, the emulator sends the bare class name.
Hypothesis: the own-account-not-found path was recently routed into the managed-namespace AccessDenied branch - the resolved issue '20261029T174237Z-lambda-microvms-getmicrovmimage-leaks-existence-with-404-resourcenotfoundexception' made GetMicrovmImage deny AWS-OWNED managed identifiers with 403, and that predicate now appears to catch identifiers that are simply absent from the caller's own account. The managed-ARN behavior must stay; the own-account-absent behavior must go back to 404 with the message template above.
Fix: GetMicrovmImage's handler must classify the identifier before refusing - AWS-owned managed namespace -> 403 AccessDeniedException; resolvable in the caller's account -> success; otherwise -> 404 ResourceNotFoundException with the fixed sentence above.

=== Cause 2 of 5: GetMicrovmImageBuild's not-found message is emulator prose instead of AWS's sentence ===
Fixture lambda-microvms-image-lifecycle, step task#5.
AWS: GetMicrovmImageBuild models ResourceNotFoundException (404, message @required). For a buildId that does not exist under an existing image/version the recorded message is 'Build not found for MicroVMImageID: <imageIdentifier>, VersionID: <imageVersion>, BuildID: <buildId>'. The fixed text is 'Build not found for MicroVMImageID: ', ', VersionID: ' and ', BuildID: '; the three variable parts are taken from the request's imageIdentifier, imageVersion and buildId path labels - never from any one recording.
Emulator: the status and class are right (404 ResourceNotFoundException) but the sentence is the crate's own wording, 'MicroVM image <arn> version <v> has no build <b>'.
Why: the message is part of the error contract - operators grep it, and test suites and support tooling match on AWS's phrasing. Emulator prose makes a correct refusal unrecognizable.
Fix: GetMicrovmImageBuild's not-found path must build the message from AWS's template; the sentence belongs with the other not-found message builders in the service state module rather than inline in the handler.

=== Cause 3 of 5: ListMicrovmImageVersions refuses 404 where AWS returns an empty page ===
Fixture lambda-microvms-image-lifecycle, step task#10.
AWS: ListMicrovmImageVersions is a paginated list whose output items member is @required. For the image identifier this step lists, the recorded response is HTTP 200 with items: [] - an addressable image with no versions left to enumerate yields an EMPTY PAGE, not a refusal; ResourceNotFoundException is reserved for an identifier that does not resolve at all.
Emulator: HTTP 404 and no items member.
Why: a client that polls a list to observe a collection draining to empty (after deleting versions, or before the first one is built) sees a hard error instead of the empty page, and a paginator that treats 404 as fatal aborts a loop AWS completes normally.
Fix: ListMicrovmImageVersions' handler must separate identifier resolution from enumeration and answer 200 with an empty items list once the identifier resolves, reserving 404 for an unresolvable identifier.

=== Cause 4 of 5: ListMicrovmImageBuilds refuses 403 where AWS returns an empty page ===
Fixture lambda-microvms-image-lifecycle, step task#11.
AWS: ListMicrovmImageBuilds is likewise paginated with a @required items member, and the recorded response for the identifier/version this step lists is HTTP 200 with items: [].
Emulator: HTTP 403 AccessDeniedException and no items member - a different wrong answer from Cause 3 for the same shape of request, which is what makes this a separate cause.
Why: as Cause 3, plus the 403 tells the client it lacks permission on a resource it just created, which is the single most misleading answer available.
Hypothesis: the same managed-namespace deny branch implicated in Cause 1 is being reached from the build-list path.
Fix: ListMicrovmImageBuilds' handler must resolve the identifier/version and then enumerate, answering 200 with an empty items list when there is nothing to list; the deny branch must stay confined to AWS-owned managed identifiers.

=== Cause 5 of 5: UpdateMicrovmImage accepts an update AWS refuses for the image's current state ===
Fixture lambda-microvms-image-lifecycle, step task#16.
AWS: UpdateMicrovmImage models ValidationException (service-common, errorKind client, httpError 400, 'The input does not satisfy the constraints specified by the service.'). An image that is not in a state that admits an update is refused: HTTP 400, x-amzn-errortype 'ValidationException:http://internal.amazon.com/coral/com.amazonaws.lambda.microvms/', message 'Cannot update MicroVM Image in its current state: <imageArn>'. 'Cannot update MicroVM Image in its current state: ' is fixed text and <imageArn> is the image the request addressed. (The MicrovmImageState enum carries the non-updatable states - DELETING, DELETED, DELETE_FAILED, CREATE_FAILED and the in-flight CREATING/UPDATING.)
Emulator: HTTP 200 - the update is applied, with no message and no x-amzn-errortype.
Why: the state guard is the only thing stopping a client from mutating an image that is being deleted or is mid-build. The emulator lets that through and returns a success body, so a client never learns its update was impossible and proceeds to run MicroVMs from a version that AWS would never have created.
Fix: UpdateMicrovmImage's handler needs the pre-mutation state guard, raising the modeled ValidationException with the message template above for an image whose current MicrovmImageState does not admit an update.

Traces: lambda-microvms-image-lifecycle#task#6, #task#5, #task#10, #task#11, #task#16.

## Evidence
Wire replay of trace set simparity-wire-aws-v1. Expected = recorded AWS response, Actual = emulator.

Cause 1 (lambda-microvms-image-lifecycle#task#6, GetMicrovmImage):
  status: expected 404, got 403
  $.message: expected 'MicroVMImage not found for MicroVMImageID: <the identifier the request carried>', got 'You do not have sufficient access to perform this action.'
  $headers.x-amzn-errortype: expected 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.lambda.microvms/', got 'AccessDeniedException'

Cause 2 (lambda-microvms-image-lifecycle#task#5, GetMicrovmImageBuild, 404 vs 404):
  $.message: expected 'Build not found for MicroVMImageID: <imageIdentifier>, VersionID: <imageVersion>, BuildID: <buildId>', got 'MicroVM image <imageIdentifier> version <imageVersion> has no build <buildId>'

Cause 3 (lambda-microvms-image-lifecycle#task#10, ListMicrovmImageVersions):
  status: expected 200, got 404
  $.items: expected [], got (absent)

Cause 4 (lambda-microvms-image-lifecycle#task#11, ListMicrovmImageBuilds):
  status: expected 200, got 403
  $.items: expected [], got (absent)

Cause 5 (lambda-microvms-image-lifecycle#task#16, UpdateMicrovmImage):
  status: expected 400, got 200
  $.message: expected 'Cannot update MicroVM Image in its current state: <imageArn>', got (absent)
  $headers.x-amzn-errortype: expected 'ValidationException:http://internal.amazon.com/coral/com.amazonaws.lambda.microvms/', got (absent)

Model grounding (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json): GetMicrovmImage errors = ResourceNotFoundException (404) + common AccessDeniedException (403) / ValidationException (400) / ThrottlingException (429) / InternalServerException (500); ResourceNotFoundException{message @required, resourceType, resourceId}, doc 'The specified resource does not exist.'; ValidationException{message}, httpError 400, doc 'The input does not satisfy the constraints specified by the service.'; UpdateMicrovmImage errors = ConflictException, ResourceNotFoundException, ServiceQuotaExceededException + the common set; ListMicrovmImageVersions / ListMicrovmImageBuilds outputs have items @required and are @paginated.

Related resolved issue: '20261029T174237Z-lambda-microvms-getmicrovmimage-leaks-existence-with-404-resourcenotfoundexception' made GetMicrovmImage deny AWS-OWNED managed identifiers with 403 AccessDeniedException. That behavior is correct and must be preserved; Cause 1 is the over-application of it to identifiers that are simply absent from the caller's own account.

## Steps to Reproduce
Cause 1: GetMicrovmImage(imageIdentifier=<an identifier in your own account that names no image, e.g. '<an existing image name>-missing'>).
  AWS: 404 ResourceNotFoundException, message 'MicroVMImage not found for MicroVMImageID: <that identifier>', x-amzn-errortype 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.lambda.microvms/'. Emulator: 403 AccessDeniedException, 'You do not have sufficient access to perform this action.'
  Control: GetMicrovmImage on an arn:aws:lambda:<region>:aws:microvm-image:<name> managed identifier must STILL answer 403 AccessDeniedException.

Cause 2: GetMicrovmImageBuild(imageIdentifier=<an existing image>, imageVersion='1.0', buildId='<no-such-build>').
  AWS: 404, message 'Build not found for MicroVMImageID: <imageIdentifier>, VersionID: 1.0, BuildID: <no-such-build>'. Emulator: 404 with 'MicroVM image ... has no build ...'.

Cause 3: ListMicrovmImageVersions(imageIdentifier=<an image whose version collection is empty>).
  AWS: 200, {'items': []}. Emulator: 404.

Cause 4: ListMicrovmImageBuilds(imageIdentifier=<the same image>, imageVersion=<that version>).
  AWS: 200, {'items': []}. Emulator: 403 AccessDeniedException.

Cause 5: drive the image into a state that does not admit an update (e.g. DeleteMicrovmImage first, or an image settled CREATE_FAILED), then UpdateMicrovmImage(imageIdentifier=<imageArn>, baseImageArn=..., buildRoleArn=..., codeArtifact=...).
  AWS: 400 ValidationException, message 'Cannot update MicroVM Image in its current state: <imageArn>', x-amzn-errortype 'ValidationException:http://internal.amazon.com/coral/com.amazonaws.lambda.microvms/'. Emulator: 200, the update is applied.
