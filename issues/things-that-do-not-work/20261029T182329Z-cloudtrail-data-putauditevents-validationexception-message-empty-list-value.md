Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail-data.put-audit-events-empty-batch-rejected cloudtrail-data.put-audit-events-id-pattern-violation-rejected
Issue-Shape-Id: com.amazonaws.cloudtraildata#CloudTrailDataService
Issue-Key: cloudtrail-data.put-audit-events-empty-batch-rejected|run#9|AssertionError|assert message == (
Issue-Key: cloudtrail-data.put-audit-events-id-pattern-violation-rejected|run#11|AssertionError|assert bad_message == (
Issue-Commit: fc350f637d730e9c7a46ff0b276c9700aec9957e

# cloudtrail-data PutAuditEvents ValidationException message: empty-list value slot renders '0 items' instead of '[]', and the @pattern slot keeps the ^...$ anchors AWS strips

> _Note: this issue was identified against the code base at commit `fc350f637d730e9c7a46ff0b276c9700aec9957e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAuditEvents

## Description
TWO net-new root causes in the shared constraint-violation message serializer reached by PutAuditEvents request validation. Both grounding checks get the HTTP status (400), the error code (ValidationException), the violation count, the member path and the modelled constraint sentence right, and both fail only on one interpolated slot of the message template. They are bundled because they share the service and the component; a fixer must close BOTH before submitting.

Note on existing issues: no open issue carries either of these two check ids in its replay key, so neither is covered today. Three open issues describe the SAME serializer from adjacent angles and a fixer holding any of them should treat this as the same work item: 20261027T001848Z-cloudtrail-data-putauditevents-validationexception-for-an-over-length.md and 20261029T175219Z-cloudtrail-data-putauditevents-validationexception-message-value-slot-diverges.md both cover the list value slot for a NON-EMPTY list violating the @length MAXIMUM (the '[AuditEvent(...), ...]' rendering), and neither states the empty-list/minimum branch of Cause 1; 20261028T004339Z-cloudtrail-data-putauditevents-validationexception-renders-the-modelled-pattern.md states the anchor-stripping rule of Cause 2 but is grounded only in cloudtrail-data.put-audit-events-rejects-in-arn-then-constraint-then-channel-order, so the check below is not bound to it and would be closed out unrun. 20261022T215433Z-...-channel-arn-syntax.md covers channel-ARN syntax and cross-account existence leakage, not these slots. The resolved issue 20261023T133927Z-...-wire-refusals-diverge-invalidchannelarn-message.md covers InvalidChannelARN text and non-ARN routing; it does not claim either slot, and neither cause below is a refusal of a call that issue claimed to fix.

=== Cause 1 of 2: the '<value>' slot of a list-member @length MINIMUM violation reports a synthesized cardinality phrase instead of the rendered empty list ===
AWS: PutAuditEvents' auditEvents member is a required list with a modelled length FLOOR as well as a ceiling. Smithy: com.amazonaws.cloudtraildata#PutAuditEventsRequest$auditEvents -> com.amazonaws.cloudtraildata#AuditEvents, required, type list, @length {min 1, max 100}, member AuditEvent; PutAuditEvents does NOT model ValidationException (its modelled errors are ChannelInsufficientPermission, ChannelNotFound, ChannelUnsupportedSchema, DuplicatedAuditEventId, InvalidChannelARN, UnsupportedOperationException, all client/400), so the refusal is produced by the shared constraint layer ahead of the handler. Sending an empty auditEvents list is refused with HTTP 400 and error code ValidationException whose message follows the standard constraint-violation template. Its FIXED text is "<N> validation error(s) detected: Value '<value>' at '<member path>' failed to satisfy constraint: Member must have length greater than or equal to <min>" (plural "validation errors detected" when N > 1). Its VARIABLE parts come from the request and the model: the violation count, the member path (the plain member name 'auditEvents' for a list-level violation), the modelled minimum (1, from the model), and the rejected VALUE taken from the request. Recorded behavior only for the one part the model does not state -- how a list value is rendered into the '<value>' slot: the rendering is the collection's Java toString form, so an EMPTY list renders as the two literal characters '[]'. That is the degenerate case of the same rule the sibling issues state for a non-empty list ('[' + a comma-joined 'AuditEvent(id=..., eventData=..., eventDataChecksum=...)' per element + ']'); with no elements, nothing is joined and only the brackets remain. The constraint layer must reach this verdict BEFORE channel resolution: the same absent channel ARN with a non-empty batch is correctly refused ChannelNotFound.
Emulator: gets the status (400), the code (ValidationException), the count (1), the member path ('auditEvents') and the modelled floor (1) all right, then fills the value slot with a synthesized cardinality phrase: "1 validation error detected: Value '0 items' at 'auditEvents' failed to satisfy constraint: Member must have length greater than or equal to 1", where AWS says "... Value '[]' at 'auditEvents' failed to satisfy constraint: Member must have length greater than or equal to 1". '0 items' is the emulator's own phrase; the caller never sent the string '0 items', and AWS never produces an '<n> items' shape for a list member.
Why: the error message is a requirement. A client, recorder or conformance test that derives the expected refusal text from its own request plus the model cannot reconcile the emulator's message with AWS's, and any consumer parsing the value slot out of a ValidationException sees a phrase shape AWS does not emit. This is the worst failure mode for a refusal, because status and code look structurally correct. Because the slot lives in the SHARED serializer, the divergence is not confined to the empty case or to this member: it reproduces for every list-typed member whose @length is violated in this service and in any other service routed through the same serializer.
Fix: the shared constraint-violation message serializer, in the branch that formats the '<value>' slot for a LIST-typed member -- the MINIMUM/empty path specifically. It must render the collection (yielding '[]' when there are no elements) rather than substituting a count. The scalar path, which already renders a value verbatim, must not change. The @length floor ENFORCEMENT itself, the violation count, the member path, the modelled minimum, the validate-before-channel-resolution ordering, the status and the error code are all already correct and must not regress.
Traces: cloudtrail-data.put-audit-events-empty-batch-rejected (fails at its line-57 assert on the empty-batch violation message, after its ChannelNotFound positive control on the same absent channel ARN passes).

=== Cause 2 of 2: the '<pattern>' slot of a @pattern violation echoes the modelled trait verbatim, including its leading ^ and trailing $ ===
AWS: an auditEvents element whose 'id' violates its modelled pattern is refused with HTTP 400 and error code ValidationException, following the same constraint-violation template with the per-constraint sentence "Member must satisfy regular expression pattern: <pattern>". Smithy: com.amazonaws.cloudtraildata#AuditEvent$id -> com.amazonaws.cloudtraildata#Uuid, required, @length {min 1, max 128}, @pattern "^[-_A-Za-z0-9]+$". As above, ValidationException is not modelled on the operation, so the message comes from the shared constraint layer. The variable parts are all derived from the request and the model: the violation count, the rejected value verbatim from the request, the 1-based element member path ('auditEvents.<n>.member.id'), and the member's modelled pattern. The rule for the pattern slot, known from the model plus the recording, is that AWS renders the modelled @pattern with the leading '^' and trailing '$' REMOVED, i.e. the model's "^[-_A-Za-z0-9]+$" is reported as "[-_A-Za-z0-9]+".
Emulator: gets the status (400), the code (ValidationException), the count (1), the rejected value (verbatim from the request) and the 1-based path ('auditEvents.1.member.id') right, and then interpolates the trait value as written: "1 validation error detected: Value '<rejected id>' at 'auditEvents.1.member.id' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[-_A-Za-z0-9]+$", where AWS ends that sentence "... regular expression pattern: [-_A-Za-z0-9]+".
Why: a client or SDK that parses the regular expression out of a ValidationException gets a different expression than AWS reports -- an anchored one, which does not compare equal to the service's and, if fed back into a client-side validator, is not the expression AWS claims to have applied. Every consumer asserting on the refusal text it can derive from its own request plus the model mismatches. Because the slot sits in the shared serializer, this reproduces for every @pattern-constrained member of this service (e.g. channelArn's ^arn:.*$ and externalId's ^[\w+=,.@:\/-]*$) and any other service routed through it.
Fix: the shared constraint-violation message serializer, in the branch that formats the '<pattern>' slot for a @pattern violation -- it must strip a single leading '^' and a single trailing '$' from the modelled pattern before interpolating it. The pattern ENFORCEMENT itself, the violation count, the 1-based member path, the verbatim rejected value, the status and the error code are already correct and must not regress.
Traces: cloudtrail-data.put-audit-events-id-pattern-violation-rejected (fails at its line-67 assert on the id-pattern violation message, after its ChannelNotFound positive control with a well-formed id on the same absent channel ARN passes).

## Evidence
From the behavioral replay against the emulator built from this checkout (service cloudtrail-data, operation PutAuditEvents, POST /PutAuditEvents?channelArn=...). Suite total 10/19; 9 cloudtrail-data checks failed, 7 of which are already OWNED by open issues' replay keys. These are the 2 unowned failures.

--- Cause 1 ---
[mismatch] cloudtrail-data.put-audit-events-empty-batch-rejected
  File check_cloudtrail_data_put_audit_events_empty_batch_rejected.py, line 57, in run
  assert message == (...)
  AssertionError: unexpected empty-batch violation message: "1 validation error detected: Value '0 items' at 'auditEvents' failed to satisfy constraint: Member must have length greater than or equal to 1"

Actual   (emulator): HTTP 400, Error.Code 'ValidationException', Error.Message
  "1 validation error detected: Value '0 items' at 'auditEvents' failed to satisfy constraint: Member must have length greater than or equal to 1"
Expected (AWS):      HTTP 400, Error.Code 'ValidationException', Error.Message
  "1 validation error detected: Value '[]' at 'auditEvents' failed to satisfy constraint: Member must have length greater than or equal to 1"

Which parts are fixed text and which are fill-ins:
  fixed text   -> "<N> validation error detected: Value '" ... "' at '" ... "' failed to satisfy constraint: Member must have length greater than or equal to " ... (plural "validation errors detected" when N > 1)
  from request -> the violation count (1), the member path ('auditEvents', the plain member name for a list-level violation), and the rejected VALUE (the empty list, rendered '[]')
  from model   -> the minimum (1, from AuditEvents @length min)
Only the value slot diverges. Nothing per-run participates in this requirement: the check's channel ARN embeds a fresh uuid4 and the emulator's own account id, but neither appears in the asserted message, and no request id or clock value is involved.

Confirmed CORRECT by the same check and must not regress: with the SAME never-created channel ARN, a well-formed NON-EMPTY one-event batch is refused Error.Code 'ChannelNotFound' with Error.Message f'The channel {channel_arn} could not be found.' -- the positive control proving the unvalidated client and the request pipeline are healthy, and that the constraint layer runs ahead of channel resolution for the empty batch. The check reaches the service at all only because it builds its client with Config(parameter_validation=False); botocore's own @length min 1 check would otherwise raise locally and never ask the emulator.

--- Cause 2 ---
[mismatch] cloudtrail-data.put-audit-events-id-pattern-violation-rejected
  File check_cloudtrail_data_put_audit_events_id_pattern_violation_rejected.py, line 67, in run
  assert bad_message == (...)
  AssertionError: unexpected id-pattern violation message: "1 validation error detected: Value 'sim2real id with spaces' at 'auditEvents.1.member.id' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[-_A-Za-z0-9]+$"

Actual   (emulator): HTTP 400, Error.Code 'ValidationException', Error.Message
  "1 validation error detected: Value '<rejected id>' at 'auditEvents.1.member.id' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[-_A-Za-z0-9]+$"
Expected (AWS):      HTTP 400, Error.Code 'ValidationException', Error.Message
  "1 validation error detected: Value '<rejected id>' at 'auditEvents.1.member.id' failed to satisfy constraint: Member must satisfy regular expression pattern: [-_A-Za-z0-9]+"

Which parts are fixed text and which are fill-ins:
  fixed text   -> "<N> validation error detected: Value '" ... "' at '" ... "' failed to satisfy constraint: Member must satisfy regular expression pattern: " ...
  from request -> the violation count (1), the rejected value (the id exactly as sent -- 'sim2real id with spaces' in this run, a fill-in and not part of the template), the 1-based element member path ('auditEvents.1.member.id')
  from model   -> the pattern, rendered WITHOUT its anchors ('[-_A-Za-z0-9]+' from the trait value '^[-_A-Za-z0-9]+$')
Only the pattern slot diverges: the single '^' and single '$' at each end of the regex are the entire defect.

Confirmed CORRECT by the same check and must not regress: the same never-created channel ARN with a well-formed id is refused ChannelNotFound/400 with f'The channel {channel_arn} could not be found.', and the pattern rejection is distinguishable from the channel-not-found rejection (different codes). This check too requires Config(parameter_validation=False) to put the malformed id on the wire.

Model evidence (smithy_query against services/service-com-amazonaws-cloudtraildata-cloudtraildataservice/smithy-model/com-amazonaws-cloudtraildata-cloudtraildataservice.json, operation PutAuditEvents): input auditEvents -> AuditEvents, required, type list, @length {min 1, max 100}, member AuditEvent; channelArn -> ChannelArn, required, @pattern '^arn:.*$', httpQuery channelArn; externalId -> ExternalId, @length {min 2, max 1224}, @pattern '^[\w+=,.@:\/-]*$', httpQuery externalId. AuditEvent$id -> Uuid, required, @length {min 1, max 128}, @pattern '^[-_A-Za-z0-9]+$'; AuditEvent$eventData -> String, required; AuditEvent$eventDataChecksum -> String, optional. Operation errors: ChannelInsufficientPermission, ChannelNotFound, ChannelUnsupportedSchema, DuplicatedAuditEventId, InvalidChannelARN, UnsupportedOperationException (all errorKind client, httpError 400). ValidationException is NOT among them, confirming both messages are produced by the shared constraint layer rather than the handler's own error construction. HTTP binding: POST /PutAuditEvents, success 200.

## Steps to Reproduce
Setup (common to both causes)
1. Boot the emulator built from this checkout and point a botocore cloudtrail-data client at it in us-west-2.
2. Build the client with Config(parameter_validation=False). This is required: botocore enforces the auditEvents @length min 1 and the AuditEvent$id @pattern locally and would raise ParamValidationError without ever asking the emulator, which lets a target that never checks these score green.
3. Resolve the emulator's own account and build a well-formed same-account channel ARN that was never created: account = sts.get_caller_identity()['Account']; absent = f"arn:aws:cloudtrail:us-west-2:{account}:channel/{uuid4()}".
4. Positive control that must keep passing for both causes: put_audit_events(channelArn=absent, auditEvents=[{'id': uuid4().hex, 'eventData': json.dumps({'version':'1.0','userIdentity':{'type':'CustomUserIdentity','principalId':'sim2real-principal'},'eventSource':'sim2real.example.com','eventName':'Sim2RealTestAction','eventTime':'2026-01-01T00:00:00','UID':uuid4().hex})}]) -> ClientError with Error.Code 'ChannelNotFound' and Error.Message f'The channel {absent} could not be found.' This proves the request reaches the service and that anything refused below is refused by the constraint layer, not by channel resolution.

Cause 1 -- empty-list value slot
5. Call put_audit_events(channelArn=absent, auditEvents=[]) on the unvalidated client.
6. Expect HTTP 400, Error.Code 'ValidationException', Error.Message exactly "1 validation error detected: Value '[]' at 'auditEvents' failed to satisfy constraint: Member must have length greater than or equal to 1".
7. Observe instead "1 validation error detected: Value '0 items' at 'auditEvents' failed to satisfy constraint: Member must have length greater than or equal to 1" -- correct status, code, count, member path and minimum, with the value slot replaced by a synthesized cardinality phrase.
8. Generalise the fix past the empty case so it does not regress the sibling MAXIMUM branch: send 101 events and confirm the value slot renders the whole rejected list element-by-element in request order ('[AuditEvent(id=..., eventData=..., eventDataChecksum=null), ...]', with eventDataChecksum printed as 'null' only where the caller omitted it) rather than any '<n> items' phrase; and confirm a 100-event batch still clears constraint validation and is refused ChannelNotFound.

Cause 2 -- @pattern slot anchors
9. Call put_audit_events(channelArn=absent, auditEvents=[{'id': 'sim2real id with spaces', 'eventData': <the same json string>}]) on the unvalidated client (the space violates @pattern ^[-_A-Za-z0-9]+$).
10. Expect HTTP 400, Error.Code 'ValidationException', and Error.Message ending "... at 'auditEvents.1.member.id' failed to satisfy constraint: Member must satisfy regular expression pattern: [-_A-Za-z0-9]+" -- the modelled pattern with no leading '^' and no trailing '$', and the rejected id echoed verbatim in the value slot.
11. Observe instead that it ends "... regular expression pattern: ^[-_A-Za-z0-9]+$": correct status, code, count, verbatim rejected value and 1-based member path, with the anchors left on the pattern.
12. Confirm the pattern rejection remains distinguishable from the channel-not-found rejection (ValidationException vs ChannelNotFound), which already holds.
13. Generalise past this one member, since the slot is produced by the shared serializer: send a pattern-violating externalId (@pattern ^[\w+=,.@:\/-]*$, within its 2..1224 length) and confirm that message also prints its pattern with the anchors stripped.

Confirm
14. Re-replay cloudtrail-data.put-audit-events-empty-batch-rejected and cloudtrail-data.put-audit-events-id-pattern-violation-rejected. Both must pass; do not regress the 7 sibling cloudtrail-data checks owned by other open issues, in particular the ARN-syntax-before-constraint-before-channel-resolution ordering and the fixed InvalidChannelARN sentence 'The specified channel ARN is not a valid channel ARN.'
