Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail-data.put-audit-events-rejects-in-arn-then-constraint-then-channel-order
Issue-Shape-Id: com.amazonaws.cloudtraildata#CloudTrailDataService
Issue-Key: cloudtrail-data.put-audit-events-rejects-in-arn-then-constraint-then-channel-order|run#11|AssertionError|assert constraint == (
Issue-Commit: 2882fa01447cddf34bfea2947358b39ec0b8885e

# cloudtrail-data PutAuditEvents ValidationException renders the modelled @pattern with the ^...$ anchors AWS strips, so the three-layer rejection-order check fails at its constraint layer

> _Note: this issue was identified against the code base at commit `2882fa01447cddf34bfea2947358b39ec0b8885e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAuditEvents

## Description
One root cause, in the shared constraint-violation message serializer as reached by PutAuditEvents request validation. This is the ONLY divergence left in the grounding check: layer 1 (channel-ARN syntax -> InvalidChannelARN/400 with the fixed sentence 'The specified channel ARN is not a valid channel ARN.') now passes, the rejection ORDER now passes (ARN syntax ahead of modelled constraint validation, a single violation reported rather than an aggregate), and the error code/status of the constraint layer are correct. The check fails on the TEXT of the constraint-violation message.

Note on existing issues: this is the same cause already written as 'Cause 8 of 8' of the open issue 20261022T215433Z-cloudtrail-data-putauditevents-rejection-fidelity-channel-arn-syntax.md, which lists this same check id. That issue's replay key for this check pins its run#7 assert (the InvalidChannelARN ordering assert), which now PASSES, so the replay no longer binds this surviving failure to it; it is filed here so the anchor rendering is not closed out unrun when the rest of that issue's causes are verified. A fixer holding that issue should treat the two as one work item. The sibling open issue 20261027T001848Z-cloudtrail-data-putauditevents-validationexception-for-an-over-length.md concerns a different slot of the SAME template (how a LIST value is rendered into the '<value>' slot) and does not cover the pattern slot. No resolved issue covers this.

=== Cause 1 of 1: the constraint-violation message echoes the Smithy @pattern verbatim, including its leading ^ and trailing $ ===
AWS: PutAuditEvents refuses an auditEvents element whose `id` violates the modelled pattern with HTTP 400 and error code ValidationException, whose message follows the standard constraint-violation template. Smithy: com.amazonaws.cloudtraildata#PutAuditEventsRequest$auditEvents -> AuditEvents (list, @length min 1 max 100) of AuditEvent; AuditEvent$id -> com.amazonaws.cloudtraildata#Uuid, required, @length 1..128, @pattern "^[-_A-Za-z0-9]+$". PutAuditEvents does not model ValidationException (its modelled errors are ChannelInsufficientPermission, ChannelNotFound, ChannelUnsupportedSchema, DuplicatedAuditEventId, InvalidChannelARN, UnsupportedOperationException), so the refusal comes from the shared constraint layer ahead of the handler. Fixed text of the template: "<N> validation error(s) detected: Value '<value>' at '<member path>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>". The variable parts are all derived from the request and the model: the violation count, the rejected value taken verbatim from the request, the 1-based member path of the offending element ('auditEvents.<n>.member.id'), and the member's modelled pattern. The rule for the pattern slot is known from the model plus the recording: AWS renders the modelled @pattern with the leading '^' and trailing '$' REMOVED, i.e. the model's "^[-_A-Za-z0-9]+$" is reported as "[-_A-Za-z0-9]+". The same rendering rule is already filed against another service (20261022T212810Z-featurestore-runtime-sagemaker-validation-message-renders-featuregroupnameorarn-pattern-with-regex-anchors.md), which does not cover cloudtrail-data.
Emulator: gets the status (400), the code (ValidationException), the violation count (1), the rejected value ('sim2real bad id!' — verbatim from the request) and the member path ('auditEvents.1.member.id', 1-based, correct) all right, and then emits the pattern slot verbatim from the model: "1 validation error detected: Value 'sim2real bad id!' at 'auditEvents.1.member.id' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[-_A-Za-z0-9]+$" where AWS ends that sentence "... regular expression pattern: [-_A-Za-z0-9]+".
Why: the error message is a requirement, and a client, SDK retry/branching logic or recorder that parses the regular expression out of a ValidationException gets a different expression than AWS reports — an anchored one, which does not compare equal to the service's and, if fed back into a client-side validator, is not the expression AWS claims to have applied. Every consumer asserting on the refusal text it can derive from its own request plus the model mismatches. Because this slot sits in the SHARED serializer, the divergence is not confined to AuditEvent$id: it will reproduce for every @pattern-constrained member of this service (and any other service routed through the same serializer), e.g. channelArn's ^arn:.*$ and externalId's ^[\w+=,.@:\/-]*$.
Fix: the shared constraint-violation message serializer, in the branch that formats the '<pattern>' slot for a @pattern violation — it must strip a single leading '^' and a single trailing '$' from the modelled pattern before interpolating it, rather than interpolating the trait value as written. The pattern ENFORCEMENT itself, the violation count, the 1-based member path, the verbatim rejected value, the status and the error code are all already correct and must not regress; neither must the ARN-before-constraint ordering nor the InvalidChannelARN text that this check now clears. Source cited in the owning issue for orientation: services/service-com-amazonaws-cloudtraildata-cloudtraildataservice/src/ops/put_audit_events.rs and .../src/state.rs.
Traces: cloudtrail-data.put-audit-events-rejects-in-arn-then-constraint-then-channel-order (fails at its line-87 assert on the constraint-violation message).

## Evidence
From the behavioral replay against the emulator (service cloudtrail-data, operation PutAuditEvents, HTTP POST /PutAuditEvents?channelArn=...). Suite total 6/11; this is the only failing cloudtrail-data check not already owned by an open issue's replay key.

[mismatch] cloudtrail-data.put-audit-events-rejects-in-arn-then-constraint-then-channel-order
  File check_cloudtrail_data_put_audit_events_rejects_in_arn_then_constraint_then_channel_order.py, line 87, in run
  assert constraint == (...)
  AssertionError: unexpected constraint-violation message: "1 validation error detected: Value 'sim2real bad id!' at 'auditEvents.1.member.id' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[-_A-Za-z0-9]+$"

Actual   (emulator): HTTP 400, Error.Code 'ValidationException', Error.Message
  "1 validation error detected: Value 'sim2real bad id!' at 'auditEvents.1.member.id' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[-_A-Za-z0-9]+$"
Expected (AWS):     HTTP 400, Error.Code 'ValidationException', Error.Message
  "1 validation error detected: Value 'sim2real bad id!' at 'auditEvents.1.member.id' failed to satisfy constraint: Member must satisfy regular expression pattern: [-_A-Za-z0-9]+"

Which parts are fixed text and which are fill-ins:
  fixed text   -> "<N> validation error detected: Value '" ... "' at '" ... "' failed to satisfy constraint: Member must satisfy regular expression pattern: " ... (and the plural "validation errors detected" when N > 1)
  from request -> the violation count (1), the rejected value ('sim2real bad id!', sent as auditEvents[0].id by the check), the 1-based member path ('auditEvents.1.member.id')
  from model   -> the pattern, rendered WITHOUT its anchors ('[-_A-Za-z0-9]+' from the trait value '^[-_A-Za-z0-9]+$')
Only the last slot diverges; the single character difference at each end of the regex is the entire defect. Nothing per-run (no id, timestamp or request id) is part of this requirement.

What the same check confirmed as already CORRECT and must not regress:
  - channelArn='sim2real-not-an-arn' with the same invalid batch -> InvalidChannelARN/400, message exactly 'The specified channel ARN is not a valid channel ARN.' (no ARN echoed), i.e. layer 1 wins over layer 2 and is payload-independent.
  - a single violation is reported for the well-formed-ARN + invalid-batch call, not an aggregate of the ARN and the id — the ARN-then-constraint ordering is right.

Model evidence (services/service-com-amazonaws-cloudtraildata-cloudtraildataservice/smithy-model/com-amazonaws-cloudtraildata-cloudtraildataservice.json): PutAuditEventsRequest$auditEvents -> AuditEvents, list, @length {min 1, max 100}, member AuditEvent; AuditEvent$id -> Uuid, required, @length {min 1, max 128}, @pattern '^[-_A-Za-z0-9]+$'; PutAuditEventsRequest$channelArn -> ChannelArn, @pattern '^arn:.*$', httpQuery channelArn; PutAuditEventsRequest$externalId -> ExternalId, @length 2..1224, @pattern '^[\w+=,.@:\/-]*$'. ValidationException is NOT among the operation's modelled errors, confirming the message is produced by the shared constraint layer rather than the handler's own error construction.

## Steps to Reproduce
1. Boot the emulator built from this checkout and point a botocore cloudtrail-data client at it in us-west-2, built with Config(parameter_validation=False) so the client-side @pattern check does not pre-empt the service.
2. Resolve the emulator's own account: account = sts.get_caller_identity()['Account'].
3. Build a well-formed same-account channel ARN that was never created: absent = f"arn:aws:cloudtrail:us-west-2:{account}:channel/{uuid4()}".
4. Build a batch with one element whose id violates the modelled pattern: bad_batch = [{'id': 'sim2real bad id!', 'eventData': json.dumps({'version':'1.0','userIdentity':{'type':'CustomIdentity','principalId':'sim2real-behav'},'eventSource':'sim2real.behavioral','eventName':'Sim2RealProbeEvent','eventTime':'2026-01-01T00:00:00Z','UID':str(uuid4())})}].
5. Call put_audit_events(channelArn=absent, auditEvents=bad_batch). Expect HTTP 400, Error.Code 'ValidationException', and Error.Message ending "... failed to satisfy constraint: Member must satisfy regular expression pattern: [-_A-Za-z0-9]+" — the modelled pattern with no leading '^' and no trailing '$'.
6. Observe instead that it ends "... regular expression pattern: ^[-_A-Za-z0-9]+$": correct status, code, count, rejected value and 1-based member path, with the anchors left on the pattern.
7. Controls that must keep passing: (a) put_audit_events(channelArn='sim2real-not-an-arn', auditEvents=bad_batch) -> InvalidChannelARN/400 with message exactly 'The specified channel ARN is not a valid channel ARN.'; (b) put_audit_events(channelArn=absent, auditEvents=[one event with a valid uuid id]) -> ChannelNotFound/400 with message f'The channel {absent} could not be found.'; (c) put_audit_events(channelArn='sim2real-not-an-arn', auditEvents=[the same valid event]) -> the identical InvalidChannelARN message as (a).
8. Generalise the fix check past this one member: send an over-1224-char-free but pattern-violating externalId (pattern ^[\w+=,.@:\/-]*$), and a channelArn that is a bare id if it still routes through the constraint layer, and confirm those messages also print their patterns with the anchors stripped — the slot is produced by the shared serializer, not by this member.
9. Re-replay cloudtrail-data.put-audit-events-rejects-in-arn-then-constraint-then-channel-order to confirm.
