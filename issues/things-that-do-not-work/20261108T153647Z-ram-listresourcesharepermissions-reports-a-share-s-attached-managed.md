Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ram.disassociate-last-permission-from-share-rejected
Issue-Shape-Id: com.amazonaws.ram#AmazonResourceSharing
Issue-Key: ram.disassociate-last-permission-from-share-rejected|_wait_permission_status#16|AssertionError|raise AssertionError(f"permission never reached arn={permission_arn!r} status={status!r}, last seen {last!r}")
Issue-Commit: 1c28ff50845be100223fd48633262341298388e5

# RAM ListResourceSharePermissions reports a share's attached managed permission with the permission-version status (ATTACHABLE) instead of the share-association status (ASSOCIATED)

> _Note: this issue was identified against the code base at commit `1c28ff50845be100223fd48633262341298388e5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListResourceSharePermissions

## Description
One cause (Cause 1 of 1), grounded in the failing behavioral check ram.disassociate-last-permission-from-share-rejected.

Cause 1 of 1: ListResourceSharePermissions projects the status of the permission VERSION (its own attachability lifecycle) onto `ResourceSharePermissionSummary$status`, rather than the status of that permission's ATTACHMENT to the resource share being read. A caller therefore sees a permission that is in fact attached to the share reported as ATTACHABLE forever, and never as ASSOCIATED.
  AWS: ListResourceSharePermissions "lists the RAM permissions that are associated with a resource share" and answers 200 with one ResourceSharePermissionSummary per managed permission attached to the share named by the required `resourceShareArn`. When a resource share is created with a resource whose type has an AWS-managed default permission, RAM attaches that default managed permission to the share, and a read of the share's permissions reports it with `permissionType: AWS_MANAGED`, `isResourceTypeDefault: true`, `defaultVersion: true`, `featureSet: STANDARD`, the attached `version` as a string, and `status` reported as `ASSOCIATED` — the status of the permission's association with THAT share, which is what distinguishes an attached permission from one that is merely available to attach. Smithy: ResourceSharePermissionSummary members {arn, version, defaultVersion, name, resourceType, status, creationTime, lastUpdatedTime, isResourceTypeDefault, permissionType (AWS_MANAGED|CUSTOMER_MANAGED), featureSet, tags}; `status` is documented only as "The current status of the permission", and `version` as "The version of the permission associated with this resource share" — the model does not enumerate the value set, so the concrete value ASSOCIATED here is recorded behavior only (the check asserts the share's attached permission settles to `status == 'ASSOCIATED'`, polling for it).
  Emulator: the read answers HTTP 200 and does list the auto-attached AWS-managed default permission for the shared resource type, but reports `"status": "ATTACHABLE"` for it — the PermissionVersionRecord's own `status` ("A `PermissionStatus`: whether this version can be associated with shares") — and never any share-association status. The value does not change with time: there is no settling path, so a client polling for the attachment to be reported ASSOCIATED polls until its own cap and fails. In the replay the summary was `{arn: arn:aws:ram::aws:permission/AWSRAMDefaultPermission<ResourceType>, version: '1', defaultVersion: true, name: AWSRAMDefaultPermission<ResourceType>, resourceType: codebuild:Project, status: 'ATTACHABLE', isResourceTypeDefault: true, permissionType: 'AWS_MANAGED', featureSet: 'STANDARD', tags: []}` — every member agrees with AWS except `status`.
  Why: `status` is the only member that tells a client whether a managed permission is attached to the share or merely attachable to it, and ATTACHABLE is a value AWS uses for a permission version's own availability, not for a share attachment. Any client that enumerates a share's permissions and gates on `status == 'ASSOCIATED'` — to decide whether to associate, replace or disassociate a permission, to wait for an association or a replace to settle, or to audit which permission governs a share — takes the wrong branch forever or never terminates. Because the value is static, retrying cannot recover: the whole permission-association lifecycle of a share is unreachable through the public read.
  Fix: the share-permission association state must record, per (share, permission) attachment, the status of that attachment (and the settling path to ASSOCIATED that the service already uses for resource and principal associations), and the ListResourceSharePermissions handler must project THAT status onto ResourceSharePermissionSummary$status instead of the permission version's `status`. Note the summary projection is shared with other permission reads, where the version's own PermissionStatus IS the right value (GetPermission/ListPermissionVersions describe a permission version, not a share attachment), so the share-scoped read needs its own status source rather than a change to the version record.
  Traces: behavioral check ram.disassociate-last-permission-from-share-rejected (it fails at the first wait for the share's auto-attached managed default to be reported ASSOCIATED, before it ever reaches the disassociation refusal it is named for).

Related open issue that does NOT cover this check: 20261108T035842Z-ram-listresourcesharepermissions-reports-no-managed-permission-for-a.md is a WIRE issue about the share's permission SET being write-blind (an associated permission never appearing in the list at all, `{"permissions": []}`) for resource type imagebuilder:Component. Here the permission IS listed — the body is non-empty and every other member is right — and the divergence is purely the status value projected for an attachment that exists from share creation, with no AssociateResourceSharePermission call involved. It carries this check's replay key in neither its Issue-Key nor its Issue-Trace-Ids, and the suite did not mark this check as owned by it. If a fixer closes that issue by making the set write-aware without changing the status source, this check still fails.

The remaining failures of this suite run are all marked ALREADY FILED against unimplemented scaffold stubs (AssociateResourceSharePermission, CreatePermissionVersion, ListPermissions) and are not re-filed here.

## Evidence
Failing check: ram.disassociate-last-permission-from-share-rejected ([mismatch])

The check creates a customer-managed permission for resource type codebuild:Project, creates a CodeBuild project, creates a resource share naming that project, waits for the RESOURCE association to reach ASSOCIATED (this succeeds), then waits for the share's auto-attached AWS-managed default permission to be reported ASSOCIATED by ListResourceSharePermissions. That wait never settles:

  AssertionError: permission never reached arn='arn:aws:ram::aws:permission/AWSRAMDefaultPermissionCodeBuildProject' status='ASSOCIATED', last seen
  [{'arn': 'arn:aws:ram::aws:permission/AWSRAMDefaultPermissionCodeBuildProject',
    'version': '1', 'defaultVersion': True,
    'name': 'AWSRAMDefaultPermissionCodeBuildProject',
    'resourceType': 'codebuild:Project',
    'status': 'ATTACHABLE',            <-- expected 'ASSOCIATED'
    'creationTime': <timestamp>, 'lastUpdatedTime': <timestamp>,
    'isResourceTypeDefault': True, 'permissionType': 'AWS_MANAGED',
    'featureSet': 'STANDARD', 'tags': []}]

  HTTP status: 200 (agrees with AWS); the body's `permissions[0].status` is the single diverging member.
  expected $.permissions[0].status = "ASSOCIATED" (status of the permission's attachment to the share being read)
  actual   $.permissions[0].status = "ATTACHABLE" (the permission version's own PermissionStatus)
  Static across the full poll window (30 attempts over 60s), so this is a wrong value, not a slow transition.

Requirement vs. per-run values: the requirement is that a managed permission currently attached to the share is reported with the share-attachment status (ASSOCIATED). The permission ARN/name (AWSRAMDefaultPermission<ResourceType>), the resourceType (codebuild:Project), version '1' and the two timestamps are values taken from the request and the emulator's state in this run and are not part of the contract.

Code the behavior comes from (reference only):
  services/service-com-amazonaws-ram-amazonresourcesharing/src/state.rs:709 — PermissionVersionRecord::status, documented as "A `PermissionStatus`: whether this version can be associated with shares"
  services/service-com-amazonaws-ram-amazonresourcesharing/src/state.rs:757-766 — PermissionRecord::summary() sets `status: Some(version.status.clone())`, i.e. the version's attachability status
  services/service-com-amazonaws-ram-amazonresourcesharing/src/state.rs:968 — Store::share_permissions(), which yields PermissionRecords and carries no per-attachment status at all
  services/service-com-amazonaws-ram-amazonresourcesharing/src/ops/list_resource_share_permissions.rs:232-252 — the handler reads share_permissions() and pushes permission.summary(version) straight onto the wire

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (simparity-behavioral-aws-v1).
2. Run check ram.disassociate-last-permission-from-share-rejected. Minimal reproduction of the diverging call:
   a. codebuild:CreateProject (any valid project) to obtain a shareable resource ARN of type codebuild:Project.
   b. ram:CreateResourceShare with resourceArns=[<that project ARN>] — RAM attaches the resource type's AWS-managed default permission to the new share.
   c. Poll ram:GetResourceShareAssociations(associationType='RESOURCE', resourceShareArns=[<share ARN>]) until the resource association is ASSOCIATED (this already works).
   d. Call ram:ListResourceSharePermissions(resourceShareArn=<share ARN>).
3. Observe: 200 with exactly one permission, the AWS-managed default for codebuild:Project, but `status` = "ATTACHABLE". AWS reports `status` = "ASSOCIATED" for a permission attached to the share. Repeating the read for 60s never changes the value.
4. Re-replay ram.disassociate-last-permission-from-share-rejected after the fix: it must get past the first wait for the managed default to be ASSOCIATED, and the share's permission reads must report the attachment status throughout the associate/replace/disassociate lifecycle.
