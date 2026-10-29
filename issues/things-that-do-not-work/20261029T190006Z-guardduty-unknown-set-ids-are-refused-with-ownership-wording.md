Issue-Source: trace-sweep-divergence
Issue-Trace: guardduty-error-paths-invalid-ids
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: guardduty-error-paths-invalid-ids guardduty-intel-and-entity-set-error-paths guardduty-set-not-found-error-paths guardduty-ip-set-error-paths
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|GetThreatEntitySet|4xx->4xx|$.__type
Issue-Key: guardduty|GetThreatEntitySet|4xx->4xx|$.message
Issue-Key: guardduty|GetThreatEntitySet|4xx->4xx|$.type
Issue-Key: guardduty|GetThreatEntitySet|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: guardduty|GetThreatEntitySet|4xx->4xx|status
Issue-Key: guardduty|GetTrustedEntitySet|4xx->4xx|$.__type
Issue-Key: guardduty|GetTrustedEntitySet|4xx->4xx|$.message
Issue-Key: guardduty|GetTrustedEntitySet|4xx->4xx|$.type
Issue-Key: guardduty|GetTrustedEntitySet|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: guardduty|GetTrustedEntitySet|4xx->4xx|status
Issue-Key: guardduty|DeleteThreatEntitySet|4xx->4xx|$.message
Issue-Key: guardduty|DeleteTrustedEntitySet|4xx->4xx|$.message
Issue-Key: guardduty|GetIPSet|4xx->4xx|$.message
Issue-Key: guardduty|GetThreatIntelSet|4xx->4xx|$.message
Issue-Key: guardduty|DeleteThreatIntelSet|4xx->4xx|$.message
Issue-Key: guardduty|DeleteIPSet|4xx->4xx|$.message
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# Unknown set ids are refused with ownership wording and the wrong status

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetThreatEntitySet

## Description
Every set-family read/delete refuses an unknown or malformed id with one emulator-authored sentence ('the parameter <member> has an invalid value'), discarding GuardDuty's three distinct refusal classes: the IPSet/ThreatIntelSet not-found text, the entity-set ResourceNotFoundException, and the Smithy pattern-constraint validation text. 10 causes.

Cause 1 of 10: GetThreatEntitySet with an unknown but well-formed threatEntitySetId.
AWS: HTTP 404, error code ResourceNotFoundException (x-amzn-errortype 'ResourceNotFoundException', $.__type and $.type 'ResourceNotFoundException'), message 'The specified entity set does not exist.' — all fixed text.
Emulator: HTTP 400 with x-amzn-errortype 'BadRequestException', $.__type/$.type 'InvalidInputException', message 'The request is rejected because the parameter threatEntitySetId has an invalid value.'
Why: an SDK raises BadRequestException where the caller catches ResourceNotFoundException, so not-found handling (create-if-absent, idempotent teardown) never runs.
Fix: the GetThreatEntitySet handler's id-resolution branch and the error it maps to.
Traces: guardduty-error-paths-invalid-ids#task#4.

Cause 2 of 10: GetTrustedEntitySet with an unknown trustedEntitySetId.
AWS: HTTP 404, ResourceNotFoundException, fixed message 'The specified entity set does not exist.'
Emulator: HTTP 400, BadRequestException / InvalidInputException, 'The request is rejected because the parameter trustedEntitySetId has an invalid value.'
Why: as cause 1, on the trusted-entity-set family.
Fix: the GetTrustedEntitySet handler's id-resolution branch.
Traces: guardduty-error-paths-invalid-ids#task#5.

Cause 3 of 10: GetThreatEntitySet with a threatEntitySetId that violates the modeled id pattern.
AWS: refuses with the standard Smithy constraint-violation text. Recorded message: "1 validation error detected: Value at 'threatEntitySetId' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9]+". Fixed text: '1 validation error detected: Value at ', "' failed to satisfy constraint: Member must satisfy regular expression pattern: "; variable parts: the member name taken from the request ('threatEntitySetId') and the pattern from the model.
Emulator: 'The request is rejected because the parameter threatEntitySetId has an invalid value.'
Why: clients and operators that parse the constraint text to learn which member and which pattern failed get nothing actionable; a malformed id is indistinguishable from an absent one.
Fix: input validation for the operation — constraint violations must render AWS's validation-error template, not a generic per-parameter sentence.
Traces: guardduty-intel-and-entity-set-error-paths#task#10.

Cause 4 of 10: DeleteThreatEntitySet with a pattern-violating threatEntitySetId.
AWS: the same validation template as cause 3, naming 'threatEntitySetId' and pattern [A-Za-z0-9]+.
Emulator: 'The request is rejected because the parameter threatEntitySetId has an invalid value.'
Why: as cause 3.
Fix: input validation for DeleteThreatEntitySet.
Traces: guardduty-intel-and-entity-set-error-paths#task#11.

Cause 5 of 10: GetTrustedEntitySet with a pattern-violating trustedEntitySetId.
AWS: the same validation template, member 'trustedEntitySetId', pattern [A-Za-z0-9]+.
Emulator: 'The request is rejected because the parameter trustedEntitySetId has an invalid value.'
Why: as cause 3.
Fix: input validation for GetTrustedEntitySet.
Traces: guardduty-intel-and-entity-set-error-paths#task#12.

Cause 6 of 10: DeleteTrustedEntitySet with a pattern-violating trustedEntitySetId.
AWS: the same validation template, member 'trustedEntitySetId'.
Emulator: 'The request is rejected because the parameter trustedEntitySetId has an invalid value.'
Why: as cause 3.
Fix: input validation for DeleteTrustedEntitySet.
Traces: guardduty-intel-and-entity-set-error-paths#task#13.

Cause 7 of 10: GetIPSet with an unknown ipSetId.
AWS: HTTP 400, BadRequestException with $.__type/$.type 'InvalidInputException', message 'The request is rejected since no such resource found.' — entirely fixed text, naming no member and no value. Smithy: GetIPSet models BadRequestException(400) and InternalServerErrorException(500) only, so 400 is the right status and only the message diverges.
Emulator: 'The request is rejected because the parameter ipSetId has an invalid value.'
Why: the IPSet family signals not-found through this one fixed sentence; a client matching on it to branch between 'absent' and 'malformed' never matches.
Fix: the GetIPSet handler's not-found refusal message.
Traces: guardduty-error-paths-invalid-ids#task#2.

Cause 8 of 10: GetThreatIntelSet with an unknown threatIntelSetId.
AWS: HTTP 400, InvalidInputException, fixed message 'The request is rejected since no such resource found.'
Emulator: 'The request is rejected because the parameter threatIntelSetId has an invalid value.'
Why: as cause 7.
Fix: the GetThreatIntelSet handler's not-found refusal message.
Traces: guardduty-error-paths-invalid-ids#task#3.

Cause 9 of 10: DeleteThreatIntelSet with an unknown threatIntelSetId.
AWS: HTTP 400, InvalidInputException, fixed message 'The request is rejected since no such resource found.'
Emulator: 'The request is rejected because the parameter threatIntelSetId has an invalid value.'
Why: as cause 7.
Fix: the DeleteThreatIntelSet handler's not-found refusal message.
Traces: guardduty-intel-and-entity-set-error-paths#task#9.

Cause 10 of 10: DeleteIPSet with an unknown ipSetId.
AWS: HTTP 400, InvalidInputException, fixed message 'The request is rejected since no such resource found.'
Emulator: 'The request is rejected because the parameter ipSetId has an invalid value.'
Why: as cause 7.
Fix: the DeleteIPSet handler's not-found refusal message.
Traces: guardduty-ip-set-error-paths#task#5.

Related open issues (neither covers these; neither carries a replay key for them): 20261029T183757Z-guardduty-resource-not-found-rejections-across-guardduty-are-reported.md and 20261029T183757Z-guardduty-invalid-input-and-already-exists-refusals-use-emulator.md.

## Evidence
Cause 1: expected 404 / 'The specified entity set does not exist.' / ResourceNotFoundException; actual 400 / 'The request is rejected because the parameter threatEntitySetId has an invalid value.' / InvalidInputException with x-amzn-errortype 'BadRequestException'.
Cause 2: expected 404 / 'The specified entity set does not exist.'; actual 400 / '...parameter trustedEntitySetId has an invalid value.'
Causes 3-6: expected "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9]+"; actual 'The request is rejected because the parameter <member> has an invalid value.'
Causes 7-10: expected 'The request is rejected since no such resource found.'; actual 'The request is rejected because the parameter <ipSetId|threatIntelSetId> has an invalid value.'

## Steps to Reproduce
1. Replay guardduty-error-paths-invalid-ids: step task#4 (GetThreatEntitySet) and task#5 (GetTrustedEntitySet) get 400/InvalidInputException instead of 404/ResourceNotFoundException; steps task#2 (GetIPSet) and task#3 (GetThreatIntelSet) get the wrong 400 message.
2. Replay guardduty-intel-and-entity-set-error-paths steps task#10-task#13 with ids that violate [A-Za-z0-9]+ and observe the generic per-parameter sentence instead of AWS's '1 validation error detected: ...' template; step task#9 for DeleteThreatIntelSet not-found.
3. Replay guardduty-ip-set-error-paths step task#5 for DeleteIPSet not-found.
