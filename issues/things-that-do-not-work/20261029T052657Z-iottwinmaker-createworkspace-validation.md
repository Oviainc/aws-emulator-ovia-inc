Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 8d0d74e15d09000e4789d5e17c7af92005cbf7c1

# The workspace `role` is stored without the assume-role trust check `CreateSyncJob` applies to a role ARN, although the shared helper claims `syncRole` is this service's only role member

_Filed: 2026-10-29T05:26:57Z_
_Operation: CreateWorkspace | Class: validation_

> _Note: this issue was identified against the code base at commit `8d0d74e15d09000e4789d5e17c7af92005cbf7c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate resolves a caller-supplied role ARN against the IAM store in the same engine and refuses, SYNCHRONOUSLY and before anything is persisted, a role whose trust policy withholds `sts:AssumeRole` from `iottwinmaker.amazonaws.com`. That rule lives in `create_sync_job.rs::assume_role_refusal` and is applied by exactly one handler.

`CreateWorkspace` (and `UpdateWorkspace`) carry the SAME kind of member — `role`, targeting the same `RoleArn` shape, documented as "The ARN of the execution role associated with the workspace" — and store it with no check at all: `role: input.role` on the create, `workspace.role = Some(role)` on the update. A workspace is therefore created (and `GetWorkspace` then reports) an execution role whose trust policy names another service principal, or explicitly denies `sts:AssumeRole` to this one — the exact state the sibling create refuses for the exact same kind of value.

The helper's own doc states that the member set it covers is complete: "`syncRole` is this service's only such member today, and a second one would move this rule to `crate::state` beside the rules every operation shares." The model contradicts that: `CreateWorkspaceRequest$role` and `UpdateWorkspaceRequest$role` are a second and third such member, both `RoleArn`. Nothing in `create_workspace.rs` justifies skipping the check either — the only statement it makes about `role` is about AUTHORIZATION (`iam:PassRole`), not about whether the service can assume the role at all, and the sync-job refusal is not an authorization decision (it is the modelled `ValidationException`, which `CreateWorkspace` and `UpdateWorkspace` both model too).

## Evidence

THE RULE A SIBLING HANDLER APPLIES — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/create_sync_job.rs:159-203:
```
/// `None` when this emulator has no reason to think the service could not act as
/// the role `role_arn` that `member` names; otherwise the message its refusal
/// carries.
///
/// Where a request names a role THIS service has to assume to carry out the work
/// the call starts, the role is resolved against the account's IAM records
/// before anything is persisted, and a role whose trust policy withholds
/// `sts:AssumeRole` from [`crate::state::SERVICE_PRINCIPAL`] is refused
/// synchronously rather than accepted and left to fail later.
...
/// `member` is the member that named the role,
/// so the message says which value the caller has to fix: `syncRole` is this
/// service's only such member today, and a second one would move this rule to
/// `crate::state` beside the rules every operation shares.
pub fn assume_role_refusal(member: &str, role_arn: &str, iam: &IamState) -> Option<String> {
```
and its one call site, src/ops/create_sync_job.rs:288-294:
```
        let role_refusal = assume_role_refusal("syncRole", &input.sync_role, &iam.lock());
        if let Some(message) = role_refusal {
            return Err(CreateSyncJobErrorModeled::ValidationException(ValidationException {
                message: Some(message),
            })
            .into());
        }
```

THE SECOND ROLE MEMBER THE MODEL DECLARES — `smithy_query(query="operation", operation="CreateWorkspace")`:
```
"role": {"target": "RoleArn", "length": {"min": 20, "max": 2048}, "pattern": "^arn:((aws)|(aws-cn)|(aws-us-gov)):iam::[0-9]{12}:role/"}
```
with `smithy_query(query="doc", shape="CreateWorkspaceRequest", member="role")`:
```
"<p>The ARN of the execution role associated with the workspace.</p>"
```
(`UpdateWorkspaceRequest$role` is the same shape and the same meaning.)

THE HANDLER THAT SKIPS IT — src/ops/create_workspace.rs:249-257 (no IAM store is reached anywhere in the file; `grep -n 'iam_state|assume_role_refusal'` matches only `create_sync_job.rs`):
```
        state.workspaces.insert(
            input.workspace_id.clone(),
            crate::state::Workspace {
                workspace_id: input.workspace_id.clone(),
                arn: arn.clone(),
                description: input.description.unwrap_or_default(),
                s3_location: input.s3_location,
                role: input.role,
```
and the mirror path, src/ops/update_workspace.rs:196-198:
```
        if let Some(role) = input.role {
            workspace.role = Some(role);
        }
```

THE ONLY STATEMENT THE CREATE MAKES ABOUT `role`, which covers authorization and not assumability — src/ops/create_workspace.rs:21-25:
```
//! Authorization is NOT emulated here (nor anywhere in this service), so the
//! modelled `AccessDeniedException` is never returned: the emulator verifies
//! request signatures centrally and carries no IAM policy or `iam:PassRole`
//! evaluation for the `role` this operation accepts. A 200 from here is not an
//! authorization decision.
```

TEST COVERAGE: tests/all/sync_job_role_trust.rs pins the rule for `syncRole` over every trust form ("a create naming a role of this account whose trust policy withholds `sts:AssumeRole` from `iottwinmaker.amazonaws.com` is refused SYNCHRONOUSLY — the modelled `ValidationException` (HTTP 400), not an accepted job left to fail later", lines 8-14). No test in tests/all/workspace_*.rs supplies a `role` at all, so nothing pins the workspace path either way.

## Steps to Reproduce

1. Through the IAM emulator in the same engine, CreateRole(RoleName="twinmaker-denied", AssumeRolePolicyDocument={"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}) and note its ARN.
2. CreateWorkspace(workspaceId="ws", role=<that ARN>, s3Location="arn:aws:s3:::bucket") -> 200: the workspace is created and GetWorkspace reports the role, although its trust policy never grants sts:AssumeRole to iottwinmaker.amazonaws.com.
3. CreateSyncJob(workspaceId="ws", syncSource="SITEWISE", syncRole=<the same ARN>) -> 400 ValidationException naming the same role and the same missing grant.
4. UpdateWorkspace(workspaceId="ws", role=<the same ARN>) -> 200 as well: both workspace paths accept the role the sync-job path refuses.
