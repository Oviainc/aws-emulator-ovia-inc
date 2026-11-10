Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ram.disassociate-last-permission-from-share-rejected
Issue-Shape-Id: com.amazonaws.ram#AmazonResourceSharing
Issue-Key: ram.disassociate-last-permission-from-share-rejected|_wait_permission_status#16|Timeout|time.sleep(2.0)
Issue-Commit: 122d2e23ed306dec374f1768a21e001304c75e9d

# RAM ListResourceSharePermissions never reports a share-attachment status (ASSOCIATED) for a share's attached managed permission, so the permission-association lifecycle is unreachable and the check exhausts its 60s budget

> _Note: this issue was identified against the code base at commit `122d2e23ed306dec374f1768a21e001304c75e9d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListResourceSharePermissions

## Description
One cause (Cause 1 of 1), grounded in the failing behavioral check ram.disassociate-last-permission-from-share-rejected ([mismatch], "Check timed out after 60.0s").

NOTE ON OVERLAP: the open issue 20261108T153647Z-ram-listresourcesharepermissions-reports-a-share-s-attached-managed.md describes the same code-level root cause and even names this check in its Issue-Trace-Ids, but its Issue-Key pins the OLD failure signature (an AssertionError raised in _wait_permission_status), and the check now fails with a harness timeout instead — so the suite did not mark this failure as OWNED and it is re-filed here, grounded in the current run, rather than dropped. A fixer may close both work items with one fix; nothing else in this run is net-new.

Cause 1 of 1: ListResourceSharePermissions projects the status of the permission VERSION (its own attachability lifecycle) onto ResourceSharePermissionSummary$status instead of the status of that permission's ATTACHMENT to the resource share being read, and there is no settling path for an attachment at all. The value is therefore static, so a client that polls a share's permissions for the attachment to be reported ASSOCIATED never terminates; in this run the whole 60s check budget is consumed by that poll and the check is killed before it can even reach the DisassociateResourceSharePermission refusal it is named for. Failing check: ram.disassociate-last-permission-from-share-rejected.
  AWS: ListResourceSharePermissions "lists the RAM permissions that are associated with a resource share"; POST /listresourcesharepermissions answers 200 with one ResourceSharePermissionSummary per managed permission attached to the share named by the required resourceShareArn. When a share is created with a resource whose type has an AWS-managed default permission, RAM attaches that default managed permission to the share and reports it with permissionType AWS_MANAGED, isResourceTypeDefault true, defaultVersion true, featureSet STANDARD, the attached version as a string, and status ASSOCIATED — the status of the permission's association with THAT share, which is what tells an attached permission apart from one that is merely attachable. Smithy (com.amazonaws.ram): ListResourceSharePermissions input {resourceShareArn required, nextToken, maxResults @range 1..500}, output {permissions: ResourceSharePermissionList, nextToken}; ResourceSharePermissionSummary members {arn, version ("The version of the permission associated with this resource share"), defaultVersion, name, resourceType, status ("The current status of the permission"), creationTime, lastUpdatedTime, isResourceTypeDefault, permissionType (AWS_MANAGED|CUSTOMER_MANAGED), featureSet, tags}. The model does not enumerate the value set for status, so the concrete value ASSOCIATED for an attached permission is recorded behavior only: the recording shows the share's attached permission settling to status == "ASSOCIATED", which the check polls for.
  Emulator: the read answers HTTP 200 and does list the auto-attached AWS-managed default permission for the shared resource type, with every other member agreeing with AWS, but reports status "ATTACHABLE" for it — the stored permission VERSION's own PermissionStatus ("whether this version can be associated with shares"). No attachment status is stored or settled anywhere, so the value never changes with time: a poll over the full window sees "ATTACHABLE" on every attempt. Because the emulator's poll window here equals the check budget, the replay reports "Check timed out after 60.0s" instead of the assertion it would otherwise raise (the previous run recorded: permission never reached status 'ASSOCIATED', last seen status 'ATTACHABLE'). Everything after that wait in the lifecycle — associating a replacement permission, and the last-permission DisassociateResourceSharePermission refusal (InvalidParameterException) the check is named for — is never exercised, so those remain unverified rather than known-good.
  Why: status is the only member that tells a client whether a managed permission is attached to a share or merely attachable to it, and ATTACHABLE is a value AWS uses for a permission version's availability, not for a share attachment. A client that enumerates a share's permissions and gates on status == "ASSOCIATED" — to decide whether to associate, replace, audit or disassociate the permission governing a share, or to wait for an association/replace to settle — takes the wrong branch forever or hangs until its own timeout. Since the value is static, retrying cannot recover, so the share's whole permission-association lifecycle is unreachable through the public read, and callers also see their call budgets burned by polls that can never converge.
  Fix: the share-permission association state must record, per (share, permission) attachment, the status of that attachment plus the owed-move/settling path to ASSOCIATED the service already uses for resource and principal associations, and the ListResourceSharePermissions handler must project THAT status onto ResourceSharePermissionSummary$status. The summary projection is shared with permission reads where the version's own PermissionStatus IS correct (GetPermission, ListPermissionVersions describe a permission version, not a share attachment), so the share-scoped read needs its own status source rather than a change to the version record.
  Traces: behavioral check ram.disassociate-last-permission-from-share-rejected.

## Evidence
Failing check: ram.disassociate-last-permission-from-share-rejected — verdict [mismatch], detail: "Check timed out after 60.0s".

What the check does before it hangs: creates a customer-managed permission for resourceType codebuild:Project, creates an IAM service role and a CodeBuild project, creates a resource share naming that project, waits for the RESOURCE association to reach ASSOCIATED (this succeeds — resource associations do settle), then polls ListResourceSharePermissions for the share's auto-attached AWS-managed default permission to be reported ASSOCIATED (_wait_permission_status, 60s cap). That poll can never converge, and it consumes the check's entire 60s budget, so the run ends in a timeout.

Diverging call — ram:ListResourceSharePermissions(resourceShareArn=<the created share ARN>):
  expected status 200, actual status 200 (the status agrees; the body does not)
  expected $.permissions[0].status = "ASSOCIATED"   (status of the permission's attachment to the share being read)
  actual   $.permissions[0].status = "ATTACHABLE"   (the permission version's own PermissionStatus)
  every other member of $.permissions[0] agrees with AWS: arn/name = the resource type's AWS-managed default permission, resourceType = the shared resource's type, version = the attached version as a string, defaultVersion = true, isResourceTypeDefault = true, permissionType = "AWS_MANAGED", featureSet = "STANDARD", tags = [].
  The value is static across the whole poll window, so this is a wrong value, not a slow transition.

Requirement vs. per-run values: the requirement is that a managed permission currently attached to the share is reported with the share-attachment status (ASSOCIATED), and that the attachment settles so a poll terminates. The permission ARN/name (AWSRAMDefaultPermission<ResourceType>), the resource type (codebuild:Project in this run), the version string and the two timestamps are values taken from the request and the emulator's state and are not part of the contract.

Blocked-by-this (not independently verified, so not filed as defects): the replacement association and the last-permission DisassociateResourceSharePermission refusal — AWS refuses removing the only permission a share's resources depend on with InvalidParameterException and the message "The permission <permissionArn> cannot be disassociated because it is the last permission being used by resources of type <resourceType> in the share." (fixed text as quoted; <permissionArn> and <resourceType> come from the request and the share's state) — are never reached.

Code the behavior comes from (reference only):
  services/service-com-amazonaws-ram-amazonresourcesharing/src/state.rs:757-766 — PermissionRecord::summary() sets status: Some(version.status.clone()), i.e. the version's attachability status
  services/service-com-amazonaws-ram-amazonresourcesharing/src/state.rs:331 — PERMISSION_STATUS_ATTACHABLE, the only permission status the store ever holds
  services/service-com-amazonaws-ram-amazonresourcesharing/src/state.rs:968-993 — Store::share_permissions(), which yields PermissionRecords and carries no per-attachment status at all
  services/service-com-amazonaws-ram-amazonresourcesharing/src/ops/list_resource_share_permissions.rs:232-253 — the handler reads share_permissions() and pushes permission.summary(version) straight onto the wire
  services/service-com-amazonaws-ram-amazonresourcesharing/src/state.rs:873-897 — settle_due_associations(), the owed-move settling path that exists for resource/principal associations and has no counterpart for permission attachments

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (simparity-behavioral-aws-v1).
2. Run check ram.disassociate-last-permission-from-share-rejected; it ends in "Check timed out after 60.0s". Minimal reproduction of the diverging call:
   a. codebuild:CreateProject (any valid project, with an IAM service role) to obtain a shareable resource ARN of type codebuild:Project.
   b. ram:CreateResourceShare with resourceArns=[<that project ARN>] — RAM attaches the resource type's AWS-managed default permission to the new share.
   c. Poll ram:GetResourceShareAssociations(associationType='RESOURCE', resourceShareArns=[<share ARN>]) until the resource association is ASSOCIATED (this already works).
   d. Call ram:ListResourceSharePermissions(resourceShareArn=<share ARN>) repeatedly for 60s.
3. Observe: 200 with exactly one permission — the AWS-managed default for the shared resource type — whose status is "ATTACHABLE" on every attempt. AWS reports status "ASSOCIATED" for a permission attached to the share, so the poll never terminates.
4. After the fix, re-replay ram.disassociate-last-permission-from-share-rejected: it must get past the wait for the managed default to be ASSOCIATED within the check budget, the share's permission reads must report the attachment status throughout the associate/replace/disassociate lifecycle, and the check must then reach (and satisfy) the last-permission disassociation refusal it is named for.
