Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.ec2mr-pip-per-region-private-ip-enumeration-excludes-other-regions ec2.e2e-ec2mr-xrg-cross-region-connectivity-lifecycle ec2.ec2mr-inv-e2e-all-regions-instance-inventory-lifecycle ec2.ec2mr-byvpc-e2e-multi-vpc-grouping-lifecycle ec2.create-default-vpc-contract ec2.ec2mr-xrg-default-vpc-is-the-grouping-discriminator ec2.ec2mr-dvpc-e2e-without-default-vpc-lifecycle ec2.ec2mr-byvpc-describe-instances-in-a-region-not-enabled-rejected ec2.ec2mr-dvpc-unenumerable-region-rejects-describe-vpcs ec2.ec2mr-pip-private-ip-address-filter-matches-only-the-instance-primary ec2.ec2mr-pip-unknown-private-ip-filter-name-rejected ec2.ec2mr-pip-describe-instances-maxresults-below-the-floor-rejected ec2.ec2mr-pip-stopped-instance-keeps-its-private-ip-and-terminated-drops-out ec2.ec2mr-xrg-absent-instance-id-rejected-but-filter-returns-empty ec2.ec2mr-byvpc-vpc-create-has-no-implicit-ipv6-and-shares-the-account-dhcp-options-set ec2.ec2mr-byvpc-launch-into-a-subnet-of-another-vpc-rejected ec2.ec2mr-xrg-launch-into-unofferring-az-rejected-unsupported
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.ec2mr-pip-per-region-private-ip-enumeration-excludes-other-regions|run#19|AssertionError|assert _by_private_ip(ec2_b, ip_a) == [], (
Issue-Key: ec2.e2e-ec2mr-xrg-cross-region-connectivity-lifecycle|phase#4|_PhaseError|raise _PhaseError(
Issue-Key: ec2.ec2mr-inv-e2e-all-regions-instance-inventory-lifecycle|phase#6|_PhaseError|raise _PhaseError(
Issue-Key: ec2|DescribeInstances|InvalidInstanceID.Malformed
Issue-Key: ec2.ec2mr-xrg-default-vpc-is-the-grouping-discriminator|run#9|AssertionError|assert len(defaults) == 1, (
Issue-Key: ec2.ec2mr-byvpc-describe-instances-in-a-region-not-enabled-rejected|_error#11|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: ec2.ec2mr-dvpc-unenumerable-region-rejects-describe-vpcs|run#12|AssertionError|assert code == "AuthFailure", (
Issue-Key: ec2|DescribeInstances|InvalidParameterValue
Issue-Key: ec2.ec2mr-pip-unknown-private-ip-filter-name-rejected|run#9|AssertionError|assert err == want, (
Issue-Key: ec2.ec2mr-pip-describe-instances-maxresults-below-the-floor-rejected|_error#5|AssertionError|raise AssertionError(f"the call succeeded; real AWS rejects it, so there is no error: {result!r}")
Issue-Key: ec2.ec2mr-pip-stopped-instance-keeps-its-private-ip-and-terminated-drops-out|run#18|AssertionError|assert "PrivateIpAddress" not in dead, (
Issue-Key: ec2|DescribeInstances|InvalidInstanceID.NotFound
Issue-Key: ec2.ec2mr-byvpc-vpc-create-has-no-implicit-ipv6-and-shares-the-account-dhcp-options-set|run#11|AssertionError|assert "Ipv6CidrBlockAssociationSet" not in read_a, (
Issue-Key: ec2.ec2mr-byvpc-launch-into-a-subnet-of-another-vpc-rejected|_error#6|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: ec2.ec2mr-xrg-launch-into-unofferring-az-rejected-unsupported|run#9|AssertionError|assert len(unofferring) > 0, (
Issue-Related: 20261029T175114Z-ec2-accepts-with-200-a-range-of-requests.md
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# EC2 state is not region-scoped and the account baseline is wrong: cross-region describes, two default VPCs, missing opt-in refusals and unsupported describe filters (10 root causes)

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstances

## Description
Ten net-new root causes in EC2's regional scoping, account baseline and describe/filter surface. The EC2 Smithy model is ec2Query and declares no per-operation errors, so the codes/messages below are the recorded AWS behavior unless stated otherwise.

Cause 1 of 10: EC2 state is shared across regions, so a region-scoped Describe enumerates other regions' resources.
AWS: every EC2 Describe is region-scoped; a resource created in one region is invisible to a client whose endpoint is another region. DescribeInstances with filter `private-ip-address` in us-east-1 cannot match an instance that lives in us-west-2.
Emulator: one global store backs every region - the replay shows us-east-1 matching an instance that belongs to us-west-2, and the multi-region lifecycle scenarios never see disjoint per-region inventories.
Why: this is the single assumption every multi-region client makes. Fail-over logic, per-region inventories, DR runbooks and "is this resource in this region?" guards all read as satisfied when they are not.
Fix: region partitioning in the EC2 state store and every Describe handler's scoping predicate.
Traces: ec2.ec2mr-pip-per-region-private-ip-enumeration-excludes-other-regions, ec2.e2e-ec2mr-xrg-cross-region-connectivity-lifecycle, ec2.ec2mr-inv-e2e-all-regions-instance-inventory-lifecycle, ec2.ec2mr-byvpc-e2e-multi-vpc-grouping-lifecycle

Cause 2 of 10: the account baseline holds more than one default VPC per region.
AWS: an account has at most ONE default VPC per region (IsDefault=true on exactly one VPC).
Emulator: DescribeVpcs reports two, e.g. `['vpc-00000000000000001', 'vpc-00000000000000024']` both default in one region.
Why: "the default VPC" is a singleton every console-style workflow and many SDK helpers resolve by filter; two breaks the derivation outright and makes default-subnet lookups ambiguous.
Fix: the account-baseline seeding plus the IsDefault invariant in VPC state.
Traces: ec2.create-default-vpc-contract, ec2.ec2mr-xrg-default-vpc-is-the-grouping-discriminator, ec2.ec2mr-dvpc-e2e-without-default-vpc-lifecycle

Cause 3 of 10: calls against a region the account has not opted into succeed (or use the wrong refusal class).
AWS: a request to a non-opted-in region is refused AuthFailure. An empty result list is the WRONG answer - it is indistinguishable from an enabled-but-empty region.
Emulator: DescribeInstances in such a region SUCCEEDS with an empty body, and DescribeVpcs answers InvalidParameterValue instead of AuthFailure.
Why: a client probing region availability concludes the region is enabled and empty, then provisions into a region AWS will refuse.
Fix: a region opt-in gate in the request-dispatch/auth layer, ahead of the Describe handlers.
Traces: ec2.ec2mr-byvpc-describe-instances-in-a-region-not-enabled-rejected, ec2.ec2mr-dvpc-unenumerable-region-rejects-describe-vpcs

Cause 4 of 10: DescribeInstances rejects filters AWS supports and uses emulator prose for unknown ones.
AWS: `network-interface.addresses.private-ip-address` is a supported DescribeInstances filter and must match. An UNKNOWN filter name is refused ('InvalidParameterValue', "The filter '<name>' is invalid") - fixed text `The filter '` / `' is invalid` with the name taken from the request.
Emulator: refuses the supported filter with `The filter 'network-interface.addresses.private-ip-address' is not one this emulator resolves`, and uses the same emulator sentence for genuinely unknown names.
Why: supported filters failing breaks server-side narrowing; and the self-referential "this emulator" wording leaks the implementation into a message clients parse.
Fix: the filter table and the unknown-filter message template in the DescribeInstances handler.
Traces: ec2.ec2mr-pip-private-ip-address-filter-matches-only-the-instance-primary, ec2.ec2mr-pip-unknown-private-ip-filter-name-rejected

Cause 5 of 10: DescribeInstances accepts a MaxResults below AWS's floor.
AWS: MaxResults below the documented minimum is refused; there is no success path.
Emulator: returns HTTP 200 with an empty Reservations list.
Why: a paginating client that mis-sizes its page gets a silently empty answer instead of a validation error, and concludes the account is empty.
Fix: MaxResults range validation in the DescribeInstances handler.
Traces: ec2.ec2mr-pip-describe-instances-maxresults-below-the-floor-rejected

Cause 6 of 10: a terminated instance keeps reporting its private IP address.
AWS: once an instance reaches `terminated` its network interfaces are released and it no longer reports a PrivateIpAddress.
Emulator: a terminated instance still reports its address (replay: `i-...` still reports '10.61.1.5').
Why: address-reuse and IP-inventory logic double-counts addresses that AWS has already freed, so a client believes a subnet is exhausted.
Fix: the terminate transition in instance state and the DescribeInstances projection.
Traces: ec2.ec2mr-pip-stopped-instance-keeps-its-private-ip-and-terminated-drops-out

Cause 7 of 10: a well-formed but unknown instance id is classified NotFound where AWS says Malformed.
AWS: recorded behavior only. For the ids this check uses, EC2 answers InvalidInstanceID.Malformed.
Emulator: answers InvalidInstanceID.NotFound.
Why: clients branch on Malformed (never retry, fix the input) versus NotFound (may be eventual consistency, retry); the swap sends them down the retry path forever.
Fix: id validation ordering in the DescribeInstances handler (shape check before lookup).
Traces: ec2.ec2mr-xrg-absent-instance-id-rejected-but-filter-returns-empty

Cause 8 of 10: DescribeVpcs emits an empty Ipv6CidrBlockAssociationSet where AWS omits the member.
AWS: recorded behavior only. For a VPC with no IPv6 association, DescribeVpcs OMITS Ipv6CidrBlockAssociationSet entirely (unlike CreateVpc's own response, which returns an empty list).
Emulator: always includes the key with an empty list.
Why: presence-of-member is how a client tells "no IPv6" from "IPv6 not reported"; the emulator erases that distinction and diverges from the shape CreateVpc returned.
Fix: the DescribeVpcs serializer's empty-list suppression.
Traces: ec2.ec2mr-byvpc-vpc-create-has-no-implicit-ipv6-and-shares-the-account-dhcp-options-set

Cause 9 of 10: RunInstances accepts a subnet that belongs to another VPC than the request's other resources.
AWS: recorded behavior only. Launching into a subnet of a different VPC is refused; there is no success path.
Emulator: the call SUCCEEDS.
Why: a launch that AWS refuses is accepted, so network-isolation tests pass locally and the instance cannot exist in production.
Fix: the VPC-consistency validation in the RunInstances handler.
Traces: ec2.ec2mr-byvpc-launch-into-a-subnet-of-another-vpc-rejected

Cause 10 of 10: instance-type offerings are identical in every Availability Zone.
AWS: DescribeInstanceTypeOfferings(LocationType='availability-zone') differs per AZ - some zones do not offer a given type, which is why launching there is refused Unsupported.
Emulator: every zone of the region offers the type (replay: all three us-east-1 zones offer t3.micro), so the offerings boundary does not exist and the Unsupported refusal is unreachable.
Why: capacity-aware placement that enumerates offerings to choose a zone sees a uniform world and never exercises the fallback AWS forces.
Fix: per-AZ offerings data behind DescribeInstanceTypeOfferings and the launch-time offering check in RunInstances.
Traces: ec2.ec2mr-xrg-launch-into-unofferring-az-rejected-unsupported

## Evidence
Replayed failing checks under `### ec2` (none marked ALREADY FILED / OWNED by):

1. ec2.ec2mr-pip-per-region-private-ip-enumeration-excludes-other-regions -> AssertionError: us-east-1 answered the private-ip-address filter for '10.61.1.4', which belongs to 'i-00000000000000001' in us-west-2; it matched ['i-00000000000000001'] - a shared global store is the failure this asserts against. Companion lifecycles ec2.e2e-ec2mr-xrg-cross-region-connectivity-lifecycle, ec2.ec2mr-inv-e2e-all-regions-instance-inventory-lifecycle and ec2.ec2mr-byvpc-e2e-multi-vpc-grouping-lifecycle fail in the same multi-region phase.
2. ec2.create-default-vpc-contract -> AssertionError: the account baseline holds exactly one default VPC per region, but describe_vpcs found ['vpc-00000000000000001', 'vpc-00000000000000024']
   ec2.ec2mr-xrg-default-vpc-is-the-grouping-discriminator -> AssertionError: us-west-2 reports 2 default VPC(s) ['vpc-00000000000000001', 'vpc-0000000000000002b'] among [...]; exactly one is the invariant the derivation relies on
   ec2.ec2mr-dvpc-e2e-without-default-vpc-lifecycle -> fails in the same baseline phase
3. ec2.ec2mr-byvpc-describe-instances-in-a-region-not-enabled-rejected -> AssertionError: the call succeeded; real AWS rejects it, so there is no error code
   ec2.ec2mr-dvpc-unenumerable-region-rejects-describe-vpcs -> AssertionError: DescribeVpcs in the not-opted-in region 'af-south-1' returned 'InvalidParameterValue', expected 'AuthFailure'. An empty Vpcs list here is the WRONG answer: it is the same body a region with no default VPC returns
4. ec2.ec2mr-pip-private-ip-address-filter-matches-only-the-instance-primary -> ClientError (InvalidParameterValue) when calling DescribeInstances: The filter 'network-interface.addresses.private-ip-address' is not one this emulator resolves
   ec2.ec2mr-pip-unknown-private-ip-filter-name-rejected -> AssertionError: the misspelt filter 'private-ip-addres' answered ('InvalidParameterValue', "The filter 'private-ip-addres' is not one this emulator resolves"), expected ('InvalidParameterValue', "The filter 'private-ip-addres' is invalid")
5. ec2.ec2mr-pip-describe-instances-maxresults-below-the-floor-rejected -> AssertionError: the call succeeded; real AWS rejects it, so there is no error: {'Reservations': [], ... 'HTTPStatusCode': 200 ...}
6. ec2.ec2mr-pip-stopped-instance-keeps-its-private-ip-and-terminated-drops-out -> AssertionError: a terminated instance no longer reports a private address; i-00000000000000002 still reports '10.61.1.5'
7. ec2.ec2mr-xrg-absent-instance-id-rejected-but-filter-returns-empty -> AssertionError: an absent instance id raised 'InvalidInstanceID.NotFound', expected 'InvalidInstanceID.Malformed'
8. ec2.ec2mr-byvpc-vpc-create-has-no-implicit-ipv6-and-shares-the-account-dhcp-options-set -> AssertionError: DescribeVpcs must agree with CreateVpc that vpc-00000000000000024 carries no IPv6 association - DescribeVpcs OMITS the key entirely for a VPC with no IPv6 block, unlike CreateVpc's own empty-list response; got [] among keys [..., 'Ipv6CidrBlockAssociationSet', ...]
9. ec2.ec2mr-byvpc-launch-into-a-subnet-of-another-vpc-rejected -> AssertionError: the call succeeded; real AWS rejects it, so there is no error code
10. ec2.ec2mr-xrg-launch-into-unofferring-az-rejected-unsupported -> AssertionError: every zone in 'us-east-1' ['us-east-1a', 'us-east-1b', 'us-east-1c'] offers t3.micro, so this region no longer expresses the offerings boundary

## Steps to Reproduce
1. git_clone, then run_behavioral_suite; the ten checks in trace_ids appear under `### ec2` with no ALREADY FILED / OWNED by marker.
2. Standalone repros:
   - RunInstances in us-west-2, then DescribeInstances in us-east-1 with Filters=[{Name:'private-ip-address',Values:['<the us-west-2 private ip>']}]: must return zero reservations.
   - DescribeVpcs(Filters=[{Name:'isDefault',Values:['true']}]) in one region: exactly one VPC.
   - DescribeInstances / DescribeVpcs against a region the account has not opted into (e.g. af-south-1): expect AuthFailure, not 200 or InvalidParameterValue.
   - DescribeInstances(Filters=[{Name:'network-interface.addresses.private-ip-address',Values:['10.61.1.4']}]): must match; then misspell the filter name and expect `The filter '<name>' is invalid`.
   - DescribeInstances(MaxResults=1): expect a validation refusal.
   - TerminateInstances, wait for `terminated`, DescribeInstances: no PrivateIpAddress.
   - DescribeInstances(InstanceIds=['i-0123456789abcdef0']) for an absent id: expect InvalidInstanceID.Malformed.
   - CreateVpc without IPv6, then DescribeVpcs: Ipv6CidrBlockAssociationSet must be absent.
   - RunInstances with SubnetId from VPC A and SecurityGroupIds from VPC B: expect a refusal.
   - DescribeInstanceTypeOfferings(LocationType='availability-zone', Filters=[{Name:'instance-type',Values:['t3.micro']}]) in us-east-1: the zone set must be a strict subset of the region's zones.
