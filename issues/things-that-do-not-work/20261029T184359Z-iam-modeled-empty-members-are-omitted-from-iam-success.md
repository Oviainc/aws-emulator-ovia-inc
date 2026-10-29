Issue-Source: trace-sweep-divergence
Issue-Trace: iam-err-delete-policy-blocked-by-attachment
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-err-delete-policy-blocked-by-attachment iam-managed-policy-lifecycle iam-managed-policy-version-lifecycle-tags-and-entities iam-managed-policy-versioning-and-tags iam-identity-providers-saml-and-oidc-batch6 iam-oidc-provider-lifecycle iam-oidc-provider-update-thumbprint iam-server-certificate-lifecycle iam-permissions-boundary-and-policy-simulation iam-policy-simulation-and-context-keys
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|GetPolicy|2xx->2xx|$.GetPolicyResponse.GetPolicyResult.Policy.Tags
Issue-Key: iam|GetOpenIDConnectProvider|2xx->2xx|$.GetOpenIDConnectProviderResponse.GetOpenIDConnectProviderResult.Tags
Issue-Key: iam|GetServerCertificate|2xx->2xx|$.GetServerCertificateResponse.GetServerCertificateResult.ServerCertificate.Tags
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member.MissingContextValues
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member.OrganizationsDecisionDetail
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member[*].MatchedStatements.member.EndPosition
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member[*].MatchedStatements.member.StartPosition
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member[*].MissingContextValues
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member[*].OrganizationsDecisionDetail
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# Modeled empty members are omitted from IAM success responses

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPolicy

## Description
Cause 1 of 4 - an empty Tags list is omitted instead of emitted as an empty element. AWS: Tags is a modelled member of the returned entity (Smithy: Role.Tags target tagListType length 0..50; GetSAMLProviderResponse.Tags likewise) and IAM emits the element even when the entity carries no tags, so the recorded responses carry an empty <Tags/>. Emulator: the element is absent - `$.<...>.Tags: missing (expected '', got None)` on GetPolicy, GetOpenIDConnectProvider, GetInstanceProfile and GetServerCertificate. Why: a client that reads response.Tags without a null guard (valid against AWS, where the member is always present) throws on an untagged entity. Fix: the IAM response serializer for tagListType members - emit the empty list element rather than skipping it.
Traces: iam-err-delete-policy-blocked-by-attachment#task#7 (GetPolicy); iam-identity-providers-saml-and-oidc-batch6#task#6 (GetOpenIDConnectProvider); iam-server-certificate-lifecycle#task#3 (GetServerCertificate); iam-instance-profile-lifecycle#task#4 (GetInstanceProfile).

Cause 2 of 4 - Role.RoleLastUsed is omitted. AWS: RoleLastUsed is a modelled Role member (Smithy: Role.RoleLastUsed target RoleLastUsed) and IAM emits it as an empty element for a role that has never been assumed; the recordings show <RoleLastUsed/> on every GetRole. Emulator: the member is absent - `Role.RoleLastUsed: missing (expected '', got None)`. Why: clients that branch on RoleLastUsed presence to decide whether a role is dormant cannot do so. Fix: the Role serializer in the IAM response path.
Traces: iam-err-service-linked-role-unmodifiable#task#2 and iam-permissions-boundary-and-policy-simulation#task#6 (GetRole); iam-err-malformed-policy-trust-policy#task#8 additionally shows Role.Description omitted after it was supplied on CreateRole.

Cause 3 of 4 - SimulatePrincipalPolicy drops modelled EvaluationResult members. AWS: EvaluationResult carries MissingContextValues (list), OrganizationsDecisionDetail (structure with AllowedByOrganizations) and, inside MatchedStatements, StartPosition/EndPosition (Line, Column of the matched statement in the policy text) - all modelled (Smithy: EvaluationResult.MissingContextValues, .OrganizationsDecisionDetail; Statement.StartPosition/.EndPosition). The recordings carry an empty <MissingContextValues/>, <OrganizationsDecisionDetail><AllowedByOrganizations>true</AllowedByOrganizations></OrganizationsDecisionDetail> and the two positions per matched statement. Emulator: all of them are absent. Why: a simulator client cannot tell 'no missing context keys' from 'field not reported', and cannot attribute a decision to a statement position. Fix: the SimulatePrincipalPolicy handler and its EvaluationResult serializer.
Traces: iam-permissions-boundary-and-policy-simulation#task#10; iam-policy-simulation-and-context-keys#task#5.

Cause 4 of 4 - PermissionsBoundaryType carries the enum member name instead of the wire value. AWS (recorded behavior only for the wire value; Smithy defines the enum PermissionsBoundaryAttachmentType whose single member is named PermissionsBoundaryPolicy): the recorded GetRole response serialises Role/PermissionsBoundary/PermissionsBoundaryType as 'Policy'. Emulator: it serialises 'PermissionsBoundaryPolicy', i.e. the member name rather than the wire value. Why: a client switching on the documented value 'Policy' never matches, so an attached boundary looks like an unknown boundary type. Fix: the AttachedPermissionsBoundary serializer - map the enum member to its wire value.
Traces: iam-permissions-boundary-and-policy-simulation#task#6.

Note: open issue 20261025T102129Z-iam-10-net-new-behavioral-root-causes-simulateprincipalpolicy describes SimulatePrincipalPolicy attribution behaviorally but carries no replay key for these fixture steps.

## Evidence
Cause 1: expected (recorded AWS, GetPolicy) GetPolicyResult/Policy/Tags present and empty; actual member absent - `$.GetPolicyResponse.GetPolicyResult.Policy.Tags: missing (expected '', got None)`. Same for GetOpenIDConnectProvider, GetServerCertificate and GetInstanceProfile.
Cause 2: expected GetRoleResult/Role/RoleLastUsed present and empty; actual `Role.RoleLastUsed: missing (expected '', got None)`. iam-err-malformed-policy-trust-policy#task#8 also shows `Role.Description: missing (expected 'sim2real probe', got None)` for a role created with a Description.
Cause 3: expected EvaluationResults/member/MissingContextValues present and empty, OrganizationsDecisionDetail{AllowedByOrganizations 'true'}, and MatchedStatements/member/StartPosition{Line,Column} and EndPosition{Line,Column}; actual all four absent (`missing (expected ..., got None)`).
Cause 4: expected Role/PermissionsBoundary/PermissionsBoundaryType = 'Policy'; actual 'PermissionsBoundaryPolicy'.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay iam-err-delete-policy-blocked-by-attachment#task#7 (GetPolicy on an untagged policy) - the response has no <Tags/> element.
3. Replay iam-err-service-linked-role-unmodifiable#task#2 (GetRole) - no <RoleLastUsed/>.
4. Replay iam-policy-simulation-and-context-keys#task#5 (SimulatePrincipalPolicy) - no MissingContextValues, OrganizationsDecisionDetail or statement Start/EndPosition.
5. Replay iam-permissions-boundary-and-policy-simulation#task#6 (GetRole with a boundary) - PermissionsBoundaryType is 'PermissionsBoundaryPolicy'.
