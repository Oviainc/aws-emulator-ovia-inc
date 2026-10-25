Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.deploy-accessentry-via-cloudformation-grants-access
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|DescribeAccessEntry|ResourceNotFoundException
Issue-Commit: ea7cea25a1e6705e0f44642ae0567838786a0067

# CloudFormation AWS::EKS::AccessEntry is a no-op: the stack reaches CREATE_COMPLETE but EKS has no access entry for the declared principal

> _Note: this issue was identified against the code base at commit `ea7cea25a1e6705e0f44642ae0567838786a0067`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAccessEntry

## Description
Cause 1 of 1: a CloudFormation stack whose only resource is AWS::EKS::AccessEntry reports CREATE_COMPLETE, but the access entry it declares does not exist in EKS afterwards, so the first read of it 404s.

AWS: Smithy (com.amazonaws.eks#AWSWesleyFrontend): DescribeAccessEntry GET /clusters/{clusterName}/access-entries/{principalArn} -> 200 with `accessEntry` (an AccessEntry whose principalArn is the requested principal); its modeled refusal ResourceNotFoundException is 404. ListAssociatedAccessPolicies GET /clusters/{clusterName}/access-entries/{principalArn}/access-policies -> 200 with `associatedAccessPolicies`. For the CloudFormation side the model says nothing (it is not an EKS operation): recorded behavior only — when a stack declaring AWS::EKS::AccessEntry (Properties ClusterName, PrincipalArn, AccessPolicies=[{PolicyArn: arn:aws:eks::aws:cluster-access-policy/AmazonEKSViewPolicy, AccessScope:{Type: cluster}}]) reaches CREATE_COMPLETE, the entry is independently readable through the EKS API: DescribeAccessEntry returns it with the declared principalArn, ListAssociatedAccessPolicies lists the declared policyArn, and after the stack is deleted DescribeAccessEntry refuses with ResourceNotFoundException (404). A resource CloudFormation cannot provision must fail the resource and the stack, not complete it.

Emulator: the stack create completes (the check's stack_create_complete waiter returns; it raises an AssertionError naming the CFN HandlerErrorCode when it does not), and the very next read of the declared entry is refused: `ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeAccessEntry operation: The requested resource does not exist.` The EKS access-entry API itself is working in this same check — the cluster is created through eks:CreateCluster, reaches ACTIVE, and a direct eks:CreateAccessEntry + eks:DeleteAccessEntry pair on the same cluster and the same principal ARN succeeds immediately before the stack is created. Only the CloudFormation-driven creation is missing, i.e. the AWS::EKS::AccessEntry provisioner reports success without calling CreateAccessEntry/AssociateAccessPolicy.

Why: a silent success. An IaC-driven client is told its access entry exists, so it believes the principal has been granted cluster access, while the cluster has no such entry and no associated access policy — every authorization that depends on the entry behaves as if it were never created, and stack status is useless as a readiness signal. The failure surfaces far from its cause, as a 404 on a resource CloudFormation said it created.

Fix: the CloudFormation resource provisioning for AWS::EKS::AccessEntry — it must drive the EKS CreateAccessEntry path plus AssociateAccessPolicy for each entry in AccessPolicies, record the real physical resource id, delete the entry on stack delete, and fail the resource (and the stack) when it cannot create it. Related but distinct, and does NOT cover this: open issue 20261022T154613Z-eks-clusters-cannot-be-provisioned-at-all-no.md, whose Cause 4 is the same silent-success shape for AWS::EKS::Cluster; the generic CFN issues (e.g. 20261023T205437Z-cloudformation-reports-create-complete-for-resources-it-never.md) name ELBv2/ECS/Logs resource types, not AWS::EKS::AccessEntry.

Traces: eks.deploy-accessentry-via-cloudformation-grants-access.

## Evidence
Suite simparity-behavioral-ga-prod-v1, 18/111 passed. Check eks.deploy-accessentry-via-cloudformation-grants-access, verdict [error] (no ALREADY FILED / OWNED marker).

Sequence inside the check, in order: ec2 VPC+2 AZ-distinct subnets created; iam role for eks.amazonaws.com with AmazonEKSClusterPolicy created; eks.create_cluster(accessConfig={authenticationMode: API_AND_CONFIG_MAP}) and the cluster_active waiter both succeed; a second iam role (the principal) is created and PROVED usable against EKS by eks.create_access_entry + eks.delete_access_entry on that cluster/principal (the check's _wait_principal_arn_recognized, which only returns when that pair succeeds); cfn.create_stack with the single AWS::EKS::AccessEntry resource; the stack_create_complete waiter returns without raising (the check converts a waiter failure into 'AccessEntry stack create failed [<HandlerErrorCode>]: <message>', which was NOT the observed failure).

Expected: eks.describe_access_entry(clusterName=<cluster>, principalArn=<role arn>) -> 200, accessEntry.principalArn == the declared ARN, then list_associated_access_policies containing arn:aws:eks::aws:cluster-access-policy/AmazonEKSViewPolicy.
Actual: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeAccessEntry operation: The requested resource does not exist. (404 on the entry the CREATE_COMPLETE stack declared.)

The divergence is the status/code pair: AWS answers 200 with the entry here, the emulator answers 404 ResourceNotFoundException. The message text is quoted only as the recorded example.

## Steps to Reproduce
1. Boot the emulator.
2. Create a VPC with two AZ-distinct subnets (ec2), an IAM role trusting eks.amazonaws.com with AmazonEKSClusterPolicy, and an EKS cluster in those subnets with accessConfig.authenticationMode=API_AND_CONFIG_MAP; wait for ACTIVE.
3. Create a second IAM role (trusting ec2.amazonaws.com) to act as the principal. Confirm the ARN is usable directly: eks.create_access_entry(clusterName=<c>, principalArn=<p>) then eks.delete_access_entry(...) -> both succeed.
4. cfn.create_stack with TemplateBody {"Resources":{"AccessEntry":{"Type":"AWS::EKS::AccessEntry","Properties":{"ClusterName":"<c>","PrincipalArn":"<p>","AccessPolicies":[{"PolicyArn":"arn:aws:eks::aws:cluster-access-policy/AmazonEKSViewPolicy","AccessScope":{"Type":"cluster"}}]}}}} and wait for CREATE_COMPLETE (it does complete).
5. eks.describe_access_entry(clusterName="<c>", principalArn="<p>") -> expect 200 with accessEntry.principalArn == <p>; observe 404 ResourceNotFoundException. eks.list_associated_access_policies(...) likewise has no AmazonEKSViewPolicy entry.
6. Delete the stack; AWS then makes DescribeAccessEntry answer 404 — in the emulator it was already 404, so creation and deletion are indistinguishable.
