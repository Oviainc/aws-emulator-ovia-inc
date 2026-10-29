Issue-Source: trace-sweep-divergence
Issue-Trace: iam-acquire-role-backup-service-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-acquire-role-backup-service-lifecycle iam-acquire-role-power-user-lifecycle
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|GetRole|2xx->4xx|$.GetRoleResponse
Issue-Key: iam|GetRole|2xx->4xx|status
Issue-Key: iam|ListAttachedRolePolicies|2xx->4xx|$.ListAttachedRolePoliciesResponse
Issue-Key: iam|ListAttachedRolePolicies|2xx->4xx|status
Issue-Key: iam|DetachRolePolicy|2xx->4xx|$.DetachRolePolicyResponse
Issue-Key: iam|DetachRolePolicy|2xx->4xx|status
Issue-Key: iam|ListRolePolicies|2xx->4xx|$.ListRolePoliciesResponse
Issue-Key: iam|ListRolePolicies|2xx->4xx|status
Issue-Key: iam|DeleteRole|2xx->4xx|$.DeleteRoleResponse
Issue-Key: iam|DeleteRole|2xx->4xx|status
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# Role templates are not provisioned so dependent role operations answer 404

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRole

## Description
Cause 1 of 1: a role acquired from an AWS role template is never created in IAM state, so every operation that names it afterwards answers 404 NoSuchEntity instead of succeeding.

AWS: acquiring a role from a role template materialises a real IAM role in the account. The recordings show GetRole on the acquired role answering 200 with GetRoleResult/Role carrying Path (e.g. '/service-role/'), RoleName, RoleId, Arn, CreateDate, MaxSessionDuration, the template's AssumeRolePolicyDocument, the template's Description, and SourceRoleTemplate{TemplateArn, TemplateMinorVersion} - the member Smithy models as Role.SourceRoleTemplate, 'present only for roles created with' the role-template API. The template also attaches its managed policies, so ListAttachedRolePolicies answers 200 with the template's AttachedPolicies (for the AWS Backup default service role: AWSBackupServiceRolePolicyForBackup and AWSBackupServiceRolePolicyForRestores), ListRolePolicies answers 200 with an empty PolicyNames, and DetachRolePolicy / DeleteRole then succeed with 200.

Emulator: the role does not exist, so GetRole, ListAttachedRolePolicies, ListRolePolicies, DetachRolePolicy and DeleteRole all answer 404 - nine clusters, every step after the acquisition in both role-template fixtures.

Why: a client that provisions a service role from a template and then reads it back, attaches to it, or tears it down gets a 404 on a role AWS reports as existing. Nothing downstream of template-based provisioning can work, and the teardown path cannot clean up either.

Fix: the role-template acquisition handler must create the role in IAM role state - name, path, ARN, generated RoleId, CreateDate, MaxSessionDuration, the template's trust policy and Description, the SourceRoleTemplate member - and attach the template's managed policies, so the existing GetRole/ListAttachedRolePolicies/DetachRolePolicy/DeleteRole handlers find it.

Traces: iam-acquire-role-backup-service-lifecycle#task#2 (GetRole), #task#3 (ListAttachedRolePolicies), #teardown#5 (DetachRolePolicy), #teardown#7 (ListRolePolicies), #teardown#8 (DeleteRole); iam-acquire-role-power-user-lifecycle#teardown#5, #teardown#6, #teardown#7, #teardown#8 (the same four on the power-user template).

## Evidence
Expected (recorded AWS, GetRole on the acquired backup service role): HTTP 200, GetRoleResult/Role{Path '/service-role/', RoleName <acquired role name>, RoleId, Arn, CreateDate, MaxSessionDuration '3600', AssumeRolePolicyDocument <template trust policy>, Description 'Standard service role the provides AWS Backup service permission to create and restore backups on your behalf across AWS services', SourceRoleTemplate{TemplateArn 'arn:aws:iam::aws:role-template/backup.amazonaws.com/AWSBackupDefaultServiceRoleTemplate:1', TemplateMinorVersion '0'}, RoleLastUsed}. Actual: HTTP 404 with no GetRoleResponse (`$.GetRoleResponse: missing (expected {...}, got None)`).
Expected ListAttachedRolePolicies: HTTP 200, AttachedPolicies/member[AWSBackupServiceRolePolicyForRestores, AWSBackupServiceRolePolicyForBackup]; actual HTTP 404.
Expected ListRolePolicies: HTTP 200, IsTruncated 'false', PolicyNames empty; actual HTTP 404.
Expected DetachRolePolicy and DeleteRole: HTTP 200 with empty results; actual HTTP 404 for both.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay iam-acquire-role-backup-service-lifecycle: the acquisition step runs, then task#2 GetRole on the acquired role name -> 404 instead of 200.
3. Continue the fixture: task#3 ListAttachedRolePolicies, teardown#5 DetachRolePolicy, teardown#7 ListRolePolicies, teardown#8 DeleteRole - all 404.
4. Replay iam-acquire-role-power-user-lifecycle teardown#5..#8 for the same cascade on the power-user template.
