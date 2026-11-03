Issue-Source: trace-sweep-divergence
Issue-Trace: ram-customer-managed-permission-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ram-customer-managed-permission-lifecycle ram-permission-error-paths ram-permission-tagging ram-promotion-error-paths ram-resource-share-permission-association
Issue-Shape-Id: com.amazonaws.ram#AmazonResourceSharing
Issue-Key: ram|GetPermission|2xx->2xx|$.permission.permission
Issue-Key: ram|GetPermission|2xx->2xx|$.permission.version
Issue-Key: ram|GetPermission|2xx->2xx|$.permission.status
Issue-Commit: d36bb525c2331ae923943e9e5ae02c5704a8c1d0

# RAM GetPermission reports stale permission state after version creation and deletion

> _Note: this issue was identified against the code base at commit `d36bb525c2331ae923943e9e5ae02c5704a8c1d0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPermission

## Description
GetPermission on a customer-managed permission keeps answering the permission as it was at CreatePermission time: neither a newly created version nor a delete is ever visible in the read path. 3 causes.

Cause 1 of 3 — a new permission version is not reflected (fixture ram-customer-managed-permission-lifecycle, step task#6).
AWS: CreatePermissionVersion (Smithy: CreatePermissionVersion, input permissionArn+policyTemplate, output ResourceSharePermissionDetail) adds a version to a CUSTOMER_MANAGED permission; a subsequent GetPermission (Smithy: GetPermission -> ResourceSharePermissionDetail with members version:String and permission:String, 'The permission's effect and actions in JSON format') returns the new version number and the policy document of the version it describes. After one additional version is created the detail's version is the incremented version number and permission is the newly supplied policy template (in the recording: version '2' and a policy whose Action list contains both imagebuilder:ListComponents and imagebuilder:GetComponent).
Emulator: GetPermission answers HTTP 200 but with version '1' and the version-1 policy (Action list holding only imagebuilder:ListComponents) — the version write never reached the state the read serves.
Why: a client that creates a permission version and then reads the permission back sees its new actions silently missing, so it associates resource shares with a policy it believes it replaced; version pinning (GetPermission.permissionVersion) and any 'is my edit live' check are unusable.
Hypothesis: the version-creating write is not implemented at all — open issue 20261103T040218Z-ram-createpermissionversion-stub.md reports ram CreatePermissionVersion as an unimplemented scaffold stub. That issue does not cover this finding: it is about the write returning 501, whereas this cause is the read path serving a stale ResourceSharePermissionDetail (HTTP 200 with wrong version/permission), which must be fixed for GetPermission once versions exist.
Fix: the ram permission state (version list per permission, default/current version pointer) plus the GetPermission handler's selection of which version's policy to project into the response.

Cause 2 of 3 — after DeletePermission the permission never reaches DELETED and still reports the pre-delete version/policy (fixture ram-customer-managed-permission-lifecycle, step teardown#10).
AWS: DeletePermission (Smithy: DeletePermission, output permissionStatus: PermissionStatus) moves a customer-managed permission through DELETING to DELETED (Smithy enum PermissionStatus = ATTACHABLE | UNATTACHABLE | DELETING | DELETED; member doc: 'DELETED – This permission or version is deleted'). GetPermission on the deleted permission therefore reports status DELETED, and the detail reflects the latest version (version '2' and its policy in the recording).
Emulator: GetPermission answers 200 with status 'ATTACHABLE' and never leaves it — the fixture polled 10 times and got ATTACHABLE every time — and still reports version '1' with the version-1 policy.
Why: the delete is a no-op as far as any reader can tell. A client waiting for DELETED waits forever, and a permission the caller believes is gone remains ATTACHABLE, i.e. still associable with resource shares.
Fix: the DeletePermission handler's state transition (set DELETING then DELETED on the stored permission) and the GetPermission handler's status projection.

Cause 3 of 3 — the same non-transition on the shared teardown path, independent of versions (fixture ram-permission-error-paths, step teardown#5; same cluster also covers ram-permission-tagging#teardown#8, ram-promotion-error-paths#teardown#10, ram-resource-share-permission-association#teardown#12).
AWS: after DeletePermission succeeds, GetPermission reports permission.status DELETED (Smithy PermissionStatus enum, as above).
Emulator: GetPermission answers 200 with status 'ATTACHABLE', still ATTACHABLE after 10 poll attempts.
Why: every lifecycle that cleans up a permission it created cannot confirm the deletion, and a deleted permission stays usable in emulator state across four separate fixtures.
Fix: as cause 2 — DeletePermission's state transition in the ram permission store, applied for permissions with and without extra versions.

Traces: ram-customer-managed-permission-lifecycle#task#6; ram-customer-managed-permission-lifecycle#teardown#10; ram-permission-error-paths#teardown#5 (plus ram-permission-tagging#teardown#8, ram-promotion-error-paths#teardown#10, ram-resource-share-permission-association#teardown#12 in the same cluster).

## Evidence
Cause 1 — ram-customer-managed-permission-lifecycle#task#6, GetPermission, expected 200 / actual 200: $.permission.version expected '2', got '1'; $.permission.permission expected '{"Effect": "Allow", "Action": ["imagebuilder:ListComponents", "imagebuilder:GetComponent"]}', got '{"Effect": "Allow", "Action": ["imagebuilder:ListComponents"]}'.
Cause 2 — ram-customer-managed-permission-lifecycle#teardown#10, GetPermission, expected 200 / actual 200: $.permission.status expected 'DELETED', got 'ATTACHABLE' (poll: not in ['DELETED'] after 10 attempts); $.permission.version expected '2', got '1'; $.permission.permission expected the two-action policy, got the one-action policy.
Cause 3 — ram-permission-error-paths#teardown#5, GetPermission, expected 200 / actual 200: $.permission.status expected 'DELETED', got 'ATTACHABLE' (poll: not in ['DELETED'] after 10 attempts). Affected count 4: ram-permission-error-paths#teardown#5, ram-permission-tagging#teardown#8, ram-promotion-error-paths#teardown#10, ram-resource-share-permission-association#teardown#12.

## Steps to Reproduce
1. CreatePermission with a customer-managed policy template for resourceType imagebuilder:Component (one action).
2. CreatePermissionVersion on that permission ARN with a policy template adding a second action.
3. GetPermission on the permission ARN -> emulator returns version '1' and the single-action policy instead of version '2' and the two-action policy (ram-customer-managed-permission-lifecycle#task#6).
4. DeletePermission on the permission ARN, then poll GetPermission -> permission.status stays 'ATTACHABLE' forever instead of becoming 'DELETED' (ram-customer-managed-permission-lifecycle#teardown#10, ram-permission-error-paths#teardown#5).
