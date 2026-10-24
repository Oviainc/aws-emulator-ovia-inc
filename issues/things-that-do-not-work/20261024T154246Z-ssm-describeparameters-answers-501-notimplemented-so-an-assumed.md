Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__ssm__sts.assumed-role-sees-the-same-account-state
Issue-Key: iam__ssm__sts|DescribeParameters|NotImplemented
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# SSM DescribeParameters answers 501 NotImplemented, so an assumed-role session cannot read back parameter state written by the base identity in the same account

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeParameters

## Description
One root cause (1 of 1).

AWS: DescribeParameters is a normal SSM Parameter Store control operation; it returns metadata for the parameters that exist in the caller's account and Region. A role assumed inside an account sees the same account-level state as the base identity — assuming a role changes the principal, not the world — so a parameter written by the base identity is described by the assumed-role session too.
Emulator: the operation is a scaffold stub. The call raises `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeParameters operation: operation DescribeParameters is scaffolded but not implemented in this emulator`, which aborts the check before either half of its claim (the assumed caller reports the SAME account id; the assumed caller reads state the base identity wrote) can be observed.
Why: the state-side half of the same-account identity contract is unverifiable, so the specific failure this check exists to catch — a target that keys state on the credential rather than the account and hands an assumed credential a fresh empty world — cannot be ruled out. Any client that enumerates parameters, or that writes with one identity and reads with another in the same account, gets a 501 instead of data.
Fix: implement the DescribeParameters handler in the SSM service against the same account/Region-scoped parameter state PutParameter and GetParameter already use, so it is independent of which principal in the account signs the request.
Traces: iam__ssm__sts.assumed-role-sees-the-same-account-state. Note: the open issue 20261024T065715Z-ssm-sts-ssm-describeparameters-is-an-unimplemented-scaffold-stub-so.md carries the same stub for a different check (ssm.parameter-last-modified-user-is-the-live-sts-session-identity); the suite did not mark this check as owned by it, so its replay key is recorded here and the two should be closed together.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1. Check iam__ssm__sts.assumed-role-sees-the-same-account-state, verdict error:

`botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeParameters operation: operation DescribeParameters is scaffolded but not implemented in this emulator`

The check creates a role, assumes it, asserts the assumed caller reports the same account id, and then reads back a parameter the base identity wrote; the NotImplemented refusal is raised on the read-back leg, so the state half of the claim is never exercised. Expected: a successful DescribeParameters response listing the parameter the base identity created.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-replay iam__ssm__sts.assumed-role-sees-the-same-account-state.
3. Minimal repro: ssm.put_parameter(Name='/sim2real/probe', Value='v', Type='String'), then iam.create_role + sts.assume_role, and with the assumed-role credentials call ssm.describe_parameters().
4. Observed: HTTP 501 with code NotImplemented. Expected: HTTP 200 with a Parameters list containing '/sim2real/probe'.
