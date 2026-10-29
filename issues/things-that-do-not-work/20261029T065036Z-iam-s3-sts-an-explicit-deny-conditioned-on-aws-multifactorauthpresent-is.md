Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__s3__sts.deny-unless-mfa-present-blocks-plain-session-but-allows-mfa-session
Issue-Key: iam__s3__sts.deny-unless-mfa-present-blocks-plain-session-but-allows-mfa-session|_poll#34|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 6c829b3943454ada5df5dfe2630a7a4c6694ce1c

# An explicit Deny conditioned on aws:MultiFactorAuthPresent is not enforced on S3: PutObject by a no-MFA session is served instead of refused AccessDenied

> _Note: this issue was identified against the code base at commit `6c829b3943454ada5df5dfe2630a7a4c6694ce1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
ONE root cause, grounded in 1 failing behavioral check (service token = the composite leading token of the check id, 'iam__s3__sts').

Already-open issue describing the same root cause whose replay key the report did NOT match for this run, so this check is marked neither OWNED nor ALREADY FILED: 20261024T065715Z-iam-s3-sts-s3-requests-under-a-scoped-session-are-never.md (it carries this check id in Issue-Trace-Ids, but its Issue-Key fingerprint is `_poll#34|Timeout|time.sleep(interval_s)` from a harness 60 s cap, whereas this run fails with an explicit TimeoutError raised by the check's own _poll at line 86). Related, also not covering this: 20261022T220446Z-iam-no-iam-authorization-is-evaluated-calls-a-session.md. They should be verified together; this issue exists so the check id below is re-replayed.

Cause 1 of 1: the emulator's S3 request-authorization step does not evaluate the caller's IAM identity policy conditions, so an explicit Deny gated on the aws:MultiFactorAuthPresent context key has no effect. An IAM user whose only inline policy is {Allow s3:PutObject/s3:GetObject on arn:aws:s3:::<bucket>/*} + {Allow the MFA bootstrap actions} + {Deny NotAction <bootstrap actions> on * when BoolIfExists aws:MultiFactorAuthPresent is "false"} can still PutObject with its long-term (non-MFA) access key: the call is served and the check's poll for an AccessDenied refusal never converges. The check establishes a positive control first (iam:GetUser, exempted from the Deny, succeeds under the same key), so the key is live and the policy is in effect before the negative assertion is made. Failing check: iam__s3__sts.deny-unless-mfa-present-blocks-plain-session-but-allows-mfa-session.

AWS: IAM policy evaluation applies to every S3 request, and an explicit Deny always wins. With no MFA in the signing session, aws:MultiFactorAuthPresent is absent, BoolIfExists treats it as false, the Deny matches, and S3 refuses the write with HTTP 403 and error code AccessDenied. The message AWS words for an identity-based explicit deny is: `User: <caller-arn> is not authorized to perform: <action> on resource: "<resource-arn>" with an explicit deny in an identity-based policy` — `User: `, ` is not authorized to perform: `, ` on resource: `, the surrounding double quotes and ` with an explicit deny in an identity-based policy` are FIXED text; <caller-arn> is the signing identity (here the IAM user ARN from GetUser), <action> is the request's action (s3:PutObject) and <resource-arn> is the object ARN built from the request (arn:aws:s3:::<bucket>/<key>) — never one recording's bucket/key/account literal. The refused write must also leave no object: HeadObject by the bucket owner answers 404. Recorded behavior only for the exact message framing (no Smithy error member carries it: S3 PutObject models no AccessDenied shape, the code is carried in the S3 REST XML error body as <Code>AccessDenied</Code> with HTTP 403). Conversely, an MFA-bearing session obtained with sts:GetSessionToken(SerialNumber, TokenCode) — same IAM user identity, ASIA-class temporary key — is permitted the identical PutObject, and the plain key stays denied afterwards.

Emulator: the no-MFA PutObject is accepted. The check polls `plain_s3.put_object(...)` for an AccessDenied refusal and never observes one, exhausting its budget: `TimeoutError: s3:PutObject by the plain (no-MFA) session became refused with AccessDenied: the run budget ended first; not graded`. No error code and no message are produced at all where AWS produces 403 AccessDenied with the explicit-deny message above, and the object the Deny should have prevented is written.

Why: 'deny unless MFA is present' is the standard AWS hardening pattern, and this is a fail-open divergence. A client relying on AWS's behavior is told its bucket is protected by an MFA requirement when the emulator serves exactly the write the policy forbids; a developer validating a least-privilege policy against the emulator ships a policy that was never enforced, and any negative test of an MFA-gated action passes vacuously.

Fix: the S3 request-authorization step (the shared IAM policy-evaluation component the S3 operation handlers call before mutating state) — evaluate the caller's identity policy and session policy including explicit Deny with Condition operators (Bool/BoolIfExists) over the request context, and populate the aws:MultiFactorAuthPresent context key from the signing credentials (false/absent for long-term keys, true for a GetSessionToken session minted with SerialNumber + TokenCode). On denial the handler must emit HTTP 403 with code AccessDenied and the explicit-deny message templated above, and must not create the object.

Traces: iam__s3__sts.deny-unless-mfa-present-blocks-plain-session-but-allows-mfa-session (fails at the negative half, line 7180 of the replayed source, before the virtual-MFA-device leg is reached — so it is not blocked by the separately filed iam CreateVirtualMFADevice/EnableMFADevice stubs).

## Evidence
Replay report, service iam__s3__sts:

  [error] iam__s3__sts.deny-unless-mfa-present-blocks-plain-session-but-allows-mfa-session
  File "check_iam__s3__sts_deny_unless_mfa_present_blocks_plain_session_but_allows_mfa_session.py", line 106, in _poll_denied
    return _poll(lambda: _denial(fn), lambda r: r != _SUCCEEDED and r[0] == expected, ...)
  File "...", line 86, in _poll
    raise TimeoutError(f"{description}: the run budget ended first; not graded")
  TimeoutError: s3:PutObject by the plain (no-MFA) session became refused with AccessDenied: the run budget ended first; not graded

Setup the check performs before that assertion:
- create bucket <bucket> (LocationConstraint = region when not us-east-1), tag it;
- create IAM user <user>, put the inline policy {Sid BootstrapMFA: Allow <MFA bootstrap actions> on *}, {Sid S3Write: Allow [s3:PutObject, s3:GetObject] on arn:aws:s3:::<bucket>/*}, {Sid DenyUnlessMFA: Deny NotAction <MFA bootstrap actions> on * Condition BoolIfExists aws:MultiFactorAuthPresent="false"};
- create a long-term access key for that user and wait for it to be active;
- POSITIVE CONTROL: iam:GetUser under that key succeeds (asserted), proving the key is live and the policy effective.

Expected (AWS), from the check's own assertions:
- plain_s3.put_object(Bucket=<bucket>, Key="deny-unless-mfa/plain-attempt.bin") -> ClientError, Error.Code == "AccessDenied" (HTTP 403), Error.Message == `User: <user-arn> is not authorized to perform: s3:PutObject on resource: "arn:aws:s3:::<bucket>/deny-unless-mfa/plain-attempt.bin" with an explicit deny in an identity-based policy`;
- HeadObject by the owner for that key -> 404 (the denied write left nothing);
- after an MFA-bearing GetSessionToken (ASIA-class key, GetCallerIdentity Arn still == <user-arn>) the identical PutObject is permitted and round-trips its bytes, while the plain key's denial repeats identically.

Actual (emulator): the plain, no-MFA PutObject is never refused — repeated attempts across the polling window produce no AccessDenied (no error code, no message), so the explicit Deny is not evaluated and the object is written by a principal AWS forbids. The remaining assertions (owner HeadObject 404, MFA-session allow, denial stability) are never reached.

## Steps to Reproduce
1. Boot the emulator; confirm IAM, S3 and STS answer.
2. Create a bucket <bucket>.
3. Create an IAM user <user> and attach the inline policy: [{Sid BootstrapMFA, Effect Allow, Action <the MFA bootstrap action list: iam:GetUser, iam:CreateVirtualMFADevice, iam:EnableMFADevice, iam:ListMFADevices, sts:GetSessionToken, ...>, Resource *}, {Sid S3Write, Effect Allow, Action [s3:PutObject, s3:GetObject], Resource arn:aws:s3:::<bucket>/*}, {Sid DenyUnlessMFA, Effect Deny, NotAction <the same bootstrap list>, Resource *, Condition {BoolIfExists: {aws:MultiFactorAuthPresent: "false"}}}].
4. Create a long-term access key for <user>; with it, call iam.get_user(UserName=<user>) and confirm it succeeds (positive control: the key is live, the policy is in effect).
5. With the SAME long-term key, call s3.put_object(Bucket=<bucket>, Key='deny-unless-mfa/plain-attempt.bin', Body=b'...'). AWS answers HTTP 403 AccessDenied with the message `User: <user-arn> is not authorized to perform: s3:PutObject on resource: "arn:aws:s3:::<bucket>/deny-unless-mfa/plain-attempt.bin" with an explicit deny in an identity-based policy`. The emulator serves the request.
6. Confirm the side effect: head_object for that key as the bucket owner must be 404 on AWS; on the emulator the object exists.
7. After the fix, verify the positive half too: enrol a virtual MFA device, obtain credentials via sts.get_session_token(SerialNumber=<mfa-arn>, TokenCode=<live TOTP>), and confirm the identical PutObject succeeds under those credentials (same user ARN from GetCallerIdentity) while the long-term key is still refused with the identical code and message.
8. Re-replay iam__s3__sts.deny-unless-mfa-present-blocks-plain-session-but-allows-mfa-session.
