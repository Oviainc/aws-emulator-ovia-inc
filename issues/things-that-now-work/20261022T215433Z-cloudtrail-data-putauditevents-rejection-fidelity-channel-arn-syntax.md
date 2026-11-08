Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail-data.channel-not-found-rejected cloudtrail-data.duplicated-audit-event-id-rejected cloudtrail-data.put-audit-events-batch-exceeds-max-size-rejected cloudtrail-data.put-audit-events-empty-batch-rejected cloudtrail-data.put-audit-events-id-max-length-boundary-rejected cloudtrail-data.put-audit-events-id-pattern-violation-rejected cloudtrail-data.put-audit-events-rejects-a-batch-over-one-hundred-events cloudtrail-data.invalid-channel-arn-rejected cloudtrail-data.put-audit-events-channel-arn-wrong-service-segment-rejected cloudtrail-data.put-audit-events-channel-arn-missing-region-segment-rejected cloudtrail-data.put-audit-events-rejects-a-cross-account-channel-as-unauthorized cloudtrail-data.channel-cross-account-access-denied-vs-same-account-not-found cloudtrail-data.put-audit-events-invalid-channel-arn-partition-boundary-rejected cloudtrail-data.channel-arn-unicode-characters-rejected cloudtrail-data.put-audit-events-rejects-a-bare-channel-id cloudtrail-data.put-audit-events-rejects-in-arn-then-constraint-then-channel-order
Issue-Shape-Id: com.amazonaws.cloudtraildata#CloudTrailDataService
Issue-Key: cloudtrail-data.channel-not-found-rejected|run#10|AssertionError|assert message == f"The channel {nonexistent_arn} could not be found.", (
Issue-Key: cloudtrail-data.duplicated-audit-event-id-rejected|run#9|AssertionError|assert dup_message == not_found, f"unexpected shared-id message: {dup_message!r}"
Issue-Key: cloudtrail-data|PutAuditEvents|ChannelNotFound
Issue-Key: cloudtrail-data.put-audit-events-empty-batch-rejected|run#6|AssertionError|assert message == f"The channel {channel_arn} could not be found.", (
Issue-Key: cloudtrail-data.put-audit-events-id-max-length-boundary-rejected|run#12|AssertionError|assert message == not_found_message, f"unexpected resolution message at the id boundary: {message!r}"
Issue-Key: cloudtrail-data.put-audit-events-id-pattern-violation-rejected|run#7|AssertionError|assert good_message == f"The channel {channel_arn} could not be found.", (
Issue-Key: cloudtrail-data.put-audit-events-rejects-a-batch-over-one-hundred-events|run#7|AssertionError|assert at_limit == not_found, f"unexpected resolution message at 100 events: {at_limit!r}"
Issue-Key: cloudtrail-data.invalid-channel-arn-rejected|run#6|AssertionError|assert message == _BAD_ARN, f"unexpected malformed-ARN message: {message!r}"
Issue-Key: cloudtrail-data.put-audit-events-channel-arn-wrong-service-segment-rejected|run#6|AssertionError|assert wrong_service_message == _BAD_ARN, f"unexpected malformed-ARN message: {wrong_service_message!r}"
Issue-Key: cloudtrail-data.put-audit-events-channel-arn-missing-region-segment-rejected|run#6|AssertionError|assert empty_region_message == "The specified channel ARN is not a valid channel ARN.", (
Issue-Key: cloudtrail-data.put-audit-events-rejects-a-cross-account-channel-as-unauthorized|run#6|AssertionError|assert code == "AccessDeniedException", (
Issue-Key: cloudtrail-data.channel-cross-account-access-denied-vs-same-account-not-found|run#8|AssertionError|assert cross_code == "AccessDeniedException", (
Issue-Key: cloudtrail-data.put-audit-events-invalid-channel-arn-partition-boundary-rejected|run#5|AssertionError|assert wrong_partition_code == "InvalidChannelARN", (
Issue-Key: cloudtrail-data.channel-arn-unicode-characters-rejected|run#8|AssertionError|assert ascii_code == code, (
Issue-Key: cloudtrail-data.put-audit-events-rejects-a-bare-channel-id|run#7|AssertionError|assert code == "InvalidChannelARN", (
Issue-Key: cloudtrail-data.put-audit-events-rejects-in-arn-then-constraint-then-channel-order|run#7|AssertionError|assert code == "InvalidChannelARN", (
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# cloudtrail-data PutAuditEvents rejection fidelity: channel-ARN syntax accepted too loosely, cross-account ARNs leak existence, and both refusal message templates diverge (8 causes)

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAuditEvents

## Description
PutAuditEvents is the only operation this service models, and every one of its refusal paths diverges from AWS. 8 distinct root causes, all in the same handler/state pair (services/service-com-amazonaws-cloudtraildata-cloudtraildataservice/src/ops/put_audit_events.rs and .../src/state.rs). The suite exercised all of them against a channel that does not exist, so the refusal classification IS the whole observable contract here.

=== Cause 1 of 8: ChannelNotFound message template has the ARN in the wrong place ===
AWS: ChannelNotFound is HTTP 400 with a request-derived message that interpolates the channelArn the caller sent into a fixed template: "The channel <channelArn> could not be found." (Smithy: com.amazonaws.cloudtraildata#ChannelNotFound, errorKind client, httpError 400 [defaulted], member `message`; the shape's own doc is "The channel could not be found." and the recorded behavior shows the ARN interpolated before "could not be found."). The rule is known because the message is built from a request member.
Emulator: emits the ARN as a trailing suffix instead — e.g. 'The channel could not be found: arn:aws:cloudtrail:us-west-2:000000000000:channel/<id>'. Code (ChannelNotFound) and status (400) are right; the template is inverted.
Why: every check that uses an absent channel as its positive control for a deeper assertion (batch-size boundary, id-length boundary, id-pattern, empty batch, duplicate-id ordering) fails at that control, so a client (or a fixture asserting on refusal text built from its own request) cannot recognize the service's own not-found rendering.
Fix: the ChannelNotFound construction in the PutAuditEvents handler (the `channel_mut` miss branch).
Traces: cloudtrail-data.channel-not-found-rejected, cloudtrail-data.duplicated-audit-event-id-rejected, cloudtrail-data.put-audit-events-batch-exceeds-max-size-rejected, cloudtrail-data.put-audit-events-empty-batch-rejected, cloudtrail-data.put-audit-events-id-max-length-boundary-rejected, cloudtrail-data.put-audit-events-id-pattern-violation-rejected, cloudtrail-data.put-audit-events-rejects-a-batch-over-one-hundred-events.

=== Cause 2 of 8: InvalidChannelARN message appends the offending ARN, AWS does not ===
AWS: InvalidChannelARN is HTTP 400 and the message is the fixed sentence the model documents, with NO ARN echoed: "The specified channel ARN is not a valid channel ARN." (Smithy: com.amazonaws.cloudtraildata#InvalidChannelARN, httpError 400, doc "The specified channel ARN is not a valid channel ARN."). The model states the message, so it is a requirement.
Emulator: appends ": <channelArn>" — e.g. 'The specified channel ARN is not a valid channel ARN: arn:aws:s3:us-west-2:000000000000:channel/<id>'.
Why: the refusal text is not the service's, and it echoes caller input into a message AWS keeps constant.
Fix: the InvalidChannelARN construction in the PutAuditEvents handler (the `parse_channel_ref` miss branch).
Traces: cloudtrail-data.invalid-channel-arn-rejected, cloudtrail-data.put-audit-events-channel-arn-wrong-service-segment-rejected, cloudtrail-data.put-audit-events-channel-arn-missing-region-segment-rejected.

=== Cause 3 of 8: a channel ARN in another account is answered ChannelNotFound/400 instead of AccessDeniedException/403 ===
AWS: recorded behavior only (the model declares no AccessDeniedException for this operation; the authorization refusal is produced by the IAM layer ahead of the service). A channelArn whose ACCOUNT segment is not the caller's is refused at that layer with error code AccessDeniedException and HTTP 403, and the denial names the action `cloudtrail-data:PutAuditEvents` and the resource ARN; it never states whether the channel exists. The identical channel id under the caller's own account gets past authorization and is refused ChannelNotFound/400.
Emulator: answers ChannelNotFound/400 for both, e.g. for arn:aws:cloudtrail:us-west-2:111122223333:channel/<id> -> 'The channel could not be found: ...'. The handler only compares `channel.owner_account_id` to the emulator account AFTER the channel has been resolved out of one flat map (src/state.rs channel_mut / src/ops/put_audit_events.rs ownership check), so an unresolvable foreign ARN never reaches an authorization verdict.
Why: a client gets 400 (a request-shape class) where AWS gives 403 (an authorization class), so retry/permission handling is wrong — and the emulator becomes an existence oracle for other accounts' channels, which AWS deliberately is not.
Fix: an account/authorization gate in the PutAuditEvents handler that runs on the parsed ARN BEFORE channel resolution.
Traces: cloudtrail-data.put-audit-events-rejects-a-cross-account-channel-as-unauthorized, cloudtrail-data.channel-cross-account-access-denied-vs-same-account-not-found.

=== Cause 4 of 8: a channel ARN in a different partition is treated as well-formed ===
AWS: an ARN built in another partition (arn:aws-cn:cloudtrail:...) is not a channel ARN the caller can reference and is refused InvalidChannelARN/400, distinctly from a same-partition absent channel (ChannelNotFound/400). Recorded behavior only; the model constrains channelArn merely to @pattern ^arn:.*$ (Smithy: PutAuditEventsRequest$channelArn, ChannelArn, arnReference).
Emulator: ChannelNotFound. src/state.rs::parse_channel_arn accepts ANY value that passes `is_arn_segment` in the partition field and keeps it, so aws-cn parses as a valid channel ARN and falls through to resolution.
Why: callers see a cross-partition reference as merely absent rather than invalid, hiding a region/partition misconfiguration.
Fix: partition validation in src/state.rs::parse_channel_arn (or the handler's ARN check) against the partition the emulator serves.
Traces: cloudtrail-data.put-audit-events-invalid-channel-arn-partition-boundary-rejected.

=== Cause 5 of 8: a channel resource id that is not a channel UUID is treated as well-formed ===
AWS: the resource segment of a channel ARN must be a channel id; an arn:aws:cloudtrail:<region>:<account>:channel/cafe-rocket-not-real is refused InvalidChannelARN, the same class as the multi-byte/unicode variant of the same string — the encoding of the id does not change which error path is taken. Recorded behavior only (the replayed check asserts the unicode form and its plain-ASCII analogue must classify IDENTICALLY, and the unicode form is InvalidChannelARN).
Emulator: classifies them DIFFERENTLY — the unicode form is InvalidChannelARN (it fails src/state.rs::is_channel_id's ASCII-only test) while the ASCII analogue 'cafe-rocket-not-real' passes is_channel_id (any [-_A-Za-z0-9]{1,128}) and comes back ChannelNotFound.
Why: the same logical reference changes error class purely with its encoding, and arbitrary non-id strings are reported as absent channels rather than invalid ARNs.
Fix: tighten the resource-suffix check in src/state.rs (is_channel_id / parse_channel_arn) to the channel-id form, so both encodings take one path.
Traces: cloudtrail-data.channel-arn-unicode-characters-rejected.

=== Cause 6 of 8: a channelArn that is not an ARN is answered ValidationException, not InvalidChannelARN ===
AWS: a bare channel id (and any other non-ARN string) in channelArn is refused by the SERVICE with InvalidChannelARN/400 and the fixed message of Cause 2 — even though the model documents the member as "The ARN or ID (the ARN suffix) of a channel" and constrains it only to @pattern ^arn:.*$ (Smithy: PutAuditEventsRequest$channelArn). The documented ID form is NOT accepted in practice; the refusal is the service's, not the constraint layer's.
Emulator: the modelled @pattern is enforced at the request boundary, so the caller gets ValidationException with "1 validation error detected: Value '<id>' at 'channelArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:.*$".
Why: a client that handles InvalidChannelARN (a modelled error of this operation) never sees it; it gets an unmodelled ValidationException instead.
Fix: route channelArn pattern failures to the handler's InvalidChannelARN path rather than the generic constraint validator, for this operation's channelArn member.
Traces: cloudtrail-data.put-audit-events-rejects-a-bare-channel-id.

=== Cause 7 of 8: rejection ORDER is constraint-then-ARN; AWS is ARN-then-constraint-then-channel ===
AWS: recorded behavior only. PutAuditEvents refuses in a fixed three-layer order — channel-ARN syntax first (InvalidChannelARN/400), then modelled constraint validation (ValidationException/400), then channel resolution (ChannelNotFound/400). A request with BOTH a malformed channelArn and a constraint-violating auditEvents[].id is reported as InvalidChannelARN, and the malformed-ARN refusal is payload-independent.
Emulator: constraint validation runs first and AGGREGATES both violations: 'ValidationException: 2 validation errors detected: Value 'sim2real bad id!' at 'auditEvents.1.member.id' ...; Value 'sim2real-not-an-arn' at 'channelArn' ...'.
Why: a client fixing errors in the order the service reports them is led to the wrong layer, and the ARN refusal becomes payload-dependent.
Fix: the request walk / validation ordering for this operation — the ARN check must precede modelled constraint validation.
Traces: cloudtrail-data.put-audit-events-rejects-in-arn-then-constraint-then-channel-order.

=== Cause 8 of 8: ValidationException messages render the modelled @pattern with the anchors AWS omits ===
AWS: the constraint-violation message quotes the pattern WITHOUT anchors: "Member must satisfy regular expression pattern: [-_A-Za-z0-9]+". The rule is known: the message is built from the request and the member's modelled pattern (Smithy: AuditEvent$id -> Uuid, @pattern ^[-_A-Za-z0-9]+$, @length 1..128), and AWS strips the leading ^ and trailing $.
Emulator: renders '^[-_A-Za-z0-9]+$' verbatim, e.g. "... at 'auditEvents.1.member.id' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[-_A-Za-z0-9]+$".
Why: clients and tooling that parse the pattern out of the message get a different expression than AWS reports. (A sibling issue filed for another service, 20261022T212810Z-featurestore-runtime-sagemaker-validation-message-renders-featuregroupnameorarn-pattern-with-regex-anchors.md, describes the same rendering rule; it does not cover this service.)
Fix: the shared constraint-violation message serializer, as reached by this operation's validation.
Traces: cloudtrail-data.put-audit-events-rejects-in-arn-then-constraint-then-channel-order.

## Evidence
All from the behavioral replay against the emulator (service cloudtrail-data, operation PutAuditEvents, HTTP POST /PutAuditEvents?channelArn=...).

Cause 1 — expected 'The channel arn:aws:cloudtrail:us-west-2:<acct>:channel/<id> could not be found.'; got 'The channel could not be found: arn:aws:cloudtrail:us-west-2:000000000000:channel/00000000-0000-0000-0000-000000000000' (code ChannelNotFound, 400, both correct).

Cause 2 — expected 'The specified channel ARN is not a valid channel ARN.'; got 'The specified channel ARN is not a valid channel ARN: arn:aws:cloudtrail:us-west-2:000000000000:not-a-channel-resource' and 'The specified channel ARN is not a valid channel ARN: arn:aws:s3:us-west-2:000000000000:channel/4ba88f6a-...' and '... : arn:aws:cloudtrail::000000000000:channel/ccf8ff3b-...'.

Cause 3 — expected code AccessDeniedException with HTTP 403, message naming 'cloudtrail-data:PutAuditEvents' and the ARN, never 'could not be found'; got code ChannelNotFound with HTTP 400: 'The channel could not be found: arn:aws:cloudtrail:us-west-2:111122223333:channel/8f84a000-...' (and the same for account 111111111111).

Cause 4 — channelArn arn:aws-cn:cloudtrail:us-west-2:<acct>:channel/<uuid>: expected InvalidChannelARN, got 'ChannelNotFound'.

Cause 5 — unicode resource id (channel/café-🚀-not-real) classified 'InvalidChannelARN' while the ASCII analogue (channel/cafe-rocket-not-real) classified 'ChannelNotFound'; AWS gives the same class for both.

Cause 6 — channelArn = a bare uuid 'a1fad3e4-74d7-4a52-ab33-109ef1153dd2': expected InvalidChannelARN, got ValidationException "1 validation error detected: Value 'a1fad3e4-...' at 'channelArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:.*$".

Cause 7 — channelArn='sim2real-not-an-arn' plus auditEvents=[{id:'sim2real bad id!'}]: expected InvalidChannelARN; got ValidationException "2 validation errors detected: ... at 'auditEvents.1.member.id' ...; ... at 'channelArn' ...".

Cause 8 — same response: pattern rendered '^[-_A-Za-z0-9]+$' where AWS renders '[-_A-Za-z0-9]+'.

Source read in the checkout: services/service-com-amazonaws-cloudtraildata-cloudtraildataservice/src/ops/put_audit_events.rs lines 287-294 (InvalidChannelARN message), 310-316 (ChannelNotFound message), 321-340 (ownership check runs only after resolution); services/service-com-amazonaws-cloudtraildata-cloudtraildataservice/src/state.rs lines 68-117 (is_channel_id / is_arn_segment / parse_channel_arn accept any partition and any [-_A-Za-z0-9]{1,128} resource id).

## Steps to Reproduce
Boot the emulator and point a botocore cloudtrail-data client at it in us-west-2, then:
1. Cause 1: put_audit_events(channelArn='arn:aws:cloudtrail:us-west-2:<account>:channel/<random-uuid>', auditEvents=[{id:'<uuid>', eventData:'{"version":"1.0",...}'}]) — inspect Error.Message of the ChannelNotFound.
2. Cause 2: same call with channelArn='arn:aws:cloudtrail:us-west-2:<account>:not-a-channel-resource' — inspect Error.Message of the InvalidChannelARN.
3. Cause 3: same call with channelArn='arn:aws:cloudtrail:us-west-2:111122223333:channel/<uuid>' — expect AccessDeniedException/403; observe ChannelNotFound/400. Repeat with the caller's own account id to see the 400 control.
4. Cause 4: channelArn='arn:aws-cn:cloudtrail:us-west-2:<account>:channel/<uuid>' — expect InvalidChannelARN.
5. Cause 5: channelArn='arn:aws:cloudtrail:us-west-2:<account>:channel/café-🚀-not-real' then '.../channel/cafe-rocket-not-real' — compare Error.Code; they must match.
6. Cause 6: channelArn='<bare uuid>' with a botocore client built with Config(parameter_validation=False) — expect InvalidChannelARN.
7. Causes 7 and 8: channelArn='sim2real-not-an-arn' with auditEvents=[{id:'sim2real bad id!', eventData:'{...}'}] (parameter_validation=False) — expect InvalidChannelARN; then fix only the ARN and expect a single-violation ValidationException whose pattern is printed without ^...$.
