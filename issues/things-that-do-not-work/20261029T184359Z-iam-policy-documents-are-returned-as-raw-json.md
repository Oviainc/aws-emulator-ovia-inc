Issue-Source: trace-sweep-divergence
Issue-Trace: iam-err-malformed-policy-put-user-policy
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-err-malformed-policy-put-user-policy iam-get-user-policy-document-round-trip-and-not-found iam-put-user-policy-overwrite-round-trip-and-not-found iam-get-policy-version-round-trip-and-nonexistent-version-rejected iam-managed-policy-lifecycle iam-managed-policy-version-lifecycle-tags-and-entities iam-managed-policy-versioning-and-tags iam-group-inline-and-managed-policies iam-group-lifecycle-and-policies iam-group-lifecycle-with-inline-and-attached-policies iam-role-lifecycle-and-policies iam-role-lifecycle-inline-managed-policy-and-boundary iam-err-malformed-policy-trust-policy iam-err-service-linked-role-unmodifiable iam-permissions-boundary-and-policy-simulation iam-role-update-lifecycle iam-instance-profile-lifecycle iam-instance-profile-role-association-and-tags iam-role-and-user-tag-lifecycle iam-service-access-analysis-and-role-template-reports iam-list-roles-path-prefix-scoping-and-malformed-error iam-pg-list-roles-marker-walk iam-service-linked-role-deletion-status-and-acquire-role-error iam-service-linked-role-lifecycle iam-service-linked-role-supported-services
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|GetUserPolicy|2xx->2xx|$.GetUserPolicyResponse.GetUserPolicyResult.PolicyDocument
Issue-Key: iam|GetPolicyVersion|2xx->2xx|$.GetPolicyVersionResponse.GetPolicyVersionResult.PolicyVersion.Document
Issue-Key: iam|GetGroupPolicy|2xx->2xx|$.GetGroupPolicyResponse.GetGroupPolicyResult.PolicyDocument
Issue-Key: iam|GetRolePolicy|2xx->2xx|$.GetRolePolicyResponse.GetRolePolicyResult.PolicyDocument
Issue-Key: iam|GetRole|2xx->2xx|$.GetRoleResponse.GetRoleResult.Role.AssumeRolePolicyDocument
Issue-Key: iam|GetRole|2xx->2xx|$.GetRoleResponse.GetRoleResult.Role.Description
Issue-Key: iam|GetRole|2xx->2xx|$.GetRoleResponse.GetRoleResult.Role.RoleId
Issue-Key: iam|GetRole|2xx->2xx|$.GetRoleResponse.GetRoleResult.Role.RoleLastUsed
Issue-Key: iam|UpdateRoleDescription|2xx->2xx|$.UpdateRoleDescriptionResponse.UpdateRoleDescriptionResult.Role.AssumeRolePolicyDocument
Issue-Key: iam|GetInstanceProfile|2xx->2xx|$.GetInstanceProfileResponse.GetInstanceProfileResult.InstanceProfile.Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|GetInstanceProfile|2xx->2xx|$.GetInstanceProfileResponse.GetInstanceProfileResult.InstanceProfile.Tags
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member.Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member.Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|CreateRole|2xx->2xx|$.CreateRoleResponse.CreateRoleResult.Role.AssumeRolePolicyDocument
Issue-Key: iam|ListRoles|2xx->2xx|$.ListRolesResponse.ListRolesResult.Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|GetRole|2xx->2xx|$.GetRoleResponse.GetRoleResult.Role.PermissionsBoundary.PermissionsBoundaryType
Issue-Key: iam|CreateServiceLinkedRole|2xx->2xx|$.CreateServiceLinkedRoleResponse.CreateServiceLinkedRoleResult.Role.AssumeRolePolicyDocument
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# IAM policy documents are returned as raw JSON not URL-encoded

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetUserPolicy

## Description
Cause 1 of 1: every response member whose target is the IAM policyDocumentType is serialised as raw JSON instead of RFC-3986 URL-encoded text. 15 replayed clusters over 11 operations differ only in this.

AWS: IAM returns policy documents URL-encoded. Smithy: PolicyVersion.Document (target policyDocumentType) documents it explicitly - 'The policy document returned in this structure is URL-encoded compliant with RFC 3986. You can use a URL decoding method to convert the policy back to plain JSON text.' The same encoding applies to the inline-policy document members (GetUserPolicy/GetGroupPolicy/GetRolePolicy PolicyDocument) and to Role.AssumeRolePolicyDocument (target policyDocumentType) wherever a Role is embedded - GetRole, CreateRole, ListRoles, UpdateRoleDescription, CreateServiceLinkedRole, GetInstanceProfile, ListInstanceProfiles, ListInstanceProfilesForRole. The requirement is the transform, not any one document: percent-encode the stored JSON (so '{' -> %7B, '"' -> %22, ':' -> %3A, ',' -> %2C, '[' -> %5B, '*' -> %2A, and a space -> %20), byte-for-byte preserving whatever whitespace the caller submitted.

Emulator: the stored JSON text is emitted verbatim, e.g. GetUserPolicy returns '{"Version":"2012-10-17","Statement":[...]}' where AWS returns '%7B%22Version%22%3A%222012-10-17%22...'.

Why: an SDK or client that URL-decodes the member (which is what the documented contract requires) corrupts the document - percent sequences are absent so decoding is a no-op for most inputs but any literal '%' or '+' in the document is mangled, and strict clients that validate the member against the documented encoding reject the response outright. Round-tripping a policy through Get -> edit -> Put therefore cannot be done against the emulator the way it is done against AWS.

Fix: the IAM response serializer for policyDocumentType-targeted members - one encode step applied wherever a document is written out, rather than per handler.

Traces: representative iam-err-malformed-policy-put-user-policy#task#7 (GetUserPolicy); the other 14 cluster representatives are in trace_ids and cover GetPolicyVersion, GetGroupPolicy, GetRolePolicy, GetRole, CreateRole, ListRoles, UpdateRoleDescription, CreateServiceLinkedRole, GetInstanceProfile, ListInstanceProfiles and ListInstanceProfilesForRole.

## Evidence
Expected (recorded AWS, GetUserPolicy): PolicyDocument = '%7B%22Version%22%3A%222012-10-17%22%2C%22Statement%22%3A%5B%7B%22Effect%22%3A%22Allow%22%2C%22Action%22%3A%22s3%3AGetObject%22%2C%22Resource%22%3A%22%2A%22%7D%5D%7D'.
Actual (emulator): PolicyDocument = '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}'.
Same divergence on Role.AssumeRolePolicyDocument: expected '%7B%22Version%22%3A%20%222012-10-17%22%2C%20%22Statement%22%3A%20%5B...' (note %20 for the submitted spaces), actual '{"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Principal": {"Service": "ec2.amazonaws.com"}, "Action": "sts:AssumeRole"}]}' - reported by CreateRole (iam-instance-profile-role-association-and-tags#setup#1), GetRole (iam-err-service-linked-role-unmodifiable#task#2), ListRoles (iam-list-roles-path-prefix-scoping-and-malformed-error#task#2), GetInstanceProfile (iam-instance-profile-lifecycle#task#4), CreateServiceLinkedRole (iam-service-linked-role-deletion-status-and-acquire-role-error#task#1) and GetPolicyVersion (iam-get-policy-version-round-trip-and-nonexistent-version-rejected#task#2).

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay iam-err-malformed-policy-put-user-policy: task#5-6 PutUserPolicy with a JSON document, then task#7 GetUserPolicy.
3. Compare PolicyDocument against the recording: the emulator returns raw JSON, AWS returns the percent-encoded form.
4. Repeat with any binding in trace_ids - all policy-document and AssumeRolePolicyDocument members reproduce it.
