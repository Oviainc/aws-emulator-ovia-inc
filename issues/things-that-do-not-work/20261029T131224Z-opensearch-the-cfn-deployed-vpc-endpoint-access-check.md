Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.deploy-vpc-endpoint-cfn-policy-enforced
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.deploy-vpc-endpoint-cfn-policy-enforced|_wait_stack_terminal#9|Timeout|time.sleep(interval_s)
Issue-Commit: dbe80abdd81a79f1a95892fbee40e178463ee847

# opensearch: the CFN-deployed VPC-endpoint access check never produces a response — a stack carrying an AWS::OpenSearchService::Domain never reaches CREATE_COMPLETE, so AuthorizeVpcEndpointAccess/ListVpcEndpointAccess/RevokeVpcEndpointAccess are never exercised

> _Note: this issue was identified against the code base at commit `dbe80abdd81a79f1a95892fbee40e178463ee847`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
One root cause, grounded in 1 failing behavioral check (opensearch.deploy-vpc-endpoint-cfn-policy-enforced), which the replay report marks neither ALREADY FILED nor OWNED by any open issue. It reports only `Check timed out after 60.0s`: the emulator never produces a status, error code or body the check can grade.

Related open issues that do NOT carry this check's replay key, and which a fixer should read alongside this one but cannot close it with: 20261028T035419Z-cloudformation-aws-opensearchservice-domain-provisioner-sends-the-accesspolicies.md and 20261028T032345Z-ec2-elbv2-opensearch-cloudformation-provisioned-aws-opensearchservice-domain-is-refused-by.md (the AccessPolicies-as-object CreateDomain refusal), 20261028T033849Z-cloudwatch-iam-lambda-logs-opensearch-s3-cfn-stack-whose-only-resource-is-an-aws.md (a domain-only stack ending in rollback), and 20261027T004336Z-opensearch-ten-checks-never-produce-a-response-freshly.md (freshly created domains/VPC endpoints never observable as settled by an AWS-sized poll). None of them names opensearch.deploy-vpc-endpoint-cfn-policy-enforced, so closing them leaves this check failing.

Cause 1 of 1: a CloudFormation stack that provisions a VPC + Subnet + SecurityGroup and an AWS::OpenSearchService::Domain with VPCOptions and an AccessPolicies policy document never becomes usable to a client that polls DescribeStacks for a terminal stack status, so the whole classic (non-Serverless) VPC-endpoint access-grant flow behind it — CreateVpcEndpoint, then AuthorizeVpcEndpointAccess / ListVpcEndpointAccess / RevokeVpcEndpointAccess — is never reached at all.

AWS: Smithy com.amazonaws.opensearch (services/service-com-amazonaws-opensearch-amazonopensearchservice/smithy-model/com-amazonaws-opensearch-amazonopensearchservice.json):
- CreateDomain POST /2021-01-01/opensearch/domain, 200; member AccessPolicies targets PolicyDocument, a STRING (@length 0..102400). The AWS::OpenSearchService::Domain resource handler accepts AccessPolicies declared as a template OBJECT, resolves the intrinsics in it, serializes it to a JSON string and calls CreateDomain; the stack reaches CREATE_COMPLETE and publishes its Outputs (here DomainArn, SubnetId, SecurityGroupId).
- CreateVpcEndpoint POST /2021-01-01/opensearch/vpcEndpoints, 200, required DomainArn (pattern ^arn:aws[a-z\-]*:[a-z]+:[a-z0-9\-]+:[0-9]+:domain\/[a-z0-9\-]+$) and VpcOptions; output VpcEndpoint whose Status settles to ACTIVE.
- AuthorizeVpcEndpointAccess POST /2021-01-01/opensearch/domain/{DomainName}/authorizeVpcEndpointAccess, 200, required output AuthorizedPrincipal; errors BaseException/ValidationException (400), ResourceNotFoundException/DisabledOperationException/LimitExceededException (409).
- ListVpcEndpointAccess GET /2021-01-01/opensearch/domain/{DomainName}/listVpcEndpointAccess, 200, required output AuthorizedPrincipalList — on AWS the authorized account appears in that list as the account id or its arn:aws:iam::<account>:root form.
- RevokeVpcEndpointAccess POST /2021-01-01/opensearch/domain/{DomainName}/revokeVpcEndpointAccess, 200 with an empty body; recorded behavior: a revoke naming a principal that was never authorized is a successful no-op (or ResourceNotFoundException, 409) and grants nothing, while revoking the real grant removes it from ListVpcEndpointAccess. Readiness is recorded behavior only: a client polls DescribeStacks / DescribeDomain / DescribeVpcEndpoints at AWS-sized intervals (15-30 s) and the state it waits for is eventually OBSERVABLE on one of those reads.

Emulator: the check is killed at the 60 s harness cap inside its first wait, having produced no gradable response. Its stack wait polls DescribeStacks every 30 s and returns on CREATE_COMPLETE or on one of CREATE_FAILED / ROLLBACK_COMPLETE / ROLLBACK_FAILED / DELETE_FAILED / UPDATE_ROLLBACK_COMPLETE; two polls fit in the cap, so the stack is reported neither created nor failed-terminal in that window. Hypothesis (neither the model nor this recording states which step it is, so it must be confirmed from the emulator's own stack events): the Domain resource fails at create because the CFN provisioner forwards AccessPolicies as a JSON object and CreateDomain answers HTTP 400 `Invalid request provided: expected string, got object start` (the fixed text of the deserializer message, naming no member and carrying no request value), and because the check passes OnFailure="DELETE" the stack is then reported in a delete state (DELETE_IN_PROGRESS / DELETE_COMPLETE) that is terminal to the emulator but is not one of the statuses this client treats as terminal — so the poll loop spins to the cap instead of surfacing a failure. A second contributor on the same path, documented in 20261027T004336Z, is that a freshly created domain and VPC endpoint are reported un-settled on the read that immediately follows their create and settle only inside a sub-second window, so the chained waits (stack terminal, DescribeDomain Processing=false, DescribeVpcEndpoints Status=ACTIVE) each cost a full poll interval.

Why: a client that deploys an OpenSearch VPC domain through CloudFormation and then grants cross-account access to its VPC endpoint gets neither a success nor an error — only a hang. It never learns that the domain was refused, so it cannot retry or correct its template; and every contract behind the wait is unverifiable: that AuthorizeVpcEndpointAccess returns the account (bare id or root ARN) as AuthorizedPrincipal, that ListVpcEndpointAccess lists exactly the authorized principals, that a revoke for a never-authorized account grants nothing, and that revoking the real grant removes it.

Fix: the CloudFormation AWS::OpenSearchService::Domain resource provisioner's property-to-request mapping (a resolved policy-document property must be serialized to a JSON string before it is placed in the CreateDomain body) together with the stack-status reporting for an OnFailure=DELETE create, so a failed create is observable as a terminal stack status rather than leaving a poller spinning; and the OpenSearch crate's settling model (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/state.rs) so a just-created domain and VPC endpoint are observable as settled by a caller polling at AWS-sized intervals. Secondarily, a genuinely ill-typed CreateDomain body must be refused under a modeled client error (ValidationException or BaseException, HTTP 400) whose message names the member and the expected type, not with the deserializer's internal parser text.

Traces: opensearch.deploy-vpc-endpoint-cfn-policy-enforced.

## Evidence
Replay report, service opensearch (suite simparity-behavioral-ga-prod-v1):

  [mismatch] opensearch.deploy-vpc-endpoint-cfn-policy-enforced
  Error: Check timed out after 60.0s

No HTTP status, error code or message is reported — the emulator never answers anything the check can grade. The check is marked neither ALREADY FILED nor OWNED in the report.

What the check does (abridged from the replayed source):
1. CreateStack(OnFailure="DELETE", Parameters DomainName/AccountId) with a template declaring AWS::EC2::VPC (10.91.0.0/16, DNS support+hostnames), AWS::EC2::Subnet (10.91.1.0/24, !Select [0, !GetAZs '']), AWS::EC2::SecurityGroup (tcp 443 from 10.91.0.0/16) and AWS::OpenSearchService::Domain (EngineVersion OpenSearch_2.11, ClusterConfig {t3.small.search, 1, no dedicated master, no zone awareness}, EBSOptions {gp3, 10 GiB}, VPCOptions {SubnetIds [!Ref Subnet], SecurityGroupIds [!Ref SecurityGroup]}, AccessPolicies as a policy-document OBJECT allowing arn:aws:iam::<account>:root to es:* on the domain ARN). Outputs: DomainArn (!GetAtt Domain.Arn), SubnetId, SecurityGroupId.
2. Poll DescribeStacks every 30 s for CREATE_COMPLETE (or a terminal failure status), assert CREATE_COMPLETE, read the three Outputs.
3. Wait for DescribeDomain Processing=false, CreateVpcEndpoint(DomainArn, VpcOptions{subnet, sg}), wait DescribeVpcEndpoints Status=ACTIVE.
4. AuthorizeVpcEndpointAccess(DomainName, Account=<this account>) and assert AuthorizedPrincipal.Principal is the account id or arn:aws:iam::<account>:root.
5. ListVpcEndpointAccess(DomainName) and assert the account is listed.
6. RevokeVpcEndpointAccess(DomainName, Account=111111111111) — an account never authorized — and assert the call either no-ops (200) or raises ResourceNotFoundException, and that ListVpcEndpointAccess afterwards still lists the real account and does NOT list the never-authorized one.
7. RevokeVpcEndpointAccess for the real account and assert it disappears from ListVpcEndpointAccess.

Expected (AWS): step 2 returns CREATE_COMPLETE with resolved OutputValues; steps 4-7 all answer 200 and the listing changes exactly as asserted.

Actual (emulator): the check never gets past its first wait — it is killed at 60 s with the stack reported neither CREATE_COMPLETE nor in any of the failure statuses this client treats as terminal, so steps 3-7 are never issued and no response from AuthorizeVpcEndpointAccess / ListVpcEndpointAccess / RevokeVpcEndpointAccess is ever observed.

Message detail for the suspected underlying refusal (same text already recorded in 20261028T035419Z for a different check): HTTP 400, service message `Invalid request provided: expected string, got object start` — that whole string is fixed text; it names no member and interpolates nothing from the request. AWS would refuse an ill-typed body under ValidationException/BaseException (400) naming the member (AccessPolicies) and the expected type. Only the CloudFormation frame's Request ID / RequestToken are per-run values.

Smithy citations: AuthorizeVpcEndpointAccess (POST .../authorizeVpcEndpointAccess, 200, required AuthorizedPrincipal; ResourceNotFoundException 409), ListVpcEndpointAccess (GET .../listVpcEndpointAccess, 200, required AuthorizedPrincipalList), RevokeVpcEndpointAccess (POST .../revokeVpcEndpointAccess, 200, empty output), CreateVpcEndpoint (POST /2021-01-01/opensearch/vpcEndpoints, 200, required DomainArn+VpcOptions), CreateDomain member AccessPolicies -> PolicyDocument (string, @length 0..102400).

## Steps to Reproduce
1. Boot the emulator; confirm CloudFormation, EC2, STS and OpenSearch answer.
2. CreateStack with OnFailure="DELETE" and a template containing AWS::EC2::VPC + AWS::EC2::Subnet + AWS::EC2::SecurityGroup and one AWS::OpenSearchService::Domain with VPCOptions referencing that subnet and security group and AccessPolicies declared as a nested policy-document OBJECT (Principal.AWS: !Sub "arn:aws:iam::${AccountId}:root", Action es:*, Resource the domain ARN). Declare Outputs DomainArn/!GetAtt Domain.Arn, SubnetId, SecurityGroupId.
3. Poll DescribeStacks every 30 s, accepting only CREATE_COMPLETE or one of CREATE_FAILED / ROLLBACK_COMPLETE / ROLLBACK_FAILED / DELETE_FAILED / UPDATE_ROLLBACK_COMPLETE as terminal. On AWS this returns CREATE_COMPLETE. Observe that against the emulator it returns none of those within 60 s; dump DescribeStackEvents and the raw StackStatus to see which status it rests in (expected from the hypothesis: the Domain resource CREATE_FAILED with 'Invalid request provided: expected string, got object start' and the stack then in a delete state).
4. Narrow the refusal: call create_domain directly with the same properties but AccessPolicies passed as json.dumps(<policy>) — that must be accepted 200 — and compare with the body the CFN provisioner emits for the same resource.
5. Independently confirm the readiness contributor: CreateDomain a minimal single-node OpenSearch_2.11 VPC domain and poll DescribeDomain every 30 s for Processing=false, then CreateVpcEndpoint and poll DescribeVpcEndpoints every 15 s for Status=ACTIVE; each settled value must be observable to a poll at those intervals.
6. After the fix the full flow must run: stack CREATE_COMPLETE with resolved Outputs; CreateVpcEndpoint -> Status ACTIVE; AuthorizeVpcEndpointAccess returns AuthorizedPrincipal.Principal equal to the account id or arn:aws:iam::<account>:root; ListVpcEndpointAccess lists it; RevokeVpcEndpointAccess for a never-authorized account answers 200 (or ResourceNotFoundException) and adds nothing to the listing; RevokeVpcEndpointAccess for the real account removes it.
7. Re-replay opensearch.deploy-vpc-endpoint-cfn-policy-enforced.
