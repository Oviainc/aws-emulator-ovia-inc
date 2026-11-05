Issue-Source: trace-sweep-divergence
Issue-Trace: agent-registry-control-registry-record-approval-workflow
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: agent-registry-control-registry-record-approval-workflow
Issue-Shape-Id: com.amazonaws.agentregistrycontrol#AgentRegistryControl
Issue-Key: agent-registry-control|SubmitRegistryRecordForApproval|2xx->2xx|$.updatedAt
Issue-Key: agent-registry-control|SubmitRegistryRecordForApproval|4xx->4xx|$.message
Issue-Key: agent-registry-control|SubmitRegistryRecordForApproval|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: agent-registry-control|SubmitRegistryRecordForApproval|4xx->4xx|status
Issue-Commit: 650142439453f256773dfdd9d373235adc353165

# SubmitRegistryRecordForApproval responses diverge from AWS on timestamp encoding and invalid-status error

> _Note: this issue was identified against the code base at commit `650142439453f256773dfdd9d373235adc353165`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitRegistryRecordForApproval

## Description
2 causes. Both are in the SubmitRegistryRecordForApproval path of the agentregistrycontrol service crate and both are net-new: neither cluster is owned by an open issue.

Cause 1 of 2 - SubmitRegistryRecordForApproval emits its `updatedAt` member as a JSON number instead of an RFC3339 date-time string (fixture agent-registry-control-registry-record-approval-workflow, step task#5).
AWS: Smithy: SubmitRegistryRecordForApprovalResponse.updatedAt is @required and targets com.amazonaws.agentregistrycontrol#DateTimestamp, a timestamp shape carrying @timestampFormat("date-time"). That trait overrides restJson1's default epoch-seconds payload encoding, so AWS serializes this member as an RFC3339/ISO-8601 date-time STRING (shape "<4-digit-year>-<MM>-<DD>T<HH>:<MM>:<SS>(.<frac>)?Z"). The operation's success status is 202 (Smithy http {method: POST, uri: /registries/{registryId}/records/{recordId}/submit-for-approval, code: 202}); the recorded response asserts the SHAPE of updatedAt, not a literal instant.
Emulator: returns the correct 202 but writes updatedAt as a bare fractional-epoch JSON number, so the replay reports `$.updatedAt: value_mismatch (expected '<shaped-not-asserted>', got <epoch number>)`.
Why: an AWS SDK deserializing a @timestampFormat("date-time") member expects a string; a number is either rejected as a parse error or read as a different instant, so any client that reads back when the record entered the approval workflow breaks - including pollers that compare updatedAt across calls.
Fix: the response serializer for SubmitRegistryRecordForApproval (the DateTimestamp serialization used by this operation's output path) - honour @timestampFormat("date-time") rather than falling back to the restJson1 epoch default. The open issue "AgentRegistryControl serializes date-time timestamps as epoch numbers" (20261104T045821Z-agent-registry-control-agentregistrycontrol-serializes-date-time-timestamps-as-epoch-numbers.md) records the same wire-type fault, but its Issue-Key scopes it to GetRegistryRecord only, so this operation's cluster is not covered by it and is filed here.
Traces: agent-registry-control-registry-record-approval-workflow#task#5.

Cause 2 of 2 - submitting a record that is not in DRAFT is refused with the wrong HTTP status and the wrong error code and message (fixture agent-registry-control-registry-record-approval-workflow, step task#9).
AWS: SubmitRegistryRecordForApproval is documented as "Submits a DRAFT registry record for approval"; the record's status is an input-state precondition, and the recorded response shows AWS refusing a non-DRAFT record with HTTP 400 - i.e. ValidationException (Smithy: ValidationException errorKind=client, httpError=400, message @required), NOT ConflictException (httpError=409). The recorded error message is "Cannot submit the record for approval in current status: APPROVED". Its FIXED text is "Cannot submit the record for approval in current status: "; the only VARIABLE part is the trailing RegistryRecordStatus value read from the stored record (one of DRAFT/PENDING_APPROVAL/APPROVED/REJECTED/DEPRECATED/CREATING/UPDATING/CREATE_FAILED/UPDATE_FAILED). It carries no record id, no registry id and no restatement of the rule.
Emulator: answers HTTP 409 with x-amzn-errortype ConflictException and the message "record <recordId> is in status <status>; only a DRAFT record can be submitted for approval" - a different status class, a different modeled error shape, and a message whose fixed text, ordering and interpolated values (it injects the record id and appends a rule clause) are all AWS-unlike.
Why: SDK and client error handling branches on the status class and on x-amzn-errortype. A client catching ValidationException for an invalid-state submit never matches, while retry/conflict-resolution logic keyed on ConflictException fires on what AWS treats as a non-retryable validation failure; 409 is also classified differently from 400 by generic retry policies. Operators matching on the message text see neither AWS's wording nor its value layout.
Fix: the SubmitRegistryRecordForApproval handler's status precondition check - raise ValidationException (400) with AWS's message template instead of ConflictException (409), interpolating only the record's current status.
Note: this step's recorded current status is APPROVED while the emulator's stored record is still PENDING_APPROVAL. That state drift is the separately filed open issue "AgentRegistryControl record reads ignore every mutation after create"; the status code, error code and message-template divergence reported here is independent of which status value gets interpolated.
Traces: agent-registry-control-registry-record-approval-workflow#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator booted from this checkout (emulator_up=true, 52 steps, 28 passed, score 53.8%).

Cause 1 - cluster service=agent-registry-control|SubmitRegistryRecordForApproval|status=202->202|keys=$.updatedAt, fixture agent-registry-control-registry-record-approval-workflow step task#5:
  status 202 as expected, but
  $.updatedAt: value_mismatch (expected '<shaped-not-asserted>', got 1793880265.3038085)
The number is this run's clock value; the requirement is the WIRE TYPE (RFC3339 date-time string), not any particular instant.
Model grounding: DateTimestamp = {"type":"timestamp","timestampFormat":"date-time"}; SubmitRegistryRecordForApprovalResponse.updatedAt targets it and is @required.

Cause 2 - cluster service=agent-registry-control|SubmitRegistryRecordForApproval|status=400->409|keys=$.message,$headers.x-amzn-errortype,status, fixture agent-registry-control-registry-record-approval-workflow step task#9:
  status: expected 400, got 409
  $.message: value_mismatch (expected 'Cannot submit the record for approval in current status: APPROVED', got 'record b7f8cf68497c is in status PENDING_APPROVAL; only a DRAFT record can be submitted for approval')
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ConflictException')
Expected: HTTP 400 / ValidationException (Smithy httpError 400) with fixed text "Cannot submit the record for approval in current status: " + the record's current RegistryRecordStatus.
Actual: HTTP 409 / ConflictException (Smithy httpError 409) with an emulator-authored sentence that interpolates the record id ('b7f8cf68497c' here is this run's generated id, not part of any template).

## Steps to Reproduce
1. Call git_clone(), boot the emulator from the checkout, and replay trace set simparity-wire-aws-v1.
2. Cause 1: run fixture agent-registry-control-registry-record-approval-workflow to step task#5 - CreateRegistry, CreateRegistryRecord, then POST /registries/{registryId}/records/{recordId}/submit-for-approval. Inspect the 202 body: updatedAt is a bare JSON number, where AWS returns an RFC3339 date-time string for this @timestampFormat("date-time") @required member.
3. Cause 2: continue the same fixture to step task#9 - POST the same /submit-for-approval URI again for a record that is no longer in DRAFT. The emulator answers HTTP 409 with x-amzn-errortype ConflictException and 'record <id> is in status <status>; only a DRAFT record can be submitted for approval'; AWS answers HTTP 400 (ValidationException) with 'Cannot submit the record for approval in current status: <status>'.
