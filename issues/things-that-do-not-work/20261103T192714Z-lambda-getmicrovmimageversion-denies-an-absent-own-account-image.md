Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-microvms-image-lifecycle
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-microvms-image-lifecycle
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda|GetMicrovmImageVersion|4xx->4xx|$.message
Issue-Key: lambda|GetMicrovmImageVersion|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda|GetMicrovmImageVersion|4xx->4xx|status
Issue-Commit: 2b5605b2de23550e6c1f97402ef3aec1933f6882

# lambda GetMicrovmImageVersion denies an absent own-account image version with 403 AccessDeniedException instead of 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `2b5605b2de23550e6c1f97402ef3aec1933f6882`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMicrovmImageVersion

## Description
One root cause, observed on replay of the wire corpus against the lambda service token. This cluster carries no stub_path and no owning issue.

=== Cause 1 of 1: GetMicrovmImageVersion refuses an unresolvable own-account image identifier with 403 AccessDeniedException instead of the modeled 404 ResourceNotFoundException ===
Fixture lambda-microvms-image-lifecycle, step task#8.

AWS: Smithy GetMicrovmImageVersion (GET /2025-09-09/microvm-images/{imageIdentifier}/versions/{imageVersion}, imageIdentifier and imageVersion both @required @httpLabel) models ResourceNotFoundException{message @required, resourceType, resourceId}, errorKind client, httpError 404, doc 'The specified resource does not exist.', alongside the service-common AccessDeniedException (httpError 403, doc/message 'You do not have sufficient access to perform this action.'). For an imageIdentifier that lives in the CALLER'S OWN account but resolves to no image, AWS answers HTTP 404 with x-amzn-errortype 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.lambda.microvms/' and message 'MicroVMImage not found for MicroVMImage: <imageIdentifier>, Version: <imageVersion>'. Fixed text: 'MicroVMImage not found for MicroVMImage: ' and ', Version: '. Variable parts: <imageIdentifier> is the identifier label the request carried, echoed verbatim, and <imageVersion> is the version label the request carried. (In the recording those happen to be an ARN ending '-missing-9a120e37' and '1'; neither value is part of the requirement.) AccessDeniedException is AWS's answer for an AWS-OWNED managed identifier, not for an identifier that is simply absent from the caller's own account.

Emulator: HTTP 403 with x-amzn-errortype 'AccessDeniedException' (bare class name, without the Coral namespace suffix AWS appends) and the generic message 'You do not have sufficient access to perform this action.' — the managed-namespace deny branch is reached for an identifier that merely does not exist.

Why: 403 and 404 are different contracts. A client that reads 404 as 'not created yet — create it' and 403 as 'permission boundary — escalate to an operator' takes the wrong branch and pages a human instead of creating the version; existence probes written as 'catch ResourceNotFoundException' do not catch AccessDeniedException and propagate a fatal error. The bare errortype header additionally breaks SDK error-shape resolution that keys on the namespaced class token.

Fix: GetMicrovmImageVersion's handler must classify the identifier before refusing — AWS-owned managed namespace -> 403 AccessDeniedException; resolvable in the caller's account -> 200; otherwise -> the modeled 404 ResourceNotFoundException with the message template above, and the errortype header must carry the namespaced class token. The not-found sentence belongs with the other not-found message builders in the service's state module rather than inline in the handler.

Related open issue, which does NOT carry this replay key: '20261030T024426Z-lambda-microvm-image-refusal-paths-answer-the-wrong.md' files the same over-application of the deny branch for GetMicrovmImage at lambda-microvms-image-lifecycle#task#6. The version-level read path at task#8 is a distinct step that issue never exercises, and its message template differs ('MicroVMImage not found for MicroVMImage: <id>, Version: <v>' vs 'MicroVMImage not found for MicroVMImageID: <id>'), so it is filed here. A fixer should expect the two to converge on the same identifier-classification change but must re-replay task#8.

Traces: lambda-microvms-image-lifecycle#task#8.

## Evidence
Wire replay of trace set simparity-wire-aws-v1 (emulator up; 81 steps, 52 passed, score 64.2%, 18 divergence clusters). Expected = recorded AWS response, Actual = emulator.

Cause 1 — lambda-microvms-image-lifecycle#task#8, GetMicrovmImageVersion:
  status: expected 404, got 403
  $.message: expected 'MicroVMImage not found for MicroVMImage: <the imageIdentifier the request carried>, Version: <the imageVersion the request carried>', got 'You do not have sufficient access to perform this action.'
  $headers.x-amzn-errortype: expected 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.lambda.microvms/', got 'AccessDeniedException'
  divergence_key: service=lambda|GetMicrovmImageVersion|status=404->403|keys=$.message,$headers.x-amzn-errortype,status

Model grounding (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json): GetMicrovmImageVersion input imageIdentifier (@required, @httpLabel, @length 1..256) + imageVersion (@required, @httpLabel, @length 1..2048, pattern ^[^\s]+$); errors = AccessDeniedException (403), InternalServerException (500), ResourceNotFoundException (404), ThrottlingException (429), ValidationException (400); ResourceNotFoundException{message @required, resourceType, resourceId}, errorKind client, httpError 404, doc 'The specified resource does not exist.'

## Steps to Reproduce
1. mvm = client('lambda-microvms'); create an image in your own account so a sibling identifier is known-good.
2. GetMicrovmImageVersion(imageIdentifier='<an identifier in your OWN account that names no image, e.g. "<an existing image arn>-missing">', imageVersion='1').
   AWS: 404, x-amzn-errortype 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.lambda.microvms/', message 'MicroVMImage not found for MicroVMImage: <that identifier>, Version: 1'.
   Emulator: 403, x-amzn-errortype 'AccessDeniedException', message 'You do not have sufficient access to perform this action.'
3. Control that must NOT regress: GetMicrovmImageVersion against an AWS-owned managed identifier (arn:aws:lambda:<region>:aws:microvm-image:<name>) must still answer 403 AccessDeniedException.
