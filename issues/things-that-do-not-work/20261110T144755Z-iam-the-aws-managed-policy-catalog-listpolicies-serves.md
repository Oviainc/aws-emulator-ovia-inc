Issue-Source: trace-sweep-divergence
Issue-Trace: iam-catalog-list-policies
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-catalog-list-policies
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|ListPolicies|2xx->2xx|$.ListPoliciesResponse.ListPoliciesResult.Policies.member[Arn="arn:aws:iam::aws:policy/root-task/IAMAuditRootUserCredentials"].CreateDate
Issue-Key: iam|ListPolicies|2xx->2xx|$.ListPoliciesResponse.ListPoliciesResult.Policies.member[Arn="arn:aws:iam::aws:policy/root-task/IAMAuditRootUserCredentials"].PolicyId
Issue-Key: iam|ListPolicies|2xx->2xx|$.ListPoliciesResponse.ListPoliciesResult.Policies.member[Arn="arn:aws:iam::aws:policy/root-task/IAMAuditRootUserCredentials"].UpdateDate
Issue-Key: iam|ListPolicies|2xx->2xx|$.ListPoliciesResponse.ListPoliciesResult.Policies.member[Arn="arn:aws:iam::aws:policy/root-task/IAMCreateRootUserPassword"].CreateDate
Issue-Key: iam|ListPolicies|2xx->2xx|$.ListPoliciesResponse.ListPoliciesResult.Policies.member[Arn="arn:aws:iam::aws:policy/root-task/IAMCreateRootUserPassword"].PolicyId
Issue-Key: iam|ListPolicies|2xx->2xx|$.ListPoliciesResponse.ListPoliciesResult.Policies.member[Arn="arn:aws:iam::aws:policy/root-task/IAMCreateRootUserPassword"].UpdateDate
Issue-Key: iam|ListPolicies|2xx->2xx|$.ListPoliciesResponse.ListPoliciesResult.Policies.member[Arn="arn:aws:iam::aws:policy/root-task/IAMDeleteRootUserCredentials"].CreateDate
Issue-Key: iam|ListPolicies|2xx->2xx|$.ListPoliciesResponse.ListPoliciesResult.Policies.member[Arn="arn:aws:iam::aws:policy/root-task/IAMDeleteRootUserCredentials"].PolicyId
Issue-Key: iam|ListPolicies|2xx->2xx|$.ListPoliciesResponse.ListPoliciesResult.Policies.member[Arn="arn:aws:iam::aws:policy/root-task/IAMDeleteRootUserCredentials"].UpdateDate
Issue-Key: iam|ListPolicies|2xx->2xx|$.ListPoliciesResponse.ListPoliciesResult.Policies.member[Arn="arn:aws:iam::aws:policy/root-task/S3UnlockBucketPolicy"].CreateDate
Issue-Key: iam|ListPolicies|2xx->2xx|$.ListPoliciesResponse.ListPoliciesResult.Policies.member[Arn="arn:aws:iam::aws:policy/root-task/S3UnlockBucketPolicy"].PolicyId
Issue-Key: iam|ListPolicies|2xx->2xx|$.ListPoliciesResponse.ListPoliciesResult.Policies.member[Arn="arn:aws:iam::aws:policy/root-task/S3UnlockBucketPolicy"].UpdateDate
Issue-Key: iam|ListPolicies|2xx->2xx|$.ListPoliciesResponse.ListPoliciesResult.Policies.member[Arn="arn:aws:iam::aws:policy/root-task/SQSUnlockQueuePolicy"].CreateDate
Issue-Key: iam|ListPolicies|2xx->2xx|$.ListPoliciesResponse.ListPoliciesResult.Policies.member[Arn="arn:aws:iam::aws:policy/root-task/SQSUnlockQueuePolicy"].PolicyId
Issue-Key: iam|ListPolicies|2xx->2xx|$.ListPoliciesResponse.ListPoliciesResult.Policies.member[Arn="arn:aws:iam::aws:policy/root-task/SQSUnlockQueuePolicy"].UpdateDate
Issue-Key: iam|ListPolicies|2xx->2xx|$.ListPoliciesResponse.ListPoliciesResult.Policies.member[Arn="arn:aws:iam::aws:policy/service-role/AmazonEFSCSIDriverPolicy"].DefaultVersionId
Issue-Related: 20261107T005227Z-iam-listpolicies-serves-a-synthesized-aws-managed-policy.md
Issue-Commit: a7d3bb56a8b20ad6b0cfb258ffd31cc1523a40ca

# iam: the AWS-managed policy catalog ListPolicies serves is incomplete and its published metadata is locally synthesized

> _Note: this issue was identified against the code base at commit `a7d3bb56a8b20ad6b0cfb258ffd31cc1523a40ca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPolicies

## Description
2 net-new causes, ONE shared fault: the catalog of AWS-managed policies the emulator serves for Scope=AWS (the policies whose Arn is arn:aws:iam::aws:policy/<path>/<name>) is not the catalog AWS publishes. Entries AWS returns are absent from it, and for the entries it does have, the per-policy metadata AWS publishes as fixed global constants (PolicyId, CreateDate, UpdateDate, DefaultVersionId) is minted locally instead. Both causes must be closed before this issue is submitted.

NOTE ON EXISTING ISSUES: the open issue 20261029T184359Z-iam-generated-iam-credential-identifiers-do-not-use-the.md covers generated IAM *credential* identifiers (access key ids minted for principals the caller created) and owns the ListAccessKeys replay keys in this corpus. It does not cover this: the identifiers and dates below are not per-run generated values at all, they are AWS's published catalog constants. The replay reports both clusters below with stub_path empty and owned_by empty, so no open issue carries their replay keys.

Cause 1 of 2: ListPolicies restricted to the /root-task/ path (fixture iam-catalog-list-policies, step task#3).
AWS: Smithy (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json) — ListPolicies input carries Scope (com.amazonaws.iam#policyScopeType, values All|AWS|Local) and PathPrefix (policyPathType); output Policies is a policyListType of com.amazonaws.iam#Policy. Policy.PolicyId targets idType (@length min 16 max 128, @pattern ^[\w]+$) and is documented as "The stable and unique string identifying the policy"; Policy.CreateDate and Policy.UpdateDate target dateType (ISO 8601 date-time); Policy.DefaultVersionId targets policyVersionIdType (@pattern ^v[1-9][0-9]*(\.[A-Za-z0-9-]*)?$). The model does not enumerate the catalog, so for the catalog itself this is recorded behavior only: the recording shows that for an AWS-managed policy the PolicyId (an ANPA... string) and the CreateDate/UpdateDate are fixed values AWS publishes globally — the same policy carries the same PolicyId and the same dates in every account and every region, because the policy is owned by AWS and not by the caller's account. They are therefore NOT the class of per-run generated id/timestamp that legitimately differs between environments. Status is 200 on both sides; no error code and no error message are involved.
Emulator: answers 200 and does return the policies, but every AWS-managed policy under this path carries a locally minted PolicyId and local timestamps for CreateDate/UpdateDate (the same value for both, i.e. the moment the catalog was seeded) rather than AWS's published identifier and dates. The divergence is reported for every member of the page.
Why: a client that pins an AWS-managed policy by PolicyId — the member AWS documents as the stable unique identifier — resolves a different policy, or none, against the emulator than against AWS; the same identifier read twice from two emulator builds is a different string, so any store that caches or asserts on it breaks. Tooling that reports or sorts the managed-policy catalog by CreateDate/UpdateDate (age-based inventory and drift reports, "policies updated since <date>" filters) sees every AWS policy as created at emulator-seed time, so every such filter returns the whole catalog or nothing.
Fix: the IAM managed-policy catalog seed data that the ListPolicies handler projects (the AWS-owned policy state, not the caller-created policy state) — the published PolicyId/CreateDate/UpdateDate must be part of the seeded record and be served verbatim, instead of being generated when the catalog is built.
Traces: iam-catalog-list-policies#task#3.

Cause 2 of 2: ListPolicies restricted to the /service-role/ path (fixture iam-catalog-list-policies, step task#4).
AWS: same operation and same shapes as Cause 1. The recording shows the AWS-managed service-role policy catalog: a large set of arn:aws:iam::aws:policy/service-role/<name> entries, each with its published PolicyName, Path (/service-role/), PolicyId, DefaultVersionId, IsAttachable and CreateDate/UpdateDate. The requirement is that every AWS-managed policy in the scope requested is present in the result set with its published metadata; AttachmentCount and PermissionsBoundaryUsageCount are account-dependent and the recording does not assert them. Status 200.
Emulator: answers 200 with a strictly smaller set — a substantial number of AWS-managed service-role policies are absent from the response entirely (none of the member entries for them is emitted), and the entries that are present repeat Cause 1's metadata defect (locally minted PolicyId, local CreateDate/UpdateDate, and in several cases a DefaultVersionId other than the published one).
Why: AttachRolePolicy/AttachUserPolicy flows in real code are commonly driven by discovering the policy through ListPolicies first; a policy that is not in the catalog cannot be discovered, so a caller that enumerates the AWS service-role policies to find, for example, the EMR, Auto Scaling, CloudHSM or Lambda execution policies finds nothing and either skips the attachment or fails. Because the response is a successful 200 with a short list rather than an error, the gap is silent: the client concludes AWS does not offer the policy. A wrong DefaultVersionId additionally sends a subsequent GetPolicyVersion to a version that does not describe the policy's effective document.
Fix: same component as Cause 1 — the IAM AWS-managed policy catalog seed; the /service-role/ scope of the catalog must be completed to the published set and each record must carry its published metadata.
Traces: iam-catalog-list-policies#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true; /__admin/health answered 200 and the data listener accepts). Both clusters: verdict mismatch, expected status 200, actual status 200 — status line, error code and error message are not involved on either side. stub_path empty, owned_by empty for both.

Cause 1 — ListPolicies, fixture iam-catalog-list-policies step task#3, 200 vs 200. Diverging keys are $.ListPoliciesResponse.ListPoliciesResult.Policies.member[Arn="arn:aws:iam::aws:policy/root-task/<PolicyName>"].PolicyId, .CreateDate and .UpdateDate for every member of the page, e.g.
  member[Arn="arn:aws:iam::aws:policy/root-task/SQSUnlockQueuePolicy"].PolicyId: expected 'ANPAZKAPJZG4HPZI4FDUV', got 'ANPAJT6MZQWKJ5PVNHLYB'
  member[Arn="arn:aws:iam::aws:policy/root-task/SQSUnlockQueuePolicy"].CreateDate: expected '2024-11-06T21:51:02Z', got '2024-11-13T16:52:43Z'
  member[Arn="arn:aws:iam::aws:policy/root-task/SQSUnlockQueuePolicy"].UpdateDate: expected '2024-11-06T21:51:02Z', got '2024-11-13T16:52:43Z'
  member[Arn="arn:aws:iam::aws:policy/root-task/S3UnlockBucketPolicy"].PolicyId: expected 'ANPAZKAPJZG4OALTIPZOF', got 'ANPAHS4QJZMWT3KVPNHLR'
  member[Arn="arn:aws:iam::aws:policy/root-task/IAMAuditRootUserCredentials"].PolicyId: expected 'ANPAZKAPJZG4BDSCREYVS', got 'ANPAEP5QJZMWT2KVPNHLB'
  (also reported for IAMCreateRootUserPassword and IAMDeleteRootUserCredentials).
The individual strings quoted here are the catalog values this recording captured; the requirement is that the PolicyId/CreateDate/UpdateDate served for an AWS-owned policy are the ones AWS publishes for that policy ARN, not that they equal these literals for some other policy. Note the shape of the actual values: the expected PolicyIds for policies created in one AWS change share the 'ANPAZKAPJZG4' prefix and all three members carry one coherent timestamp per policy, while the emulator's are unrelated per-policy strings all stamped with one seed time ('2024-11-13T16:52:...').

Cause 2 — ListPolicies, fixture iam-catalog-list-policies step task#4, 200 vs 200, path /service-role/. Two kinds of divergence in one cluster:
  (a) whole members missing, e.g.
      member[Arn="arn:aws:iam::aws:policy/service-role/AmazonElasticMapReduceforEC2Role"]: missing (expected {'Path': '/service-role/', 'UpdateDate': '2017-08-11T23:57:30Z', 'DefaultVersionId': 'v3', 'PolicyId': 'ANPAIGALS5RCDLZLB3PGS', 'IsAttachable': 'true', 'PolicyName': 'AmazonElasticMapReduceforEC2Role', 'Arn': '...', 'CreateDate': '2015-02-06T18:41:21Z', 'AttachmentCount': '<shaped-not-asserted>', 'PermissionsBoundaryUsageCount': '<shaped-not-asserted>'}, got None)
      member[Arn="arn:aws:iam::aws:policy/service-role/AutoScalingNotificationAccessRole"]: missing (expected {... 'PolicyId': 'ANPAIO2VMUPGDC5PZVXVA' ...}, got None)
      member[Arn="arn:aws:iam::aws:policy/service-role/AWSCloudHSMRole"]: missing (expected {... 'PolicyId': 'ANPAI7QIUU4GC66SF26WE' ...}, got None)
      the cluster's key list enumerates many more absent members, among them AWSBatchServiceRole, AWSCodeDeployRole, AWSLambdaDynamoDBExecutionRole, AWSLambdaKinesisExecutionRole, AWSLambdaVPCAccessExecutionRole, AmazonEC2ContainerServiceRole, AmazonEC2RoleforSSM, AmazonDMSVPCManagementRole, AmazonMacieServiceRole, AmazonRDSEnhancedMonitoringRole, AWSConfigRulesExecutionRole, CloudWatchEventsInvocationAccess and AmazonElasticTranscoderRole.
  (b) present members with wrong published metadata, e.g.
      member[Arn="arn:aws:iam::aws:policy/service-role/AmazonElasticMapReduceRole"].PolicyId: expected 'ANPAIDI2BQT2LKXZG36TW', got 'ANPADN3ZQTJWK7PVNHLYR'
      member[Arn="arn:aws:iam::aws:policy/service-role/AmazonElasticMapReduceRole"].UpdateDate: expected '2020-06-24T22:24:20Z', got '2023-05-04T19:27:13Z'
      plus .PolicyId/.CreateDate/.UpdateDate/.DefaultVersionId keys for AWSGlueServiceRole, AmazonEBSCSIDriverPolicy, AmazonEFSCSIDriverPolicy, AWSLambdaBasicExecutionRole and AmazonECSTaskExecutionRolePolicy.
No other member of either page diverges, so the fault is confined to the AWS-owned catalog records themselves.

## Steps to Reproduce
1. Clone this checkout and boot the emulator built from it.
2. Replay fixture iam-catalog-list-policies.
3. At step task#3 (ListPolicies over the /root-task/ path) compare Policies.member[*].PolicyId, .CreateDate and .UpdateDate against the recorded AWS response: the emulator serves locally minted ANPA... ids and one seed timestamp for every policy.
4. At step task#4 (ListPolicies over the /service-role/ path) diff the set of returned Arn values against the recorded set: a large number of AWS-managed service-role policies are absent, and several that are present carry a different PolicyId/DefaultVersionId/CreateDate/UpdateDate.
5. Or directly: call ListPolicies with Scope=AWS and PathPrefix=/service-role/ (paging with Marker/MaxItems to the end) and look up any of the absent policies, e.g. GetPolicy on arn:aws:iam::aws:policy/service-role/AmazonElasticMapReduceforEC2Role, then compare the PolicyId and CreateDate of a policy that IS returned (e.g. arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole) with the value AWS publishes for that ARN.
