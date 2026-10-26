Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.e2e-service-role-setup
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.e2e-service-role-setup|_assert_error#39|AssertionError|assert err.get("Code") == code, f"expected {code}, got {err.get('Code')!r}"
Issue-Commit: 851c7b8e55a8e0663fe932e2290fb361fc961d0e

# IAM CreatePolicyVersion on a deleted policy ARN answers NotImplemented instead of NoSuchEntity 404

> _Note: this issue was identified against the code base at commit `851c7b8e55a8e0663fe932e2290fb361fc961d0e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePolicyVersion

## Description
One net-new root cause.

Cause 1 of 1: `CreatePolicyVersion` never reaches a real handler, so the "mutate a dead policy ARN" leg of the managed-policy lifecycle is refused with the scaffold's generic 501 instead of IAM's modelled not-found refusal. Failing check id: `iam.e2e-service-role-setup`.

AWS: `CreatePolicyVersion` takes a required `PolicyArn` (Smithy: `CreatePolicyVersion.PolicyArn` -> `arnType`, @required, @length min 20 max 2048) and a required `PolicyDocument`. Among its modelled errors is `NoSuchEntityException` (Smithy: errorKind client, httpError 404, awsQuery error code `NoSuchEntity` / httpResponseCode 404), documented as "The request was rejected because it referenced a resource entity that does not exist. The error message describes the resource." So when the ARN names a managed policy that does not exist — here one that was just deleted with `DeletePolicy`, and whose absence the same check already confirmed through `GetPolicy` (NoSuchEntity 404) and through a `ListPolicies` path-prefix listing — AWS refuses the version create with HTTP 404, error code `NoSuchEntity`, and a non-empty message naming the missing policy. The fixed text of that message is IAM's not-found wording for a managed policy referenced by ARN; its only variable part is the policy ARN taken verbatim from the request's `PolicyArn`. `NotImplemented` is not in the operation's modelled error set at all.

Emulator: the call is answered by the unmodified scaffold — `operation CreatePolicyVersion is scaffolded but not implemented in this emulator`, error code `NotImplemented`, HTTP 501. The replay shows `AssertionError: expected NoSuchEntity, got 'NotImplemented'` at the `_assert_error(..., "NoSuchEntity", 404)` step that follows `DeletePolicy`. Because the refusal is a 501 and not a 4xx, the check's status-class assertion (`got_status // 100 == 4`) also cannot hold, and no managed-policy version is ever persisted on the success path.

Why: a client doing exactly what IaC does — create a managed policy, attach it, detach it, delete it, then prove the dead ARN is unusable — cannot distinguish "the policy is gone" from "the service is broken". `NoSuchEntity` is a retriable/idempotent signal that deletion converged; `NotImplemented` is a 501 server fault that botocore does not classify as a client error of the entity-not-found class, so cleanup and reconciliation loops that branch on `NoSuchEntity` (the check's own `_quiet` teardown does exactly this) either abort or loop forever. Equally, no caller can ever add a version to a managed policy, so policy versioning, default-version selection and policy updates are unreachable.

Fix: implement the `CreatePolicyVersion` handler and its validation — `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_policy_version.rs` — so it resolves `PolicyArn` against the managed-policy state and returns the modelled `NoSuchEntityException` (awsQuery code `NoSuchEntity`, HTTP 404, message naming the requested ARN) when no such policy exists, rather than the scaffold's `Scaffolded::not_implemented`.

Related open issue: `20261025T000319Z-iam-createpolicyversion-stub.md` ("Implement iam CreatePolicyVersion: unimplemented scaffold stub") covers the same stub but carries only 9 replay keys and does not list `iam.e2e-service-role-setup`, nor does it state the not-found requirement above; the suite did not mark this check as owned by it. This issue adds that replay key and the `NoSuchEntity`-on-deleted-ARN requirement.

Traces: `iam.e2e-service-role-setup` — fails at `check_iam_e2e_service_role_setup.py` line 118 (`_assert_error(_eventual_error(lambda: iam.create_policy_version(PolicyArn=policy_arn, PolicyDocument=s3_policy_doc)), "NoSuchEntity", 404)`), graded in `_assert_error` at line 173.

## Evidence
Expected (real AWS): after `DeletePolicy(PolicyArn=<arn>)` converges, `CreatePolicyVersion(PolicyArn=<arn>, PolicyDocument=<doc>)` is refused with HTTP 404, `Error.Code == "NoSuchEntity"`, and a non-empty `Error.Message` describing the referenced policy. Modelled as `NoSuchEntityException` on `CreatePolicyVersion` (awsQueryError code `NoSuchEntity`, httpResponseCode 404).

Actual (emulator): HTTP 501, `Error.Code == "NotImplemented"`, `Error.Message == "operation CreatePolicyVersion is scaffolded but not implemented in this emulator"`.

Replay output:
```
#### [mismatch] iam.e2e-service-role-setup
File "check_iam_e2e_service_role_setup.py", line 118, in run
  _assert_error(
    _eventual_error(lambda: iam.create_policy_version(PolicyArn=policy_arn, PolicyDocument=s3_policy_doc)),
    "NoSuchEntity", 404,
  )
File "check_iam_e2e_service_role_setup.py", line 173, in _assert_error
  assert err.get("Code") == code, f"expected {code}, got {err.get('Code')!r}"
AssertionError: expected NoSuchEntity, got 'NotImplemented'
```
The fixed text of the expected message is IAM's managed-policy not-found wording; the only request-derived value in it is the `PolicyArn` that was sent. The emulator's message is the scaffold's generic text and names neither the operation's entity nor the ARN as a resource reference.

## Steps to Reproduce
1. Boot the emulator and point an `iam` client at it.
2. `CreateRole` with a Lambda trust policy under a unique path; `CreatePolicy` with a minimal `s3:GetObject` document under the same path.
3. `AttachRolePolicy`, then `DetachRolePolicy`, confirming `GetPolicy(...).Policy.AttachmentCount` goes 1 -> 0.
4. `DeletePolicy(PolicyArn=<arn>)`. Confirm `GetPolicy(PolicyArn=<arn>)` is refused `NoSuchEntity` 404 and that the ARN no longer appears in `ListPolicies(Scope="Local", PathPrefix=<path>)`.
5. Call `CreatePolicyVersion(PolicyArn=<the deleted arn>, PolicyDocument=<the same document>)`.
6. Observe HTTP 501 / `NotImplemented` instead of HTTP 404 / `NoSuchEntity` with a message describing the missing policy.

Or replay the check directly: `iam.e2e-service-role-setup`.
