Issue-Source: trace-sweep-divergence
Issue-Trace: eks-access-entry-and-policy-operations-on-missing-cluster
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-access-entry-and-policy-operations-on-missing-cluster eks-addon-operations-on-missing-cluster eks-capability-error-paths eks-capability-errors-unknown-cluster eks-capability-not-found-error-paths eks-capability-operations-on-missing-cluster eks-insights-and-identity-provider-operations-on-missing-cluster eks-nodegroup-and-fargate-profile-errors-unknown-cluster eks-nodegroup-operations-on-missing-cluster
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|DescribeAccessEntry|4xx->4xx|$.clusterName
Issue-Key: eks|AssociateAccessPolicy|4xx->4xx|$.clusterName
Issue-Key: eks|ListAssociatedAccessPolicies|4xx->4xx|$.clusterName
Issue-Key: eks|CreateAddon|4xx->4xx|$.addonName
Issue-Key: eks|DescribeAddon|4xx->4xx|$.addonName
Issue-Key: eks|DescribeAddon|4xx->4xx|$.clusterName
Issue-Key: eks|DescribeAddon|4xx->4xx|$.message
Issue-Key: eks|DescribeCapability|4xx->4xx|$.clusterName
Issue-Key: eks|DescribeCapability|4xx->4xx|$.message
Issue-Key: eks|DeleteCapability|4xx->4xx|$.clusterName
Issue-Key: eks|DeleteCapability|4xx->4xx|$.message
Issue-Key: eks|DescribeIdentityProviderConfig|4xx->4xx|$.clusterName
Issue-Key: eks|DescribeNodegroup|4xx->4xx|$.clusterName
Issue-Key: eks|DescribeNodegroup|4xx->4xx|$.message
Issue-Key: eks|DescribeNodegroup|4xx->4xx|$.nodegroupName
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# EKS not-found errors on a missing cluster echo request path members AWS leaves null

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAccessEntry

## Description
Replaying simparity-wire-aws-v1 against the EKS emulator (service crate services/service-com-amazonaws-eks-awswesleyfrontend) shows NINE distinct operations whose 404 not-found body is built from a single shared template that populates the exception's resource-identity members. On AWS these members are NOT set on these paths: the HTTP status (404) and the error code (ResourceNotFoundException) are right, but the serialized body carries extra members, so the error payload shape diverges on every one of them. Smithy (com-amazonaws-eks-awswesleyfrontend): ResourceNotFoundException has OPTIONAL members clusterName, nodegroupName, fargateProfileName, addonName, subscriptionId, message - none required, and the recordings show AWS leaving the resource-identity members null for these operations and conveying the identity in `message` only. Each cause below is one operation's error payload and must be closed; the issue is NOT complete until all nine are. An open behavioral issue (20261022T154613Z-eks-control-plane-operations-answer-501-notimplemented-or.md) covers capability/addon/nodegroup operations answering 501 NotImplemented; it does not carry any of the replay keys below and says nothing about the error body's members, so these causes are net-new and must be re-replayed here.

Cause 1 of 9 - DescribeAccessEntry on a cluster that does not exist.
AWS: Smithy: DescribeAccessEntry errors = InvalidRequestException (400), ResourceNotFoundException (404), ServerException (500); clusterName/principalArn are @httpLabel. Recorded: 404 ResourceNotFoundException with $.clusterName absent/null.
Emulator: 404 ResourceNotFoundException but $.clusterName is set to the cluster name taken from the URI label.
Why: a client that reads e.response['Error'] plus the exception's clusterName member to decide WHICH resource was missing gets a value AWS never sends, so emulator-backed tests encode a payload real EKS does not produce.
Fix: the shared not-found error constructor used by the access-entry handlers in the eks crate's error/serializer layer - it must not populate identity members for these operations.
Traces: eks-access-entry-and-policy-operations-on-missing-cluster#task#3.

Cause 2 of 9 - AssociateAccessPolicy on a cluster that does not exist.
AWS: Smithy: AssociateAccessPolicy models ResourceNotFoundException (404). Recorded: 404 with $.clusterName null.
Emulator: 404 with $.clusterName populated from the request.
Why: same as cause 1 - the error payload carries an identity member AWS omits.
Fix: the AssociateAccessPolicy handler's not-found path / shared error constructor.
Traces: eks-access-entry-and-policy-operations-on-missing-cluster#task#5.

Cause 3 of 9 - ListAssociatedAccessPolicies on a cluster that does not exist.
AWS: Smithy: ResourceNotFoundException (404) is modeled. Recorded: 404 with $.clusterName null.
Emulator: 404 with $.clusterName populated.
Why: same as cause 1.
Fix: the ListAssociatedAccessPolicies handler's not-found path.
Traces: eks-access-entry-and-policy-operations-on-missing-cluster#task#6.

Cause 4 of 9 - CreateAddon on a cluster that does not exist.
AWS: Smithy: CreateAddon models ResourceNotFoundException (404) and InvalidParameterException (400). Recorded: 404 with $.addonName null.
Emulator: 404 with $.addonName set to the requested addon (example: 'vpc-cni').
Why: a client branching on the exception's addonName to retry a specific addon sees a member AWS does not send.
Fix: the CreateAddon handler's cluster-resolution failure path.
Traces: eks-addon-operations-on-missing-cluster#task#2.

Cause 5 of 9 - DescribeAddon on a cluster that does not exist.
AWS: Smithy: DescribeAddon errors = ClientException (400), InvalidParameterException (400), InvalidRequestException (400), ResourceNotFoundException (404), ServerException (500). Recorded: 404 with BOTH $.clusterName and $.addonName null (recorded message example: 'The requested resource does not exist.').
Emulator: 404 with $.clusterName and $.addonName both populated from the URI labels (message example: 'No cluster found for name: <cluster>.'). The message text itself is an example, not the requirement here; the diverging requirement is the set of members serialized.
Why: the response shape differs from AWS on a path clients hit constantly while polling for an addon.
Fix: the DescribeAddon handler's not-found path / shared error constructor.
Traces: eks-addon-operations-on-missing-cluster#task#3.

Cause 6 of 9 - DescribeCapability on a cluster that does not exist.
AWS: Smithy: DescribeCapability models ResourceNotFoundException (404). Recorded: 404 with $.clusterName null.
Emulator: 404 with $.clusterName populated. This cluster covers 4 fixtures (eks-capability-error-paths, eks-capability-errors-unknown-cluster, eks-capability-not-found-error-paths, eks-capability-operations-on-missing-cluster).
Why: same as cause 1.
Fix: the DescribeCapability handler's cluster-resolution failure path.
Traces: eks-capability-error-paths#task#2.

Cause 7 of 9 - DeleteCapability on a cluster that does not exist.
AWS: Smithy: DeleteCapability models ResourceNotFoundException (404). Recorded: 404 with $.clusterName null.
Emulator: 404 with $.clusterName populated. This cluster also covers 4 fixtures.
Why: same as cause 1.
Fix: the DeleteCapability handler's cluster-resolution failure path.
Traces: eks-capability-error-paths#task#5.

Cause 8 of 9 - DescribeIdentityProviderConfig on a cluster that does not exist.
AWS: Smithy: DescribeIdentityProviderConfig models ResourceNotFoundException (404). Recorded: 404 with $.clusterName null.
Emulator: 404 with $.clusterName populated.
Why: same as cause 1.
Fix: the DescribeIdentityProviderConfig handler's cluster-resolution failure path.
Traces: eks-insights-and-identity-provider-operations-on-missing-cluster#task#6.

Cause 9 of 9 - DescribeNodegroup on a cluster that does not exist.
AWS: Smithy: DescribeNodegroup models ResourceNotFoundException (404). Recorded: 404 with BOTH $.clusterName and $.nodegroupName null.
Emulator: 404 with $.clusterName and $.nodegroupName populated from the URI labels.
Why: nodegroup pollers that inspect the exception's nodegroupName member get a value AWS never sets.
Fix: the DescribeNodegroup handler's not-found path / shared error constructor.
Traces: eks-nodegroup-and-fargate-profile-errors-unknown-cluster#task#2 (also eks-nodegroup-operations-on-missing-cluster#task#2).

## Evidence
Corpus simparity-wire-aws-v1, 35/216 steps passed. All nine clusters below returned the correct status (404) and error code (ResourceNotFoundException) but a body with extra members.

Cause 1: eks-access-entry-and-policy-operations-on-missing-cluster#task#3 DescribeAccessEntry - $.clusterName: type_mismatch (expected None, got '<cluster>').
Cause 2: eks-access-entry-and-policy-operations-on-missing-cluster#task#5 AssociateAccessPolicy - $.clusterName: type_mismatch (expected None, got '<cluster>').
Cause 3: eks-access-entry-and-policy-operations-on-missing-cluster#task#6 ListAssociatedAccessPolicies - $.clusterName: type_mismatch (expected None, got '<cluster>').
Cause 4: eks-addon-operations-on-missing-cluster#task#2 CreateAddon - $.addonName: type_mismatch (expected None, got 'vpc-cni').
Cause 5: eks-addon-operations-on-missing-cluster#task#3 DescribeAddon - $.clusterName and $.addonName: type_mismatch (expected None); $.message value differs as well (expected 'The requested resource does not exist.', got 'No cluster found for name: <cluster>.').
Cause 6: eks-capability-error-paths#task#2 DescribeCapability - $.clusterName: type_mismatch (expected None); affected_count 4 (eks-capability-errors-unknown-cluster#task#1, eks-capability-not-found-error-paths#task#1, eks-capability-operations-on-missing-cluster#task#2).
Cause 7: eks-capability-error-paths#task#5 DeleteCapability - $.clusterName: type_mismatch (expected None); affected_count 4 (eks-capability-errors-unknown-cluster#task#3, eks-capability-not-found-error-paths#task#3, eks-capability-operations-on-missing-cluster#task#3).
Cause 8: eks-insights-and-identity-provider-operations-on-missing-cluster#task#6 DescribeIdentityProviderConfig - $.clusterName: type_mismatch (expected None).
Cause 9: eks-nodegroup-and-fargate-profile-errors-unknown-cluster#task#2 DescribeNodegroup - $.clusterName and $.nodegroupName: type_mismatch (expected None); affected_count 2 (eks-nodegroup-operations-on-missing-cluster#task#2).

## Steps to Reproduce
1. Boot the emulator and point an EKS client at it. Use a cluster name that was never created, e.g. CLUSTER=sim2real-probe-eks-ghost.
2. Cause 1: describe_access_entry(clusterName=CLUSTER, principalArn='arn:aws:iam::000000000000:role/x') -> expect 404 ResourceNotFoundException whose body has NO clusterName member.
3. Cause 2: associate_access_policy(clusterName=CLUSTER, principalArn=..., policyArn=..., accessScope={'type':'cluster'}) -> expect the same body with no clusterName.
4. Cause 3: list_associated_access_policies(clusterName=CLUSTER, principalArn=...) -> same.
5. Cause 4: create_addon(clusterName=CLUSTER, addonName='vpc-cni') -> expect 404 with NO addonName member.
6. Cause 5: describe_addon(clusterName=CLUSTER, addonName='vpc-cni') -> expect 404 with neither clusterName nor addonName.
7. Cause 6: describe_capability(clusterName=CLUSTER, capabilityName=...) -> expect 404 with no clusterName.
8. Cause 7: delete_capability(clusterName=CLUSTER, capabilityName=...) -> expect 404 with no clusterName.
9. Cause 8: describe_identity_provider_config(clusterName=CLUSTER, identityProviderConfig={'type':'oidc','name':'x'}) -> expect 404 with no clusterName.
10. Cause 9: describe_nodegroup(clusterName=CLUSTER, nodegroupName='ng') -> expect 404 with neither clusterName nor nodegroupName.
11. In every case inspect the RAW response body, not just the error code: the status and code already match.
