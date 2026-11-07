Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail-data.put-audit-events-requires-the-cloudtrail-data-action-not-cloudtrail-wildcard
Issue-Shape-Id: com.amazonaws.cloudtraildata#CloudTrailDataService
Issue-Key: cloudtrail-data.put-audit-events-requires-the-cloudtrail-data-action-not-cloudtrail-wildcard|_wait_for_code#33|AssertionError|raise AssertionError(
Issue-Commit: 47509ee38907103620e4b84df19d39bf76901d44

# cloudtrail-data PutAuditEvents applies no IAM authorization: a principal holding only cloudtrail:* is answered ChannelNotFound/400 instead of AccessDeniedException/403

> _Note: this issue was identified against the code base at commit `47509ee38907103620e4b84df19d39bf76901d44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAuditEvents

## Description
One root cause. Related open issue 20261024T045955Z-cloudtrail-data-putauditevents-is-never-authorized-against-the.md describes the same gap but carries a different replay key (it was keyed on the check timing out inside _wait_for_code); this replay reaches the assertion itself and the suite did not mark the check as owned, so it is filed here with the current failing key. Also distinct from 20261022T215433Z-cloudtrail-data-putauditevents-rejection-fidelity-channel-arn-syntax.md (Cause 3), which is about the ARN's ACCOUNT segment / resource ownership and needs no authorizer at all; this cause holds the ARN fixed inside the caller's own account and varies only the caller's inline policy.

Cause 1 of 1: PutAuditEvents performs no authorization decision for the action cloudtrail-data:PutAuditEvents.

AWS: PutAuditEvents is authorized under the `cloudtrail-data:` IAM action prefix, which is disjoint from `cloudtrail:`. A principal that does not hold `cloudtrail-data:PutAuditEvents` is refused by the authorization layer in front of the service with error code `AccessDeniedException` and HTTP `403`, and that refusal happens BEFORE channel resolution, so the response reveals nothing about whether the channel exists and carries neither `successful` nor `failed`. A principal that DOES hold the action passes authorization and is then refused by resolution: `ChannelNotFound`, HTTP `400`, message `The channel <channelArn> could not be found.` (fixed text `The channel ` / ` could not be found.`, variable part = the channelArn from the request). Smithy (services/service-com-amazonaws-cloudtraildata-cloudtraildataservice/smithy-model/com-amazonaws-cloudtraildata-cloudtraildataservice.json): PutAuditEvents errors = ChannelInsufficientPermission, ChannelNotFound, ChannelUnsupportedSchema, DuplicatedAuditEventId, InvalidChannelARN, UnsupportedOperationException, all httpError 400 (defaulted); NO AccessDeniedException is modelled, which confirms the 403 is produced by the authorization layer ahead of the operation rather than by the operation. Output members `successful` and `failed` are @required on SUCCESS only. The AccessDeniedException/403 pairing and its message shape are recorded behavior only: the message names the refused action under the cloudtrail-data prefix and the refused principal, i.e. `User: <caller-principal-arn> is not authorized to perform: cloudtrail-data:PutAuditEvents on resource: <channelArn> because no identity-based policy allows the cloudtrail-data:PutAuditEvents action` — fixed text is `User: `, ` is not authorized to perform: `, ` on resource: `, ` because no identity-based policy allows the `, ` action`; the principal ARN, the action name and the resource ARN are taken from the request and the caller's credentials, never from one recorded run.

Emulator: the verdict does not depend on the caller at all. The check creates a role, assumes it, and issues the byte-identical PutAuditEvents call (same in-account channel ARN arn:aws:cloudtrail:<region>:<caller-account>:channel/<uuid>, same single-event batch) under successive inline policies. With only `cloudtrail:*` on `*` granted (arm 2) the emulator keeps answering the resolution-layer verdict ChannelNotFound / HTTP 400 with message `The channel arn:aws:cloudtrail:us-west-2:000000000000:channel/<uuid> could not be found.`, and the expected AccessDeniedException never arrives inside the check's 90 s eventual-consistency budget. Arms 0 and 1, which only require ChannelNotFound/400, pass — so the request itself is acceptable and the missing 403 is attributable to the policy path, not to a malformed ARN or batch. Arm 4 (the right action name under the wrong `cloudtrail:` prefix) is unreachable behind arm 2 and is covered by the same cause.

Why: a client cannot distinguish 'you may not call this' from 'the channel is not there'. It receives a 400 request-class refusal where AWS gives a 403 authorization-class refusal, so credential-vs-policy triage, error handling and retry classification are all wrong. Least-privilege policies cannot be validated against the emulator: a role holding only `cloudtrail:*`, or `cloudtrail:PutAuditEvents` under the wrong service prefix, is silently treated as entitled. And because resolution runs for unentitled callers, the emulator remains an existence oracle for principals AWS would have refused before resolution.

Fix: an authorization gate on the PutAuditEvents request path (the operation's handler, services/service-com-amazonaws-cloudtraildata-cloudtraildataservice/src/ops/put_audit_events.rs) that evaluates the signing principal's effective identity-based policy for the action `cloudtrail-data:PutAuditEvents` on the parsed channel ARN and, on deny, serializes AccessDeniedException with HTTP 403 and no `successful`/`failed` member — running BEFORE ARN parsing/resolution and before any owner-account check. The action must be matched on the full `cloudtrail-data:` prefix, not on the operation name alone, so `cloudtrail:*` and `cloudtrail:PutAuditEvents` do not satisfy it.

Traces: cloudtrail-data.put-audit-events-requires-the-cloudtrail-data-action-not-cloudtrail-wildcard.

## Evidence
Behavioral replay against the emulator, service cloudtrail-data, operation PutAuditEvents (POST /PutAuditEvents?channelArn=...).

Failing check [mismatch] cloudtrail-data.put-audit-events-requires-the-cloudtrail-data-action-not-cloudtrail-wildcard:
  AssertionError: arm 2 (only cloudtrail:* granted): PutAuditEvents never answered 'AccessDeniedException' within 90.0s of the policy change; the last answer was ('ChannelNotFound', 'The channel arn:aws:cloudtrail:us-west-2:000000000000:channel/afc69f92-517e-4928-b9be-d6d452248d81 could not be found.', 400).

Expected (AWS): HTTP 403, Error.Code = AccessDeniedException, Error.Message = 'User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: cloudtrail-data:PutAuditEvents on resource: <channelArn> because no identity-based policy allows the cloudtrail-data:PutAuditEvents action', response carrying neither `successful` nor `failed`.
Actual (emulator): HTTP 400, Error.Code = ChannelNotFound, Error.Message = 'The channel <channelArn> could not be found.' — the same answer the fully entitled runner principal gets in arm 0, i.e. unchanged by the policy swap.

Controls inside the same check that DID pass: arm 0 (fully entitled runner principal) and arm 1 (session granted cloudtrail-data:PutAuditEvents) both got ChannelNotFound/400 with the channel-naming message, proving the ARN and the batch are themselves acceptable; sts:GetCallerIdentity from the limited session returns the assumed-role session ARN, so the credential is live.

Smithy: PutAuditEvents input channelArn @required, httpQuery 'channelArn', @pattern ^arn:.*$; auditEvents @required @length(1,100); modelled errors are the six 400-class ones listed above with no AccessDeniedException, so the denial belongs to the authorization layer in front of the operation.

## Steps to Reproduce
Boot the emulator. In the emulated account/region:
1. iam.create_role with a trust policy allowing arn:aws:iam::<account>:root to sts:AssumeRole.
2. iam.put_role_policy granting {Allow sts:GetCallerIdentity *} plus {Allow cloudtrail-data:PutAuditEvents *}.
3. sts.assume_role on that role; build a cloudtrail-data client from the returned session credentials.
4. Pick channel_arn = arn:aws:cloudtrail:<region>:<account>:channel/<random-uuid> (create no channel) and batch = [{id: <uuid>, eventData: '{"version":"1.0","userIdentity":{"type":"CustomIdentity","principalId":"probe"},"eventSource":"example","eventName":"Probe","eventTime":"2026-01-01T00:00:00Z","UID":"<uuid>"}'}].
5. put_audit_events(channelArn=channel_arn, auditEvents=batch) with the session client -> expect ChannelNotFound / 400 with message 'The channel <channel_arn> could not be found.' (positive control; already holds).
6. iam.put_role_policy replacing the inline policy with {Allow sts:GetCallerIdentity *} plus {Allow cloudtrail:* *} (no cloudtrail-data action). Re-issue the IDENTICAL call from step 5, polling for up to 90 s.
   Expect AccessDeniedException / HTTP 403, a message naming cloudtrail-data:PutAuditEvents and the assumed-role session ARN, and no `successful`/`failed` member.
   Observe instead the unchanged ChannelNotFound / HTTP 400.
7. Repeat step 6 with {Allow cloudtrail:PutAuditEvents *} (right action name, wrong service prefix) and expect the same AccessDeniedException / 403.
8. Restore {Allow cloudtrail-data:PutAuditEvents *} and confirm the identical call returns to ChannelNotFound / 400.
9. Confirm sts.get_caller_identity from the limited session still returns the session ARN, so the 403 in steps 6-7 is attributable to the action and not to a dead credential.
