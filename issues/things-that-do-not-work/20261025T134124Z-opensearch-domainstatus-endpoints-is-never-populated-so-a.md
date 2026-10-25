Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.update-vpc-endpoint-invalid-subnet-rejected
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.update-vpc-endpoint-invalid-subnet-rejected|_wait_domain_active#11|Timeout|time.sleep(20)
Issue-Commit: fafe0142a789ac7a37106edfeaee5b3c191a47ea

# opensearch: DomainStatus.Endpoints is never populated, so a VPC-access domain never publishes a VPC endpoint and readiness polling never completes

> _Note: this issue was identified against the code base at commit `fafe0142a789ac7a37106edfeaee5b3c191a47ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDomain

## Description
One distinct root cause in the opensearch service.

Cause 1 of 1: a domain created with VPCOptions (VPC access) never reports the DomainStatus.Endpoints map, so no client can learn the domain's VPC endpoint or tell when a VPC domain is reachable.

AWS: a domain created with VPCOptions is reached through a VPC endpoint rather than a public one, and DomainStatus reports it in the Endpoints map. Smithy: opensearch#DomainStatus$Endpoints — "The key-value pair that exists if the OpenSearch Service domain uses VPC endpoints. For example: IPv4 IP addresses - 'vpc','vpc-endpoint-<opaque-id>.<region>.es.amazonaws.com'; Dual stack IP addresses - 'vpcv2':'vpc-endpoint-<opaque-id>.aos.<region>.on.aws'". The sibling member opensearch#DomainStatus$Endpoint is the "Domain-specific endpoint used to submit index, search, and data upload requests to the domain" — the PUBLIC endpoint, which a VPC-access domain does not have. The key names ('vpc', and 'vpcv2' for dualstack) and the fact that the map is present for a VPC domain are the requirement; the opaque host id in the example is a per-domain generated value, not part of the contract. For a public (no VPCOptions) domain the inverse holds: Endpoint is present and Endpoints is absent.

Emulator: DomainStatus.Endpoints is hardcoded absent for every domain, VPC-access or not, while Endpoint (search-<domain>-<suffix>.<region>.es.amazonaws.com) is emitted unconditionally — including for a domain created with VPCOptions, which in AWS has no such public endpoint. CreateDomain stores the requested VPCOptions and answers 200, and DescribeDomain then reports a VPC domain as if it were a public one, with no Endpoints member at all. A client that polls DescribeDomain for the standard VPC-domain readiness signal (Created=true, Processing=false, Endpoints present) therefore never converges: the behavioral check for UpdateVpcEndpoint's same-VPC subnet validation runs out the 60 s harness cap inside exactly that wait, before any UpdateVpcEndpoint call is made.

Why: for a VPC-access domain the Endpoints map is the only place the reachable host is published, so a client gets no address to send index/search traffic to, and gets a public endpoint it must not use instead. The presence of Endpoints is also the conventional readiness signal for a VPC domain, so waiters hang indefinitely rather than proceeding, and every downstream VPC-endpoint scenario (CreateVpcEndpoint/UpdateVpcEndpoint/DescribeVpcEndpoints) is unreachable even once those handlers exist.

Fix: the DomainStatus projection in the OpenSearch service crate's domain state (the to_status() projection and the domain record that backs it) — when a domain holds VPCOptions, mint and store a per-domain VPC endpoint host, report it under the 'vpc' key of Endpoints (plus 'vpcv2' when IPAddressType is dualstack), and withhold the public Endpoint for that domain; keep Endpoint-only for a domain created without VPCOptions.

Related but not covering this: open issue 20261024T055421Z-opensearch-createdomain-skips-documented-validation-describedomain-fabricates-empty.md lists this same check under its Cause 7, but that cause is about EC2 CreateTags rejecting its own subnet ids and a missing Service Quotas 'vpc' quota; the check now gets past both and fails on the domain-readiness/Endpoints gap described here. Open issue 20261024T055421Z-opensearch-domain-data-plane-is-not-served-and.md Cause 6 asks for Endpoint to be WITHHELD while a domain is still Creating; it does not state that a VPC domain must publish Endpoints at all.

Traces: opensearch.update-vpc-endpoint-invalid-subnet-rejected.

## Evidence
Check opensearch.update-vpc-endpoint-invalid-subnet-rejected — verdict mismatch, detail: "Error: Check timed out after 60.0s".

The check creates VPC A (10.30.0.0/16) with two subnets and a security group, VPC B (10.40.0.0/16) with one subnet, then calls opensearch.create_domain(DomainName=<name>, ClusterConfig={'InstanceType':'t3.small.search','InstanceCount':1,...}, EBSOptions={...}, VPCOptions={'SubnetIds':[subnet_a1],'SecurityGroupIds':[sg_a]}) and polls describe_domain until status.get('Created') and not status.get('Processing') and status.get('Endpoints'). The emulator's DomainStatus satisfies the first two conditions but never the third, so the poll loop runs to the harness cap and the UpdateVpcEndpoint assertions the check exists to grade (ValidationException for a subnet in a different VPC than the security group, subnet unchanged afterwards, same-VPC update accepted) are never reached.

Emulator source confirming the cause: services/service-com-amazonaws-opensearch-amazonopensearchservice/src/state.rs, in Domain::to_status(): `endpoint: Some(self.endpoint.clone())` is unconditional, and `endpoints: None` carries the comment "The `Endpoints` map exists only for a VPC domain's endpoints, which `CreateVpcEndpoint` would mint; it is still a stub." The domain record does hold the requested VPC configuration (Domain::vpc_options, set from CreateDomain input), so the information needed to publish the map is present.

## Steps to Reproduce
1. ec2.create_vpc(CidrBlock='10.30.0.0/16'); ec2.create_subnet(...); ec2.create_security_group(... VpcId=<vpc>).
2. opensearch.create_domain(DomainName=<d>, ClusterConfig={'InstanceType':'t3.small.search','InstanceCount':1,'DedicatedMasterEnabled':False,'ZoneAwarenessEnabled':False}, EBSOptions={'EBSEnabled':True,'VolumeType':'gp3','VolumeSize':10}, VPCOptions={'SubnetIds':[<subnet>],'SecurityGroupIds':[<sg>]}) -> 200.
3. opensearch.describe_domain(DomainName=<d>)['DomainStatus'] -> no 'Endpoints' member at all, and 'Endpoint': 'search-<d>-<suffix>.<region>.es.amazonaws.com' is present even though the domain is VPC-access. Expected: 'Endpoints' present as {'vpc': 'vpc-endpoint-<id>.<region>.es.amazonaws.com'} (plus 'vpcv2' for a dualstack domain) and no public 'Endpoint'.
4. Poll describe_domain waiting for Created=true, Processing=false and a non-empty Endpoints -> never satisfied; the check times out after 60 s.

Replay the check ids listed in trace_ids.
