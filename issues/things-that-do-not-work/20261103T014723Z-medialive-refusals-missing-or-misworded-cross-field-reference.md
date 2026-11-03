Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: medialive.batch-delete-rejects-mixed-resource-lists-and-deletes-inputs medialive.create-channel-rejects-a-role-arn-that-does-not-exist medialive.create-channel-rejects-a-role-that-does-not-trust-medialive medialive.create-channel-rejects-an-attachment-of-a-deleted-input medialive.create-channel-rejects-single-pipeline-class-with-two-destination-urls medialive.create-channel-rejects-standard-class-with-one-destination-url medialive.rtmp-push-input-rejected-without-a-security-group medialive.rtmp-push-input-rejects-three-destinations medialive.second-channel-on-an-attached-input-rejected medialive.delete-security-group-rejected-while-an-input-references-it
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive.batch-delete-rejects-mixed-resource-lists-and-deletes-inputs|_error#4|AssertionError|raise AssertionError(f"the call succeeded; real AWS refuses it. Response: {resp!r}")
Issue-Key: medialive.create-channel-rejects-a-role-arn-that-does-not-exist|_error#7|AssertionError|raise AssertionError(f"the call succeeded; real AWS refuses it. Response: {resp!r}")
Issue-Key: medialive.create-channel-rejects-a-role-that-does-not-trust-medialive|_error#9|AssertionError|raise AssertionError(f"the call succeeded; real AWS refuses it. Response: {resp!r}")
Issue-Key: medialive.create-channel-rejects-an-attachment-of-a-deleted-input|run#11|AssertionError|assert (code, message) == ("BadRequestException", expected_message), (
Issue-Key: medialive.create-channel-rejects-single-pipeline-class-with-two-destination-urls|_error#5|AssertionError|raise AssertionError(f"the call succeeded; real AWS refuses it. Response: {resp!r}")
Issue-Key: medialive.create-channel-rejects-standard-class-with-one-destination-url|_error#6|AssertionError|raise AssertionError(f"the call succeeded; real AWS refuses it. Response: {resp!r}")
Issue-Key: medialive.rtmp-push-input-rejected-without-a-security-group|run#5|AssertionError|assert got == want, f"an RTMP_PUSH input without a group raised {got!r}, want {want!r}"
Issue-Key: medialive.rtmp-push-input-rejects-three-destinations|_error#4|AssertionError|raise AssertionError(f"the call succeeded; real AWS refuses it. Response: {resp!r}")
Issue-Key: medialive.second-channel-on-an-attached-input-rejected|_error#7|AssertionError|raise AssertionError(f"the call succeeded; real AWS refuses it. Response: {resp!r}")
Issue-Key: medialive.delete-security-group-rejected-while-an-input-references-it|_error#4|AssertionError|raise AssertionError(f"the call succeeded; real AWS refuses it. Response: {resp!r}")
Issue-Commit: a03f808379986133e0f00cdb9a2ead2b78e9a26e

# medialive refusals missing or misworded: cross-field, reference and conflict checks on BatchDelete, CreateChannel, CreateInput and DeleteInputSecurityGroup

> _Note: this issue was identified against the code base at commit `a03f808379986133e0f00cdb9a2ead2b78e9a26e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
MediaLive's control-plane refusals diverge from AWS: requests AWS refuses are accepted with a success status (and the resource is created or deleted anyway), or the refusal carries the wrong error class or the emulator's own wording. All ten causes are cross-field / cross-resource rules that no single-member model constraint expresses, so each needs an explicit check in its handler. This bundle has 10 causes; all ten must be closed.

Cause 1 of 10 - BatchDelete accepts a request naming two resource kinds and performs both deletes.
AWS: BatchDelete (Smithy: POST /prod/batch/delete, code 200; input members channelIds, inputIds, inputSecurityGroupIds, multiplexIds, each a list of string; errors include BadRequestException httpError 400) requires exactly one non-empty id list. A request carrying two non-empty lists is refused with 400 BadRequestException and the fixed message "Exactly one non-empty list of channel, multiplex, input, or input security group IDs is required" (no variable parts), and nothing is deleted - the input stays DETACHED and the group stays IDLE. The cross-field rule itself is recorded behavior only; the model declares no constraint between the four lists.
Emulator: HTTP 200 with Failed=[] and Successful carrying BOTH resources (the input reported State "DELETING", the input security group State "DELETED") - both deletes were executed.
Why: a client that mixes kinds by mistake loses resources AWS would have left untouched, and never sees the 400 that tells it to split the call.
Fix: BatchDelete's request validation in its handler, ahead of any state mutation (count the non-empty lists first, delete nothing on refusal).
Traces: medialive.batch-delete-rejects-mixed-resource-lists-and-deletes-inputs.

Cause 2 of 10 - CreateChannel accepts a well-formed roleArn that names no existing role.
AWS: CreateChannel (Smithy: POST /prod/channels, code 201; errors include BadRequestException 400, ConflictException 409, UnprocessableEntityException httpError 422 whose members are Message jsonName "message" and ValidationErrors jsonName "validationErrors" of ValidationError{elementPath, errorMessage}) resolves RoleArn against IAM and refuses an ARN that resolves to no role with 422 UnprocessableEntityException carrying exactly one validation error: elementPath "roleArn", errorMessage "Make sure the role '<roleArn from the request>' exists in the user account and medialive.amazonaws.com is a trusted service" - fixed text around the roleArn echoed from the request. No channel appears in ListChannels.
Emulator: HTTP 201 with a fully formed channel; no error, no validationErrors.
Why: the misconfiguration the client is probing for silently succeeds, so a channel exists that real MediaLive would never have created, and the client never learns its role is wrong.
Fix: CreateChannel's validation - resolve RoleArn against IAM state and emit the modeled UnprocessableEntityException with its validationErrors list. (The open wire issue 20261102T091340Z-medialive-request-validation-missing-or-misworded-across-channel.md describes the same handler gap from a wire fixture; it does not carry this behavioral check's replay key.)
Traces: medialive.create-channel-rejects-a-role-arn-that-does-not-exist.

Cause 3 of 10 - CreateChannel accepts a role whose trust policy does not name medialive.amazonaws.com.
AWS: the same 422 UnprocessableEntityException as cause 2 - validationErrors exactly [{elementPath: "roleArn", errorMessage: "Make sure the role '<roleArn from the request>' exists in the user account and medialive.amazonaws.com is a trusted service"}] - when the role DOES exist but trusts another service principal (here lambda.amazonaws.com). The refused create leaves no channel in ListChannels and does NOT attach its input (DescribeInput stays State "DETACHED").
Emulator: HTTP 201 with a created channel; the trust document is never consulted.
Why: a channel whose role MediaLive cannot assume is accepted, so a client's authorization test passes against the emulator and fails in production; the accepted channel also takes an attachment AWS never granted.
Fix: CreateChannel's validation - check the resolved role's trust policy for the medialive.amazonaws.com service principal, not only the role's existence.
Traces: medialive.create-channel-rejects-a-role-that-does-not-trust-medialive.

Cause 4 of 10 - CreateChannel's refusal of an attachment of a DELETED input uses the wrong error class and an opaque message.
AWS: attaching an input whose lifecycle state is DELETED (the id is real and was valid moments earlier) is refused with 400 BadRequestException and the message "Input <input id> is DELETED" - fixed text "Input " and " is DELETED" around the input id taken from the request's inputAttachments.
Emulator: 422 UnprocessableEntityException with the message "Channel contains errors." - the wrong status, the wrong modeled error and a message naming neither the input nor its state.
Why: a client branching on 400/BadRequestException (fix the request) versus 422 (a semantic validation report with validationErrors) takes the wrong branch, and the message tells it nothing about which input is unusable.
Fix: CreateChannel's input-attachment validation and its error mapping - resolve each attachment's input state and raise BadRequestException with MediaLive's wording.
Traces: medialive.create-channel-rejects-an-attachment-of-a-deleted-input.

Cause 5 of 10 - CreateChannel accepts a SINGLE_PIPELINE channel whose destination carries two settings.
AWS: the number of settings on each destination must equal the channel class's pipeline count exactly. A SINGLE_PIPELINE channel with two destination settings is refused with 400 BadRequestException and the message "Exactly 1 destinations settings required for destination <destination id>" - fixed text around the destination's Id from the request. Recorded behavior only: OutputDestination.Settings carries no modeled bound, and the rule is cross-field (ChannelClass against Destinations[].Settings).
Emulator: HTTP 201 with ChannelClass "SINGLE_PIPELINE" and both URLs retained on destination1.
Why: a client builds a channel configuration real MediaLive rejects and only discovers it on a real account; its own one-URL-per-pipeline invariant is never enforced.
Fix: CreateChannel's validation - compare each destination's settings count against the pipeline count implied by ChannelClass.
Traces: medialive.create-channel-rejects-single-pipeline-class-with-two-destination-urls.

Cause 6 of 10 - the mirror case: CreateChannel accepts a STANDARD channel whose destination carries one setting.
AWS: refused with 400 BadRequestException and the message "Exactly 2 destinations settings required for destination <destination id>" - same fixed text, the count taken from the class's pipeline count and the id from the request. Recorded behavior only, as in cause 5.
Emulator: HTTP 201 with ChannelClass "STANDARD" and a single-URL destination (and two egressEndpoints).
Why: as cause 5, in the other direction - a two-pipeline channel with only one output path is accepted.
Fix: the same destination-settings/pipeline-count check in CreateChannel's validation (both directions).
Traces: medialive.create-channel-rejects-standard-class-with-one-destination-url.

Cause 7 of 10 - CreateInput's missing-security-group refusal for an RTMP_PUSH input is worded by the emulator.
AWS: CreateInput (Smithy: POST /prod/inputs, code 201; input member InputSecurityGroups jsonName "inputSecurityGroups", optional in the model; errors include BadRequestException httpError 400) requires exactly one input security group for a push input and refuses an RTMP_PUSH CreateInput with none with 400 BadRequestException and the fixed message "Must specify exactly one security group per input." (no variable parts).
Emulator: the right status and error class (400 BadRequestException) but its own text: "inputSecurityGroups is required for an input of type RTMP_PUSH".
Why: the message is a requirement of the error class; anything surfacing or matching on AWS's sentence (operator tooling, tests, log scrapers) diverges, and AWS's wording also states the "exactly one" rule that the emulator's wording hides.
Fix: the message emitted by CreateInput's security-group validation.
Traces: medialive.rtmp-push-input-rejected-without-a-security-group.

Cause 8 of 10 - CreateInput accepts a push input declaring three destinations.
AWS: a push input may declare at most two destinations (one per pipeline). Three is refused with 400 BadRequestException and the fixed message "At least one destination and at most two destinations required"; no input is created (it is absent from ListInputs) and the named security group keeps an empty Inputs list. Recorded behavior only: the Destinations list carries no modeled length constraint.
Emulator: HTTP 201 with an input holding all three destinations (three generated rtmp:// URLs) and a reference on the group.
Why: a client is allowed to declare more ingest endpoints than MediaLive supports, and the extra reference on the security group then blocks the group's teardown.
Fix: CreateInput's destination validation, before the input is persisted and before the group reference is taken.
Traces: medialive.rtmp-push-input-rejects-three-destinations.

Cause 9 of 10 - CreateChannel accepts an input that is already attached to another channel.
AWS: an input may be attached to one channel at a time; a second CreateChannel naming an already-attached input is refused with 409 ConflictException and the message "inputId=<input id> is already attached" - fixed text "inputId=" and " is already attached" around the id from the request. The refused channel is absent from ListChannels and the input's AttachedChannels still names only the first channel. Deleting the first channel lets the identical call through.
Emulator: HTTP 201 - a second channel is created on the same input.
Why: a client relying on the 409 to detect that it is double-booking an input builds a configuration AWS forbids, and the duplicate attachment then corrupts the input's teardown.
Fix: CreateChannel's attachment validation against each input's attachedChannels in state. (Overlaps the open wire issue 20261102T091340Z-medialive-input-lifecycle-diverges-deletes-never-detach-conflicts.md, which does not carry this behavioral check's replay key.)
Traces: medialive.second-channel-on-an-attached-input-rejected.

Cause 10 of 10 - DeleteInputSecurityGroup accepts a group an input still references.
AWS: DeleteInputSecurityGroup (Smithy: DELETE /prod/inputSecurityGroups/{InputSecurityGroupId}, code 200; ConflictException httpError 409 modeled) refuses the delete while inputs still reference the group, with 409 ConflictException and the message "Input Security Group <group id> cannot be deleted because it is still being used by inputs." - fixed text around the group id from the request path. The group stays State "IN_USE" with the referencing input in its Inputs list; once the input is deleted the same call succeeds.
Emulator: HTTP 200 - the delete is accepted while an RTMP_PUSH input still references the group.
Why: a client deleting in the wrong order gets no warning and pulls the group out from under a live input, where AWS would have told it to delete the inputs first.
Fix: DeleteInputSecurityGroup's precondition check against the group's inputs list in state. (Same overlap note as cause 9.)
Traces: medialive.delete-security-group-rejected-while-an-input-references-it.

## Evidence
Expected vs actual, from the replay of simparity-behavioral-aws-v1 against this checkout (all ten verdicts: mismatch, service medialive):

1. BatchDelete, check medialive.batch-delete-rejects-mixed-resource-lists-and-deletes-inputs: expected ('BadRequestException', 'Exactly one non-empty list of channel, multiplex, input, or input security group IDs is required'); got HTTP 200 {'Failed': [], 'Successful': [{'Id': '<input id>', 'State': 'DELETING'}, {'Id': '<group id>', 'State': 'DELETED'}]} - the harness reports "the call succeeded; real AWS refuses it".
2. CreateChannel, medialive.create-channel-rejects-a-role-arn-that-does-not-exist: expected 422 UnprocessableEntityException with ValidationErrors [{ElementPath: 'roleArn', ErrorMessage: "Make sure the role '<ghost role arn>' exists in the user account and medialive.amazonaws.com is a trusted service"}]; got HTTP 201 with a created channel.
3. CreateChannel, medialive.create-channel-rejects-a-role-that-does-not-trust-medialive: expected the same 422/roleArn validation error for a role trusting lambda.amazonaws.com; got HTTP 201 with a created channel.
4. CreateChannel, medialive.create-channel-rejects-an-attachment-of-a-deleted-input: expected ('BadRequestException', 'Input <input id> is DELETED'); got ('UnprocessableEntityException', 'Channel contains errors.').
5. CreateChannel, medialive.create-channel-rejects-single-pipeline-class-with-two-destination-urls: expected ('BadRequestException', 'Exactly 1 destinations settings required for destination <destination id>'); got HTTP 201, ChannelClass 'SINGLE_PIPELINE' with two Settings URLs on destination1.
6. CreateChannel, medialive.create-channel-rejects-standard-class-with-one-destination-url: expected ('BadRequestException', 'Exactly 2 destinations settings required for destination <destination id>'); got HTTP 201, ChannelClass 'STANDARD' with one Settings URL.
7. CreateInput, medialive.rtmp-push-input-rejected-without-a-security-group: expected ('BadRequestException', 'Must specify exactly one security group per input.'); got ('BadRequestException', 'inputSecurityGroups is required for an input of type RTMP_PUSH').
8. CreateInput, medialive.rtmp-push-input-rejects-three-destinations: expected ('BadRequestException', 'At least one destination and at most two destinations required'); got HTTP 201 with an Input carrying three Destinations and SecurityGroups ['<group id>'].
9. CreateChannel, medialive.second-channel-on-an-attached-input-rejected: expected ('ConflictException', 'inputId=<input id> is already attached'); got HTTP 201 with a second channel on the same input.
10. DeleteInputSecurityGroup, medialive.delete-security-group-rejected-while-an-input-references-it: expected ('ConflictException', 'Input Security Group <group id> cannot be deleted because it is still being used by inputs.'); got HTTP 200.

Model references used: BatchDelete POST /prod/batch/delete code 200, errors {BadRequestException 400, ConflictException 409, NotFoundException 404, ...}, input {channelIds, inputIds, inputSecurityGroupIds, multiplexIds}; CreateChannel POST /prod/channels code 201, errors include BadRequestException 400, ConflictException 409, UnprocessableEntityException 422 {message, validationErrors: [ValidationError{elementPath, errorMessage}]}; CreateInput POST /prod/inputs code 201, errors include BadRequestException 400, input member InputSecurityGroups (optional in the model); DeleteInputSecurityGroup DELETE /prod/inputSecurityGroups/{InputSecurityGroupId} code 200 with ConflictException 409.

The ids, names, ARNs and generated URLs in the replay output (channel 1000000/1000001, input 1000000, group 3000000, s3://sim2real-ml-out-*/stream*/index, arn:aws:iam::*:role/sim2real-ml-ghost-*) are per-run fillers for the variable slots of these messages, not literal requirements.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite simparity-behavioral-aws-v1; every step below is one of its medialive checks.
2. Cause 1: create an input security group and two URL_PULL inputs, then POST /prod/batch/delete with BOTH inputIds=[input] and inputSecurityGroupIds=[group] -> expect 400 BadRequestException 'Exactly one non-empty list of channel, multiplex, input, or input security group IDs is required' and no deletions; observe 200 with both ids Successful.
3. Cause 2: POST /prod/channels with a roleArn whose role name does not exist -> expect 422 with validationErrors[0].elementPath 'roleArn'; observe 201.
4. Cause 3: create an IAM role trusting lambda.amazonaws.com and POST /prod/channels with it -> expect the same 422; observe 201 (and the probe input flips out of DETACHED).
5. Cause 4: create a URL_PULL input, DELETE it, wait for State DELETED, then POST /prod/channels attaching that input -> expect 400 'Input <id> is DELETED'; observe 422 'Channel contains errors.'.
6. Cause 5: POST /prod/channels with ChannelClass SINGLE_PIPELINE and a destination carrying two Settings URLs -> expect 400 'Exactly 1 destinations settings required for destination <id>'; observe 201.
7. Cause 6: POST /prod/channels with ChannelClass STANDARD and a destination carrying one Settings URL -> expect 400 'Exactly 2 destinations settings required for destination <id>'; observe 201.
8. Cause 7: POST /prod/inputs with Type RTMP_PUSH and no InputSecurityGroups -> expect message 'Must specify exactly one security group per input.'; observe 'inputSecurityGroups is required for an input of type RTMP_PUSH'.
9. Cause 8: POST /prod/inputs with Type RTMP_PUSH, one security group and three Destinations -> expect 400 'At least one destination and at most two destinations required' and no input; observe 201 with three destinations.
10. Cause 9: attach an input to a channel, wait for IDLE, then POST /prod/channels again attaching the same input -> expect 409 'inputId=<id> is already attached'; observe 201.
11. Cause 10: create a group and an RTMP_PUSH input in it, then DELETE /prod/inputSecurityGroups/{id} -> expect 409 'Input Security Group <id> cannot be deleted because it is still being used by inputs.' and the group still IN_USE; observe 200.
