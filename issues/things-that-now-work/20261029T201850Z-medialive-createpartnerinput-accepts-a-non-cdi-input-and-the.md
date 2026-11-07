Issue-Source: trace-sweep-divergence
Issue-Trace: medialive-create-partner-input-on-regular-input-fails
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medialive-create-partner-input-on-regular-input-fails
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive|CreatePartnerInput|4xx->2xx|$.message
Issue-Key: medialive|CreatePartnerInput|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: medialive|CreatePartnerInput|4xx->2xx|status
Issue-Key: medialive|DeleteInput|2xx->2xx|$.inputPartnerIds[*]
Issue-Key: medialive|DeleteInputSecurityGroup|2xx->2xx|$.inputs[*]
Issue-Key: medialive|DeleteInputSecurityGroup|2xx->2xx|$.state
Issue-Commit: 0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f

# CreatePartnerInput accepts a non-CDI input and the phantom partner then corrupts input and security-group teardown

> _Note: this issue was identified against the code base at commit `0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePartnerInput

## Description
This issue bundles 3 causes with one origin: CreatePartnerInput does not enforce the AWS-CDI precondition, so it creates a partner input that AWS would have refused, and that resource then shows up in two later responses that AWS reports differently. Causes 2 and 3 are observable consequences of Cause 1 but are separate wire divergences on separate operations and must each be re-replayed; fixing the precondition is expected to clear all three, and all three must be confirmed closed before submitting.

Cause 1 of 3: CreatePartnerInput answers 201 for an input that is not an AWS CDI input.
AWS: Smithy: CreatePartnerInput ('Create a partner input') -> POST /prod/inputs/{InputId}/partners, http success code 201, required httpLabel InputId, idempotencyToken RequestId, output member Input; modeled client errors include BadRequestException (client, httpError 400) whose member is `message` (jsonName 'message'). A partner input exists only to be the second feed of an AWS-CDI input pair, so the target input's type is a precondition. Recorded behavior: when InputId names an input that is not of the AWS CDI type, AWS refuses with HTTP 400, header x-amzn-errortype 'BadRequestException', and the message 'Creating a partner is only supported for AWS CDI inputs.' That sentence is entirely fixed text — it interpolates nothing from the request, so it must be reproduced verbatim, including the trailing period.
Emulator: answers 201 Created, with no error body at all ($.message and $headers.x-amzn-errortype both absent), and creates a partner input on the non-CDI input.
Why: a client's negative path inverts — code that relies on the refusal to detect 'this input cannot take a partner' proceeds as though the pairing succeeded. Worse, the accept is not inert: it mints a real partner-input id that the emulator then tracks as a child of the regular input (see Causes 2 and 3), so the account is left in a state AWS cannot produce. This is a missing validation, not a wording difference: the status, the error class and the whole error body are absent.
Fix: input-type validation in the medialive crate's CreatePartnerInput handler — before creating anything, reject an InputId whose input type is not AWS CDI with 400 BadRequestException and the fixed sentence above.
Traces: medialive-create-partner-input-on-regular-input-fails#task#3.

Cause 2 of 3: DeleteInput reports the phantom partner in inputPartnerIds.
AWS: Smithy: DeleteInput -> the response projects the input, including its inputPartnerIds list. Recorded behavior: deleting the regular input of this fixture returns a projection whose inputPartnerIds is empty/absent, because AWS refused the partner creation in Cause 1 and the input therefore never acquired a partner.
Emulator: answers 200 with $.inputPartnerIds[0] present, carrying the id the over-permissive CreatePartnerInput minted.
Why: the deleted input advertises a partner relationship that cannot exist for its type, so a client reconciling partner pairs (or cleaning up partners before deleting an input) is pointed at a partner id whose existence AWS would deny.
Fix: resolved by Cause 1's precondition (no partner is ever attached); the inputPartnerIds projection in the DeleteInput/input-describe path must reflect only partners the service accepted.
Traces: medialive-create-partner-input-on-regular-input-fails#teardown#4.

Cause 3 of 3: DeleteInputSecurityGroup reports the phantom input and keeps the group IN_USE instead of IDLE.
AWS: Smithy: the input-security-group projection carries Inputs (jsonName 'inputs', list of input ids currently using the group) and State (jsonName 'state', enum InputSecurityGroupState with values IDLE, IN_USE, UPDATING, DELETED). Recorded behavior: after the only input that referenced the group is deleted, the group's inputs list is empty and its state is IDLE, so the delete proceeds against an unused group.
Emulator: answers 200 but with $.inputs[0] still naming the phantom partner input and $.state = 'IN_USE'.
Why: the group is pinned IN_USE by a resource AWS says was never created, so a teardown or reconciliation loop that waits for IDLE before deleting a security group never converges, and the inputs list names an id the account should not hold. State is wrong on the wire, not merely a message.
Fix: resolved by Cause 1's precondition; independently, the input-security-group usage computation and State derivation in the medialive crate's input-security-group state must not count a partner input that was never validly created.
Traces: medialive-create-partner-input-on-regular-input-fails#teardown#5.

## Evidence
Replay of simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false, score 83.2%), fixture medialive-create-partner-input-on-regular-input-fails, 3 mismatch clusters:

1. task#3 CreatePartnerInput, expected_status 400 but actual_status 201:
   - status: expected 400, got 201
   - $.message: missing (expected 'Creating a partner is only supported for AWS CDI inputs.', got None)
   - $headers.x-amzn-errortype: missing (expected 'BadRequestException', got None)
   The expected message is wholly fixed text with no interpolated request or state values.
2. teardown#4 DeleteInput, 200->200: $.inputPartnerIds[0] 'extra (expected None, got <partner-input-id>)' — the recorded response has no partner ids; the emulator lists the one it minted in step 1.
3. teardown#5 DeleteInputSecurityGroup, 200->200: $.inputs[0] 'extra (expected None, got <partner-input-id>)' and $.state 'value_mismatch (expected IDLE, got IN_USE)'.

Smithy grounding: CreatePartnerInput POST /prod/inputs/{InputId}/partners, success code 201, modeled BadRequestException httpError 400 with a 'message' member; InputSecurityGroupState enum = IDLE | IN_USE | UPDATING | DELETED. The partner-input id in the recording is a generated identifier and is not itself a requirement; the requirements are that no partner id appears and that state is IDLE.

## Steps to Reproduce
1. Create an input that is NOT an AWS CDI input (the fixture creates a regular input with an input security group).
2. POST /prod/inputs/<thatInputId>/partners (CreatePartnerInput). Replay medialive-create-partner-input-on-regular-input-fails#task#3: AWS answers 400 with x-amzn-errortype 'BadRequestException' and message 'Creating a partner is only supported for AWS CDI inputs.'; the emulator answers 201 with no error body and creates a partner input.
3. Delete the regular input (replay the same fixture's teardown#4): the DeleteInput projection's inputPartnerIds must be empty — the emulator lists the partner id minted in step 2.
4. Delete the input security group (replay teardown#5): the group's inputs list must be empty and its state must be IDLE — the emulator still names the phantom partner input and reports IN_USE, so a wait-for-IDLE teardown loop never completes.
5. Re-run all three steps after adding the input-type precondition to confirm the accept, the stray inputPartnerIds entry and the IN_USE state all clear.
