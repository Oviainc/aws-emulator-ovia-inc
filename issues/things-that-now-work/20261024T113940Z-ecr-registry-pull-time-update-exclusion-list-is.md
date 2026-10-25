Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.register-pull-time-exclusion-limit-exceeded-rejected
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr.register-pull-time-exclusion-limit-exceeded-rejected|run#10|AssertionError|raise AssertionError(
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# ECR registry pull-time-update-exclusion list is capped at 1000 instead of 100, so RegisterPullTimeUpdateExclusion never returns LimitExceededException (1 cause)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterPullTimeUpdateExclusion

## Description
One root cause, in ECR, found by the behavioral suite.

Cause 1 of 1: the registry's pull-time-update-exclusion list is bounded by an emulator-chosen cap of 1000 rather than the registry quota of 100, so a caller can register far more exclusions than AWS allows and the operation's modeled LimitExceededException is unreachable.

AWS: Smithy (services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/smithy-model/com-amazonaws-ecr-amazonec2containerregistry-v20150921.json): RegisterPullTimeUpdateExclusion(principalArn @required, @length 0..200, @pattern ^arn:aws(-[a-z]+)*:iam::[0-9]{12}:(role|user)/[\w+=,.@-]+(/[\w+=,.@-]+)*$) declares LimitExceededException (client error, HTTP 400, doc: "The operation did not succeed because it would have exceeded a service limit for your account."), and the read side bounds the same collection: ListPullTimeUpdateExclusions output member pullTimeUpdateExclusions targets PullTimeUpdateExclusionList, a list with @length min 0 max 100. So the model itself states that a registry holds at most 100 exclusions. The recorded behavior agrees and names the number: once the registry holds the maximum, a further register is refused with HTTP 400 LimitExceededException whose message is built from the request's registry id and that quota (recorded message, as an example: "The registry with id '<account>' has the maximum number of pull time update exclusions allowed, which is 100."), and the refused principal does not appear in ListPullTimeUpdateExclusions. The requirement here is the status, the error code and the fact that the cap is 100 — not the recorded sentence.

Emulator: the handler does implement a cap and does model the error, but the cap is set to a self-chosen 1000: services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/state.rs declares `pub const MAX_PULL_TIME_UPDATE_EXCLUSIONS: usize = 1_000;` (its own doc comment says "AWS publishes no fixed quota ... so this is the emulator's own bound"), and services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/register_pull_time_update_exclusion.rs refuses only at `store.pull_time_update_exclusions.len() >= crate::state::MAX_PULL_TIME_UPDATE_EXCLUSIONS`. Replay: 200 consecutive RegisterPullTimeUpdateExclusion calls, each naming a distinct IAM role ARN in the caller's account, all returned HTTP 200 and were all stored; no call was ever refused, so the check could not observe LimitExceededException at all. As a consequence ListPullTimeUpdateExclusions can also answer with a pullTimeUpdateExclusions list longer than the model's @length max 100.

Why: a client that relies on AWS's quota gets two wrong answers. First, code that handles LimitExceededException (reconciliation loops that prune exclusions before adding, provisioning tools that surface the quota to the operator) never takes that branch against the emulator and silently "succeeds" past a boundary where real AWS stops it — the state built up in test is not reachable in production. Second, a strictly validating SDK or proxy reading ListPullTimeUpdateExclusions can be handed a list that violates the modeled @length max 100 constraint.

Fix: the exclusion-list bound in the ECR engine state and the precondition in the RegisterPullTimeUpdateExclusion handler — the cap must be the registry quota of 100 that the model's PullTimeUpdateExclusionList @length and the recorded LimitExceededException message both state, and the refusal must leave the rejected principal unstored (file and line citations above are for navigation only).

Traces: ecr.register-pull-time-exclusion-limit-exceeded-rejected.

## Evidence
Check ecr.register-pull-time-exclusion-limit-exceeded-rejected, verdict mismatch:

  File "check_ecr_register_pull_time_exclusion_limit_exceeded_rejected.py", line 56, in run
    raise AssertionError(...)
  AssertionError: could not reach the (undocumented) pull-time-update-exclusion account limit within 200 registrations; closing this requires the exact published quota value to bound the attempt loop precisely

Expected (real AWS): the register that would exceed the registry's exclusion quota is refused with HTTP 400, Error.Code == "LimitExceededException" (recorded message, as an example: "The registry with id '<account>' has the maximum number of pull time update exclusions allowed, which is 100."); every exclusion registered before the limit is still listed by ListPullTimeUpdateExclusions and the rejected principalArn is not.

Actual (emulator): all 200 register calls returned HTTP 200 and were stored; no LimitExceededException was ever produced. Model evidence that the cap is 100: ListPullTimeUpdateExclusions.pullTimeUpdateExclusions -> PullTimeUpdateExclusionList with @length {min: 0, max: 100}. Emulator evidence that the cap is 1000: state.rs MAX_PULL_TIME_UPDATE_EXCLUSIONS = 1_000, used as the only gate in register_pull_time_update_exclusion.rs (the LimitExceededException variant is modeled and constructed there, it is simply unreachable below 1000 entries).

## Steps to Reproduce
Against the emulator's ECR endpoint, in a single process (the exclusion list is registry-wide in-memory state):
1. account = sts.get_caller_identity()['Account'].
2. For i in 0..200: ecr.register_pull_time_update_exclusion(principalArn=f"arn:aws:iam::{account}:role/some-unique-role-{i}"). Observe that every call succeeds with HTTP 200 and a createdAt; real AWS refuses the 101st with HTTP 400 LimitExceededException.
3. Page ecr.list_pull_time_update_exclusions() with nextToken and count the returned pullTimeUpdateExclusions: the emulator reports more than 100 entries, which the modeled PullTimeUpdateExclusionList (@length max 100) cannot hold.
4. Expected after the fix: the register that would make the list exceed 100 is refused with HTTP 400 LimitExceededException, the 100 earlier exclusions are all still listed, and the rejected principalArn is absent from ListPullTimeUpdateExclusions.
