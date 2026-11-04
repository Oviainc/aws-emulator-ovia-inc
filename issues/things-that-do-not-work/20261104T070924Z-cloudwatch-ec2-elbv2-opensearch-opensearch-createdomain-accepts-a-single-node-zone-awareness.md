Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2__elbv2__opensearch.albos-single-node-vpc-domain-given-both-private-subnets-rejected
Issue-Key: cloudwatch__ec2__elbv2__opensearch.albos-single-node-vpc-domain-given-both-private-subnets-rejected|_refusal#9|AssertionError|raise AssertionError("the call was expected to be refused, but it succeeded")
Issue-Commit: 71ad45cb8263c75348f691f106f1f184e4e6c4d9

# opensearch CreateDomain accepts a single-node, zone-awareness-off VPC domain given two subnets instead of refusing it with ValidationException "You must specify exactly one subnet."

> _Note: this issue was identified against the code base at commit `71ad45cb8263c75348f691f106f1f184e4e6c4d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
ONE net-new root cause in the `cloudwatch__ec2__elbv2__opensearch` service grouping, grounded in 1 failing behavioral check that the replay report marks neither ALREADY FILED nor OWNED.

=== Cause 1 of 1: CreateDomain does not cross-check VPCOptions.SubnetIds count against the cluster's zone-awareness/node topology, so a one-node, ZoneAwarenessEnabled=false VPC domain naming TWO subnets is accepted (HTTP 200) instead of refused ===
Failing check ids: cloudwatch__ec2__elbv2__opensearch.albos-single-node-vpc-domain-given-both-private-subnets-rejected.

AWS: Smithy com.amazonaws.opensearch#CreateDomain — POST /2021-01-01/opensearch/domain, success 200, output DomainStatus; input DomainName (@required, @length 3..28, pattern ^[a-z][a-z0-9\-]+$), ClusterConfig, VPCOptions, EncryptionAtRestOptions, NodeToNodeEncryptionOptions, DomainEndpointOptions; modeled client errors ValidationException (400), BaseException (400, defaulted), InvalidTypeException (409), LimitExceededException (409), ResourceAlreadyExistsException (409), DisabledOperationException (409). Smithy: VPCOptions.SubnetIds documents "If your domain uses multiple Availability Zones, you need to provide two subnet IDs, one per zone. Otherwise, provide [one]"; ClusterConfig.ZoneAwarenessEnabled gates multi-AZ. The refusal itself is recorded behavior only: a domain with ZoneAwarenessEnabled=false (one data node, no zone awareness) given more than one subnet is refused SYNCHRONOUSLY with HTTP 400 ValidationException whose message is the fixed sentence "You must specify exactly one subnet." (the whole message is fixed text for this class — it carries no request or state values), and NOTHING is created: DescribeDomain on that name then answers ResourceNotFoundException with the message "Domain not found: <DomainName>" (fixed text "Domain not found: " plus the requested DomainName) and ListDomainNames does not list it. The identical request naming exactly one subnet is accepted (200, Created=true, Deleted=false) and DescribeDomain reports VPCOptions.VPCId = that subnet's VPC with SubnetIds = that one subnet and SecurityGroupIds = the requested group.

Emulator: the call is not refused at all. The check's refusal probe raises `AssertionError: the call was expected to be refused, but it succeeded` at the CreateDomain with VPCOptions.SubnetIds = [subnet-a, subnet-b] while ClusterConfig is {InstanceType: t3.small.search, InstanceCount: 1, ZoneAwarenessEnabled: false, MultiAZWithStandbyEnabled: false}. The emulator answers 200 and creates the domain, so the whole "effect absent" leg (DescribeDomain ResourceNotFoundException, name absent from ListDomainNames) is unreachable.

Why: a client relying on AWS's behavior never learns that its subnet list contradicts its zone-awareness setting. On AWS the misconfiguration is caught at submit time with an actionable message; against the emulator the request silently succeeds and the client believes it provisioned a single-AZ domain pinned to two subnets — a topology AWS never allows. IaC/drift tooling and negative-path tests that assert the refusal pass on AWS and fail here, and a template that AWS rejects appears valid.

Fix: the VPCOptions validation in the CreateDomain handler of the OpenSearch service crate (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/create_domain.rs and the shared cluster/VPC validation it calls) — reject before any domain record is written when ClusterConfig.ZoneAwarenessEnabled is false (or absent) and VPCOptions.SubnetIds holds more than one id, raising the modeled ValidationException with AWS's exact-one-subnet sentence, and leave no domain behind so DescribeDomain/ListDomainNames still report it absent.

Traces: cloudwatch__ec2__elbv2__opensearch.albos-single-node-vpc-domain-given-both-private-subnets-rejected.

## Evidence
Replay report, suite simparity-behavioral-aws-v1, service heading `### cloudwatch__ec2__elbv2__opensearch` (0/2 passed). The check below is marked neither ALREADY FILED nor OWNED.

[mismatch] cloudwatch__ec2__elbv2__opensearch.albos-single-node-vpc-domain-given-both-private-subnets-rejected
  Setup (already passing): the shared network VPC is described, its two network subnets (one per AZ, sorted by AZ) and the VPC's `default` security group are resolved, and iam CreateServiceLinkedRole(AWSServiceName='opensearchservice.amazonaws.com') is create-or-adopted.
  Request: opensearch CreateDomain(DomainName=<unique>, EngineVersion='OpenSearch_2.11', ClusterConfig={'InstanceType':'t3.small.search','InstanceCount':1,'ZoneAwarenessEnabled':False,'MultiAZWithStandbyEnabled':False}, EBSOptions={'EBSEnabled':True,'VolumeType':'gp3','VolumeSize':10}, EncryptionAtRestOptions={'Enabled':True}, NodeToNodeEncryptionOptions={'Enabled':True}, DomainEndpointOptions={'EnforceHTTPS':True,'TLSSecurityPolicy':'Policy-Min-TLS-1-2-2019-07'}, AccessPolicies=<account-scoped es:* policy>, VPCOptions={'SubnetIds':[<subnet in AZ-1>, <subnet in AZ-2>],'SecurityGroupIds':[<default sg>]}, TagList=[...]).
  Expected (AWS): HTTP 400, error code ValidationException, message exactly "You must specify exactly one subnet." (fixed text, no variable parts); then DescribeDomain(DomainName=<same>) -> ResourceNotFoundException with message "Domain not found: <DomainName>"; ListDomainNames does not contain the name.
  Actual (emulator): the CreateDomain call returns successfully. Traceback: check line 34 -> `code, msg = _refusal(lambda: osc.create_domain(**_request(ctx, bad, ids, sg)))` -> check line 111 -> `AssertionError: the call was expected to be refused, but it succeeded`.
  Smithy citation: CreateDomain — POST /2021-01-01/opensearch/domain, 200; errors ValidationException {client,400}, BaseException {client,400,defaulted}, InvalidTypeException/LimitExceededException/ResourceAlreadyExistsException/DisabledOperationException {client,409}, InternalException {server,500}. VPCOptions.SubnetIds doc: two subnet ids only when the domain uses multiple Availability Zones; ClusterConfig.ZoneAwarenessEnabled is the multi-AZ switch.
  Positive control that must keep working (not yet reached): the identical request with SubnetIds=[<subnet in AZ-1>] alone returns DomainStatus with (DomainName, Created, Deleted) == (<name>, True, False) and DescribeDomain reports VPCOptions (VPCId, SubnetIds, SecurityGroupIds) == (<vpc>, [<subnet in AZ-1>], [<default sg>]).

## Steps to Reproduce
Boot the emulator and confirm ec2, iam and opensearch answer.
1. ec2 DescribeVpcs / DescribeSubnets to pick one VPC with two subnets in two different Availability Zones, and ec2 DescribeSecurityGroups(Filters=[{vpc-id},{group-name:default}]) for that VPC's default security group.
2. iam CreateServiceLinkedRole(AWSServiceName='opensearchservice.amazonaws.com') (tolerate the already-taken InvalidInput).
3. opensearch CreateDomain(DomainName=<name>, EngineVersion='OpenSearch_2.11', ClusterConfig={'InstanceType':'t3.small.search','InstanceCount':1,'ZoneAwarenessEnabled':False,'MultiAZWithStandbyEnabled':False}, EBSOptions={'EBSEnabled':True,'VolumeType':'gp3','VolumeSize':10}, EncryptionAtRestOptions={'Enabled':True}, NodeToNodeEncryptionOptions={'Enabled':True}, DomainEndpointOptions={'EnforceHTTPS':True,'TLSSecurityPolicy':'Policy-Min-TLS-1-2-2019-07'}, VPCOptions={'SubnetIds':[<both subnet ids>],'SecurityGroupIds':[<default sg>]}).
   AWS: HTTP 400 ValidationException "You must specify exactly one subnet."  Emulator: HTTP 200 and the domain is created.
4. opensearch DescribeDomain(DomainName=<name>). AWS: ResourceNotFoundException "Domain not found: <name>". Emulator: returns a DomainStatus.
5. opensearch ListDomainNames. AWS: <name> absent. Emulator: present.
6. Positive control: repeat step 3 with only the first subnet id. AWS and the fixed emulator: 200 with Created=true/Deleted=false, and DescribeDomain reports VPCOptions.VPCId = that subnet's VPC, SubnetIds = [that subnet], SecurityGroupIds = [<default sg>].
7. Re-replay cloudwatch__ec2__elbv2__opensearch.albos-single-node-vpc-domain-given-both-private-subnets-rejected.
