Issue-Source: trace-sweep-divergence
Issue-Trace: guardduty-detector-basic-lifecycle
Issue-Step: teardown#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: guardduty-detector-basic-lifecycle guardduty-detector-enable-disable-lifecycle guardduty-detector-error-paths guardduty-detector-features-and-tags guardduty-detector-lifecycle-basic guardduty-detector-list-update-lifecycle-no-features guardduty-detector-not-found-errors guardduty-filter-and-detector-error-paths guardduty-investigation-lifecycle guardduty-publishing-destination-list-empty guardduty-usage-and-free-trial-statistics guardduty-detector-duplicate-create-error guardduty-error-handling-invalid-inputs guardduty-findings-error-paths
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|GetDetector|4xx->4xx|$.__type
Issue-Key: guardduty|GetDetector|4xx->4xx|$.message
Issue-Key: guardduty|GetDetector|4xx->4xx|$.type
Issue-Key: guardduty|CreateDetector|4xx->4xx|$.__type
Issue-Key: guardduty|CreateDetector|4xx->4xx|$.message
Issue-Key: guardduty|CreateDetector|4xx->4xx|$.type
Issue-Key: guardduty|UpdateDetector|4xx->4xx|$.__type
Issue-Key: guardduty|UpdateDetector|4xx->4xx|$.message
Issue-Key: guardduty|UpdateDetector|4xx->4xx|$.type
Issue-Key: guardduty|DeleteDetector|4xx->4xx|$.__type
Issue-Key: guardduty|DeleteDetector|4xx->4xx|$.message
Issue-Key: guardduty|DeleteDetector|4xx->4xx|$.type
Issue-Key: guardduty|GetFindings|4xx->4xx|$.__type
Issue-Key: guardduty|GetFindings|4xx->4xx|$.message
Issue-Key: guardduty|GetFindings|4xx->4xx|$.type
Issue-Key: guardduty|ListFindings|4xx->4xx|$.__type
Issue-Key: guardduty|ListFindings|4xx->4xx|$.message
Issue-Key: guardduty|ListFindings|4xx->4xx|$.type
Issue-Commit: 0b8884c4ab978d23b4fe746f795a1277a2a36df3

# GuardDuty client rejections answered as BadRequestException without the InvalidInputException type member

> _Note: this issue was identified against the code base at commit `0b8884c4ab978d23b4fe746f795a1277a2a36df3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDetector

## Description
Every modeled client rejection in GuardDuty is serialized with the wrong error identity: the emulator emits `__type: BadRequestException` and no `type` member, where the service emits the exception's `type` member set to the specific error class (InvalidInputException) — and it mis-classifies which rejection applies, rebuilding the message from internal state. Six causes, one per operation, all sharing the same serializer/validation fault. Smithy: BadRequestException {message (jsonName "message"), type (jsonName "type", "The error type")}, httpError 400, bound as the only client error on GetDetector, CreateDetector, UpdateDetector, DeleteDetector, GetFindings and ListFindings. The HTTP status (400) and the body's error class are requirements; the recorded message strings are examples of AWS's rule, which classifies by the *input* (malformed detectorId -> "the parameter detectorId has an invalid value"; well-formed but unknown/foreign detectorId -> "the input detectorId is not owned by the current account"; second CreateDetector -> "a detector already exists for the current account") and never echoes the caller's detector id or a region.

Cause 1 of 6: GetDetector on a deleted/unknown detector. AWS: 400 with body {"message": "The request is rejected because the input detectorId is not owned by the current account.", "__type": "InvalidInputException", "type": "InvalidInputException"}. Emulator: 400 with `__type: BadRequestException`, no `type` member, and a message rebuilt from internal state that echoes the detector id ("...the specified detector ID does not exist: <id>"). Why: a client switching on the error type sees a generic BadRequestException and cannot distinguish an invalid input from any other 400; SDK error-shape unmarshalling leaves `Type` null. Fix: the shared GuardDuty error serializer (error class in `__type`/`type`) plus the detector-lookup rejection classifier. Traces: guardduty-detector-basic-lifecycle#teardown#7 (cluster of 11, incl. guardduty-detector-not-found-errors#task#1, guardduty-filter-and-detector-error-paths#task#8, guardduty-investigation-lifecycle#teardown#7).

Cause 2 of 6: CreateDetector when a detector already exists. AWS: 400 InvalidInputException, message "The request is rejected because a detector already exists for the current account." Emulator: `__type: BadRequestException`, no `type`, and a message that names the region and the existing detector id. Why: an idempotent bootstrap that catches the duplicate-detector rejection by error type does not match. Fix: CreateDetector's conflict path + the error serializer. Traces: guardduty-detector-duplicate-create-error#task#2.

Cause 3 of 6: UpdateDetector with a malformed detectorId. AWS: 400 InvalidInputException, message "The request is rejected because the parameter detectorId has an invalid value." Emulator: BadRequestException with no `type`, and a not-found message echoing the 62-char id — i.e. a syntactically invalid id is treated as a lookup miss instead of a parameter validation failure. Fix: UpdateDetector input validation (detectorId form) ahead of lookup, plus the error serializer. Traces: guardduty-detector-error-paths#task#2 (also guardduty-detector-not-found-errors#task#2).

Cause 4 of 6: DeleteDetector with a malformed detectorId. Same AWS rule and same emulator divergence as cause 3. Fix: DeleteDetector input validation + error serializer. Traces: guardduty-detector-error-paths#task#3 (also guardduty-detector-not-found-errors#task#3, guardduty-filter-and-detector-error-paths#task#9).

Cause 5 of 6: GetFindings with an invalid detectorId. AWS: 400 InvalidInputException, parameter-invalid message. Emulator: BadRequestException with no `type` and an id-echoing not-found message. Fix: GetFindings input validation + error serializer. Traces: guardduty-error-handling-invalid-inputs#task#2.

Cause 6 of 6: ListFindings with an invalid detectorId. AWS: 400 InvalidInputException, parameter-invalid message. Emulator: BadRequestException with no `type` and an id-echoing not-found message. Fix: ListFindings input validation + error serializer. Traces: guardduty-findings-error-paths#task#4.

## Evidence
Replay of simparity-wire-aws-v1; all six clusters are expected 400 / actual 400 with the same body divergence pattern.

Cause 1 — guardduty-detector-basic-lifecycle#teardown#7, GetDetector (cluster of 11 steps):
  $.__type: value_mismatch (expected 'InvalidInputException', got 'BadRequestException')
  $.type: missing (expected 'InvalidInputException', got None)
  $.message: value_mismatch (expected 'The request is rejected because the input detectorId is not owned by the current account.', got 'The request is rejected because the specified detector ID does not exist: <detectorId>.')

Cause 2 — guardduty-detector-duplicate-create-error#task#2, CreateDetector:
  $.__type: 'InvalidInputException' -> 'BadRequestException'; $.type missing
  $.message: expected 'The request is rejected because a detector already exists for the current account.', got '...already exists for this account in us-west-2: <detectorId>.'

Cause 3 — guardduty-detector-error-paths#task#2, UpdateDetector (cluster of 2):
  $.__type 'InvalidInputException' -> 'BadRequestException'; $.type missing
  $.message: expected 'The request is rejected because the parameter detectorId has an invalid value.', got 'The request is rejected because the specified detector ID does not exist: <62-char id>.'

Cause 4 — guardduty-detector-error-paths#task#3, DeleteDetector (cluster of 3): identical to cause 3.
Cause 5 — guardduty-error-handling-invalid-inputs#task#2, GetFindings: identical pattern.
Cause 6 — guardduty-findings-error-paths#task#4, ListFindings: identical pattern.

The missing `type` member and the wrong `__type` value are the wire requirements here; the quoted message texts are examples of AWS's input-classification rule (classify by input validity, do not echo the caller's detector id or the region).

## Steps to Reproduce
1. Boot the emulator built from mainline and replay the wire corpus (simparity-wire-aws-v1).
2. Inspect the six clusters: guardduty-detector-basic-lifecycle#teardown#7 (GetDetector), guardduty-detector-duplicate-create-error#task#2 (CreateDetector), guardduty-detector-error-paths#task#2 (UpdateDetector), guardduty-detector-error-paths#task#3 (DeleteDetector), guardduty-error-handling-invalid-inputs#task#2 (GetFindings), guardduty-findings-error-paths#task#4 (ListFindings).
3. Equivalently by hand: GET /detector/<deleted-detector-id>; POST /detector a second time with enable=true; POST/DELETE /detector/<malformed-id>; POST /detector/<malformed-id>/findings/get and /findings. Each answers 400 with {"__type":"BadRequestException", "message": "...<id>."} and no "type" member, where AWS answers {"__type":"InvalidInputException","type":"InvalidInputException"} with an input-classified message.
