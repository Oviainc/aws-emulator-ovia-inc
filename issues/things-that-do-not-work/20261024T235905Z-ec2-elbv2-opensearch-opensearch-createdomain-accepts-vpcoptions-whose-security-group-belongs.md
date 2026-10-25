Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__elbv2__opensearch.vpc-domain-and-internal-alb-with-a-security-group-from-another-vpc-rejected
Issue-Key: ec2__elbv2__opensearch.vpc-domain-and-internal-alb-with-a-security-group-from-another-vpc-rejected|run#13|AssertionError|assert code == "ValidationException", f"CreateDomain: expected ValidationException, got {code!r}: {message!r}"
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# OpenSearch CreateDomain accepts VPCOptions whose security group belongs to a different VPC than the subnets (200 Creating instead of ValidationException)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
One root cause, grounded in 1 failing behavioral check whose id's leading token is the composite 'ec2__elbv2__opensearch'.

Related open issue that does NOT carry this cause: 20261023T025808Z-ec2-elbv2-opensearch-opensearch-alb-diagnosis-scenarios-blocked-cfn-stack-outputs.md. Its Cause 2 is the ELBv2 side of the same scenario (CreateLoadBalancer answering InvalidAction instead of InvalidConfigurationRequest) and its replay key for this check is the CreateLoadBalancer frame (`run#6|AssertionError|assert code == "InvalidConfigurationRequest"`). The check now gets PAST that assertion — CreateLoadBalancer is refused with InvalidConfigurationRequest and the message 'One or more security groups are invalid', and the refused ALB is not describable — and fails further on, in the OpenSearch half of the scenario. This is a different cause in a different service's handler.

Cause 1 of 1: OpenSearch CreateDomain performs no cross-VPC consistency check between VPCOptions.SubnetIds and VPCOptions.SecurityGroupIds, so a domain placed in VPC A with a security group from VPC B is created instead of refused.

AWS: Smithy com.amazonaws.opensearch#CreateDomain (POST /2021-01-01/opensearch/domain, success code 200; DomainName required, @length 3..28, @pattern ^[a-z][a-z0-9\-]+$) models ValidationException as a client error with HTTP 400. A VPC domain's security groups must belong to the same VPC as its subnets; each id is valid on its own, so the service resolves the subnet's VPC and the security group's VPC in EC2 and compares them. When they differ, CreateDomain is refused with HTTP 400 ValidationException and NO domain is created, so a following DescribeDomain answers ResourceNotFoundException. Recorded behavior only for the refusal's message text, which the recording shows as 'Subnets and security groups must be in the same VPC.' — an example, not a requirement.

Emulator: the same CreateDomain call SUCCEEDS. The check reports `CreateDomain: expected ValidationException, got 'OK'` and the returned DomainStatus is a live, creating domain: DomainName/ARN minted, Created=true, Processing=true, DomainProcessingStatus='Creating', ChangeProgressDetails.ConfigChangeStatus='ApplyingChanges', and VPCOptions echoed back verbatim as the mismatched pair (SubnetIds from VPC A, SecurityGroupIds from VPC B). Nothing in the response signals the inconsistency, and the domain then exists, so the check's follow-on assertion that the refused CreateDomain leaves no domain can never hold either.

Why: a client relying on AWS's behavior uses this refusal as the signal that its VPC wiring is wrong. Against the emulator the misconfigured domain is reported as accepted and enters Creating, so an IaC deploy or a provisioning script proceeds as if the estate were sound; the domain's ENIs would never be reachable from the intended VPC, and the error surfaces (on real AWS) only much later, in traffic, instead of synchronously at create time. It also makes 'overly permissive' the emulator's answer on a negative-path gate: a test suite asserting that the bad combination is rejected passes vacuously.

Fix: the OpenSearch CreateDomain handler's input validation (the VPCOptions cross-field check that resolves SubnetIds' VPC and SecurityGroupIds' VPC from EC2 state and raises ValidationException/400 when they differ), and the handler's state write, which must not persist a domain when that validation fails. The ELBv2 CreateLoadBalancer equivalent of this check already behaves correctly and can be used as the reference for the comparison.

Traces: ec2__elbv2__opensearch.vpc-domain-and-internal-alb-with-a-security-group-from-another-vpc-rejected (check line 54, `assert code == "ValidationException"`).

## Evidence
Replay report, service ec2__elbv2__opensearch:
  [mismatch] ec2__elbv2__opensearch.vpc-domain-and-internal-alb-with-a-security-group-from-another-vpc-rejected
  AssertionError: CreateDomain: expected ValidationException, got 'OK': {'DomainId': '000000000000/sim2real-sg-...', 'DomainName': 'sim2real-sg-...', 'ARN': 'arn:aws:es:us-west-2:000000000000:domain/sim2real-sg-...', 'Created': True, 'Deleted': False, 'Processing': True, 'VPCOptions': {'SubnetIds': ['subnet-...'], 'SecurityGroupIds': ['sg-...']}, 'DomainProcessingStatus': 'Creating', 'ChangeProgressDetails': {'ConfigChangeStatus': 'ApplyingChanges', ...}, ...}

Expected (AWS): CreateDomain with VPCOptions = {SubnetIds: [<subnet of VPC A>], SecurityGroupIds: [<sg of VPC B>]} is refused with HTTP 400 and Error.Code 'ValidationException' (recorded message, as an example: 'Subnets and security groups must be in the same VPC.'), and no domain exists afterwards — DescribeDomain answers ResourceNotFoundException.

Actual (emulator): HTTP 200 with a DomainStatus whose Created=true / Processing=true / DomainProcessingStatus='Creating' and whose VPCOptions echo the cross-VPC pair unchanged; the domain exists afterwards.

Control within the same check (not reached): with the VPC's OWN security group, CreateDomain must be accepted and its VPCOptions must read back VPCId=<VPC A>, the same SubnetIds and the same SecurityGroupIds — so the fix must reject only the mismatched pair, not all VPC domains.

Smithy citation (services/service-com-amazonaws-opensearch-amazonopensearchservice/smithy-model/com-amazonaws-opensearch-amazonopensearchservice.json): CreateDomain errors include ValidationException {errorKind: client, httpError: 400}; http {method: POST, uri: /2021-01-01/opensearch/domain, code: 200}.

## Steps to Reproduce
1. Boot the emulator.
2. With EC2, create VPC A (two subnets in two AZs, one security group) and VPC B (same shape). Note subnet_a = a subnet of VPC A and sg_b = the security group of VPC B.
3. Confirm the ELBv2 control is in place: boto3.client('elbv2').create_load_balancer(Name=tok, Subnets=<VPC A subnets>, SecurityGroups=[sg_b], Scheme='internal', Type='application') is refused with InvalidConfigurationRequest — this already passes.
4. Call boto3.client('opensearch').create_domain(DomainName=tok, EngineVersion='OpenSearch_2.11', ClusterConfig={'InstanceType': 't3.small.search', 'InstanceCount': 1}, EBSOptions={'EBSEnabled': True, 'VolumeType': 'gp3', 'VolumeSize': 10}, VPCOptions={'SubnetIds': [subnet_a], 'SecurityGroupIds': [sg_b]}).
5. Observe HTTP 200 with a creating DomainStatus. AWS refuses this with HTTP 400 ValidationException and creates nothing.
6. Call describe_domain(DomainName=tok): on AWS it answers ResourceNotFoundException; on the emulator the domain is there.
7. Positive control: repeat step 4 with VPCOptions.SecurityGroupIds = [<security group of VPC A>]; this must still be accepted and its VPCOptions must read back VPCId=<VPC A>.
8. Re-replay ec2__elbv2__opensearch.vpc-domain-and-internal-alb-with-a-security-group-from-another-vpc-rejected.
