Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: workspaces-instances.delete-workspace-instance-state-transition-to-deallocated
Issue-Shape-Id: com.amazonaws.workspacesinstances#EUCMIFrontendAPIService
Issue-Key: workspaces-instances.delete-workspace-instance-state-transition-to-deallocated|_poll_until#2|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Commit: 0e09cb7e3d3cc9d98bf7e715f2d4f9d39ba7fa1a

# workspaces-instances: the service-linked role for workspaces-instances.amazonaws.com is stored under a derived name, so iam:GetRole for AWS's published name answers NoSuchEntity and DeleteWorkspaceInstance's state machine is unverifiable

> _Note: this issue was identified against the code base at commit `0e09cb7e3d3cc9d98bf7e715f2d4f9d39ba7fa1a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateServiceLinkedRole

## Description
ONE root cause, grounded in one failing behavioral check. Cause 1 of 1.

=== Cause 1 of 1: CreateServiceLinkedRole(AWSServiceName="workspaces-instances.amazonaws.com") mints the role under a derived name, so the published name never resolves ===

AWS: Smithy (IAM model, services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json): CreateServiceLinkedRole takes AWSServiceName (groupNameType, @length 1..128, pattern ^[\w+=,.@-]+$, required) and returns Role; its member documentation says the CustomSuffix "is combined with the service-provided prefix to form the complete role name", i.e. the prefix is SERVICE-provided, not derived by the caller or by a name transform. GetRole takes RoleName (roleNameType, @length 1..64) and models exactly one client error, NoSuchEntityException (HTTP 404, awsQueryError code NoSuchEntity) — so NoSuchEntity is the assertion that the named role does not exist. The model carries no table of per-principal role names, so the NAME itself is recorded behavior only: AWS publishes the service-linked role for principal workspaces-instances.amazonaws.com as AWSServiceRoleForWorkspacesInstances (no hyphen, both labels capitalised), on path /aws-service-role/workspaces-instances.amazonaws.com/. The recording shows that after a successful CreateServiceLinkedRole for that principal (or after it is refused InvalidInput because the role already exists), iam:GetRole(RoleName="AWSServiceRoleForWorkspacesInstances") resolves the role — IAM's own read-your-writes settles within seconds. That role is the documented prerequisite for CreateWorkspaceInstance, so every WorkSpaces Instances workflow begins by creating it and reading it back under that published name.

Emulator: CreateServiceLinkedRole succeeds (the check only tolerates InvalidInput, and no other error escaped), but iam:GetRole(RoleName="AWSServiceRoleForWorkspacesInstances") answers HTTP 404 NoSuchEntity indefinitely — the check's bounded 60s read-your-writes poll on that exact name never converges and ends with AssertionError: condition not met within 60.0s; last=False, where False is the poll predicate observing NoSuchEntity on every attempt. The cause is in the IAM crate: services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_service_linked_role.rs resolves the role name through service_linked_role_name(), which consults crate::state::published_service_linked_role_name() and, when the principal is absent from crate::state::PUBLISHED_SERVICE_LINKED_ROLE_NAMES, falls back to derived_service_linked_role_name() (lines 117-149). That fallback capitalises only the principal's FIRST dot-separated label and keeps everything else verbatim, so "workspaces-instances.amazonaws.com" becomes the role name AWSServiceRoleForWorkspaces-instances — a name AWS never publishes. The role exists in state, just not under the name any client asks for.

Why: a client that follows AWS's documented prerequisite — create the SLR, then confirm it with GetRole under its published name before calling CreateWorkspaceInstance — either loops forever or fails with a 404 for a role the emulator just created. Read-your-writes on IAM appears broken for this principal, and because the WorkSpaces Instances checks gate on that confirmation, the whole DeleteWorkspaceInstance transition ALLOCATED -> DEALLOCATING -> DEALLOCATED (including the ResourceNotFoundException-equals-DEALLOCATED terminal reading) is never exercised at all, so its fidelity is still entirely unknown. Any other service whose principal is likewise absent from the published table inherits the same wrong name.

Fix: the published-name table consulted by CreateServiceLinkedRole's handler — crate::state::PUBLISHED_SERVICE_LINKED_ROLE_NAMES / published_service_linked_role_name() in the IAM crate's state, used by src/ops/create_service_linked_role.rs — must carry the row for principal workspaces-instances.amazonaws.com so the handler stops deriving the name; the derived fallback must not be what names a principal AWS publishes a name for. Nothing in the workspaces-instances crate's own state machine is implicated (this issue is filed under service workspaces-instances because that is the service heading the failing check is reported under; the code to change lives in the IAM crate).

Relation to an already-open issue: 20261029T190356Z-workspaces-instances-6-fidelity-divergences-across-createvolume-listregions.md describes this same name-derivation cause as its "Cause 6 of 6". The replay did NOT report that issue as owning this check — its recorded replay key for it is a Timeout key (_poll_until#2|Timeout), whereas the check now fails as an AssertionError out of the same 60s poll — so the check is unowned and filed here rather than treated as covered. A fixer closing either should close both.

Traces: workspaces-instances.delete-workspace-instance-state-transition-to-deallocated.

## Evidence
Replay report: workspaces-instances 10/25 (40%), 15 failed. Every other failing check was skipped per the report's own markings (12 OWNED by 20261029T190356Z-workspaces-instances-6-fidelity-divergences-across-createvolume-listregions.md, 2 OWNED by 20261107T120211Z-workspaces-instances-createworkspaceinstance-3-divergences-wrong-conflictexception-message-for-a.md).

[mismatch] workspaces-instances.delete-workspace-instance-state-transition-to-deallocated

Expected: after iam:CreateServiceLinkedRole(AWSServiceName="workspaces-instances.amazonaws.com"), iam:GetRole(RoleName="AWSServiceRoleForWorkspacesInstances") returns HTTP 200 with that Role, so the check proceeds to CreateWorkspaceInstance -> poll to ALLOCATED -> DeleteWorkspaceInstance -> poll to the terminal DEALLOCATED (or ResourceNotFoundException, read as DEALLOCATED).

Actual: GetRole for that name answers HTTP 404 NoSuchEntity on every attempt for 60s. Harness output:
  File "check_workspaces_instances_delete_workspace_instance_state_transition_to_deallocated.py", line 104, in _ensure_service_linked_role
    _poll_until(_role_visible, lambda visible: visible, timeout_s=60.0, interval_s=3.0)
  File "check_workspaces_instances_delete_workspace_instance_state_transition_to_deallocated.py", line 58, in _poll_until
    raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
  AssertionError: condition not met within 60.0s; last=False
(_role_visible returns False exactly when GetRole raises error code NoSuchEntity; any other error code is re-raised, and none was, so the refusal observed is NoSuchEntity.)

Message-text note: the requirement here is the role NAME and GetRole's 200, not a message. "AWSServiceRoleFor" is the fixed service-provided prefix and "WorkspacesInstances" is AWS's published label for the principal workspaces-instances.amazonaws.com; the emulator instead emits the prefix plus a transform of the request's own AWSServiceName value (first label capitalised, hyphen retained) giving AWSServiceRoleForWorkspaces-instances. No per-run value (id, timestamp, request id) is part of this assertion.

Code: services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_service_linked_role.rs lines 117-149 (derived_service_linked_role_name / service_linked_role_name), reached because the principal is absent from crate::state::PUBLISHED_SERVICE_LINKED_ROLE_NAMES.

## Steps to Reproduce
Boot the multi-service emulator with iam and workspaces-instances mounted, then:

1. iam:CreateServiceLinkedRole(AWSServiceName="workspaces-instances.amazonaws.com") — observe HTTP 200 (or InvalidInput if a prior run already created it).
2. iam:GetRole(RoleName="AWSServiceRoleForWorkspacesInstances") — expect HTTP 200 with the Role; observe HTTP 404 NoSuchEntity, repeatedly, with no convergence.
3. iam:ListRoles(PathPrefix="/aws-service-role/workspaces-instances.amazonaws.com/") — the role is present but named "AWSServiceRoleForWorkspaces-instances".
4. Consequence: re-run the behavioral check workspaces-instances.delete-workspace-instance-state-transition-to-deallocated; it dies in its 60s GetRole poll before CreateWorkspaceInstance, so the ALLOCATED -> DEALLOCATING -> DEALLOCATED transition is never asserted. After fixing step 2, the remainder of that check must also pass.
