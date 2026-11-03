Issue-Source: trace-sweep-divergence
Issue-Trace: iam-acquire-role-backup-service-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-acquire-role-backup-service-lifecycle iam-acquire-role-power-user-lifecycle
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|AcquireRole|2xx->4xx|$.AcquireRoleResponse
Issue-Key: iam|AcquireRole|2xx->4xx|status
Issue-Commit: f9571cbe65aa0d276fa5ca35992cbed3efbd51b6

# IAM AcquireRole answers 404 so no role template can be acquired

> _Note: this issue was identified against the code base at commit `f9571cbe65aa0d276fa5ca35992cbed3efbd51b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AcquireRole

## Description
Cause 1 of 1: AcquireRole never produces a role.
AWS: AcquireRole is bound to this service in the checked-out Smithy model — input AcquireRoleRequest (TemplateArn @required arnType, TemplateMinorVersion, ReplacementValues), output AcquireRoleResponse with Role @required; its modeled client errors are NoSuchEntity (404), InvalidInput (400), EntityAlreadyExists/NameConflict/ConcurrentModification/LimitExceeded/RoleModified (409), MalformedPolicyDocument / RoleTemplateDisabled (400). Recording: 200 with AcquireRoleResult.Role carrying RoleName, Arn, Path (/service-role/), RoleId, CreateDate and the URL-encoded AssumeRolePolicyDocument built from the template with the caller's ReplacementValues substituted.
Emulator: 404 — no AcquireRoleResponse is produced at all, for a TemplateArn the fixture's recording resolves.
Why: every role acquired from a template is missing, so the whole template-based role flow is unusable: the caller gets a not-found for a template AWS resolves, and each dependent step (trust-policy inspection, attachment, assumption of the acquired role) has nothing to act on.
Fix: the AcquireRole handler plus the role-template state it resolves TemplateArn against (the template catalog must be populated and the acquired Role projected with Path, Arn and the substituted, URL-encoded AssumeRolePolicyDocument).
Traces: iam-acquire-role-backup-service-lifecycle#task#1.

The open issue 20261029T184359Z-iam-role-templates-are-not-provisioned-so-dependent-role.md describes the same missing template provisioning but carries no replay key for this cluster's steps.

## Evidence
Replay of trace set simparity-wire-aws-v1: iam-acquire-role-backup-service-lifecycle#task#1, AcquireRole (cluster of 3, also iam-acquire-role-power-user-lifecycle#task#1 and #task#2) — status expected 200, got 404; $.AcquireRoleResponse expected {'AcquireRoleResult': {'Role': {'AssumeRolePolicyDocument': '%7B%22Version%22%3A%222012-10-17%22%2C%22Statement%22%3A%5B%7B%22Effect%22%3A%22Allow%22%2C%22Principal%22%3A%7B%22Service%22%3A%22backup.amazonaws.com%22%7D%2C%22Action%22%3A%22sts%3AAssumeRole%22%2C%22Condition%22%3A...%7D%5D%7D', 'RoleId': '<shaped-not-asserted>', 'Path': '/service-role/', 'RoleName': 'sim2real-probe-iam-backup-svc-role-df3c92c1', 'Arn': 'arn:aws:iam::000000000000:role/service-role/sim2real-probe-iam-backup-svc-role-df3c92c1', 'CreateDate': '<shaped-not-asserted>'}}, 'ResponseMetadata': {...}}, got None.

## Steps to Reproduce
1. Boot the emulator built from this checkout.
2. Replay fixture iam-acquire-role-backup-service-lifecycle step task#1 (AcquireRole with a TemplateArn and ReplacementValues) and observe 404 instead of 200 with AcquireRoleResult.Role.
3. The same happens at iam-acquire-role-power-user-lifecycle#task#1 and #task#2.
