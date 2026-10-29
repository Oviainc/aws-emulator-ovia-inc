Issue-Source: trace-sweep-divergence
Issue-Trace: iam-catalog-list-policies
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-catalog-list-policies iam-service-access-analysis-and-role-template-reports
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|ListPolicies|2xx->2xx|$.ListPoliciesResponse.ListPoliciesResult.Policies.member[Arn="arn
Issue-Key: iam|ListPolicies|2xx->2xx|$.ListPoliciesResponse.ListPoliciesResult.Policies
Issue-Key: iam|GetServiceLastAccessedDetails|2xx->2xx|$.GetServiceLastAccessedDetailsResponse.GetServiceLastAccessedDetailsResult.IsTruncated
Issue-Key: iam|GetServiceLastAccessedDetails|2xx->2xx|$.GetServiceLastAccessedDetailsResponse.GetServiceLastAccessedDetailsResult.Marker
Issue-Key: iam|GetServiceLastAccessedDetails|2xx->2xx|$.GetServiceLastAccessedDetailsResponse.GetServiceLastAccessedDetailsResult.ServicesLastAccessed.member[*]
Issue-Key: iam|GetServiceLastAccessedDetails|2xx->2xx|$.GetServiceLastAccessedDetailsResponse.GetServiceLastAccessedDetailsResult.ServicesLastAccessed.member[*].ServiceName
Issue-Key: iam|GetServiceLastAccessedDetails|2xx->2xx|$.GetServiceLastAccessedDetailsResponse.GetServiceLastAccessedDetailsResult.ServicesLastAccessed.member[*].ServiceNamespace
Issue-Key: iam|GetServiceLastAccessedDetails|2xx->2xx|$.GetServiceLastAccessedDetailsResponse.GetServiceLastAccessedDetailsResult.ServicesLastAccessed.member[*].TotalAuthenticatedEntities
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# Seeded IAM reference data is incomplete for policies and service access

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPolicies

## Description
Cause 1 of 3 - the AWS-managed service-role policy catalog is missing entries. AWS: ListPolicies with Scope=AWS returns the account's view of the AWS-managed policy catalog, including the /aws-service-role/ policies (AmazonElasticsearchServiceRolePolicy, AWSElasticBeanstalkServiceRolePolicy, AWSEC2SpotServiceRolePolicy, AmazonRedshiftServiceLinkedRolePolicy and the rest), each with Arn, PolicyName, PolicyId, Path, DefaultVersionId, IsAttachable, CreateDate and UpdateDate. Emulator: those entries are absent from the result. Why: a client that resolves a service-role policy ARN by listing the catalog before attaching it cannot find a policy that exists in every real account, so service-role provisioning fails at the lookup. Fix: the seeded AWS-managed policy catalog in IAM state must include the /aws-service-role/ policies.
Traces: iam-catalog-list-policies#task#1.

Cause 2 of 3 - the job-function policy catalog is missing. AWS: the same catalog contains the /job-function/ policies (ViewOnlyAccess, SupportUser, SystemAdministrator, DatabaseAdministrator, DataScientist and the rest) with the same member fields. Emulator: the Policies element for the job-function-scoped page is serialised with none of them. Why: the same lookup failure for the job-function family, which is what human-role provisioning attaches. Fix: extend the same seeded catalog with the /job-function/ policies.
Traces: iam-catalog-list-policies#task#2.

Cause 3 of 3 - GetServiceLastAccessedDetails reports a different, unpaginated service set. AWS: the report enumerates every AWS service namespace the entity could have used, sorted by ServiceNamespace, each entry carrying ServiceName, ServiceNamespace and TotalAuthenticatedEntities; the result is truncated with IsTruncated 'true' and a Marker when the set exceeds one page. Emulator: it returns a short hand-picked set (cloudformation, cloudwatch, dynamodb, ...) in a different order, omits TotalAuthenticatedEntities on every entry, and reports IsTruncated 'false' with no Marker. Why: a last-accessed consumer reading page one concludes the entity touched a handful of services and never pages, so least-privilege analysis built on the report is wrong. Fix: seed the full service-namespace table and paginate the report (IsTruncated + Marker), emitting TotalAuthenticatedEntities per entry.
Traces: iam-service-access-analysis-and-role-template-reports#task#4.

## Evidence
Cause 1: expected (recorded AWS) ListPoliciesResult/Policies/member with Arn 'arn:aws:iam::aws:policy/aws-service-role/AmazonElasticsearchServiceRolePolicy' (and the other /aws-service-role/ ARNs), each with PolicyName, PolicyId, Path '/aws-service-role/', DefaultVersionId, IsAttachable 'true', CreateDate, UpdateDate; actual those members are absent (`missing (expected {...}, got None)` per ARN).
Cause 2: expected Policies/member to be the /job-function/ list (ViewOnlyAccess, SupportUser, SystemAdministrator, DatabaseAdministrator, DataScientist, ...); actual the Policies element does not carry them (type_mismatch against the expected member list).
Cause 3: expected IsTruncated 'true' plus a Marker, and ServicesLastAccessed/member[0..n] sorted by ServiceNamespace starting 'a4b' (Alexa for Business), 'access-analyzer' (AWS IAM Access Analyzer), 'account' (AWS Account Management), each with TotalAuthenticatedEntities '0'; actual IsTruncated 'false', no Marker, member[0]='cloudformation', [1]='cloudwatch', [2]='dynamodb', TotalAuthenticatedEntities absent, and members [12]..[199] missing entirely.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay iam-catalog-list-policies#task#1 (ListPolicies Scope=AWS, aws-service-role page) and #task#2 (job-function page); diff the Policies member set against the recording.
3. Replay iam-service-access-analysis-and-role-template-reports#task#4 (GetServiceLastAccessedDetails) and compare IsTruncated, Marker and the ServicesLastAccessed ordering/length.
