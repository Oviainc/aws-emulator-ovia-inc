Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.change-password-requires-valid-old-password
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.change-password-requires-valid-old-password|run#18|AssertionError|assert message == (
Issue-Commit: 52a79640a1ceceb858c6fc834fcbcb6a886924f9

# IAM ChangePassword: a wrong OldPassword is refused with the generic iam:ChangePassword authorization text instead of AWS's "Either the new password does not conform to the account password policy or the old password was incorrect."

> _Note: this issue was identified against the code base at commit `52a79640a1ceceb858c6fc834fcbcb6a886924f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ChangePassword

## Description
REOPENS 20261025T050617Z-iam-changepassword-stub.md: its fix left the call refused.

Cause 1 of 1: the message IAM returns when ChangePassword is called with an OldPassword that is not the password in force.

AWS: ChangePassword changes the console password of the IAM user who signed the request, and OldPassword is modeled as "The IAM user's current password" (Smithy: ChangePassword.OldPassword / NewPassword, target passwordType, both @required, @length 1..128, @sensitive; modeled errors are only EntityTemporarilyUnmodifiableException 409, InvalidUserTypeException 400, LimitExceededException 409, NoSuchEntityException 404, PasswordPolicyViolationException 400, ServiceFailureException 500 — the model declares NO error for "the supplied old password is wrong"). Recorded behavior only for that case: AWS refuses it on the authorization channel with HTTP 403, error code AccessDenied, and the deliberately non-disclosing message whose text is entirely fixed — "Either the new password does not conform to the account password policy or the old password was incorrect." There are no variable parts in this message: AWS does NOT name the principal, the action, the resource, or the reason, precisely because the one message covers both the policy-violation and the wrong-old-password case and must not reveal which condition failed. (The check that pins this also proves the caller is otherwise authorized for iam:ChangePassword: IAMUserChangePassword is attached and the immediately preceding ChangePassword with the correct old password succeeds.)

Emulator: the same call is refused with HTTP 403 AccessDenied — status and code match — but the body carries the service's single identity-based denial wording instead: "User: arn:aws:iam::<account>:user/<user-name> is not authorized to perform: iam:ChangePassword on resource: user <user-name> because the old password supplied is not the user's current password". Its fixed text is the implicit-deny template "User: ... is not authorized to perform: ... on resource: ... because ..." plus the reason clause "because the old password supplied is not the user's current password"; the caller ARN, the account id and the user name are values read from the emulator's own state (the recorded run shows arn:aws:iam::000000000000:user/sim2real-cp-oldpw-b654d3b2577f, which is a per-run value and not part of any template). AWS's wording is not produced at all, so the check's exact-match assertion on the message fails.

Why: a client cannot distinguish "your old password was wrong / your new password violates the account policy" from "this principal is not permitted to call iam:ChangePassword". Console-style and self-service password-rotation flows branch on exactly that message (retry the prompt vs. surface a permissions error to an administrator), and credential-hygiene tooling that asserts the documented string treats the emulator's answer as an authorization failure. The emulator's wording also discloses more than AWS does — it states which of the two conditions failed, which is the information AWS's single message exists to withhold.

Fix: the ChangePassword handler's old-password check. It currently routes this refusal through the crate's shared identity-denial formatter (crate::state::authorization::not_authorized with the OLD_PASSWORD_NOT_IN_FORCE reason clause); this refusal is not an identity-based denial and must carry its own fixed AccessDenied text, emitted with the existing 403/AccessDenied status and code and with no principal, resource or reason interpolated. The same text is required for a NewPassword that violates an account password policy, so it belongs with the operation's refusal path, not with the authorization formatter.

Traces: iam.change-password-requires-valid-old-password (mismatch). Handler and the exact wording under test: services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/change_password.rs lines 86-92 (OLD_PASSWORD_NOT_IN_FORCE) and 304-320 (the refusal). The prior fix 20261025T050617Z-iam-changepassword-stub.md recorded this exact message mismatch as a residual divergence it could not resolve; the expected string is stated in the check source as the literal above.

## Evidence
Check iam.change-password-requires-valid-old-password, assertion at check_iam_change_password_requires_valid_old_password.py:60.

Sequence: CreateUser -> AttachUserPolicy(arn:aws:iam::aws:policy/IAMUserChangePassword) -> CreateLoginProfile(password p0) -> CreateAccessKey -> ChangePassword(OldPassword=p0, NewPassword=p1) succeeds (positive control: this caller IS authorized for iam:ChangePassword) -> ChangePassword(OldPassword=p0 (now stale), NewPassword=<bogus>).

Expected (AWS): HTTP 403, Error.Code = "AccessDenied", Error.Message = "Either the new password does not conform to the account password policy or the old password was incorrect." (entire message is fixed text).

Actual (emulator): HTTP 403, Error.Code = "AccessDenied" (matches), Error.Message = "User: arn:aws:iam::000000000000:user/sim2real-cp-oldpw-b654d3b2577f is not authorized to perform: iam:ChangePassword on resource: user sim2real-cp-oldpw-b654d3b2577f because the old password supplied is not the user's current password" — the generic implicit-deny template, with the ARN/account/user-name filled from emulator state (those values are per-run, the template is the defect).

AssertionError reported by the replay: wrong-old-password ChangePassword message: "User: arn:aws:iam::000000000000:user/sim2real-cp-oldpw-b654d3b2577f is not authorized to perform: iam:ChangePassword on resource: user sim2real-cp-oldpw-b654d3b2577f because the old password supplied is not the user's current password".

State effect is correct: the rejected call mutates nothing and a subsequent ChangePassword with the real current password succeeds — only the message diverges.

## Steps to Reproduce
1. Boot the iam emulator.
2. CreateUser <name>; AttachUserPolicy(UserName=<name>, PolicyArn=arn:aws:iam::aws:policy/IAMUserChangePassword).
3. CreateLoginProfile(UserName=<name>, Password=P0, PasswordResetRequired=false); CreateAccessKey(UserName=<name>).
4. Signing with that access key, call ChangePassword(OldPassword=P0, NewPassword=P1) — it succeeds, proving the principal is authorized for iam:ChangePassword.
5. Signing with the same key, call ChangePassword(OldPassword=P0, NewPassword=P2). P0 is no longer the password in force.
6. Observe HTTP 403 AccessDenied whose message is the generic "User: ... is not authorized to perform: iam:ChangePassword on resource: user ... because the old password supplied is not the user's current password" instead of AWS's fixed "Either the new password does not conform to the account password policy or the old password was incorrect."
7. Replay iam.change-password-requires-valid-old-password to confirm.
