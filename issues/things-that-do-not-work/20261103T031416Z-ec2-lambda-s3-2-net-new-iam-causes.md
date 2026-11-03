Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda__s3.iam-rejects-neptune-service-principal-in-trust-policy ec2__lambda__s3.neptune-loader-role-trust-policy-round-trips-rds-principal
Issue-Key: ec2__lambda__s3.iam-rejects-neptune-service-principal-in-trust-policy|_err_code_and_message#5|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: ec2__lambda__s3.neptune-loader-role-trust-policy-round-trips-rds-principal|run#12|AssertionError|assert "RoleLastUsed" in got, (
Issue-Commit: dc7d1cb059ddc05800728693f0165ec8a2a1b13e

# ec2__lambda__s3: 2 net-new IAM causes — CreateRole accepts a service principal that does not exist, and GetRole omits RoleLastUsed

> _Note: this issue was identified against the code base at commit `dc7d1cb059ddc05800728693f0165ec8a2a1b13e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRole

## Description
Two distinct root causes under the suite heading `ec2__lambda__s3`, both in IAM's role surface. A fixer must close both before submitting.

Cause 1 of 2: CreateRole accepts a trust policy naming `neptune.amazonaws.com`, a service principal AWS does not recognise.
AWS: IAM validates the service principals named in `AssumeRolePolicyDocument` against the set that exists. A trust policy whose `Principal.Service` is `neptune.amazonaws.com` is refused with HTTP 400 `MalformedPolicyDocument`, message `Invalid principal in policy: "SERVICE":"neptune.amazonaws.com"` — the fixed text is `Invalid principal in policy: "SERVICE":"` + the rejected principal from the request + `"`. The role is not created, so a following GetRole answers `NoSuchEntity`.
Emulator: CreateRole SUCCEEDS and the role exists. Replay detail: `AssertionError: the call succeeded; real AWS rejects it, so there is no error code`.
Why: a client (or a CloudFormation template) that names a non-existent service principal is told everything is fine; the role is created but can never be assumed, so the failure surfaces much later as an opaque AccessDenied instead of at the create call AWS refuses.
Fix: iam's CreateRole (and UpdateAssumeRolePolicy) trust-policy validation — validate `Principal.Service` against the known service-principal set and emit MalformedPolicyDocument naming the rejected principal.
Traces: ec2__lambda__s3.iam-rejects-neptune-service-principal-in-trust-policy

Cause 2 of 2: GetRole omits `RoleLastUsed` for a role that has never been used.
AWS: `GetRole` always returns the `Role.RoleLastUsed` structure; for a never-used role it is present but empty (no `LastUsedDate`/`Region`), alongside Arn, AssumeRolePolicyDocument, CreateDate, MaxSessionDuration, Path, RoleId, RoleName and Tags.
Emulator: the response's Role keys are `['Arn', 'AssumeRolePolicyDocument', 'CreateDate', 'MaxSessionDuration', 'Path', 'RoleId', 'RoleName', 'Tags']` — `RoleLastUsed` is absent entirely.
Why: a client that reads `role['RoleLastUsed']` (the documented way to tell "never used" from "used at T") raises KeyError instead of seeing an empty structure, so last-used reporting and unused-role cleanup logic break.
Fix: iam's GetRole serializer / role state — always emit the RoleLastUsed structure, empty when the role has never been used.
Traces: ec2__lambda__s3.neptune-loader-role-trust-policy-round-trips-rds-principal

## Evidence
Cause 1 — expected ('MalformedPolicyDocument', 'Invalid principal in policy: "SERVICE":"neptune.amazonaws.com"') and a following GetRole answering NoSuchEntity. Actual: CreateRole returned success and the role exists.
Cause 2 — expected `GetRole().Role` to contain `RoleLastUsed` (an empty structure for a never-used role). Actual: keys returned were ['Arn', 'AssumeRolePolicyDocument', 'CreateDate', 'MaxSessionDuration', 'Path', 'RoleId', 'RoleName', 'Tags'] — RoleLastUsed missing.

## Steps to Reproduce
1. Replay the behavioral suite and re-run the two checks in trace_ids.
2. Cause 1: `iam create-role --role-name x --assume-role-policy-document '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"neptune.amazonaws.com"},"Action":"sts:AssumeRole"}]}'`; observe success instead of MalformedPolicyDocument.
3. Cause 2: create a role with a valid service principal (e.g. rds.amazonaws.com), never use it, then `iam get-role --role-name <name>` and look for `RoleLastUsed`.
