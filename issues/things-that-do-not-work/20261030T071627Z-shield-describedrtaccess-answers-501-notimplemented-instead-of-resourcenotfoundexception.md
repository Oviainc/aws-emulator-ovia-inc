Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: shield.disassociate-drt-role-requires-subscription
Issue-Shape-Id: com.amazonaws.shield#AWSShield_20160616
Issue-Key: shield.disassociate-drt-role-requires-subscription|run#8|AssertionError|assert access_code == "ResourceNotFoundException", (
Issue-Commit: e635fc8db7925c597029a243d97ecda6419e6d1a

# shield DescribeDRTAccess answers 501 NotImplemented instead of ResourceNotFoundException on an unsubscribed account (second blocked check)

> _Note: this issue was identified against the code base at commit `e635fc8db7925c597029a243d97ecda6419e6d1a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDRTAccess

## Description
Cause 1 of 1: `DescribeDRTAccess` is still an unimplemented scaffold stub, so every call is refused with HTTP 501 `NotImplemented` instead of the modeled Shield behavior. This is the SAME root cause as the already-open issue `20261029T200731Z-shield-describedrtaccess-stub.md`, but that issue's `Issue-Trace-Ids` names only `shield.describe-drt-access-absent-without-association`; the failing check `shield.disassociate-drt-role-requires-subscription` also fails on this refusal and is NOT recorded anywhere, so a fixer closing that issue would never re-replay it. This issue carries that second replay key. Failing check: shield.disassociate-drt-role-requires-subscription.

AWS: Smithy model `com.amazonaws.shield#DescribeDRTAccess` takes an empty input and returns `DescribeDRTAccessResponse` with optional `RoleArn` (len 1..2048, pattern `^arn:aws:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`) and `LogBucketList` (len 0..10); its modeled client error is `ResourceNotFoundException` (HTTP 400) — "Exception indicating the specified resource does not exist." Recorded behavior against real AWS on an account with no Shield Advanced subscription: the operation is refused with HTTP 400, error code `ResourceNotFoundException`, message `The subscription does not exist.` — the whole message is fixed text for this error class (it names the missing subscription, not a missing DRT role or log bucket) and carries no values from the request or state. This is the same unsubscribed-account rejection the emulator ALREADY produces correctly for `DisassociateDRTRole` in the very same check, so the two reads must agree.

Emulator: the call is refused with HTTP 501, error code `NotImplemented`, message `operation DescribeDRTAccess is scaffolded but not implemented in this emulator` (services/service-com-amazonaws-shield-awsshield-20160616/src/ops/describe_drt_access.rs:155 returns `Err(crate::scaffolded::Scaffolded::not_implemented("DescribeDRTAccess"))`, and the file still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker at line 4). Nothing else in the check diverges: `GetSubscriptionState` correctly returns `INACTIVE` and `DisassociateDRTRole` correctly returns `ResourceNotFoundException` / `The subscription does not exist.`

Why: a client that reads DRT/SRT access configuration cannot distinguish "no subscription" from "server does not support this API". `NotImplemented`/501 is not in the operation's modeled error set, so SDK error handling, retry classification (501 is not a client error) and any code branching on `ResourceNotFoundException` to mean "Shield Advanced is not enabled" all take the wrong path. It also makes the emulator internally inconsistent: `DisassociateDRTRole` reports the absent subscription while `DescribeDRTAccess` reports a server-side gap for the identical account state.

Fix: implement the handler in services/service-com-amazonaws-shield-awsshield-20160616/src/ops/describe_drt_access.rs — read the service's subscription/DRT-access state via `crate::state::service_state` and, when no subscription exists, return `Err(DescribeDRTAccessErrorModeled::ResourceNotFoundException(..).into())` with the fixed message `The subscription does not exist.`; return `RoleArn`/`LogBucketList` from state when a subscription and DRT association do exist. Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line only once the handler is real. Verify by replaying BOTH `shield.describe-drt-access-absent-without-association` (covered by the existing stub issue) and `shield.disassociate-drt-role-requires-subscription` (covered here).

Traces: shield.disassociate-drt-role-requires-subscription — assertion at check_shield_disassociate_drt_role_requires_subscription.py:36, `expected DescribeDRTAccess to also report no DRT configuration, got 'NotImplemented'`.

## Evidence
Failing check shield.disassociate-drt-role-requires-subscription (verdict: mismatch):

  File "check_shield_disassociate_drt_role_requires_subscription.py", line 36, in run
    assert access_code == "ResourceNotFoundException", (
  AssertionError: expected DescribeDRTAccess to also report no DRT configuration, got 'NotImplemented'

Expected (recorded AWS, unsubscribed account): HTTP 400, Error.Code `ResourceNotFoundException`, Error.Message `The subscription does not exist.` (fixed text; no per-request or per-state substitutions).
Actual (emulator): HTTP 501, Error.Code `NotImplemented`, Error.Message `operation DescribeDRTAccess is scaffolded but not implemented in this emulator`.

Source evidence: services/service-com-amazonaws-shield-awsshield-20160616/src/ops/describe_drt_access.rs line 4 carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 155 returns `Err(crate::scaffolded::Scaffolded::not_implemented("DescribeDRTAccess"))`.

Smithy: `DescribeDRTAccess` input is empty; output members `RoleArn` (len 1..2048, pattern `^arn:aws:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`) and `LogBucketList` (len 0..10); modeled errors `ResourceNotFoundException` (400, client) and `InternalErrorException` (500, server). `NotImplemented` is not modeled for this operation.

Controls inside the same check that PASSED (so the account premise and the Shield API path are healthy): `GetSubscriptionState` -> `INACTIVE`; `DisassociateDRTRole` -> `ResourceNotFoundException` / `The subscription does not exist.`

Related already-open issue: 20261029T200731Z-shield-describedrtaccess-stub.md records the same stub but lists only the trace id `shield.describe-drt-access-absent-without-association`, so this second blocked check is not covered by its replay key.

## Steps to Reproduce
1. Boot the shield emulator (service-com-amazonaws-shield-awsshield-20160616) with an account that has NO Shield Advanced subscription.
2. Call GetSubscriptionState and observe SubscriptionState == INACTIVE (passes today).
3. Call DisassociateDRTRole and observe HTTP 400 ResourceNotFoundException with message 'The subscription does not exist.' (passes today).
4. Call DescribeDRTAccess with an empty request.
5. Observe HTTP 501 with Error.Code 'NotImplemented' and message 'operation DescribeDRTAccess is scaffolded but not implemented in this emulator', instead of HTTP 400 ResourceNotFoundException with message 'The subscription does not exist.'
6. Replay check shield.disassociate-drt-role-requires-subscription to confirm.
