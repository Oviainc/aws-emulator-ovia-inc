Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks__iam.efs-csi-pod-identity-recommended-policies-resolve-in-iam
Issue-Key: eks__iam.efs-csi-pod-identity-recommended-policies-resolve-in-iam|run#2|AssertionError|assert sorted(recs) == sorted([_CONTROLLER_SA, _NODE_SA]), (
Issue-Commit: 3879dd05c1f9087ace2750498c91b002c8086d49

# eks: the aws-efs-csi-driver add-on's podIdentityConfiguration names only the controller service account

> _Note: this issue was identified against the code base at commit `3879dd05c1f9087ace2750498c91b002c8086d49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAddonVersions

## Description
Cause 1 of 1: the add-on catalog's pod-identity configuration is incomplete for aws-efs-csi-driver.
AWS: recorded behavior only. The add-on's podIdentityConfiguration lists one entry per service account the driver uses — both 'efs-csi-controller-sa' and 'efs-csi-node-sa' — each with the AWS-managed policy ARNs EKS recommends for it.
Emulator: names only 'efs-csi-controller-sa', so the node service account and its recommended policies are absent.
Why: a cluster operator (or an add-on installer) that creates pod identity associations from this catalog never creates the node association, and the driver's node component is left without the permissions AWS prescribes.
Fix: the add-on catalog entry for aws-efs-csi-driver behind DescribeAddonVersions must carry every service account's pod-identity configuration.
Traces: eks__iam.efs-csi-pod-identity-recommended-policies-resolve-in-iam.

## Evidence
"AssertionError: podIdentityConfiguration names service accounts ['efs-csi-controller-sa']" where ['efs-csi-controller-sa', 'efs-csi-node-sa'] was required.

## Steps to Reproduce
1. Call eks DescribeAddonVersions for addonName='aws-efs-csi-driver' and read each version's podIdentityConfiguration.
2. Or replay eks__iam.efs-csi-pod-identity-recommended-policies-resolve-in-iam.
