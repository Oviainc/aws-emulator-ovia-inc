Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks__iam.efs-csi-driver-policy-denies-delete-of-untagged-access-point eks__iam.efs-csi-driver-policy-denies-tag-resource-outside-access-point-creation eks__iam.efs-csi-driver-policy-denies-untagged-access-point-create
Issue-Key: eks__iam|GetPolicy|NotImplemented
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# eks__iam: IAM GetPolicy answers an unmodeled NotImplemented for AWS-managed policy AmazonEFSCSIDriverPolicy, which the emulator's managed-policy catalog omits

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPolicy

## Description
One root cause, reached by all three EFS-CSI-driver policy-scoping checks.

Cause 1 of 1: the emulator's AWS-managed policy catalog does not contain arn:aws:iam::aws:policy/service-role/AmazonEFSCSIDriverPolicy, and GetPolicy answers an UNMODELED `NotImplemented` error instead of the policy's metadata.
AWS: Smithy: iam#GetPolicy, required input PolicyArn -> arnType (@length 20..2048); output Policy; modeled errors are exactly InvalidInputException (awsQueryError InvalidInput, HTTP 400), NoSuchEntityException (awsQueryError NoSuchEntity, HTTP 404) and ServiceFailureException (awsQueryError ServiceFailure, HTTP 500). `NotImplemented` is not in that set and IAM never returns it for this operation. For an ARN in the aws-managed namespace that AWS publishes, GetPolicy returns HTTP 200 with Policy carrying Arn, PolicyName, PolicyId, DefaultVersionId, IsAttachable and the counts, after which GetPolicyVersion(PolicyArn, VersionId=DefaultVersionId) returns the policy document. AmazonEFSCSIDriverPolicy is a published AWS managed policy and is the policy EKS itself names in DescribeAddonConfiguration's podIdentityConfiguration[].recommendedManagedPolicies for the aws-efs-csi-driver add-on's efs-csi-controller-sa service account — so the emulator points callers at an ARN it then refuses to describe.
Emulator: GetPolicy(PolicyArn='arn:aws:iam::aws:policy/service-role/AmazonEFSCSIDriverPolicy') is refused with error code `NotImplemented` and the message 'AWS-managed policy arn:aws:iam::aws:policy/service-role/AmazonEFSCSIDriverPolicy is not in this emulator's catalog of AWS-managed policies, so its metadata cannot be reported; the policy is not claimed to be absent'. That text is emulator prose that AWS produces for no error class: the code is outside the operation's modeled error set, and the message names the emulator's own catalog. Two defects in one cause — the catalog omits a published AWS managed policy, and the miss is reported as `NotImplemented` rather than as either the policy's metadata (correct) or NoSuchEntity (if the service genuinely had no such policy).
Why: a client that resolves the EKS-recommended driver policy by the ARN EKS hands it cannot read the policy at all, so it cannot attach it, cannot fetch its default version, and cannot simulate it. Because the code is unmodeled, SDK error handling has no branch for it: botocore raises a generic ClientError instead of the NoSuchEntityException/InvalidInputException a caller is written to catch, and retry/fallback logic keyed on IAM's modeled codes misroutes. The whole policy-scoping contract behind it is then unobservable: with the document in hand, SimulateCustomPolicy must evaluate elasticfilesystem:DeleteAccessPoint as 'allowed' when aws:ResourceTag/efs.csi.aws.com/cluster is present, 'implicitDeny' when it is absent, 'implicitDeny' when the tag arrives on aws:RequestTag instead of the resource, and 'implicitDeny' for elasticfilesystem:DeleteFileSystem, which no statement grants.
Fix: the AWS-managed policy catalog in this service's state (the table the GetPolicy / GetPolicyVersion / ListPolicies handlers read for the arn:aws:iam::aws:policy/ namespace) must carry the published AWS managed policies — including the service-role policy AmazonEFSCSIDriverPolicy with its real document, DefaultVersionId and IsAttachable flag — and the GetPolicy handler must stop emitting the unmodeled `NotImplemented` code for this operation: a catalog miss is NoSuchEntityException (awsQueryError NoSuchEntity, HTTP 404) at most, never NotImplemented.
Related but not covering: open issue 20261025T153722Z-iam-3-net-new-root-causes-the-built.md reports the same catalog as omitting well-known policies (ReadOnlyAccess) and carrying no non-attachable policy, reached through ListPolicies under service token `iam`. It does not name GetPolicy, the unmodeled NotImplemented code, AmazonEFSCSIDriverPolicy, or these three checks' replay keys, so it does not cover this finding. 20261026T024842Z-iam-getpolicy-s-nosuchentity-message-says-does-not.md concerns GetPolicy's NoSuchEntity wording for a customer-managed ARN and is a different path.
Traces: eks__iam.efs-csi-driver-policy-denies-delete-of-untagged-access-point, eks__iam.efs-csi-driver-policy-denies-tag-resource-outside-access-point-creation, eks__iam.efs-csi-driver-policy-denies-untagged-access-point-create.

## Evidence
Suite simparity-behavioral-ga-prod-v1; eks__iam 1/4 (25%), 3 failed. None of the three is marked ALREADY FILED or OWNED.

All three checks fail identically, on the same call:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetPolicy operation: AWS-managed policy arn:aws:iam::aws:policy/service-role/AmazonEFSCSIDriverPolicy is not in this emulator's catalog of AWS-managed policies, so its metadata cannot be reported; the policy is not claimed to be absent

Expected: HTTP 200 with Policy{Arn='arn:aws:iam::aws:policy/service-role/AmazonEFSCSIDriverPolicy', PolicyName='AmazonEFSCSIDriverPolicy', DefaultVersionId=<the catalog's default version>, IsAttachable=<the policy's flag>}, followed by GetPolicyVersion returning its document. Per Smithy iam#GetPolicy the only permissible refusals are InvalidInput/400, NoSuchEntity/404 and ServiceFailure/500.
Actual: error code `NotImplemented`, a code the operation does not model, with an emulator-authored message whose fixed text is 'AWS-managed policy ' / ' is not in this emulator's catalog of AWS-managed policies, so its metadata cannot be reported; the policy is not claimed to be absent' and whose only variable part is the request's PolicyArn. AWS produces no message of this shape for GetPolicy.

The path up to that call all succeeded, which localises the defect to IAM's managed-policy catalog: eks.describe_cluster_versions(defaultOnly=True) answered; eks.describe_addon_versions(addonName='aws-efs-csi-driver', kubernetesVersion=<default>) returned exactly one add-on with exactly one defaultVersion build; eks.describe_addon_configuration for that build returned a podIdentityConfiguration whose efs-csi-controller-sa entry recommends exactly one '.../AmazonEFSCSIDriverPolicy' ARN. The emulator therefore recommends the ARN and then refuses to describe it.

Never reached because of this cause (what the fix must unblock, same single cause): iam.simulate_custom_policy with PolicyInputList=[<the driver policy document>], ResourceArns=['*'] must give EvaluationResults[0].EvalDecision == 'allowed' for elasticfilesystem:DeleteAccessPoint with ContextEntries aws:ResourceTag/efs.csi.aws.com/cluster='true', 'implicitDeny' with no context entries, 'implicitDeny' with aws:RequestTag/efs.csi.aws.com/cluster='true' instead, and 'implicitDeny' for elasticfilesystem:DeleteFileSystem. (SimulateCustomPolicy itself is also an unimplemented stub owned by open issue 20261025T000319Z-iam-simulatecustompolicy-stub.md under service token iam; that stub is not this finding and is not re-filed here.)

## Steps to Reproduce
Boot the emulator and sign requests as a principal permitted to call IAM and EKS.

1. k8s = eks.describe_cluster_versions(defaultOnly=True)['clusterVersions'][0]['clusterVersion'].
2. From eks.describe_addon_versions(addonName='aws-efs-csi-driver', kubernetesVersion=k8s) take the addonVersion flagged defaultVersion for k8s.
3. eks.describe_addon_configuration(addonName='aws-efs-csi-driver', addonVersion=<that version>) and read podIdentityConfiguration -> the efs-csi-controller-sa entry recommends arn:aws:iam::aws:policy/service-role/AmazonEFSCSIDriverPolicy.
4. iam.get_policy(PolicyArn='arn:aws:iam::aws:policy/service-role/AmazonEFSCSIDriverPolicy') -> expect HTTP 200 with Policy metadata (or, if the service truly had no such policy, NoSuchEntity/404); observe error code NotImplemented with the emulator-catalog message.
5. (Blocked by 4) iam.get_policy_version(PolicyArn=<same>, VersionId=<DefaultVersionId>) for the document, then iam.simulate_custom_policy(PolicyInputList=[document], ActionNames=['elasticfilesystem:DeleteAccessPoint'], ResourceArns=['*'], ContextEntries=[...]) for each of the four decisions in the evidence.
   Replay: eks__iam.efs-csi-driver-policy-denies-delete-of-untagged-access-point, eks__iam.efs-csi-driver-policy-denies-tag-resource-outside-access-point-creation, eks__iam.efs-csi-driver-policy-denies-untagged-access-point-create
