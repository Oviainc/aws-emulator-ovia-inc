Issue-Source: trace-sweep-divergence
Issue-Trace: fis-get-safety-lever-not-found-error
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: fis-get-safety-lever-not-found-error fis-safety-lever-baseline-get fis-safety-lever-get-and-noop-update fis-safety-lever-not-found-errors fis-safety-lever-status-enum-validation-error fis-update-safety-lever-not-found-error fis-update-safety-lever-validation-error
Issue-Shape-Id: com.amazonaws.fis#FaultInjectionSimulator
Issue-Key: fis|GetSafetyLever|2xx->4xx|$.safetyLever
Issue-Key: fis|GetSafetyLever|2xx->4xx|status
Issue-Key: fis|GetSafetyLever|4xx->4xx|$.message
Issue-Key: fis|UpdateSafetyLeverState|4xx->4xx|$.message
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# FIS default safety lever is missing and safety-lever error messages diverge from AWS wording

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSafetyLever, UpdateSafetyLeverState

## Description
FIS gives every account one pre-existing safety lever with the id 'default'; the emulator has no safety lever at all, so the account-level lever cannot be read or driven, and the two error paths around it also report non-AWS message text. 4 causes.

Cause 1 of 4 (the account's 'default' safety lever does not exist).
AWS: Smithy: GetSafetyLever GET /safetyLevers/{id} -> GetSafetyLeverResponse.safetyLever : SafetyLever {id, arn : ResourceArn, state : SafetyLeverState}; the only modeled error is ResourceNotFoundException (404). The lever with id 'default' exists for the account without being created: the recorded 200 body carries id 'default', the account id of the serving account, an ARN of the form arn:aws:fis:<region>:<account-id>:safety-lever/default, and state {status: 'disengaged', reason: 'Initial default status'} — i.e. a disengaged lever whose initial reason is that fixed text.
Emulator: answers 404 with no $.safetyLever (recorded: status expected 200, got 404). This single gap blocks 9 recorded steps across the baseline-get, get-and-noop-update, status-enum-validation and update-not-found fixtures, so nothing in the safety-lever surface can be exercised.
Why: the safety lever is the account-wide kill switch for fault injection. A client that reads it to decide whether injection is permitted, or engages it to halt experiments, is told the lever does not exist — there is no id it could use instead, since AWS does not expose a create operation for levers.
Fix: the safety-lever state store must be seeded with the account's 'default' lever (disengaged, initial reason) at account initialisation, so GetSafetyLever and UpdateSafetyLeverState resolve it.
Traces: fis-get-safety-lever-not-found-error#task#2 (also fis-safety-lever-baseline-get#task#1, fis-safety-lever-get-and-noop-update#task#1, fis-safety-lever-not-found-errors#task#3, fis-safety-lever-status-enum-validation-error#task#2, fis-update-safety-lever-not-found-error#task#1 and #task#3, fis-update-safety-lever-validation-error#task#1 and #task#3).

Cause 2 of 4 (GetSafetyLever not-found message).
AWS: Smithy: GetSafetyLever error ResourceNotFoundException (404), message member ExceptionMessage. Recorded message: 'Safety lever not found' — entirely fixed text: it does NOT echo the requested lever id and has no trailing period.
Emulator: 404 with 'Safety lever with ID <id> not found.' (recorded: expected 'Safety lever not found', got 'Safety lever with ID sim2real-probe-fis-nonexistent-lever-fcc150a0 not found.'; the id in that string is the fixture's request value, the divergence is the message template itself).
Why: a client matching the service's message text, or asserting on it, never matches; the emulator also leaks the requested id into a message AWS keeps generic.
Fix: the not-found message in the GetSafetyLever handler's lookup.
Traces: fis-get-safety-lever-not-found-error#task#1 (also fis-safety-lever-not-found-errors#task#1).

Cause 3 of 4 (UpdateSafetyLeverState not-found message).
AWS: Smithy: UpdateSafetyLeverState PATCH /safetyLevers/{id}/state, errors ConflictException (409) / ResourceNotFoundException (404) / ValidationException (400). Recorded 404 message: 'Safety lever not found' — fixed text, no id echoed, no trailing period.
Emulator: 404 with 'Safety lever with ID <id> not found.' (recorded: expected 'Safety lever not found', got 'Safety lever with ID sim2real-probe-fis-nonexistent-lever-c71fe938 not found.').
Why: as cause 2, on the write path.
Fix: the not-found message in the UpdateSafetyLeverState handler's lookup.
Traces: fis-safety-lever-not-found-errors#task#2 (also fis-update-safety-lever-not-found-error#task#2).

Cause 4 of 4 (UpdateSafetyLeverState invalid status message).
AWS: Smithy: UpdateSafetyLeverStateRequest.state is required and state.status targets SafetyLeverStatusInput, an enum whose values are exactly [disengaged, engaged]; the violation surfaces as ValidationException (400). Recorded message: 'Invalid status: ' followed by the rejected value — fixed prefix 'Invalid status: ' plus the status string taken verbatim from the request.
Emulator: 400 with the generic constraint report "1 validation error detected: Value '<v>' at 'state.status' failed to satisfy constraint: Member must satisfy enum value set: [disengaged, engaged]" (recorded: expected 'Invalid status: not-a-real-status', got that generic text; 'not-a-real-status' is the request value in both).
Why: the status is right (400) but the message is a different error-reporting dialect, so a client that parses the message to extract the offending status, or asserts the service's wording, diverges from AWS.
Fix: status-enum validation in the UpdateSafetyLeverState handler must emit the operation-specific 'Invalid status: <value>' message rather than the generic constraint formatter.
Traces: fis-safety-lever-status-enum-validation-error#task#1 (also fis-update-safety-lever-validation-error#task#2).

## Evidence
Cause 1 — fis-get-safety-lever-not-found-error#task#2, GetSafetyLever: status expected 200, got 404; $.safetyLever expected {'id': 'default', 'accountId': <account>, 'arn': 'arn:aws:fis:<region>:<account>:safety-lever/default', 'state': {'status': 'disengaged', 'reason': 'Initial default status'}}, got None. 9 steps affected (fis-safety-lever-baseline-get#task#1, fis-safety-lever-get-and-noop-update#task#1, fis-safety-lever-not-found-errors#task#3, fis-safety-lever-status-enum-validation-error#task#2, fis-update-safety-lever-not-found-error#task#1/#task#3, fis-update-safety-lever-validation-error#task#1/#task#3).
Cause 2 — fis-get-safety-lever-not-found-error#task#1, GetSafetyLever 404/404: $.message expected 'Safety lever not found', got 'Safety lever with ID sim2real-probe-fis-nonexistent-lever-fcc150a0 not found.' (2 steps affected).
Cause 3 — fis-safety-lever-not-found-errors#task#2, UpdateSafetyLeverState 404/404: $.message expected 'Safety lever not found', got 'Safety lever with ID sim2real-probe-fis-nonexistent-lever-c71fe938 not found.' (2 steps affected).
Cause 4 — fis-safety-lever-status-enum-validation-error#task#1, UpdateSafetyLeverState 400/400: $.message expected 'Invalid status: not-a-real-status', got "1 validation error detected: Value 'not-a-real-status' at 'state.status' failed to satisfy constraint: Member must satisfy enum value set: [disengaged, engaged]" (2 steps affected).
Smithy cited: GetSafetyLever GET /safetyLevers/{id} -> safetyLever : SafetyLever {id, arn, state}, error ResourceNotFoundException(404); UpdateSafetyLeverState PATCH /safetyLevers/{id}/state, state @required, errors ConflictException(409)/ResourceNotFoundException(404)/ValidationException(400); SafetyLeverStatusInput enum values [disengaged, engaged]; ResourceNotFoundException/ValidationException carry member message : ExceptionMessage.

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus).
2. Cause 1: GET /safetyLevers/default on a fresh account — expect 200 with the disengaged default lever, observe 404 (fis-get-safety-lever-not-found-error#task#2).
3. Cause 2: GET /safetyLevers/<id that does not exist> and read $.message (fis-get-safety-lever-not-found-error#task#1).
4. Cause 3: PATCH /safetyLevers/<id that does not exist>/state and read $.message (fis-safety-lever-not-found-errors#task#2).
5. Cause 4: PATCH /safetyLevers/default/state with state.status set to a value outside [disengaged, engaged] and read $.message (fis-safety-lever-status-enum-validation-error#task#1).
