Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks__iam.efs-csi-driver-policy-denies-untagged-access-point-create eks__iam.efs-csi-driver-policy-denies-tag-resource-outside-access-point-creation
Issue-Key: eks__iam.efs-csi-driver-policy-denies-untagged-access-point-create|run#8|AssertionError|assert extra == "implicitDeny", (
Issue-Key: eks__iam.efs-csi-driver-policy-denies-tag-resource-outside-access-point-creation|run#5|AssertionError|assert allowed == "allowed", f"{act} during CreateAccessPoint evaluated {allowed!r}"
Issue-Commit: 9ad1a007c88406c445bb3f2d55ab006d23610bba

# eks__iam: 2 net-new root causes — the ForAllValues:StringEquals set operator on aws:TagKeys is not enforced, and the CreateAction-conditioned TagResource allow in the EKS-recommended AmazonEFSCSIDriverPolicy never fires

> _Note: this issue was identified against the code base at commit `9ad1a007c88406c445bb3f2d55ab006d23610bba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SimulateCustomPolicy

## Description
Two distinct net-new root causes, both reached by simulating the EFS CSI driver managed policy that EKS recommends for the efs-csi-controller-sa service account (DescribeAddonConfiguration -> podIdentityConfiguration.recommendedManagedPolicies -> AmazonEFSCSIDriverPolicy, whose default version is read back from IAM and then evaluated with SimulateCustomPolicy). Each must be closed before this issue is submitted.

Cause 1 of 2: the ForAllValues: set operator is not applied, so a condition that restricts aws:TagKeys to an exact set admits requests carrying extra keys.
AWS: ForAllValues:StringEquals tests that EVERY value of the request's multivalued context key is a member of the condition's value set; if the request supplies a key outside the set, the condition fails and the guarded Allow does not fire. For the AmazonEFSCSIDriverPolicy CreateAccessPoint statement — Allow elasticfilesystem:CreateAccessPoint guarded by a Null test on aws:RequestTag/efs.csi.aws.com/cluster plus ForAllValues:StringEquals on aws:TagKeys — a simulation supplying aws:TagKeys = ["efs.csi.aws.com/cluster", "team"] evaluates to implicitDeny, while aws:TagKeys = ["efs.csi.aws.com/cluster"] alone evaluates to allowed and an untagged request evaluates to implicitDeny. Smithy: SimulateCustomPolicy accepts ContextEntries (ContextEntry.ContextKeyType includes 'stringList') and returns EvalDecision from PolicyEvaluationDecisionType (allowed|explicitDeny|implicitDeny). The tag key name is a value from the request, not fixed text.
Emulator: the exactly-tagged branch and the untagged branch answer correctly, but the extra-key branch answers 'allowed'. The set operator is therefore treated as an any-value match (or ignored), so the subset test is never enforced.
Why: a client validating a tag-scoped least-privilege policy is told a request carrying arbitrary EXTRA tag keys is permitted. Tag-key restriction is the whole mechanism this managed policy uses to confine the driver to resources it owns, so the simulator reports a strictly over-permissive verdict and an attribute-based access-control review passes a policy that should fail.
Fix: set-operator (ForAllValues:/ForAnyValue:) handling in the condition evaluator used by the simulate_custom_policy handler.
Traces: eks__iam.efs-csi-driver-policy-denies-untagged-access-point-create

Cause 2 of 2: elasticfilesystem:TagResource guarded on elasticfilesystem:CreateAction never evaluates to allowed, so the policy's tag-on-create grant is unreachable.
AWS: for the same recommended AmazonEFSCSIDriverPolicy, elasticfilesystem:TagResource simulated with aws:RequestTag/efs.csi.aws.com/cluster="true", aws:TagKeys=["efs.csi.aws.com/cluster"] and the service-specific key elasticfilesystem:CreateAction="CreateAccessPoint" evaluates to 'allowed' — tagging is permitted only as part of the access-point create. The same action with the CreateAction key ABSENT, or with CreateAction="CreateFileSystem", evaluates to implicitDeny.
Emulator: the allowed branch evaluates 'implicitDeny', so the first discriminating assertion fails and the grant is unreachable in simulation.
Hypothesis: either the seeded AWS-managed policy document for AmazonEFSCSIDriverPolicy (whose default version the check reads straight back from the emulator's IAM) omits or mis-states the CreateAction-conditioned TagResource statement, or the evaluator does not match a service-specific condition key (elasticfilesystem:CreateAction) supplied through ContextEntries. Both components are named in Fix because the recording cannot separate them.
Why: a caller checking that the driver's recommended policy actually permits the tagging the driver performs during CreateAccessPoint is told it is denied, so a correct EKS add-on setup looks mis-permissioned and operators add broader grants than AWS requires.
Fix: the seeded AWS-managed policy document for AmazonEFSCSIDriverPolicy, and/or service-specific condition-key matching from ContextEntries in the simulate_custom_policy handler's condition evaluator.
Traces: eks__iam.efs-csi-driver-policy-denies-tag-resource-outside-access-point-creation

## Evidence
Cause 1 — eks__iam.efs-csi-driver-policy-denies-untagged-access-point-create: AssertionError "elasticfilesystem:CreateAccessPoint tagged efs.csi.aws.com/cluster plus 'team' evaluated 'allowed'; ForAllValues must reject the extra key". Expected EvalDecision 'implicitDeny' for ContextEntries aws:RequestTag/efs.csi.aws.com/cluster='true' (string) + aws:TagKeys=['efs.csi.aws.com/cluster','team'] (stringList); actual 'allowed'. The two sibling branches of the same simulation (exactly-tagged -> allowed, untagged -> implicitDeny) both matched AWS, so the divergence is isolated to the set operator.

Cause 2 — eks__iam.efs-csi-driver-policy-denies-tag-resource-outside-access-point-creation: AssertionError "elasticfilesystem:TagResource during CreateAccessPoint evaluated 'implicitDeny'". Expected EvalDecision 'allowed' for ContextEntries aws:RequestTag/efs.csi.aws.com/cluster='true', aws:TagKeys=['efs.csi.aws.com/cluster'], elasticfilesystem:CreateAction='CreateAccessPoint' against the default version of the AmazonEFSCSIDriverPolicy the emulator itself recommends for efs-csi-controller-sa; actual 'implicitDeny'.

## Steps to Reproduce
Resolve the policy the way the checks do, so the document under test is the emulator's own: eks:DescribeClusterVersions(defaultOnly=True) -> clusterVersion; eks:DescribeAddonVersions(addonName='aws-efs-csi-driver', kubernetesVersion=<that version>) -> the default addonVersion; eks:DescribeAddonConfiguration(addonName='aws-efs-csi-driver', addonVersion=<that>) -> podIdentityConfiguration entry for serviceAccount 'efs-csi-controller-sa' -> the recommendedManagedPolicies ARN ending '/AmazonEFSCSIDriverPolicy'; iam:GetPolicy + iam:GetPolicyVersion(DefaultVersionId) -> the document.

Cause 1: call iam:SimulateCustomPolicy with PolicyInputList=[that document], ActionNames=['elasticfilesystem:CreateAccessPoint'], ResourceArns=['*'] and ContextEntries for aws:RequestTag/efs.csi.aws.com/cluster='true' (string) plus aws:TagKeys (stringList). With aws:TagKeys=['efs.csi.aws.com/cluster'] expect 'allowed'; with no context expect 'implicitDeny'; with aws:TagKeys=['efs.csi.aws.com/cluster','team'] expect 'implicitDeny' (ForAllValues subset test).

Cause 2: same call with ActionNames=['elasticfilesystem:TagResource'] and ContextEntries aws:RequestTag/efs.csi.aws.com/cluster='true', aws:TagKeys=['efs.csi.aws.com/cluster'], elasticfilesystem:CreateAction='CreateAccessPoint' -> expect 'allowed'; drop the CreateAction key, or set it to 'CreateFileSystem' -> expect 'implicitDeny'.
