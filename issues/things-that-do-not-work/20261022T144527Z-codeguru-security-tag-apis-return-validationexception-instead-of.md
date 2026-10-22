Issue-Source: trace-sweep-divergence
Issue-Trace: codeguru-security-tag-resource-validation-error
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeguru-security-tag-resource-validation-error
Issue-Shape-Id: com.amazonaws.codegurusecurity#AwsCodeGuruSecurity
Issue-Key: codeguru-security|ListTagsForResource|4xx->4xx|$.message
Issue-Key: codeguru-security|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: codeguru-security|TagResource|4xx->4xx|$.message
Issue-Key: codeguru-security|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: codeguru-security|UntagResource|4xx->4xx|$.message
Issue-Key: codeguru-security|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: b755dbbf9c35939a76222e1fb1150f3e98238502

# codeguru-security tag APIs return ValidationException instead of BadRequestException for a malformed resourceArn

> _Note: this issue was identified against the code base at commit `b755dbbf9c35939a76222e1fb1150f3e98238502`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
All three codeguru-security resource-tagging operations bind `resourceArn` as an @httpLabel constrained by the ScanNameArn pattern. When the label is not a parseable CodeGuru Security scan ARN, the recorded service rejects the call as `BadRequestException` (HTTP 400) with a short syntax message. The emulator instead rejects it with the generic constraint-violation shape `ValidationException` (HTTP 400). The status is right in every case; the error CODE carried in `x-amzn-errortype` is wrong, and it is wrong in the same way for all three operations, so this is one shared fault in how the shared tagging path reports a bad ARN label. 3 causes, one per operation.

Cause 1 of 3: ListTagsForResource (fixture codeguru-security-tag-resource-validation-error, step task#1).
AWS: ListTagsForResource is GET /tags/{resourceArn}; `resourceArn` is required, @httpLabel, target ScanNameArn with @length 1..300 and @pattern ^arn:aws:codeguru-security:[\S]+:[\d]{12}:scans\/[a-zA-Z0-9-_$:.]*$ (Smithy: ListTagsForResource.resourceArn / shape ScanNameArn). The model's bound client errors for the operation are AccessDeniedException(403), ConflictException(409), ResourceNotFoundException(404), ThrottlingException(429) and ValidationException(400); BadRequestException is NOT in that list, so for the error code AWS actually emits here the authority is the recording: status 400 with header x-amzn-errortype: BadRequestException (recorded message, as an example only: 'Invalid input resource arn: bad syntax'). Hypothesis: a syntactically unparseable ARN in the URI path is rejected by the front-end ARN parser before the modelled member-constraint validator runs, which is why the wire code is BadRequestException rather than the modelled ValidationException.
Emulator: answers 400 but with x-amzn-errortype: ValidationException and a generic constraint-violation body ("1 validation error detected: Value 'not-a-valid-arn' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ...").
Why: the error code selects the SDK exception class. A client that catches CodeGuruSecurity BadRequestException to report a user-supplied ARN as malformed sees an unrelated ValidationException instead, so the catch does not fire and the call surfaces as an unexpected/unmodelled failure; retry and error-classification logic keyed on the error type also mis-branches.
Fix: the ListTagsForResource request-validation step for the resourceArn @httpLabel in the codeguru-security operation handler/validation layer - a label that does not parse as a scan ARN must be mapped to the BadRequestException error shape (400), not routed into the generic member-constraint ValidationException path.
Traces: codeguru-security-tag-resource-validation-error#task#1.

Cause 2 of 3: TagResource (fixture codeguru-security-tag-resource-validation-error, step task#2).
AWS: TagResource is POST /tags/{resourceArn} (success code 204) with the same required @httpLabel resourceArn -> ScanNameArn, @length 1..300 and the same @pattern (Smithy: TagResource.resourceArn). Same modelled error set, BadRequestException again not modelled; recorded behavior for a malformed ARN is 400 with x-amzn-errortype: BadRequestException (recorded message 'Invalid input resource arn: bad syntax', example only).
Emulator: 400 with x-amzn-errortype: ValidationException and the generic pattern-constraint message.
Why: same as cause 1 - a tagging client cannot distinguish "the ARN you gave me is not an ARN" via the documented/recorded error type, so its handler for malformed input never runs.
Fix: same validation step for TagResource in the codeguru-security handler/validation layer; the ARN-label parse failure must raise BadRequestException.
Traces: codeguru-security-tag-resource-validation-error#task#2.

Cause 3 of 3: UntagResource (fixture codeguru-security-tag-resource-validation-error, step task#3).
AWS: UntagResource is DELETE /tags/{resourceArn} (success code 204), @idempotent, same required @httpLabel resourceArn -> ScanNameArn with the same length/pattern constraints (Smithy: UntagResource.resourceArn). Same modelled error set; recorded behavior for a malformed ARN is 400 with x-amzn-errortype: BadRequestException (recorded message 'Invalid input resource arn: bad syntax', example only).
Emulator: 400 with x-amzn-errortype: ValidationException and the generic pattern-constraint message.
Why: same as above; additionally an idempotent cleanup path that treats only specific error types as terminal will mis-handle the response.
Fix: same validation step for UntagResource in the codeguru-security handler/validation layer; the ARN-label parse failure must raise BadRequestException.
Traces: codeguru-security-tag-resource-validation-error#task#3.

Note on messages: the diverging requirement filed here is the error code in x-amzn-errortype (and the 400 status, which already matches). The recorded message text is quoted only as an example of the recorded response; the rule that generates AWS's exact wording is not documented, so the message difference on its own is not the defect being claimed - though re-shaping the error to BadRequestException should also drop the ValidationException-style 'N validation error(s) detected...' prose.

Unrelated open issue 20261022T143245Z-codeguru-security-getaccountconfiguration-stub.md covers an unimplemented GetAccountConfiguration stub and does not cover any of these three causes.

## Evidence
Fixture codeguru-security-tag-resource-validation-error, all three steps send a resourceArn of 'not-a-valid-arn'.

Step task#1 (ListTagsForResource): expected 400 / x-amzn-errortype: BadRequestException, $.message 'Invalid input resource arn: bad syntax'. Actual 400 / x-amzn-errortype: ValidationException, $.message "1 validation error detected: Value 'not-a-valid-arn' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws:codeguru-security:[\S]+:[\d]{12}:scans\/[a-zA-Z0-9-_$:.]*$".

Step task#2 (TagResource): identical divergence - expected x-amzn-errortype BadRequestException, got ValidationException; same expected vs actual message.

Step task#3 (UntagResource): identical divergence - expected x-amzn-errortype BadRequestException, got ValidationException; same expected vs actual message.

Replay summary: 5 steps total, 0 passed, 5 mismatched, 3 divergence clusters, score_pct 0.0, state_reset_failures 0, bootstrap_signal false, emulator_up true. Divergence keys: service=codeguru-security|ListTagsForResource|status=400->400|keys=$.message,$headers.x-amzn-errortype (and the TagResource / UntagResource equivalents).

Smithy evidence (services/service-com-amazonaws-codegurusecurity-awscodegurusecurity/smithy-model/com-amazonaws-codegurusecurity-awscodegurusecurity.json): ScanNameArn = string, length 1..300, pattern ^arn:aws:codeguru-security:[\S]+:[\d]{12}:scans\/[a-zA-Z0-9-_$:.]*$; ValidationException = client error, httpError 400, members errorCode/message/reason required.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture codeguru-security-tag-resource-validation-error.
3. Step task#1: GET /tags/not-a-valid-arn (ListTagsForResource with resourceArn='not-a-valid-arn'). Observe HTTP 400 with x-amzn-errortype: ValidationException; AWS records x-amzn-errortype: BadRequestException.
4. Step task#2: POST /tags/not-a-valid-arn with a tags map (TagResource). Same wrong error type.
5. Step task#3: DELETE /tags/not-a-valid-arn?tagKeys=... (UntagResource). Same wrong error type.
6. All three must answer 400 with x-amzn-errortype: BadRequestException.
