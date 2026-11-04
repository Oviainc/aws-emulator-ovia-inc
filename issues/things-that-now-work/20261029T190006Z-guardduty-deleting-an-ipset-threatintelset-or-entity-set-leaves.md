Issue-Source: trace-sweep-divergence
Issue-Trace: guardduty-all-set-types-coexist-on-detector
Issue-Step: task#19
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: guardduty-all-set-types-coexist-on-detector guardduty-threatentityset-lifecycle-with-metadata guardduty-trusted-entity-set-lifecycle guardduty-trustedentityset-lifecycle-with-metadata guardduty-ip-and-threat-intel-set-lifecycle guardduty-ip-set-lifecycle guardduty-ipset-lifecycle-with-tags guardduty-threat-entity-set-lifecycle guardduty-threat-intel-set-lifecycle guardduty-threatintelset-lifecycle guardduty-ipset-inactive-state
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|GetThreatEntitySet|4xx->2xx|$.__type
Issue-Key: guardduty|GetThreatEntitySet|4xx->2xx|$.message
Issue-Key: guardduty|GetThreatEntitySet|4xx->2xx|$.type
Issue-Key: guardduty|GetThreatEntitySet|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: guardduty|GetThreatEntitySet|4xx->2xx|status
Issue-Key: guardduty|GetTrustedEntitySet|4xx->2xx|$.__type
Issue-Key: guardduty|GetTrustedEntitySet|4xx->2xx|$.message
Issue-Key: guardduty|GetTrustedEntitySet|4xx->2xx|$.type
Issue-Key: guardduty|GetTrustedEntitySet|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: guardduty|GetTrustedEntitySet|4xx->2xx|status
Issue-Key: guardduty|DeleteIPSet|4xx->2xx|$
Issue-Key: guardduty|DeleteIPSet|4xx->2xx|status
Issue-Key: guardduty|DeleteThreatIntelSet|4xx->2xx|$
Issue-Key: guardduty|DeleteThreatIntelSet|4xx->2xx|status
Issue-Key: guardduty|DeleteIPSet|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: guardduty|DeleteThreatEntitySet|4xx->2xx|$
Issue-Key: guardduty|DeleteThreatEntitySet|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: guardduty|DeleteThreatEntitySet|4xx->2xx|status
Issue-Key: guardduty|DeleteThreatIntelSet|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: guardduty|DeleteTrustedEntitySet|4xx->2xx|$
Issue-Key: guardduty|DeleteTrustedEntitySet|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: guardduty|DeleteTrustedEntitySet|4xx->2xx|status
Issue-Key: guardduty|GetIPSet|2xx->2xx|$.status
Issue-Key: guardduty|GetThreatIntelSet|2xx->2xx|$.status
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# Deleting an IPSet, ThreatIntelSet or entity set leaves it readable and re-deletable

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteIPSet

## Description
The emulator's set-deletion path for the five detector-scoped set families (IPSet, ThreatIntelSet, ThreatEntitySet, TrustedEntitySet) never removes the record and never drives its terminal status, so a deleted set stays visible and stays deletable. 10 causes.

Cause 1 of 10: GetThreatEntitySet after DeleteThreatEntitySet.
AWS: once the entity set is deleted the detector no longer has it, and a read of its id is refused. Recorded behavior: HTTP 404, error code ResourceNotFoundException (x-amzn-errortype, $.__type and $.type all 'ResourceNotFoundException'), message 'The specified entity set does not exist.' — that whole message is fixed text with no variable part. Smithy: GetThreatEntitySet models only BadRequestException(400)/InternalServerErrorException(500), so the 404 family is recorded behavior only.
Emulator: answers HTTP 200 with the full set body — no message, no __type, no type, no x-amzn-errortype.
Why: a client that deletes a set and polls for its disappearance never sees it disappear; teardown/liveness logic loops forever and a re-create of the same name collides with a ghost.
Fix: the GetThreatEntitySet handler's state lookup — deletion must remove the entity-set record (or mark it unreadable) rather than leave it addressable.
Traces: guardduty-all-set-types-coexist-on-detector#task#19.

Cause 2 of 10: GetTrustedEntitySet after DeleteTrustedEntitySet.
AWS: same as cause 1 for the trusted-entity-set family — HTTP 404, ResourceNotFoundException, fixed message 'The specified entity set does not exist.'
Emulator: HTTP 200 with the set body.
Why: identical client breakage on the trusted-entity-set lifecycle.
Fix: the GetTrustedEntitySet handler's state lookup.
Traces: guardduty-all-set-types-coexist-on-detector#task#21.

Cause 3 of 10: DeleteIPSet repeated on an already-deleted ipSetId.
AWS: the second delete is refused. Recorded behavior: HTTP 400, BadRequestException (x-amzn-errortype 'BadRequestException'), $.__type and $.type 'InvalidInputException', message 'The request is rejected since no such resource found.' — entirely fixed text. Smithy: DeleteIPSet models BadRequestException(400) only, consistent with the 400.
Emulator: HTTP 200 with an empty body.
Why: delete is reported as succeeding on a set that does not exist, so a client cannot distinguish 'I deleted it' from 'it was never there', and cleanup code that counts deletions over-counts.
Fix: the DeleteIPSet handler's existence check (state), before it reports success.
Traces: guardduty-ip-and-threat-intel-set-lifecycle#task#12.

Cause 4 of 10: DeleteThreatIntelSet repeated on an already-deleted threatIntelSetId.
AWS: HTTP 400, BadRequestException / InvalidInputException, fixed message 'The request is rejected since no such resource found.'
Emulator: HTTP 200, empty body.
Why: as cause 3, for the threat-intel-set family.
Fix: the DeleteThreatIntelSet handler's existence check.
Traces: guardduty-ip-and-threat-intel-set-lifecycle#task#15.

Cause 5 of 10: DeleteIPSet repeated in the plain IPSet lifecycle.
AWS: HTTP 400 with x-amzn-errortype 'BadRequestException' and body {message:'The request is rejected since no such resource found.', __type:'InvalidInputException', type:'InvalidInputException'}.
Emulator: HTTP 200, empty body, and no x-amzn-errortype header at all.
Why: the error-type header is what AWS SDKs use to pick an exception class, so a client gets a success where it must get BadRequestException.
Fix: the DeleteIPSet handler's existence check plus the error serializer that must set x-amzn-errortype.
Traces: guardduty-ip-set-lifecycle#task#6.

Cause 6 of 10: DeleteThreatEntitySet repeated on an already-deleted threatEntitySetId.
AWS: HTTP 404 with x-amzn-errortype 'ResourceNotFoundException' and body {message:'The specified entity set does not exist.', __type:'ResourceNotFoundException', type:'ResourceNotFoundException'}.
Emulator: HTTP 200, empty body, no x-amzn-errortype.
Why: a repeated delete is reported as a fresh success, hiding that the set was already gone.
Fix: the DeleteThreatEntitySet handler's existence check.
Traces: guardduty-threat-entity-set-lifecycle#task#6.

Cause 7 of 10: DeleteThreatIntelSet repeated in the threat-intel-set lifecycle.
AWS: HTTP 400, BadRequestException / InvalidInputException, fixed message 'The request is rejected since no such resource found.'
Emulator: HTTP 200, empty body, no x-amzn-errortype.
Why: as cause 4.
Fix: the DeleteThreatIntelSet handler's existence check.
Traces: guardduty-threat-intel-set-lifecycle#task#7.

Cause 8 of 10: DeleteTrustedEntitySet repeated on an already-deleted trustedEntitySetId.
AWS: HTTP 404, ResourceNotFoundException, fixed message 'The specified entity set does not exist.'
Emulator: HTTP 200, empty body, no x-amzn-errortype.
Why: as cause 6, for the trusted-entity-set family.
Fix: the DeleteTrustedEntitySet handler's existence check.
Traces: guardduty-trustedentityset-lifecycle-with-metadata#task#7.

Cause 9 of 10: GetIPSet reports a non-terminal status after the set is deleted.
AWS: after a successful DeleteIPSet the set's status reaches the terminal value DELETED. Smithy: GetIPSet.$.status targets IpSetStatus, whose values are INACTIVE | ACTIVATING | ACTIVE | DEACTIVATING | ERROR | DELETE_PENDING | DELETED; the recording shows 'DELETED'.
Emulator: reports 'DELETE_PENDING' and never advances past it.
Why: a client that waits for DELETED before re-creating the set with the same name waits forever.
Fix: the DeleteIPSet state transition, which must settle the record on DELETED rather than parking it in DELETE_PENDING.
Traces: guardduty-all-set-types-coexist-on-detector#task#15.

Cause 10 of 10: GetThreatIntelSet reports a non-terminal status after the set is deleted.
AWS: terminal status DELETED. Smithy: ThreatIntelSetStatus values INACTIVE | ACTIVATING | ACTIVE | DEACTIVATING | ERROR | DELETE_PENDING | DELETED.
Emulator: reports 'DELETE_PENDING'.
Why: as cause 9, for the threat-intel-set family.
Fix: the DeleteThreatIntelSet state transition.
Traces: guardduty-all-set-types-coexist-on-detector#task#17.

Related open issue (does NOT cover these; it carries no replay key for them): 20261029T183757Z-guardduty-lifecycle-state-is-wrong-missing-dry-run.md.

## Evidence
Cause 1: expected status 404 / $.message 'The specified entity set does not exist.' / $.__type, $.type, x-amzn-errortype 'ResourceNotFoundException'; actual status 200 and all four absent.
Cause 2: same expectation on GetTrustedEntitySet; actual status 200.
Cause 3: expected 400 with body {'message': 'The request is rejected since no such resource found.', '__type': 'InvalidInputException', 'type': 'InvalidInputException'}; actual 200 with {}.
Cause 4: same expectation on DeleteThreatIntelSet; actual 200 with {}.
Cause 5: expected 400 + x-amzn-errortype 'BadRequestException'; actual 200 with {} and no error-type header.
Cause 6: expected 404 + x-amzn-errortype 'ResourceNotFoundException' and body {'message': 'The specified entity set does not exist.', '__type': 'ResourceNotFoundException', 'type': 'ResourceNotFoundException'}; actual 200 with {}.
Cause 7: expected 400 + x-amzn-errortype 'BadRequestException'; actual 200 with {}.
Cause 8: expected 404 + x-amzn-errortype 'ResourceNotFoundException'; actual 200 with {}.
Cause 9: $.status expected 'DELETED', got 'DELETE_PENDING'.
Cause 10: $.status expected 'DELETED', got 'DELETE_PENDING'.

## Steps to Reproduce
1. Replay fixture guardduty-all-set-types-coexist-on-detector; steps task#15, task#17, task#19 and task#21 diverge after the sets are deleted.
2. Replay guardduty-ip-set-lifecycle step task#6, guardduty-ip-and-threat-intel-set-lifecycle steps task#12 and task#15, guardduty-threat-entity-set-lifecycle step task#6, guardduty-threat-intel-set-lifecycle step task#7, guardduty-trustedentityset-lifecycle-with-metadata step task#7: each issues a second Delete* for a set id already deleted and gets 200 instead of the modeled refusal.
3. In each case Get* the deleted set id and observe a 200 body with status DELETE_PENDING instead of a refusal / DELETED.
