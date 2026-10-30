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
Issue-Key: iam|GetServiceLastAccessedDetails|2xx->2xx|$.GetServiceLastAccessedDetailsResponse.GetServiceLastAccessedDetailsResult.ServicesLastAccessed.member[*]
Issue-Key: iam|GetServiceLastAccessedDetails|2xx->2xx|$.GetServiceLastAccessedDetailsResponse.GetServiceLastAccessedDetailsResult.ServicesLastAccessed.member[*].ServiceName
Issue-Key: iam|GetServiceLastAccessedDetails|2xx->2xx|$.GetServiceLastAccessedDetailsResponse.GetServiceLastAccessedDetailsResult.ServicesLastAccessed.member[*].ServiceNamespace
Issue-Commit: 46ea0afdef4b8d7aa8ecb7f663be31c677c6bd6d

# iam: built-in AWS reference catalogs diverge from AWS (managed-policy metadata, /root-task/ policies, Access Advisor service list)

> _Note: this issue was identified against the code base at commit `46ea0afdef4b8d7aa8ecb7f663be31c677c6bd6d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPolicies

## Description
Three net-new wire divergences in the iam crate share one root cause: the service's built-in, non-account AWS reference data (the AWS-managed policy catalog and the Access Advisor service-namespace catalog) is synthesized from names/defaults rather than reproducing the catalog AWS serves. Every one answers HTTP 200 with a well-formed but wrong body, so no client sees an error — they just read wrong facts about AWS itself. THREE CAUSES, all must be closed.

Cause 1 of 3 — ListPolicies Scope=AWS: every AWS-managed policy carries synthesized PolicyId/DefaultVersionId/CreateDate/UpdateDate/IsAttachable.
AWS: Smithy ListPolicies(input Scope: policyScopeType, PathPrefix: policyPathType, output Policies: policyListType) returns a Policy member per AWS-managed policy. Smithy Policy.PolicyId -> idType @length(min 16, max 128) @pattern ^[\w]+$ and is documented as "the stable and unique string identifying the policy"; AWS mints it as the 21-character ANPA-prefixed unique id of that specific policy (ANPA + 17 uppercase alphanumerics), never a transform of the policy name. Policy.DefaultVersionId -> policyVersionIdType @pattern ^v[1-9][0-9]*(\.[A-Za-z0-9-]*)?$ is the version currently set as default for that policy, which for a long-lived AWS-managed policy is typically well past v1. Policy.CreateDate and Policy.UpdateDate are that policy's own publication and last-revision instants, so they differ per policy. Policy.IsAttachable states whether the policy can be attached to a user, group or role; AWS reports true for the aws-service-role/ policies in this catalog (recorded behavior for arn:aws:iam::aws:policy/aws-service-role/AmazonElasticsearchServiceRolePolicy and .../AWSElasticBeanstalkServiceRolePolicy).
Emulator: every catalog entry is stamped from the same template instead of from per-policy catalog data — one shared CreateDate and UpdateDate ('2015-02-06T18:39:46Z') for all policies, DefaultVersionId always 'v1', IsAttachable 'false' for the aws-service-role/ policies, and PolicyId derived mechanically from the policy name and path ('ANPA_AWSSERVICEROLE_AMAZONELASTICSEARCHSERVICEROLEPOLICY', 'ANPA_AWSSERVICEROLE_AWSELASTICBEANSTALKSERVICEROLEPOLICY') rather than the policy's own ANPA id. The divergence is structural (the same five members are wrong for every member of the list); the specific recorded dates/versions/ids are the catalog's data, not a per-run value, and the requirement is that each policy carry its own values, not that these literals be echoed.
Why: a client that keys a managed policy by PolicyId cannot correlate it with anything AWS returns elsewhere (GetPolicy, GetAccountAuthorizationDetails, CloudTrail), a client that pins or compares DefaultVersionId before calling GetPolicyVersion asks for a version that does not describe the live policy, a client that sorts/filters the catalog by CreateDate/UpdateDate sees one undifferentiated timestamp, and a tool that honours IsAttachable skips the service-role policies entirely as unattachable so an AttachRolePolicy step is never attempted.
Fix: the AWS-managed policy catalog seed data the iam ListPolicies handler enumerates, and the Policy member serializer that fills these five members from it.

Cause 2 of 3 — ListPolicies with a /root-task/ PathPrefix returns no policies: the catalog has no root-task slice.
AWS: Smithy ListPolicies input PathPrefix -> policyPathType @pattern ^((/[A-Za-z0-9\.,\+@=_-]+)*)/$ filters the catalog by policy path; the output Policies list then carries the matching Policy members. Recorded behavior shows AWS serves a populated member list for the AWS-managed policies published under Path '/root-task/' — SQSUnlockQueuePolicy, S3UnlockBucketPolicy, IAMAuditRootUserCredentials, IAMCreateRootUserPassword, IAMDeleteRootUserCredentials (arn:aws:iam::aws:policy/root-task/<name>), each with its own ANPA PolicyId, DefaultVersionId 'v1', IsAttachable 'true' and its own CreateDate/UpdateDate.
Emulator: the replay reports $.ListPoliciesResponse.ListPoliciesResult.Policies as a type_mismatch against that expected member list — the emulator answers 200 with no member list at all for this path prefix, i.e. the built-in catalog contains no /root-task/ policies, so the filtered page is empty.
Why: an empty 200 is indistinguishable from "this account/partition publishes no root-task policies", so a client enumerating the root-session unlock policies to attach or audit them finds nothing and silently skips the whole step rather than failing loudly.
Fix: the built-in AWS-managed policy catalog seed (it must carry the /root-task/ scoped policies) and the PathPrefix filter in the ListPolicies handler that selects from it. Note: a previously RESOLVED issue, 20261025T153722Z-iam-3-net-new-root-causes-the-built.md, covered other omissions from this same catalog; it is closed and does not cover this path slice.

Cause 3 of 3 — GetServiceLastAccessedDetails: the Access Advisor service-namespace catalog is incomplete and short, so every entry after the first gap names the wrong service.
AWS: Smithy GetServiceLastAccessedDetails output ServicesLastAccessed is @required; each ServiceLastAccessed member has @required ServiceNamespace (serviceNamespaceType @length(min 1, max 64) @pattern ^[\w-]*$) and @required ServiceName. AWS reports one member per service namespace covered by the report, in a stable namespace-sorted order, and the report covers the full set of AWS service namespaces (the recording carries 200+ members).
Emulator: the catalog it enumerates is missing namespaces and ends early. Where AWS reports 'action-recommendations'/'AWS Action Recommendations', 'agent-registry'/'AWS Agent Registry', 'aidevops'/'AWS DevOps Agent Service', 'aiops'/'Amazon AI Operations' at the head of the list, the emulator's list jumps straight to 'airflow'/'Amazon Managed Workflows for Apache Airflow', 'amplify'/'AWS Amplify', 'amplifybackend'/'AWS Amplify Admin', 'amplifyuibuilder'/'AWS Amplify UI Builder', 'apigateway'/'Amazon API Gateway' — so each absent namespace shifts every later member up one position, and the replay reports the expected members at the tail of the list (indexes 100 through 199) as missing outright.
Why: Access Advisor is read to decide whether a principal ever used a service. A client that looks up a namespace to check it sees no entry and concludes the service was never accessed; a client that reads the list positionally attributes one service's LastAuthenticated / LastAuthenticatedEntity / TotalAuthenticatedEntities to a different service, which turns a least-privilege pruning pass into a wrong revocation.
Fix: the Access Advisor service-namespace catalog (the seed/state the GetServiceLastAccessedDetails handler enumerates) must cover the full namespace set with AWS's namespace and display name per entry, and the handler's page assembly must not truncate the list before the report ends.

Traces (examples, not requirements): cause 1 — iam-catalog-list-policies#task#1 (also task#2, task#4); cause 2 — iam-catalog-list-policies#task#3; cause 3 — iam-service-access-analysis-and-role-template-reports#task#4.

## Evidence
Cause 1 — fixture iam-catalog-list-policies, step task#1 (cluster also covers task#2, task#4), ListPolicies, expected 200 / actual 200:
  $...Policies.member[Arn="arn:aws:iam::aws:policy/aws-service-role/AmazonElasticsearchServiceRolePolicy"].UpdateDate: expected '2026-06-22T12:27:26Z', got '2015-02-06T18:39:46Z'
  ....DefaultVersionId: expected 'v8', got 'v1'
  ....PolicyId: expected 'ANPAJFEWZPHXKLCVHEUIC', got 'ANPA_AWSSERVICEROLE_AMAZONELASTICSEARCHSERVICEROLEPOLICY'
  ....IsAttachable: expected 'true', got 'false'
  ....CreateDate: expected '2017-07-07T00:15:31Z', got '2015-02-06T18:39:46Z'
  same five members wrong for arn:aws:iam::aws:policy/aws-service-role/AWSElasticBeanstalkServiceRolePolicy (expected DefaultVersionId 'v8', PolicyId 'ANPAIID62QSI3OSIPQXTM')
  -> the emulator returns one shared timestamp, v1, name-derived PolicyId and IsAttachable=false for every catalog entry; AWS returns per-policy values.

Cause 2 — fixture iam-catalog-list-policies, step task#3, ListPolicies, expected 200 / actual 200:
  $.ListPoliciesResponse.ListPoliciesResult.Policies: type_mismatch — expected a member list of the AWS-managed /root-task/ policies (SQSUnlockQueuePolicy, S3UnlockBucketPolicy, IAMAuditRootUserCredentials, IAMCreateRootUserPassword, IAMDeleteRootUserCredentials, each Path '/root-task/', DefaultVersionId 'v1', IsAttachable 'true', ANPA PolicyId); the emulator's Policies node carries no member list.

Cause 3 — fixture iam-service-access-analysis-and-role-template-reports, step task#4, GetServiceLastAccessedDetails, expected 200 / actual 200:
  $...ServicesLastAccessed.member[5].ServiceNamespace: expected 'action-recommendations', got 'airflow'
  member[5].ServiceName: expected 'AWS Action Recommendations', got 'Amazon Managed Workflows for Apache Airflow'
  member[6]: expected 'agent-registry'/'AWS Agent Registry', got 'amplify'/'AWS Amplify'
  member[7]: expected 'aidevops'/'AWS DevOps Agent Service', got 'amplifybackend'/'AWS Amplify Admin'
  member[8]: expected 'aiops'/'Amazon AI Operations', got 'amplifyuibuilder'/'AWS Amplify UI Builder'
  member[9]: expected 'airflow'/'Amazon Managed Workflows for Apache Airflow', got 'apigateway'/'Amazon API Gateway'
  members[100]..[199]: missing entirely.
  -> four namespaces absent at the head shift the whole list, and the list terminates ~100 members before AWS's.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture iam-catalog-list-policies step task#1 (and task#2, task#4) — IAM ListPolicies with Scope=AWS. Inspect any aws-service-role/ policy member in ListPoliciesResult.Policies and compare PolicyId, DefaultVersionId, CreateDate, UpdateDate and IsAttachable against the recorded AWS response: all five are template values rather than that policy's own.
3. Cause 2: replay fixture iam-catalog-list-policies step task#3 — IAM ListPolicies filtered to the '/root-task/' path prefix. The emulator answers 200 with no Policies member list; AWS returns the five AWS-managed /root-task/ policies.
4. Cause 3: replay fixture iam-service-access-analysis-and-role-template-reports step task#4 — GenerateServiceLastAccessedDetails then GetServiceLastAccessedDetails for the returned JobId. Walk ServicesLastAccessed: the 'action-recommendations', 'agent-registry', 'aidevops' and 'aiops' namespaces are absent so member[5] onward names the wrong service, and the list stops before index 100 where AWS still has entries.
5. After fixing, re-replay both fixtures' listed steps; all three clusters must pass.
