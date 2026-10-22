Issue-Source: trace-sweep-divergence
Issue-Trace: medical-imaging-tag-resource-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medical-imaging-tag-resource-error-paths
Issue-Shape-Id: com.amazonaws.medicalimaging#AHIGatewayService
Issue-Key: medical-imaging|TagResource|4xx->4xx|$.message
Issue-Key: medical-imaging|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: medical-imaging|UntagResource|4xx->4xx|$.message
Issue-Key: medical-imaging|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: medical-imaging|ListTagsForResource|4xx->4xx|$.message
Issue-Key: medical-imaging|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 688840645544bc9bcffb33ac47ee3cc89851c839

# medical-imaging tagging operations raise ValidationException instead of BadRequestException for a malformed resourceArn

> _Note: this issue was identified against the code base at commit `688840645544bc9bcffb33ac47ee3cc89851c839`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
All three resource-tagging operations of AWS HealthImaging (medical-imaging) reject a syntactically invalid `resourceArn` path label with the WRONG error code: the emulator returns `ValidationException` where the service returns `BadRequestException`. The HTTP status (400) is correct in every case; only the error code (and its message) diverge, so clients that branch on the error code take the wrong branch. 3 causes, one per tagging operation.

Cause 1 of 3: TagResource with resourceArn='not-a-valid-arn' (POST /tags/{resourceArn}).
AWS: Smithy: TagResource.resourceArn is an @httpLabel @required member targeting com.amazonaws.medicalimaging#Arn with @pattern ^arn:aws((-us-gov)|(-iso)|(-iso-b)|(-cn))?:medical-imaging:[a-z0-9-]+:[0-9]{12}:datastore/[0-9a-z]{32}(/imageset/[0-9a-z]{32})?$. The model defines com.amazonaws.medicalimaging#BadRequestException (@error client, @httpError 400, doc 'The request is invalid or malformed.'), and the recording shows the service answering this path with HTTP 400 and x-amzn-errortype: BadRequestException, message e.g. 'Invalid input resource arn: bad syntax' (that text is an example of the rule 'malformed resource ARN on a /tags/{resourceArn} route is a BadRequestException, not a constraint-validation failure').
Emulator: HTTP 400 with x-amzn-errortype: ValidationException and the generic constraint message "1 validation error detected: Value 'not-a-valid-arn' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws((-us-gov)|...$".
Why: a client (or the AWS SDK's modeled error dispatch) catching BadRequestException for a malformed ARN sees an unmodelled-for-this-path ValidationException instead, so retry/abort and user-facing error handling pick the wrong branch; SDK exception-type assertions fail even though the status matches.
Fix: the TagResource handler's resourceArn validation in the medical-imaging service crate (services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/, tagging operations) must classify an unparsable resourceArn label as BadRequestException rather than routing it through the generic Smithy pattern-constraint validator.
Traces: medical-imaging-tag-resource-error-paths#task#1.

Cause 2 of 3: UntagResource with resourceArn='not-a-valid-arn' (DELETE /tags/{resourceArn}).
AWS: Smithy: UntagResource.resourceArn, same Arn shape/@pattern; same BadRequestException shape available; recorded response is HTTP 400 + x-amzn-errortype: BadRequestException (message example 'Invalid input resource arn: bad syntax').
Emulator: HTTP 400 + x-amzn-errortype: ValidationException with the generic 'Value ... at 'resourceArn' failed to satisfy constraint' message.
Why: same as cause 1 — error-code-driven client logic for untagging misroutes a malformed-ARN request as a validation failure.
Fix: UntagResource handler's resourceArn validation, same component.
Traces: medical-imaging-tag-resource-error-paths#task#2.

Cause 3 of 3: ListTagsForResource with resourceArn='not-a-valid-arn' (GET /tags/{resourceArn}).
AWS: Smithy: ListTagsForResource.resourceArn, same Arn shape/@pattern; recorded response is HTTP 400 + x-amzn-errortype: BadRequestException (message example 'Invalid input resource arn: bad syntax').
Emulator: HTTP 400 + x-amzn-errortype: ValidationException with the generic constraint message.
Why: same as above; read-path tag listing surfaces the wrong modeled exception type.
Fix: ListTagsForResource handler's resourceArn validation, same component.
Traces: medical-imaging-tag-resource-error-paths#task#3.

## Evidence
Replay of fixture medical-imaging-tag-resource-error-paths (3 of 3 steps mismatched, divergence keys service=medical-imaging|{TagResource,UntagResource,ListTagsForResource}|status=400->400|keys=$.message,$headers.x-amzn-errortype):

Step task#1 (TagResource)
  expected: 400, $headers.x-amzn-errortype = 'BadRequestException', $.message = 'Invalid input resource arn: bad syntax'
  actual:   400, $headers.x-amzn-errortype = 'ValidationException', $.message = "1 validation error detected: Value 'not-a-valid-arn' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws((-us-gov)|(-iso)|(-iso-b)|(-cn))?:medical-imaging:[a-z0-9-]+:[0-9]{12}:datastore/[0-9a-z]{32}(/imageset/[0-9a-z]{32})?$"

Step task#2 (UntagResource) — identical expected/actual error-type divergence.
Step task#3 (ListTagsForResource) — identical expected/actual error-type divergence.

Model evidence: com.amazonaws.medicalimaging#BadRequestException exists with @error client / @httpError 400 ('The request is invalid or malformed.'), so the recorded code is a modeled one and the status is 400 either way — the divergence is purely the error code the emulator picks.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture medical-imaging-tag-resource-error-paths, step task#1: POST /tags/not-a-valid-arn with a tags body against the medical-imaging endpoint.
3. Observe HTTP 400 with x-amzn-errortype: ValidationException; AWS answers HTTP 400 with x-amzn-errortype: BadRequestException.
4. Repeat with step task#2 (DELETE /tags/not-a-valid-arn?tagKeys=...) and step task#3 (GET /tags/not-a-valid-arn) for UntagResource and ListTagsForResource — both show the same error-code divergence.
