Issue-Source: trace-sweep-divergence
Issue-Trace: api-registry-policy-and-scanning-configuration
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-registry-policy-and-scanning-configuration ecr-registry-policy-document-unicode-round-trip ecr-registry-policy-unicode-document-round-trip api-repository-lifecycle-with-policy-and-tags
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|PutRegistryPolicy|2xx->2xx|$.policyText
Issue-Key: ecr|GetRegistryPolicy|2xx->2xx|$.policyText
Issue-Key: ecr|DeleteRegistryPolicy|2xx->2xx|$.policyText
Issue-Key: ecr|SetRepositoryPolicy|2xx->2xx|$.policyText
Issue-Key: ecr|GetRepositoryPolicy|2xx->2xx|$.policyText
Issue-Key: ecr|DeleteRepositoryPolicy|2xx->2xx|$.policyText
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# ECR registry and repository policy documents are returned re-serialized, not in AWS canonical form

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRegistryPolicy

## Description
This issue bundles 6 DISTINCT root causes — three on the registry policy and three on the repository policy — that share one fault: the IAM policy document returned in policyText is produced by the emulator's own JSON writer instead of in the form AWS returns. All six must be closed before submitting; each carries its own fixture and step.

AWS (common to all six): ECR echoes an attached IAM policy document in the canonical IAM-policy rendering: pretty-printed, two-space indented, with a space on BOTH sides of the key separator ('"Version" : "2012-10-17"'), statements on their own lines, and single-element Action/Resource values left as the SCALAR string the client wrote rather than promoted to a one-element array. The recordings show exactly that form on PutRegistryPolicy / GetRegistryPolicy / DeleteRegistryPolicy and on SetRepositoryPolicy / GetRepositoryPolicy / DeleteRepositoryPolicy.
Emulator (common to all six): emits a single-line compact document with the default '": "' separator and normalises single-element Action and Resource to JSON arrays ("Action": ["ecr:ReplicateImage"] where the client wrote and AWS returned "Action": "ecr:ReplicateImage").
Why (common): the policy document is an opaque document member that clients hash, diff and compare against the file they submitted. Reformatting it makes drift detection in Terraform/CDK/CloudFormation report a permanent difference and re-apply forever, and the scalar-to-array promotion is a semantic edit of the caller's document: code that reads policyText back and asserts on Action's type (string vs list) takes the wrong branch. Round-tripping the document through a parser also destroys any non-ASCII escaping the client chose.
Fix (common): keep the submitted policy document in registry/repository policy state verbatim (or render it with the canonical IAM-policy writer) and have the policy serializers echo it; never re-serialize a parsed value and never normalise scalar members into arrays.

Cause 1 of 6: PutRegistryPolicy reformats the document in its own response. Traces: api-registry-policy-and-scanning-configuration#task#4 (representative; ecr-registry-policy-document-unicode-round-trip#task#2 is in the same cluster).
Cause 2 of 6: GetRegistryPolicy reformats the stored document. Traces: api-registry-policy-and-scanning-configuration#task#5 (same cluster: ecr-registry-policy-document-unicode-round-trip#task#3, ecr-registry-policy-unicode-document-round-trip#task#2).
Cause 3 of 6: DeleteRegistryPolicy reformats the document it returns on removal. Traces: api-registry-policy-and-scanning-configuration#task#9.
Cause 4 of 6: SetRepositoryPolicy reformats the document in its own response. Traces: api-repository-lifecycle-with-policy-and-tags#task#4.
Cause 5 of 6: GetRepositoryPolicy reformats the stored document. Traces: api-repository-lifecycle-with-policy-and-tags#task#5.
Cause 6 of 6: DeleteRepositoryPolicy reformats the document it returns on removal. Traces: api-repository-lifecycle-with-policy-and-tags#task#6.

Note: this is NOT the already-filed message defect in open issue 20261022T234922Z-ecr-policy-lifecycle-document-validation-errors-carry-emulator.md, which is about the TEXT of validation rejections. Here the calls all succeed with HTTP 200 and the defect is the shape of the policy document in the success body; that issue carries none of these fixture steps.

## Evidence
All six steps returned status 200 on both sides; the divergence is $.policyText.
Registry policy (causes 1-3) — expected '{\n  "Version" : "2012-10-17",\n  "Statement" : [ {\n    "Sid" : "AllowCrossAccountPull",\n    "Effect" : "Allow",\n    "Principal" : {\n      "AWS" : "arn:aws:iam::000000000000:root"\n    },\n    "Action" : "ecr:ReplicateImage",\n    "Resource" : "arn:aws:ecr:us-west-2:000000000000:repository/*"\n  } ]\n}', got '{"Version": "2012-10-17", "Statement": [{"Sid": "AllowCrossAccountPull", "Effect": "Allow", "Principal": {"AWS": "arn:aws:iam::000000000000:root"}, "Action": ["ecr:ReplicateImage"], "Resource": ["arn:aws:ecr:us-west-2:000000000000:repository/*"]}]}'. Note Action/Resource promoted from scalar to array as well as the whitespace change. Observed on api-registry-policy-and-scanning-configuration#task#4 (PutRegistryPolicy), #task#5 (GetRegistryPolicy) and #task#9 (DeleteRegistryPolicy).
Repository policy (causes 4-6) — expected '{\n  "Version" : "2012-10-17",\n  "Statement" : [ {\n    "Sid" : "AllowPull",\n    "Effect" : "Allow",\n    "Principal" : {\n      "AWS" : "arn:aws:iam::000000000000:root"\n    },\n    "Action" : [ "ecr:GetDownloadUrlForLayer", "ecr:BatchGetImage" ]\n  } ]\n}', got '{"Version": "2012-10-17", "Statement": [{"Sid": "AllowPull", "Effect": "Allow", "Principal": {"AWS": "arn:aws:iam::000000000000:root"}, "Action": ["ecr:GetDownloadUrlForLayer", "ecr:BatchGetImage"]}]}'. Observed on api-repository-lifecycle-with-policy-and-tags#task#4 (SetRepositoryPolicy), #task#5 (GetRepositoryPolicy) and #task#6 (DeleteRepositoryPolicy).

## Steps to Reproduce
Replay the wire corpus and re-run the six fixture steps listed per cause. Manually:
1. PutRegistryPolicy(policyText=P) where P has a single scalar Action and a single scalar Resource -> compare the returned policyText against AWS's canonical rendering; assert Action is still a scalar.
2. GetRegistryPolicy() then DeleteRegistryPolicy() -> same comparison on each.
3. CreateRepository R; SetRepositoryPolicy(repositoryName=R, policyText=Q) -> same comparison; then GetRepositoryPolicy(R) and DeleteRepositoryPolicy(R) -> same comparison on each.
