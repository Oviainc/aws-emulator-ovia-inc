Issue-Source: trace-sweep-divergence
Issue-Trace: iam-error-paths-nonexistent-user
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-error-paths-nonexistent-user iam-user-mutation-guard-clauses-on-nonexistent-principal iam-user-mutation-not-found-error-paths iam-user-not-found-error-paths iam-user-policy-and-boundary-not-found-errors iam-user-policy-and-permissions-boundary-not-found iam-user-policy-operations-not-found-errors iam-user-policy-operations-reject-nonexistent-user iam-detach-user-policy-contract iam-list-policy-versions-default-and-error-contract iam-saml-and-oidc-provider-untag-lifecycle iam-service-linked-role-deletion-status-and-acquire-role-error iam-user-error-paths-nonexistent
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|PutUserPermissionsBoundary|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|PutUserPermissionsBoundary|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|DeleteUserPermissionsBoundary|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|DeleteUserPermissionsBoundary|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|DetachUserPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|ListPolicyVersions|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|ListPolicyVersions|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|UntagOpenIDConnectProvider|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|UntagOpenIDConnectProvider|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|GetServiceLinkedRoleDeletionStatus|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|GetServiceLinkedRoleDeletionStatus|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|RemoveUserFromGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Related: 20261029T184359Z-iam-error-responses-omit-the-responsemetadata-requestid-element.md
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# IAM NoSuchEntity messages use emulator prose instead of AWS wording

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutUserPermissionsBoundary

## Description
Seven not-found paths return the right status (404) and the right Code (NoSuchEntity) but a message the emulator words itself. An error message is part of the wire contract, so each is a divergence.

Cause 1 of 6 - user-not-found on the permissions-boundary operations. AWS: 'User <userName> was not found.' - fixed text with the request's UserName substituted. Emulator: 'The user with name <userName> cannot be found.' Fix: the Put/DeleteUserPermissionsBoundary handlers' not-found message.
Traces: iam-error-paths-nonexistent-user#task#8 (PutUserPermissionsBoundary), #task#9 (DeleteUserPermissionsBoundary).

Cause 2 of 6 - DetachUserPolicy when the policy is not attached. AWS: 'Policy <policyArn> was not found.' - fixed text with the request's PolicyArn, and nothing about the user. Emulator: 'Policy <policyArn> was not found attached to user <userName>.' - it appends the user name, which AWS does not. Why: a client matching the message (or redacting it) sees an extra principal name leak and a text it cannot match. Fix: the DetachUserPolicy handler's message template.
Traces: iam-detach-user-policy-contract#task#6.

Cause 3 of 6 - ListPolicyVersions on a missing policy ARN. AWS: 'Policy <policyArn> does not exist or is not attachable.' Emulator: 'Policy <policyArn> was not found.' Fix: the policy-lookup not-found message on the ListPolicyVersions path. (Open issue 20261027T051038Z-iam-createpolicyversion-on-a-deleted-policy-arn-answers--r2 reports the same wording defect on CreatePolicyVersion; it does not cover ListPolicyVersions and carries no replay key for this step.)
Traces: iam-list-policy-versions-default-and-error-contract#task#4.

Cause 4 of 6 - UntagOpenIDConnectProvider on a missing provider. AWS: 'OpenId connect Provider <providerArn> cannot be found.' - note the exact casing 'OpenId connect Provider'. Emulator: 'OpenIDConnect Provider not found for arn <providerArn>'. Fix: the OIDC-provider lookup message. (Open issue 20261029T074728Z-iam-tag-untagopenidconnectprovider-return-the-wrong-nosuchentity-message reports this behaviorally; it carries no replay key for this fixture step.)
Traces: iam-saml-and-oidc-provider-untag-lifecycle#task#12.

Cause 5 of 6 - GetServiceLinkedRoleDeletionStatus with an unknown task id. AWS: the fixed text 'Cannot find deletion status for given id.' with no value interpolated. Emulator: 'The deletion task with ID <deletionTaskId> cannot be found.' - it echoes the submitted task id, which AWS does not. Fix: the deletion-status handler's not-found message.
Traces: iam-service-linked-role-deletion-status-and-acquire-role-error#task#3.

Cause 6 of 6 - RemoveUserFromGroup names the wrong entity. AWS: when the user does not exist, the message names the user - 'The user with name <userName> cannot be found.' Emulator: it names the group instead - 'The group with name <groupName> cannot be found.' - i.e. it reports the wrong missing entity for the request. Why: a caller is told to create a group that already exists while the real cause (the missing user) is invisible. Fix: the RemoveUserFromGroup handler must check and report the user before the group.
Traces: iam-user-error-paths-nonexistent#task#5.

## Evidence
Cause 1: expected 404 NoSuchEntity 'User <userName> was not found.'; actual 404 NoSuchEntity 'The user with name <userName> cannot be found.'
Cause 2: expected 'Policy <policyArn> was not found.'; actual 'Policy <policyArn> was not found attached to user <userName>.'
Cause 3: expected 'Policy <policyArn> does not exist or is not attachable.'; actual 'Policy <policyArn> was not found.'
Cause 4: expected 'OpenId connect Provider <providerArn> cannot be found.'; actual 'OpenIDConnect Provider not found for arn <providerArn>'.
Cause 5: expected 'Cannot find deletion status for given id.'; actual 'The deletion task with ID <deletionTaskId> cannot be found.'
Cause 6: expected 'The user with name <userName> cannot be found.'; actual 'The group with name <groupName> cannot be found.'
In every case the status (404) and Code (NoSuchEntity) match; only the Message differs. The names/ARNs shown as <...> above are taken from the request, not fixed text.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay iam-error-paths-nonexistent-user#task#8 - PutUserPermissionsBoundary against a user that does not exist; compare the 404 Message against the recording.
3. Replay each other binding in trace_ids and diff only the Message element.
