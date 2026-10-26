Issue-Source: trace-sweep-divergence
Issue-Trace: cloudtrail-data-invalid-channel-arn-error
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudtrail-data-invalid-channel-arn-error cloudtraildata-audit-event-batch-size-ceiling cloudtraildata-audit-event-constraint-rejections cloudtraildata-audit-events-batch-exceeds-max-size-rejected cloudtraildata-channel-resolution-rejections
Issue-Shape-Id: com.amazonaws.cloudtraildata#CloudTrailDataService
Issue-Key: cloudtrail-data|PutAuditEvents|4xx->4xx|$.message
Issue-Key: cloudtrail-data|PutAuditEvents|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cloudtrail-data|PutAuditEvents|4xx->4xx|$.Message
Issue-Key: cloudtrail-data|PutAuditEvents|4xx->4xx|status
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# cloudtrail-data PutAuditEvents wire refusals diverge: InvalidChannelARN message echoes the ARN, non-ARN channelArn routed to ValidationException, and a cross-account channel answered 400 ChannelNotFound instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAuditEvents

## Description
Wire replay of the cloudtrail-data corpus (5 fixtures, 14 steps, 11 mismatched) shows every PutAuditEvents refusal path diverging from the recorded AWS wire responses. 3 distinct root causes, all in this one operation's ARN-validation / authorization / error-construction path. There is an open BEHAVIORAL issue on the same handler (20261022T215433Z-cloudtrail-data-putauditevents-rejection-fidelity-channel-arn-syntax.md); it carries behavioral check ids only and NO wire replay keys, so it does not cover these fixture steps — the bindings below are what a fixer must re-replay.

=== Cause 1 of 3: InvalidChannelARN message appends the offending ARN; AWS keeps the sentence fixed ===
AWS: InvalidChannelARN is HTTP 400 with a constant message, no caller input echoed: 'The specified channel ARN is not a valid channel ARN.' (Smithy: com.amazonaws.cloudtraildata#InvalidChannelARN, errorKind client, httpError 400 [defaulted], single member `message`; the shape doc is exactly 'The specified channel ARN is not a valid channel ARN.'). The model states the message, so the text is a requirement, not an example.
Emulator: returns the right status (400) and error code but appends ': <channelArn>' to the sentence, interpolating whatever the caller sent.
Why: a client or recorder matching the service's documented refusal text does not recognise the emulator's rendering, and caller input leaks into a message AWS holds constant.
Fix: the InvalidChannelARN construction in the PutAuditEvents handler's malformed-ARN branch (the ARN-parse miss path) — emit the fixed sentence with no suffix.
Traces: cloudtrail-data-invalid-channel-arn-error#task#1 (representative; cluster of 9 steps, also covering cloudtraildata-audit-event-batch-size-ceiling#task#1,#task#2,#task#3,#task#5, cloudtraildata-audit-event-constraint-rejections#task#1,#task#3, cloudtraildata-audit-events-batch-exceeds-max-size-rejected#task#1, cloudtraildata-channel-resolution-rejections#task#3).

=== Cause 2 of 3: a channelArn that is not an ARN is refused by the constraint layer as ValidationException instead of by the service as InvalidChannelARN ===
AWS: a non-ARN value in channelArn is refused by the SERVICE with error code InvalidChannelARN and HTTP 400, carrying the fixed message of Cause 1 — even though the member is modelled only with @pattern ^arn:.*$ (Smithy: PutAuditEventsRequest$channelArn -> ChannelArn, required, httpQuery channelArn, @pattern ^arn:.*$, arnReference). InvalidChannelARN is a modelled error of this operation; ValidationException is not modelled on it at all. The recorded response for this step is 400 with x-amzn-errortype 'InvalidChannelARN:http://internal.amazon.com/coral/com.amazonaws.cloudtraildata/'.
Emulator: enforces the modelled pattern at the request boundary and answers 400 with x-amzn-errortype 'ValidationException' and a generic constraint message (example: "1 validation error detected: Value 'sim2real-probe-not-an-arn' at 'channelArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:.*$").
Why: a client with a handler for the operation's modelled InvalidChannelARN never sees it and instead gets an error code that is absent from the service model, so the refusal is unclassifiable by a generated SDK's error matcher.
Fix: route channelArn pattern failures for this operation to the handler's InvalidChannelARN path rather than the shared constraint validator (validation/request-walk ordering for PutAuditEvents).
Traces: cloudtraildata-channel-resolution-rejections#task#2.

=== Cause 3 of 3: a channel ARN in another account is answered ChannelNotFound/400 instead of AccessDeniedException/403 ===
AWS: recorded behavior only — the operation models no AccessDeniedException (modelled errors are ChannelInsufficientPermission, ChannelNotFound, ChannelUnsupportedSchema, DuplicatedAuditEventId, InvalidChannelARN, UnsupportedOperationException), so this refusal comes from the authorization layer ahead of the service. A channelArn whose ACCOUNT segment is not the caller's is refused with HTTP 403, x-amzn-errortype 'AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/', and a request-derived Message naming the principal, the action cloudtrail-data:PutAuditEvents and the resource ARN (that rule is known: the message is built from the request's identity and channelArn) — it never reveals whether the channel exists.
Emulator: answers HTTP 400 with x-amzn-errortype 'ChannelNotFound' and no Message member at all, because ownership is only compared after the channel has been resolved out of the single channel map, so an unresolvable foreign ARN never reaches an authorization verdict.
Why: the status class changes from 403 (authorization) to 400 (request shape), so permission/retry handling and credential diagnostics are wrong; and the emulator becomes an existence oracle for other accounts' channels, which AWS deliberately is not.
Fix: an account/authorization gate in the PutAuditEvents handler that runs on the parsed ARN BEFORE channel resolution, emitting AccessDeniedException with HTTP 403.
Traces: cloudtraildata-channel-resolution-rejections#task#4.

Replay context: emulator_up true, bootstrap_signal false, state_reset_failures 0 — so none of these divergences is contaminated by leftover fixture state.

## Evidence
From the wire replay of trace set simparity-wire-aws-v1 against the emulator (service cloudtrail-data, operation PutAuditEvents, POST /PutAuditEvents?channelArn=...). Summary: steps_total 14, steps_passed 3, steps_mismatched 11, score_pct 21.4, state_reset_failures 0.

Cause 1 — fixture cloudtrail-data-invalid-channel-arn-error, step task#1 (divergence_key service=cloudtrail-data|PutAuditEvents|status=400->400|keys=$.message), verdict mismatch, expected status 400 / actual 400:
  $.message value_mismatch — expected 'The specified channel ARN is not a valid channel ARN.', got 'The specified channel ARN is not a valid channel ARN: arn:aws:cloudtrail:us-west-2:111111111111:not-a-channel-resource'.
  Cluster size 9 (affected_count 9): cloudtrail-data-invalid-channel-arn-error#task#1, cloudtraildata-audit-event-batch-size-ceiling#task#1/#task#2/#task#3/#task#5, cloudtraildata-audit-event-constraint-rejections#task#1/#task#3, cloudtraildata-audit-events-batch-exceeds-max-size-rejected#task#1, cloudtraildata-channel-resolution-rejections#task#3. Status and error code match; only the message template diverges (the recorded ARN value is an example — the requirement is the fixed sentence the model documents).

Cause 2 — fixture cloudtraildata-channel-resolution-rejections, step task#2 (divergence_key service=cloudtrail-data|PutAuditEvents|status=400->400|keys=$.message,$headers.x-amzn-errortype), expected status 400 / actual 400:
  $headers.x-amzn-errortype shape_mismatch — expected 'InvalidChannelARN:http://internal.amazon.com/coral/com.amazonaws.cloudtraildata/', got 'ValidationException'.
  $.message value_mismatch — expected 'The specified channel ARN is not a valid channel ARN.', got "1 validation error detected: Value 'sim2real-probe-not-an-arn' at 'channelArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:.*$".

Cause 3 — fixture cloudtraildata-channel-resolution-rejections, step task#4 (divergence_key service=cloudtrail-data|PutAuditEvents|status=403->400|keys=$.Message,$headers.x-amzn-errortype,status):
  status — expected 403, got 400.
  $headers.x-amzn-errortype shape_mismatch — expected 'AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/', got 'ChannelNotFound'.
  $.Message missing — expected a denial naming the principal, 'cloudtrail-data:PutAuditEvents' and the resource ARN (recorded example: 'User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: cloudtrail-data:PutAuditEvents on resource: arn:aws:cloudtrail:us-west-2:111122223333:channel/11111111-2222-3333-4444-555555555555 because no resource-based policy allows the cloudtrail-data:PutAuditEvents action'), got None. The account id, user ARN and channel uuid are environment-specific examples; the requirements are the 403 status, the AccessDeniedException code, and a Message built from the request that names the action and resource without disclosing existence.

Model evidence used: Smithy PutAuditEvents (http POST /PutAuditEvents, code 200; errors ChannelInsufficientPermission, ChannelNotFound, ChannelUnsupportedSchema, DuplicatedAuditEventId, InvalidChannelARN, UnsupportedOperationException — no AccessDeniedException); PutAuditEventsRequest$channelArn -> ChannelArn, required, httpQuery, @pattern ^arn:.*$, arnReference; com.amazonaws.cloudtraildata#InvalidChannelARN httpError 400, doc 'The specified channel ARN is not a valid channel ARN.'; com.amazonaws.cloudtraildata#ChannelNotFound httpError 400.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it, then replay the wire corpus (trace set simparity-wire-aws-v1). All three steps below are in that corpus.
2. Cause 1 — replay fixture cloudtrail-data-invalid-channel-arn-error, step task#1: POST /PutAuditEvents?channelArn=arn:aws:cloudtrail:us-west-2:<account>:not-a-channel-resource with a one-event auditEvents body. Expect 400, x-amzn-errortype InvalidChannelARN..., message exactly 'The specified channel ARN is not a valid channel ARN.'; observe the same sentence with ': <channelArn>' appended. The other 8 steps of the cluster (cloudtraildata-audit-event-batch-size-ceiling#task#1/#2/#3/#5, cloudtraildata-audit-event-constraint-rejections#task#1/#3, cloudtraildata-audit-events-batch-exceeds-max-size-rejected#task#1, cloudtraildata-channel-resolution-rejections#task#3) exercise the same branch and must pass once it is fixed.
3. Cause 2 — replay fixture cloudtraildata-channel-resolution-rejections, step task#2: same call with channelArn='sim2real-probe-not-an-arn' (use a client with parameter validation disabled so the request reaches the emulator). Expect 400 with x-amzn-errortype 'InvalidChannelARN:http://internal.amazon.com/coral/com.amazonaws.cloudtraildata/' and the fixed message; observe x-amzn-errortype 'ValidationException' with a constraint-violation message.
4. Cause 3 — replay fixture cloudtraildata-channel-resolution-rejections, step task#4: same call with channelArn='arn:aws:cloudtrail:us-west-2:111122223333:channel/<uuid>' (an account other than the emulator's). Expect HTTP 403 with x-amzn-errortype 'AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/' and a Message naming the action cloudtrail-data:PutAuditEvents and the resource ARN; observe HTTP 400, ChannelNotFound and no Message. Then repeat with the emulator's own account id to confirm the same-account control still returns ChannelNotFound/400.
5. Re-replay all three bindings (cloudtrail-data-invalid-channel-arn-error#task#1, cloudtraildata-channel-resolution-rejections#task#2, cloudtraildata-channel-resolution-rejections#task#4) before closing — each is a separate cause.
