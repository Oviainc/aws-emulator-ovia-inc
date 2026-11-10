Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail-data.put-audit-events-batch-exceeds-max-size-rejected cloudtrail-data.put-audit-events-rejects-a-batch-over-one-hundred-events
Issue-Shape-Id: com.amazonaws.cloudtraildata#CloudTrailDataService
Issue-Key: cloudtrail-data|PutAuditEvents|ValidationException
Issue-Key: cloudtrail-data.put-audit-events-rejects-a-batch-over-one-hundred-events|run#11|AssertionError|assert over_limit == (
Issue-Commit: aeabc7614a597a3eb2961dbaa54178223081b119

# cloudtrail-data PutAuditEvents ValidationException for an over-length auditEvents list summarizes the rejected value as 'N items' instead of rendering the list value AWS echoes

> _Note: this issue was identified against the code base at commit `aeabc7614a597a3eb2961dbaa54178223081b119`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAuditEvents

## Description
One root cause, in the shared constraint-violation message serializer as reached by PutAuditEvents request validation. Both grounding checks now get the correct HTTP status (400), the correct error code (ValidationException) and the correct channel-resolution control (a 100-event batch reaches ChannelNotFound with the right message, so the ceiling itself sits correctly at an inclusive 100). The only divergence left is how the serializer renders the offending VALUE for a LIST-typed member whose @length is violated.

=== Cause 1 of 1: the list-member constraint-violation message substitutes a count summary for the rejected value ===
AWS: PutAuditEvents' auditEvents member is a list with a modelled length ceiling (Smithy: com.amazonaws.cloudtraildata#PutAuditEventsRequest$auditEvents -> com.amazonaws.cloudtraildata#AuditEvents, list, @length min 1 max 100, member AuditEvent; AuditEvent members id -> Uuid @length 1..128 @pattern ^[-_A-Za-z0-9]+$, eventData, eventDataChecksum). Exceeding it is refused with HTTP 400 and error code ValidationException, whose message follows the standard constraint-violation template: fixed text "<N> validation error(s) detected: Value '<value>' at '<member path>' failed to satisfy constraint: Member must have length less than or equal to <max>". The variable parts come from the request and the model: the violation count, the rejected VALUE, the member path ('auditEvents'), and the modelled maximum (100). Recorded behavior only for the one part the model does not state — how a list value is rendered into the '<value>' slot: the recording shows the service echoing the whole rejected list in its Java toString form, i.e. "[" + a comma-joined "AuditEvent(id=<id>, eventData=<eventData>, eventDataChecksum=<checksum-or-null>)" per element + "]", one element per event actually sent, in request order, with the id/eventData values taken verbatim from the request and eventDataChecksum rendered as 'null' when the caller omitted it. The count, the ids, the eventData JSON and the number of elements are all request-derived fill-ins; the fixed text is the template around them.
Emulator: emits the right status, the right code, the right member path and the right limit, but replaces the value slot with a count summary: "1 validation error detected: Value '101 items' at 'auditEvents' failed to satisfy constraint: Member must have length less than or equal to 100". It never renders the list's elements at all — '101 items' is the emulator's own synthesized phrase, not any value the caller sent.
Why: a client (or a recorder, or a test asserting on the refusal text it can derive from its own request) cannot reconcile the emulator's message with AWS's. AWS's message is self-describing — it identifies exactly which events were in the rejected batch, so a caller can locate the offending records and resplit the batch; the emulator's message discards that entirely and reports only a cardinality. Any consumer parsing the value slot out of a ValidationException gets a phrase of a shape ('<n> items') that AWS never produces for a list member.
Fix: the shared constraint-violation message serializer reached by this operation's request validation — the branch that formats the rejected value for a LIST-typed member. Scalar members already render their value verbatim (the same serializer's string path is what produces the id/pattern violations the sibling checks assert); the list path must render the collection's elements, not a count. The @length ceiling enforcement itself and the validate-before-channel-resolution ordering are already correct and must not change.
Note on existing issues: the open issue 20261022T215433Z-cloudtrail-data-putauditevents-rejection-fidelity-channel-arn-syntax.md lists both of these check ids and its Cause 1 (the ChannelNotFound message template) is what used to make them fail; that cause now passes for these two checks, which fail past it on the list-value rendering described here. Its Cause 8 concerns anchors in the rendered @pattern of a STRING member, a different slot of the same template. The resolved issue 20261023T133927Z-cloudtrail-data-putauditevents-wire-refusals-diverge-invalidchannelarn-message.md covers InvalidChannelARN text, non-ARN routing and cross-account authorization, none of which is this. No open issue carries this cause.
Traces: cloudtrail-data.put-audit-events-batch-exceeds-max-size-rejected, cloudtrail-data.put-audit-events-rejects-a-batch-over-one-hundred-events.

## Evidence
From the behavioral replay against the emulator (service cloudtrail-data, operation PutAuditEvents, POST /PutAuditEvents?channelArn=...).

Both failing checks send a 101-event auditEvents batch (one past the modelled max of 100) against a same-account channel ARN that was never created, and both assert on Error.Message of the resulting ValidationException.

[mismatch] cloudtrail-data.put-audit-events-batch-exceeds-max-size-rejected
  AssertionError: unexpected over-size violation message: "1 validation error detected: Value '101 items' at 'auditEvents' failed to satisfy constraint: Member must have length less than or equal to 100"
  Expected: "1 validation error detected: Value '[AuditEvent(id=<id-1>, eventData=<eventData-1>, eventDataChecksum=null), ... , AuditEvent(id=<id-101>, eventData=<eventData-101>, eventDataChecksum=null)]' at 'auditEvents' failed to satisfy constraint: Member must have length less than or equal to 100"

[mismatch] cloudtrail-data.put-audit-events-rejects-a-batch-over-one-hundred-events
  AssertionError: unexpected over-size violation message: "1 validation error detected: Value '101 items' at 'auditEvents' failed to satisfy constraint: Member must have length less than or equal to 100"
  Same expected template. This check then goes on to assert the id-length boundary (129 chars), the id-pattern violation and the two-violations-aggregated message with 1-based member paths; it never reaches them, so those paths are unverified by this run and are not claimed here.

What matched and must not regress: error code ValidationException and HTTP 400 for the 101-event batch; the member path 'auditEvents'; the limit 100 in the fixed text; the violation count 1; and the positive control — a 100-event batch against the same absent channel clears constraint validation and is refused ChannelNotFound/400 with 'The channel <channelArn> could not be found.'

What diverges: only the value slot of the template. Fixed text of that template: "1 validation error detected: Value '" ... "' at 'auditEvents' failed to satisfy constraint: Member must have length less than or equal to 100". Request-derived fill-ins: the violation count (1), the member path, the modelled max (100), and the rejected list value. The emulator's '101 items' is not a request-derived value at all; the per-element ids and eventData JSON in the expected rendering are request values (and differ every run), and 'null' for eventDataChecksum reflects the caller omitting that member.

Model evidence: com.amazonaws.cloudtraildata#AuditEvents is a list with @length {min: 1, max: 100} of AuditEvent; AuditEvent has required id (Uuid, @length 1..128, @pattern ^[-_A-Za-z0-9]+$), required eventData (String) and optional eventDataChecksum (String). PutAuditEvents does not model ValidationException, so the refusal comes from the shared constraint layer ahead of the handler — which is why the fix belongs in that serializer and not in the operation's own error construction.

## Steps to Reproduce
1. Boot the emulator built from this checkout and point a botocore cloudtrail-data client at it in us-west-2.
2. Build a channel ARN for the emulator's own account that was never created: channel_arn = f"arn:aws:cloudtrail:us-west-2:{sts.get_caller_identity()['Account']}:channel/{uuid4()}".
3. Positive control (must keep passing): put_audit_events(channelArn=channel_arn, auditEvents=[{'id': <uuid-hex>, 'eventData': <json string>} for 100 events]). Expect ClientError with Error.Code 'ChannelNotFound' and Error.Message f'The channel {channel_arn} could not be found.' — this proves the 101-event call below is rejected by the constraint layer and not by the size check being off by one.
4. Repeat the same call with 101 events. Expect HTTP 400, Error.Code 'ValidationException', and Error.Message "1 validation error detected: Value '<the rejected list rendered as [AuditEvent(id=..., eventData=..., eventDataChecksum=null), ...] for all 101 events in request order>' at 'auditEvents' failed to satisfy constraint: Member must have length less than or equal to 100".
5. Observe instead Error.Message "1 validation error detected: Value '101 items' at 'auditEvents' failed to satisfy constraint: Member must have length less than or equal to 100" — correct status, code, member path and limit; the value slot replaced by a synthesized count.
6. Vary the batch to 150 events and to 101 events where some events carry an explicit eventDataChecksum, to confirm the fix renders every element in request order and prints the supplied checksum (and 'null' only when the member was omitted) rather than any count.
7. Re-replay cloudtrail-data.put-audit-events-batch-exceeds-max-size-rejected and cloudtrail-data.put-audit-events-rejects-a-batch-over-one-hundred-events to confirm.
