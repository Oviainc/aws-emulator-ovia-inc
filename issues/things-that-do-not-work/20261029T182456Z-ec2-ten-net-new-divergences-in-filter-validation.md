Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-delete-traffic-mirror-filter ec2.op-contract-delete-traffic-mirror-session ec2.op-contract-delete-traffic-mirror-target ec2.op-contract-describe-traffic-mirror-targets ec2.op-contract-describe-ipam-scopes ec2.op-contract-describe-ipam-scopes-superseded-stub ec2.op-contract-deprovision-ipam-pool-cidr ec2.op-contract-describe-public-ipv4-pools ec2.op-contract-deprovision-public-ipv4-pool-cidr-unprovisioned-rejected ec2.network-acl-entry-rule-number-out-of-range-rejected ec2.network-performance-subscription-invalid-region-rejected ec2.ipam-e2e-ipam-pool-and-vpc-lifecycle ec2.reject-transit-gateway-peering-attachment-wrong-state-rejected ec2.op-contract-modify-ipam-policy-allocation-rules ec2.op-contract-update-interruptible-capacity-reservation-allocation ec2.vpc-name-resolution-dhcp-black-hole-nameserver-fails-guest-lookups-while-default-resolves
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-delete-traffic-mirror-filter|_latest_amazon_linux_ami#6|IndexError|return images[0]["ImageId"]
Issue-Key: ec2.op-contract-delete-traffic-mirror-session|_latest_amazon_linux_ami#6|IndexError|return images[0]["ImageId"]
Issue-Key: ec2.op-contract-delete-traffic-mirror-target|_latest_amazon_linux_ami#7|IndexError|return images[0]["ImageId"]
Issue-Key: ec2.op-contract-describe-traffic-mirror-targets|run#17|KeyError|eni_id = ec2.describe_instances(InstanceIds=[instance_id])["Reservations"][0]["Instances"][
Issue-Key: ec2|DescribeIpamScopes|InvalidParameterValue
Issue-Key: ec2|DescribePublicIpv4Pools|InvalidParameterValue
Issue-Key: ec2|DeprovisionPublicIpv4PoolCidr|InvalidParameterValue
Issue-Key: ec2.network-acl-entry-rule-number-out-of-range-rejected|run#7|AssertionError|assert code == "InvalidParameterValue", f"expected InvalidParameterValue, got {code}"
Issue-Key: ec2.network-performance-subscription-invalid-region-rejected|run#7|AssertionError|assert code == "InvalidParameter", f"expected InvalidParameter for Statistic={bogus_statistic!r}, got {code!r}"
Issue-Key: ec2|CreateStack|ValidationError
Issue-Key: ec2.reject-transit-gateway-peering-attachment-wrong-state-rejected|run#9|AssertionError|raise AssertionError(
Issue-Key: ec2.op-contract-modify-ipam-policy-allocation-rules|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Key: ec2.op-contract-update-interruptible-capacity-reservation-allocation|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: ec2.vpc-name-resolution-dhcp-black-hole-nameserver-fails-guest-lookups-while-default-resolves|_poll#2|Timeout|time.sleep(interval_s)
Issue-Commit: ab6490681e1ab26d5567ff83a1d290924eb5dab0

# EC2: ten net-new divergences in filter validation, IPAM/public-IPv4-pool refusals, the AMI catalogue, DescribeInstances projection, CFN IPAM resource types and non-converging lifecycles (10 causes)

> _Note: this issue was identified against the code base at commit `ab6490681e1ab26d5567ff83a1d290924eb5dab0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeIpamScopes

## Description
Ten distinct root causes in the EC2 emulator, all grounded in failing behavioral checks of the ec2 service that no open issue carries the replay key for. A fixer must close all ten before submitting; each cause below names its own failing check ids.

Cause 1 of 10: the seeded AMI catalogue contains no Amazon Linux 2 AMI, so any scenario that has to launch an instance from one cannot start.
AWS: DescribeImages(Owners=['amazon'], Filters=[{name: ['amzn2-ami-hvm-*-x86_64-gp2']}, {state: ['available']}, {root-device-type: ['ebs']}]) returns the public Amazon-owned Amazon Linux 2 AMIs. Smithy: DescribeImages models no errors (EC2 uses the ec2Query common error set); the requirement here is catalogue content, so this is recorded behavior only — the recording shows at least one matching image with an ImageId and a CreationDate.
Emulator: the filtered DescribeImages answers 200 with an empty Images list, so the check's `images[0]['ImageId']` raises `IndexError: list index out of range`.
Why: a client that resolves "the latest Amazon Linux 2 AMI" — the single most common way to pick an image in scripts, launch templates and CDK — gets nothing back and cannot launch anything. Every traffic-mirror scenario below is blocked before it issues its first traffic-mirror call.
Fix: the seeded AMI catalogue in EC2 state (and the name-wildcard matching DescribeImages' `name` filter applies to it).
Note: open issue 20261029T175114Z-ec2-read-paths-wrong-or-missing-members-in.md describes the same missing-catalogue family in its Cause 8 but does not carry these check ids, so these checks are not covered by it.
Traces: ec2.op-contract-delete-traffic-mirror-filter, ec2.op-contract-delete-traffic-mirror-session, ec2.op-contract-delete-traffic-mirror-target.

Cause 2 of 10: the DescribeInstances instance projection omits NetworkInterfaces.
AWS: Smithy models Instance.NetworkInterfaces (InstanceNetworkInterfaceList, xmlName networkInterfaceSet); AWS always serializes it for a running instance, with one entry per attached ENI carrying NetworkInterfaceId.
Emulator: the described instance has no NetworkInterfaces key at all, so a client indexing it raises `KeyError: 'NetworkInterfaces'`.
Why: a client cannot discover the ENI an instance it just launched owns, so every ENI-keyed follow-up (traffic mirroring, secondary IPs, ENI modification) is unreachable even though the instance exists.
Fix: the Instance serializer shared by DescribeInstances/RunInstances.
Traces: ec2.op-contract-describe-traffic-mirror-targets.

Cause 3 of 10: DescribeIpamScopes rejects the documented `ipam-id` filter.
AWS: Smithy: DescribeIpamScopes.Filters targets FilterList and is documented as 'One or more filters for the request'; `ipam-id` is a supported filter name for this operation, and AWS answers 200 with the scopes of that IPAM.
Emulator: answers the ec2Query client error `InvalidParameterValue` with message `The filter 'ipam-id' is invalid`. Fixed text of that message is `The filter '` ... `' is invalid`; the variable part is the filter name taken from the request.
Why: the only supported way to enumerate an IPAM's scopes is refused, so no client can walk IPAM -> scopes -> pools. It also blocks DeprovisionIpamPoolCidr's whole setup.
Fix: the filter-name whitelist DescribeIpamScopes' handler validates against.
Traces: ec2.op-contract-describe-ipam-scopes, ec2.op-contract-describe-ipam-scopes-superseded-stub, ec2.op-contract-deprovision-ipam-pool-cidr.

Cause 4 of 10: DescribePublicIpv4Pools validates filter names, where AWS ignores unrecognized ones.
AWS: Smithy: DescribePublicIpv4Pools.Filters targets FilterList, models no errors. Recorded behavior: an unrecognized filter *name* is not validated by this operation — the call succeeds with the documented shape and the unfiltered pool set, so a pool created moments earlier is still present.
Emulator: answers `InvalidParameterValue` / `The filter 'totally-bogus-filter-key' is invalid` (fixed text `The filter '...' is invalid`, variable part = the filter name from the request).
Why: a client passing a forward-compatible or service-specific filter name gets a hard refusal where AWS returns data, so pool enumeration breaks on input AWS accepts.
Fix: DescribePublicIpv4Pools' filter handling (unknown names must be ignored, not rejected).
Traces: ec2.op-contract-describe-public-ipv4-pools.

Cause 5 of 10: DeprovisionPublicIpv4PoolCidr refuses a CIDR that was never provisioned into the pool, instead of treating it as a no-op.
AWS: Smithy: DeprovisionPublicIpv4PoolCidr takes required PoolId and Cidr, returns PoolId + DeprovisionedAddresses, and models no errors. Recorded behavior: against a real, existing pool, deprovisioning a CIDR that is not in the pool is a success that echoes PoolId (there is nothing to remove); the pool's PoolAddressRanges stays empty.
Emulator: refuses with `InvalidParameterValue` / `Value (<cidr>) for parameter cidr is invalid. The CIDR <cidr> is not provisioned to the pool <poolId>.` — the fixed text is emulator-authored; the CIDR and pool id are values from the request and state.
Why: an idempotent teardown (deprovision-then-delete, replayed after a partial failure) fails on the second run where AWS succeeds, so cleanup scripts cannot be made idempotent.
Fix: the DeprovisionPublicIpv4PoolCidr handler's validation.
Traces: ec2.op-contract-deprovision-public-ipv4-pool-cidr-unprovisioned-rejected.

Cause 6 of 10: CreateNetworkAclEntry performs no range validation on RuleNumber because the operation is still a scaffold stub.
AWS: Smithy: CreateNetworkAclEntry.RuleNumber is a required integer; AWS refuses a value outside 1..32766 with error code `InvalidParameterValue` and message `Value (<ruleNumber>) for parameter ruleNumber is invalid. Rulenumber must be in range 1..32766` — fixed text apart from the rule number, which is the value from the request. The rejected entry must not appear in DescribeNetworkAcls Entries, and a valid rule number on the same ACL must succeed.
Emulator: answers error code `NotImplemented` (`operation CreateNetworkAclEntry is scaffolded but not implemented in this emulator`), so neither the refusal nor the positive control is reachable.
Why: a client cannot distinguish "my rule number is out of range" from "the API is unavailable", and no NACL rule can be written at all.
Fix: the CreateNetworkAclEntry handler and its validation (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_network_acl_entry.rs).
Note: open issue 20261028T135109Z-ec2-createnetworkaclentry-stub.md asks for the operation to be implemented but does not carry this check id; this cause pins the specific range refusal the implementation must produce.
Traces: ec2.network-acl-entry-rule-number-out-of-range-rejected.

Cause 7 of 10: EnableAwsNetworkPerformanceMetricSubscription misclassifies an out-of-enum Statistic.
AWS: Smithy: EnableAwsNetworkPerformanceMetricSubscription.Statistic targets the StatisticType enum whose only value is `p50`. AWS refuses any other value with error code `InvalidParameter` and message `Invalid value for Statistic: <value> Valid values are: [p50]` — the fixed text is `Invalid value for Statistic: ` ... ` Valid values are: [p50]`, the variable part is the rejected value from the request. No subscription is created, and the identical call with Statistic=p50 succeeds and becomes visible in DescribeAwsNetworkPerformanceMetricSubscriptions.
Emulator: refuses with error code `ValidationError`.
Why: `ValidationError` is not in EC2's ec2Query error vocabulary for this operation; a client branching on `InvalidParameter` to surface a usable message to the user falls through to its generic failure path.
Fix: the EnableAwsNetworkPerformanceMetricSubscription handler's enum validation and the error code it raises.
Traces: ec2.network-performance-subscription-invalid-region-rejected.

Cause 8 of 10: the CloudFormation resource-type registry reached through an EC2-led stack does not know the IPAM resource types.
AWS: a CreateStack whose template declares AWS::EC2::IPAM, AWS::EC2::IPAMScope and AWS::EC2::IPAMPool is accepted and provisions the corresponding EC2 IPAM resources. Recorded behavior only — the model says nothing about CFN templates.
Emulator: CreateStack is refused with `ValidationError` / `Template format error: Unrecognized resource types: [AWS::EC2::IPAM, AWS::EC2::IPAMScope, AWS::EC2::IPAMPool]`. The fixed text is `Template format error: Unrecognized resource types: [...]`; the bracketed list is taken from the request's template.
Why: the whole IPAM pool/VPC lifecycle cannot be expressed as infrastructure-as-code, so no CFN/CDK user can provision IPAM at all.
Fix: the CloudFormation resource-type registry's EC2 IPAM entries and their mapping onto the EC2 IPAM state.
Traces: ec2.ipam-e2e-ipam-pool-and-vpc-lifecycle.

Cause 9 of 10: CreateTransitGatewayPeeringAttachment refuses outright, so the Reject wrong-state boundary is unreachable.
AWS: CreateTransitGatewayPeeringAttachment against a peer transit gateway in a genuinely different region of the same account is accepted and returns a TransitGatewayPeeringAttachment whose state becomes `pendingAcceptance`; after Accept it becomes `available`, and RejectTransitGatewayPeeringAttachment on an `available` attachment is then refused with `IncorrectState` / `<attachmentId> is in invalid state` (fixed text ` is in invalid state`, variable part = the attachment id), leaving the state `available`. Smithy models no operation-specific errors for these.
Emulator: CreateTransitGatewayPeeringAttachment raises a client error, so no attachment is ever created and the check cannot reach any of the Accept/Reject transitions.
Why: the entire transit-gateway peering state machine (pendingAcceptance -> available, pendingAcceptance -> rejected) is unreachable, so no client can build or tear down an inter-region TGW peering.
Fix: the CreateTransitGatewayPeeringAttachment handler and the peering-attachment state machine (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_transit_gateway_peering_attachment.rs).
Note: that operation is also named by an already-filed stub issue, which does not carry this check id; this cause pins the state-machine behavior the implementation must then expose.
Traces: ec2.reject-transit-gateway-peering-attachment-wrong-state-rejected.

Cause 10 of 10: three further scenarios poll an EC2-owned observable that never becomes true, so they are cut off by the harness instead of converging.
AWS: each polled condition is reached within seconds and then readable: an IPAM created with Tier=free and moved to Tier=advanced reports State != modify-in-progress and Tier=advanced, after which ModifyIpamPolicyAllocationRules establishes a (locale, resource-type) rule that GetIpamPolicyAllocationRules then reports; a capacity reservation reaches State=active and its interruptible allocation reaches Status=active so UpdateInterruptibleCapacityReservationAllocation can shrink TargetInstanceCount; and a launched instance in a VPC with a custom DHCP option set becomes reachable so the guest's name resolution can be observed. Recorded behavior only for the timings.
Emulator: each check reports exactly `Error: Check timed out after 60.0s` with no exception and no non-2xx — every call answered, but the state it reported never advanced to the terminal value. These three checks contain no unconditional sleep; every wait is a poll that returns as soon as its predicate holds, so exhausting the budget means the transition never happened.
Why: a client that waits on these transitions (every SDK waiter, every CDK deploy, every teardown script) blocks forever rather than failing, which is strictly worse than an error because there is no code to catch.
Fix: the EC2 state machines that advance IPAM tier/policy-allocation-rule state, capacity-reservation and interruptible-allocation state, and instance reachability (EC2 state, not a single op handler).
Note: open issue 20261029T175114Z-ec2-led-lifecycle-scenarios-never-reach-their-observable.md owns the same family for five OTHER check ids (including ec2.interruptible-capacity-reservation-allocation-lifecycle) and does not carry the three below.
Traces: ec2.op-contract-modify-ipam-policy-allocation-rules, ec2.op-contract-update-interruptible-capacity-reservation-allocation, ec2.vpc-name-resolution-dhcp-black-hole-nameserver-fails-guest-lookups-while-default-resolves.

## Evidence
Verbatim replay diagnoses, one per cause.

Cause 1 — `File "check_ec2_op_contract_delete_traffic_mirror_filter.py", line 155, in _latest_amazon_linux_ami / return images[0]["ImageId"] / IndexError: list index out of range` (identically for ..._delete_traffic_mirror_session.py line 145 and ..._delete_traffic_mirror_target.py line 157). Expected: DescribeImages(Owners=['amazon'], name=['amzn2-ami-hvm-*-x86_64-gp2'], state=['available'], root-device-type=['ebs']) returns a non-empty Images list. Actual: an empty list.

Cause 2 — `File "check_ec2_op_contract_describe_traffic_mirror_targets.py", line 75, in run / eni_id = ec2.describe_instances(InstanceIds=[instance_id])["Reservations"][0]["Instances"][...] / KeyError: 'NetworkInterfaces'`. Expected: Instance.NetworkInterfaces present with one entry per attached ENI. Actual: the member is absent.

Cause 3 — `botocore.exceptions.ClientError: An error occurred (InvalidParameterValue) when calling the DescribeIpamScopes operation: The filter 'ipam-id' is invalid` (raised by ec2.op-contract-describe-ipam-scopes, ec2.op-contract-describe-ipam-scopes-superseded-stub and, during setup, ec2.op-contract-deprovision-ipam-pool-cidr). Expected: HTTP 200 with the IPAM's two default scopes (one private, one public, both IsDefault=true, State=create-complete). Actual: a 4xx InvalidParameterValue.

Cause 4 — `botocore.exceptions.ClientError: An error occurred (InvalidParameterValue) when calling the DescribePublicIpv4Pools operation: The filter 'totally-bogus-filter-key' is invalid`. Expected: HTTP 200 with PublicIpv4Pools still containing the just-created pool (unrecognized filter names are ignored). Actual: a 4xx InvalidParameterValue.

Cause 5 — `botocore.exceptions.ClientError: An error occurred (InvalidParameterValue) when calling the DeprovisionPublicIpv4PoolCidr operation: Value (203.0.113.0/24) for parameter cidr is invalid. The CIDR 203.0.113.0/24 is not provisioned to the pool ipv4pool-ec2-...`. Expected: HTTP 200 echoing PoolId, with the pool's PoolAddressRanges still empty. Actual: a 4xx refusal. The CIDR and the pool id in that message are per-run request/state values; the divergence is that the call is refused at all.

Cause 6 — `AssertionError: expected InvalidParameterValue, got NotImplemented`, from `ec2.create_network_acl_entry(NetworkAclId=<aclId>, RuleNumber=40000, Protocol='6', RuleAction='allow', Egress=False, CidrBlock='0.0.0.0/0', PortRange={'From':80,'To':80})`. Expected: code `InvalidParameterValue`, message `Value (40000) for parameter ruleNumber is invalid. Rulenumber must be in range 1..32766`. Actual: code `NotImplemented`.

Cause 7 — `AssertionError: expected InvalidParameter for Statistic='p99', got 'ValidationError'`. Expected: code `InvalidParameter`, message `Invalid value for Statistic: p99 Valid values are: [p50]`. Actual: code `ValidationError`.

Cause 8 — `_PhaseError: [phase 1/4 deploy] ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::EC2::IPAM, AWS::EC2::IPAMScope, AWS::EC2::IPAMPool]`. Expected: CreateStack accepted and the stack reaching CREATE_COMPLETE with the IPAM, scope and pool created in EC2 state. Actual: the template is rejected at submit time.

Cause 9 — `AssertionError: CreateTransitGatewayPeeringAttachment to a peer in a genuinely different region was rejected outright in this account/partition -- the accepted/rejected wrong-state boundary is unreachable here`. Expected: the create returns a TransitGatewayPeeringAttachment that reaches `pendingAcceptance`, then `available` after Accept, after which Reject answers `IncorrectState` / `<attachmentId> is in invalid state` and leaves the state `available`. Actual: the create raises a client error and no attachment exists.

Cause 10 — all three checks report only `Error: Check timed out after 60.0s`, with no exception and no non-2xx response. Expected: the polled predicate becomes true (IPAM Tier=advanced and State!=modify-in-progress, then GetIpamPolicyAllocationRules reporting the established SourceIpamPoolId; capacity reservation State=active then allocation Status=active and TargetInstanceCount shrinking to 1; the guest resolving names through the VPC's DHCP options). Actual: still false when the budget expires. The 60s figure is the harness budget, not a requirement — the requirement is that the transition happens at all.

## Steps to Reproduce
1. Boot the emulator and point botocore EC2/CloudFormation clients at it.
2. Cause 1: DescribeImages(Owners=['amazon'], Filters=[{'Name':'name','Values':['amzn2-ami-hvm-*-x86_64-gp2']},{'Name':'state','Values':['available']},{'Name':'root-device-type','Values':['ebs']}]) — observe the empty Images list.
3. Cause 2: RunInstances with a NetworkInterfaces entry, then DescribeInstances(InstanceIds=[id]) and read Instances[0]['NetworkInterfaces'].
4. Cause 3: CreateIpam(Tier='free'), then DescribeIpamScopes(Filters=[{'Name':'ipam-id','Values':[ipamId]}]).
5. Cause 4: CreatePublicIpv4Pool, then DescribePublicIpv4Pools(Filters=[{'Name':'totally-bogus-filter-key','Values':['x']}]).
6. Cause 5: CreatePublicIpv4Pool, then DeprovisionPublicIpv4PoolCidr(PoolId=<that pool>, Cidr='203.0.113.0/24').
7. Cause 6: CreateVpc, CreateNetworkAcl, then CreateNetworkAclEntry with RuleNumber=40000; then repeat with RuleNumber=100 as the positive control.
8. Cause 7: EnableAwsNetworkPerformanceMetricSubscription(Source=<region>, Destination=<other region>, Metric='aggregate-latency', Statistic='p99').
9. Cause 8: CreateStack with a template declaring AWS::EC2::IPAM, AWS::EC2::IPAMScope and AWS::EC2::IPAMPool.
10. Cause 9: CreateTransitGateway in two regions, then CreateTransitGatewayPeeringAttachment from the requester to the accepter with PeerAccountId/PeerRegion set.
11. Cause 10: run each of the three scenarios and watch the poll loops never satisfy their predicate.
12. Re-replay every check id listed in trace_ids to confirm the fix.
