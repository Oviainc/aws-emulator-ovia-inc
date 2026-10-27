Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda__sts.iam-propagation-is-a-retry-contract-not-a-sleep
Issue-Key: iam__lambda__sts.iam-propagation-is-a-retry-contract-not-a-sleep|_error_code#36|AssertionError|raise AssertionError(
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# STS AssumeRole on a role that was never created succeeds, so the IAM-propagation retry contract cannot be observed (1 cause)

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRole

## Description
ONE root cause (cause 1 of 1).

AWS: sts:AssumeRole against a role ARN that was never created is refused with AccessDenied, HTTP 403 — STS deliberately does NOT disclose non-existence (it never answers NoSuchEntity or 404 there), which is what makes the "retry until the grant propagates" contract well defined: the caller retries AccessDenied and stops when the call succeeds. The companion iam:GetRole on the same name answers NoSuchEntity, proving the role really is absent.
Emulator: the AssumeRole on the absent role ARN SUCCEEDS (the check's helper reports "the call was expected to be rejected by the service but it SUCCEEDED -- a target that accepts this input is more broken than one that returns the wrong error code"), so no refusal is ever produced and the propagation contract is untestable.
Why: a client cannot distinguish "not yet propagated" from "this role does not exist"; worse, credentials are minted for an identity that does not exist, so any retry/backoff code built against the emulator takes a path AWS never offers.
Fix: the AssumeRole handler's role-resolution/authorization step (resolve the RoleArn in IAM state and answer AccessDenied/403 when it resolves to nothing). This is the same handler defect reported for service token `sts` in the AssumeRole admission issue filed alongside this one; it is filed here because this check carries its own replay key under iam__lambda__sts.
Traces: iam__lambda__sts.iam-propagation-is-a-retry-contract-not-a-sleep (the check fails at its non-disclosure leg, so its later Lambda-side legs remain unverified).

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1, check iam__lambda__sts.iam-propagation-is-a-retry-contract-not-a-sleep:

"File check_iam__lambda__sts_iam_propagation_is_a_retry_contract_not_a_sleep.py, line 300, in run: code = _error_code(lambda: sts.assume_role(RoleArn=absent_arn, RoleSessionName='sim2real-behav-absent')) ... AssertionError: the call was expected to be rejected by the service but it SUCCEEDED"

The check asserts code == "AccessDenied" for arn:aws:iam::<account>:role/<name never created>, and separately asserts iam.get_role on that name answers NoSuchEntity.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-replay iam__lambda__sts.iam-propagation-is-a-retry-contract-not-a-sleep.
3. Manual repro: sts.assume_role(RoleArn='arn:aws:iam::000000000000:role/never-created', RoleSessionName='probe') — the emulator returns 200 with Credentials where AWS returns AccessDenied/403.
