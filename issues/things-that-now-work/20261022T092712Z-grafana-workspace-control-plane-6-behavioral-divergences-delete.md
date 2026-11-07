Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: grafana.delete-workspace-removes-resource-and-double-delete-conflicts grafana.update-workspace-conflict-while-updating-rejected grafana.missing-iam-permission-create-workspace-rejected grafana.describe-workspace-nonexistent-id-not-found grafana.describe-workspace-malformed-id-rejected grafana.deploy-workspace-via-cloudformation-persists-update
Issue-Shape-Id: com.amazonaws.grafana#AWSGrafanaControlPlane
Issue-Key: grafana|DeleteWorkspace|ResourceNotFoundException
Issue-Key: grafana.update-workspace-conflict-while-updating-rejected|_error#7|AssertionError|raise AssertionError(
Issue-Key: grafana.missing-iam-permission-create-workspace-rejected|_error#5|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: grafana.describe-workspace-nonexistent-id-not-found|run#5|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: grafana.describe-workspace-malformed-id-rejected|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: grafana|DescribeWorkspace|ValidationException
Issue-Commit: 0b8884c4ab978d23b4fe746f795a1277a2a36df3

# Grafana workspace control plane: 6 behavioral divergences (delete lifecycle, update conflict gating, IAM denial, error messages, CFN Ref)

> _Note: this issue was identified against the code base at commit `0b8884c4ab978d23b4fe746f795a1277a2a36df3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteWorkspace, UpdateWorkspace, DescribeWorkspace, CreateWorkspace

## Description
The Amazon Managed Grafana control-plane emulator answers (3/19 behavioral checks pass), but six distinct root causes diverge from real AWS. All six are in the `grafana` service and are filed together as ONE work item with SIX causes; a fixer must close all six before submitting.

=== Cause 1 of 6: DeleteWorkspace destroys the record synchronously, so an in-flight re-delete is rejected instead of being idempotent ===
AWS: Smithy `DeleteWorkspace` is `@idempotent`, HTTP `DELETE /workspaces/{workspaceId}`, success status 202, output `workspace: WorkspaceDescription` (required), and models `ConflictException` (409) and `ResourceNotFoundException` (404). Recorded behavior: DeleteWorkspace on an ACTIVE workspace returns 202 with `status=DELETING`; a SECOND DeleteWorkspace issued immediately, while the deletion is still in flight, is accepted idempotently — another 202 reporting the SAME workspace id still in `DELETING` — and is NOT rejected (neither ConflictException nor ResourceNotFoundException). Only after the workspace has actually been removed does a further delete fail, and then with `ResourceNotFoundException` (404).
Emulator: the handler removes the record from the store on the first call and only decorates the removed copy with `status="DELETING"` for the response; there is no DELETING-then-removed lifecycle. The immediate second DeleteWorkspace therefore returns 404 `ResourceNotFoundException` (observed: `An error occurred (ResourceNotFoundException) when calling the DeleteWorkspace operation: Workspace g-0000000001 not found`).
Why: a client that retries a delete (the SDK's own retry of a 202, or any at-least-once caller) sees a hard 404 for a workspace it just successfully asked to delete, and cannot distinguish "deletion in progress" from "never existed". Pollers that read the workspace during deletion also never observe the DELETING state the API promises.
Fix: `DeleteWorkspace`'s handler plus the workspace lifecycle in this service's state — mark the record DELETING with a pending transition to removal (the same mechanism `Workspace::pending`/`settle` already uses for CREATING→ACTIVE) and answer a repeat delete of a DELETING workspace with 202 and the same record, removing it only once the transition falls due.
Traces: grafana.delete-workspace-removes-resource-and-double-delete-conflicts. Code: services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/ops/delete_workspace.rs:163-184 (store.workspaces.remove), src/state.rs:120-129 (settle).

=== Cause 2 of 6: UpdateWorkspace is not gated on the workspace's in-flight state — a concurrent update is accepted instead of 409 ConflictException ===
AWS: Smithy `UpdateWorkspace` is `PUT /workspaces/{workspaceId}` with success status 202 and models `ConflictException` (HTTP 409, documented "A resource was in an inconsistent state during an update or a deletion"). The 202 contract means the workspace is already `UPDATING` when the response returns, so a second UpdateWorkspace issued while the first mutation is in flight is rejected with `ConflictException` (recorded message example: "Workspace operation failed. Workspace is currently in the UPDATING state.") and its value does not reach the persisted state — the first update's value is what settles.
Emulator: the second UpdateWorkspace succeeds with HTTP 202 and the workspace is returned with `status=UPDATING` and `description='behav-second-update'` — the concurrent write is silently applied on top of the in-flight one, and no ConflictException is ever produced.
Why: a client relying on the conflict to serialize mutations (read-modify-write, or two operators racing) gets last-writer-wins with no signal, so an update it was told to retry/abandon is the one that lands. Error-handling paths for 409 are never exercised.
Fix: `UpdateWorkspace`'s handler — refuse a workspace whose current status is a transient one (UPDATING, CREATING, DELETING) with the modeled `ConflictException` (409) and leave the stored record untouched, while still accepting an identical call once the workspace has settled back to ACTIVE.
Traces: grafana.update-workspace-conflict-while-updating-rejected. Code: services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/ops/update_workspace.rs.

=== Cause 3 of 6: no identity-based authorization — CreateWorkspace succeeds for a principal with no permissions instead of 403 AccessDeniedException ===
AWS: Smithy `CreateWorkspace` models `AccessDeniedException` (HTTP 403, "You do not have sufficient permissions to perform this action"). Recorded behavior: a role session assumed from an IAM role with no attached policies (implicit deny) gets `AccessDeniedException` from CreateWorkspace, with a message built from the request's own principal and action — of the form `User: <assumed-role ARN> is not authorized to perform: grafana:CreateWorkspace on resource: arn:aws:grafana:<region>:<account>:/workspaces because no identity-based policy allows the grafana:CreateWorkspace action` — and NO workspace is created by the refused attempt.
Emulator: the call from the powerless role session is accepted with HTTP 202 and a workspace is minted (observed response: `status=CREATING`, `id=g-0000000001`, `name=sim2real-behav-noperm-...`). No authorization decision is made at all.
Why: a client (or a test suite, or a least-privilege rollout) validating that an under-privileged principal is refused sees success, so a permissions regression passes against the emulator; and the emulator's state is mutated by a call AWS would have refused.
Fix: request authorization for this service — evaluate the caller's identity-based policy (the signing principal / assumed-role session) before the handler mutates state, and answer the modeled `AccessDeniedException` (403) with the principal/action/resource-derived message; applies to CreateWorkspace and the other workspace operations.
Traces: grafana.missing-iam-permission-create-workspace-rejected. Code: services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/ops/create_workspace.rs (authorization seam).

=== Cause 4 of 6: ResourceNotFoundException message for an unknown workspace id does not follow the service's message rule ===
AWS: Smithy: `DescribeWorkspace`/`DeleteWorkspace`/`UpdateWorkspace` model `ResourceNotFoundException` (HTTP 404). The message is built from the request and the caller's account, with the rule `Workspace with id=<workspaceId> of account=<accountId> does not exist.` (recorded example for a well-formed but never-issued id `g-0000000000` in account `000000000000`: "Workspace with id=g-0000000000 of account=000000000000 does not exist."). The status (404) and code (`ResourceNotFoundException`) match today; the message composition rule is the divergence.
Emulator: one shared constructor emits `Workspace <id> not found`, which names neither the account nor the `id=` form, for every workspace-not-found path (describe, delete, update).
Why: a client that parses or asserts on the not-found message (and the behavioral contract that the same rule holds for every workspace operation) sees a different sentence than AWS produces for the same request; the account dimension — which is what makes a cross-account id distinguishable — is absent entirely.
Fix: the shared `ResourceNotFoundException` builder in this service's state, which must compose the message from the requested workspace id and the caller's account id rather than a fixed sentence.
Traces: grafana.describe-workspace-nonexistent-id-not-found (asserted directly); the same rule is also asserted by grafana.delete-workspace-removes-resource-and-double-delete-conflicts and grafana.deploy-workspace-via-cloudformation-persists-update, which fail earlier on causes 1 and 6. Code: services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/state.rs:287-293 (workspace_not_found).

=== Cause 5 of 6: ValidationException for a malformed workspaceId uses the generic boundary-validator message, not Grafana's pattern message ===
AWS: Smithy: `DescribeWorkspaceRequest$workspaceId` is `@required`, `@httpLabel`, pattern `^g-[0-9a-f]{10}$`; the operation models `ValidationException` (HTTP 400). Recorded behavior for a value failing that pattern: 400 `ValidationException` whose message is produced from the member's own constraint in Grafana's form — `The workspaceId should satisfy the pattern ^g-[0-9a-f]{10}$.` The status and code match today; the message rule is the divergence.
Emulator: the centralized boundary validator emits the generic multi-service form `1 validation error detected: Value 'not-a-valid-workspace-id' at 'workspaceId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^g-[0-9a-f]{10}$`.
Why: the message rule is part of this service's rejection contract; a client surfacing or matching Grafana's pattern message gets a differently-shaped sentence (and one that echoes the rejected value, which AWS's form does not).
Fix: this operation's validation layer for the Grafana service — override the generic pattern-violation message for `WorkspaceId` members with the service's own `The workspaceId should satisfy the pattern <pattern>.` form while keeping 400/`ValidationException`.
Traces: grafana.describe-workspace-malformed-id-rejected.

=== Cause 6 of 6: CloudFormation AWS::Grafana::Workspace resolves Ref / PhysicalResourceId to the workspace NAME, not the workspace id ===
AWS: For `AWS::Grafana::Workspace`, `{"Ref": "<logical id>"}` and the stack resource's `PhysicalResourceId` are the workspace's **Id** — the value matching `^g-[0-9a-f]{10}$` that `CreateWorkspace` mints and that `DescribeWorkspace`'s `workspaceId` label accepts (Smithy: `WorkspaceId` pattern `^g-[0-9a-f]{10}$`). A stack that provisions a workspace therefore yields an id usable directly against the Grafana control plane, and deleting the stack removes that workspace.
Emulator: the stack reaches CREATE_COMPLETE, but the `Ref`-derived stack output is the template's `Name` property value (observed `sim2real-behav-64c6edd62443`), so the very next `DescribeWorkspace` with it is rejected 400 `ValidationException` (`... at 'workspaceId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^g-[0-9a-f]{10}$`). The physical identity of the CFN-provisioned workspace is not a workspace id at all.
Why: every CFN-driven workflow breaks at the handoff — `Ref`/`GetAtt` outputs, `DescribeStackResources().PhysicalResourceId`, cross-stack references and any direct Grafana API call made with the stack's output are all unusable; and because the physical id is not a workspace id, stack deletion cannot be verified to tear the workspace down.
Fix: the CloudFormation resource provider for `AWS::Grafana::Workspace` — it must create the workspace through the Grafana control plane and record the minted `g-` id as the resource's PhysicalResourceId / Ref value (and delete that workspace on stack delete).
Traces: grafana.deploy-workspace-via-cloudformation-persists-update.

## Evidence
Behavioral suite: total 3/19 passed; grafana 3/19 (16%), 16 failed. 10 failures are already filed as unimplemented-operation gaps (TagResource, DescribeWorkspaceAuthentication, CreateWorkspaceServiceAccount, ListVersions, DescribeWorkspaceConfiguration, CreateWorkspaceApiKey) and are excluded here. The six causes above:

1. grafana.delete-workspace-removes-resource-and-double-delete-conflicts [error]
   Expected: second DeleteWorkspace while the first is in flight -> HTTP 202, workspace with same id and status DELETING.
   Actual: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DeleteWorkspace operation: Workspace g-0000000001 not found.

2. grafana.update-workspace-conflict-while-updating-rejected [mismatch]
   Expected: ConflictException (HTTP 409) for the second UpdateWorkspace while status is UPDATING; first update's description persists.
   Actual: AssertionError: expected the concurrent UpdateWorkspace to be rejected, but it SUCCEEDED: HTTPStatusCode 202, workspace {id g-0000000001, status 'UPDATING', description 'behav-second-update'}.

3. grafana.missing-iam-permission-create-workspace-rejected [mismatch]
   Expected: AccessDeniedException (HTTP 403) with message of the form 'User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: grafana:CreateWorkspace on resource: arn:aws:grafana:<region>:<account>:/workspaces because no identity-based policy allows the grafana:CreateWorkspace action'; no workspace created.
   Actual: AssertionError: expected the call to be rejected, but it SUCCEEDED and returned HTTPStatusCode 202 with workspace {id 'g-0000000001', status 'CREATING', name 'sim2real-behav-noperm-c26a26e65238'}.

4. grafana.describe-workspace-nonexistent-id-not-found [mismatch]
   Code matches (ResourceNotFoundException). Expected message rule: 'Workspace with id=<id> of account=<account> does not exist.' (example: 'Workspace with id=g-0000000000 of account=000000000000 does not exist.').
   Actual: 'Workspace g-0000000000 not found'.

5. grafana.describe-workspace-malformed-id-rejected [mismatch]
   Code matches (ValidationException, 400). Expected message rule: '<member> should satisfy the pattern <modeled pattern>.' (example: 'The workspaceId should satisfy the pattern ^g-[0-9a-f]{10}$.').
   Actual: "1 validation error detected: Value 'not-a-valid-workspace-id' at 'workspaceId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^g-[0-9a-f]{10}$".

6. grafana.deploy-workspace-via-cloudformation-persists-update [error]
   Expected: the stack output {'Ref': 'GrafanaWorkspace'} (and DescribeStackResources PhysicalResourceId) is the workspace id matching ^g-[0-9a-f]{10}$, usable with DescribeWorkspace.
   Actual: the Ref output was the workspace NAME, so DescribeWorkspace failed: botocore.errorfactory.ValidationException: 1 validation error detected: Value 'sim2real-behav-64c6edd62443' at 'workspaceId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^g-[0-9a-f]{10}$.

## Steps to Reproduce
Boot the emulator and replay the grafana behavioral checks listed in trace_ids. Per cause:

1. CreateWorkspace (accountAccessType=CURRENT_ACCOUNT, permissionType=SERVICE_MANAGED, authenticationProviders=[SAML], workspaceRoleArn=<role>); wait for ACTIVE; DeleteWorkspace -> 202/DELETING; immediately DeleteWorkspace again -> emulator answers 404 ResourceNotFoundException, AWS answers 202 with the same id still DELETING.

2. Create a workspace, wait for ACTIVE, UpdateWorkspace(workspaceDescription='first') -> 202/UPDATING, then immediately UpdateWorkspace(workspaceDescription='second') -> emulator answers 202 and applies 'second'; AWS answers 409 ConflictException and keeps 'first'.

3. iam.CreateRole with a trust policy for the account root and NO attached policies; sts.AssumeRole; with those session credentials call grafana.CreateWorkspace -> emulator answers 202 and mints a workspace; AWS answers 403 AccessDeniedException and creates nothing.

4. grafana.DescribeWorkspace(workspaceId='g-0000000000') (well-formed, never issued) -> compare Error.Message against 'Workspace with id=g-0000000000 of account=<caller account> does not exist.'

5. grafana.DescribeWorkspace(workspaceId='not-a-valid-workspace-id') -> compare Error.Message against 'The workspaceId should satisfy the pattern ^g-[0-9a-f]{10}$.'

6. cloudformation.CreateStack with a template containing one AWS::Grafana::Workspace (AccountAccessType=CURRENT_ACCOUNT, AuthenticationProviders=[SAML], PermissionType=SERVICE_MANAGED, RoleArn=<role>, Name=<name>, Description=<desc>) and Outputs.WorkspaceId = {'Ref': 'GrafanaWorkspace'}; wait for CREATE_COMPLETE; read the output and DescribeStackResources PhysicalResourceId -> both are the Name, and grafana.DescribeWorkspace with that value is rejected as a malformed workspaceId.
