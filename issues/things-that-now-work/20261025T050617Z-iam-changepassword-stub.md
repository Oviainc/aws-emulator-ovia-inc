Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.change-password-requires-valid-old-password iam.change-password-updates-login-profile
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|ChangePassword|NotImplemented
Issue-Commit: 4244fec3063c58aa4a2476d7619962eecf74c002

# Implement iam ChangePassword: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `4244fec3063c58aa4a2476d7619962eecf74c002`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ChangePassword

## Description
`ChangePassword` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/change_password.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `ChangePassword` call, so each one unblocks only when `ChangePassword` works: `iam.change-password-requires-valid-old-password`, `iam.change-password-updates-login-profile`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/change_password.rs`. Replaying the corpus, the emulator refused the `ChangePassword` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ChangePassword operation: operation ChangePassword is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the iam emulator
Call ChangePassword and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

## Resolution and residual divergence
`ChangePassword` is implemented in
`services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/change_password.rs`
and the scaffold marker is gone: it resolves the IAM user the signing access key
belongs to, checks the presented `OldPassword` against the verifier the account holds
for the password in force (`state::PasswordVerifier`, set by `CreateLoginProfile`,
`UpdateLoginProfile` and this operation alike), replaces it, clears the password-reset
demand and leaves `CreateDate` where the create put it. A request that does not present
the password in force is refused HTTP 403 `AccessDenied` — the recorded status and error
code — carrying the ONE fixed sentence AWS words this refusal with, which covers both a
stale `OldPassword` and a `NewPassword` an account password policy would not admit and
interpolates nothing of the request or of the account. A caller whose credential belongs
to no IAM user is refused `InvalidUserTypeException`, and one holding no console password
at all `NoSuchEntityException`. Covered by `tests/all/change_password.rs`, whose rows span
the password class and both published `passwordType` bounds, the name alphabet, the reset
demand, and which write left the password in force — the create, the administrative
update, an earlier change, or none because `DeleteLoginProfile` removed it.

Residual: none. Both recorded units — `iam.change-password-updates-login-profile` and
`iam.change-password-requires-valid-old-password`, whose check compares the refusal's
message text — replay as passed against the wording above, so the operation is served
from persisted state on both the accepting and the refusing path. The refusal is worded
by this operation's own fixed sentence rather than by the service's identity-denial
formatter (`state::authorization::not_authorized`), because the refusal is about the
credential material the request carried and not about what its caller is permitted to do,
and it interpolates nothing, so it discloses neither which of the two conditions failed
nor anything the account holds.
