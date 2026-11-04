Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.ec2mr-inv-instances-attribute-to-their-own-region-and-are-absent-from-the-others ec2.ec2mr-xrg-instance-counts-are-per-region-not-merged ec2.ec2mr-usg-same-name-groups-are-region-scoped ec2.ec2mr-pub-public-subnet-derivation-is-attributed-per-region ec2.ec2mr-inv-describe-instances-rejects-an-instance-id-from-another-region ec2.ec2mr-inv2-describe-security-groups-rejects-a-group-id-from-another-region-while-the-filter-form-returns-empty ec2.ec2mr-pub-cli-and-sdk-agree-on-public-route-tables ec2.ec2mr-pub-e2e-public-subnets-lifecycle ec2.ec2mr-xrg-private-subnet-instance-omits-public-ip
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.ec2mr-inv-instances-attribute-to-their-own-region-and-are-absent-from-the-others|run#5|AssertionError|assert not leaked, (
Issue-Key: ec2.ec2mr-xrg-instance-counts-are-per-region-not-merged|run#13|AssertionError|assert before[region] == launched[region], (
Issue-Key: ec2.ec2mr-usg-same-name-groups-are-region-scoped|run#6|AssertionError|assert seen == [created[region]], (
Issue-Key: ec2.ec2mr-pub-public-subnet-derivation-is-attributed-per-region|run#5|AssertionError|assert shared == set(), (
Issue-Key: ec2.ec2mr-inv-describe-instances-rejects-an-instance-id-from-another-region|_error#11|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: ec2.ec2mr-inv2-describe-security-groups-rejects-a-group-id-from-another-region-while-the-filter-form-returns-empty|_error#12|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error")
Issue-Key: ec2.ec2mr-pub-cli-and-sdk-agree-on-public-route-tables|run#2|AssertionError|assert sdk, (
Issue-Key: ec2.ec2mr-pub-e2e-public-subnets-lifecycle|phase#4|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: ec2.ec2mr-xrg-private-subnet-instance-omits-public-ip|run#32|AssertionError|assert public.get("PublicIpAddress"), (
Issue-Commit: c832bd0d619782e17fabc455ea6e3058ac5fd237

# EC2 regional endpoints share one store: cross-region resources leak, default VPCs carry no internet route, and MapPublicIpOnLaunch assigns no public IP

> _Note: this issue was identified against the code base at commit `c832bd0d619782e17fabc455ea6e3058ac5fd237`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstances

## Description
This issue bundles 3 net-new root causes, all in the EC2 emulator. A related open issue, 20261103T021956Z-ec2-state-is-not-region-scoped-and-the.md, owns a different set of replay keys; the checks below are not covered by it.

Cause 1 of 3: EC2 state is not scoped per region, so every regional endpoint describes every other region's resources and accepts their ids.
AWS: an EC2 endpoint is regional. DescribeInstances/DescribeSecurityGroups/DescribeSubnets on us-east-1 return only resources created through the us-east-1 endpoint; an id minted in another region is not a resource of this one, so the ById form is refused (InvalidInstanceID.NotFound / InvalidGroup.NotFound) while the Filter form returns an empty set. Smithy: the ec2 model (services/service-com-amazonaws-ec2-amazonec2) binds awsQuery with no modeled operation errors (CreateRoute/AssociateAddress/ReleaseHosts/DescribeNetworkInterfaces all have errors: []), so the codes above are recorded behavior only; the regional scoping itself is the service's endpoint contract.
Emulator: one global store backs all regions. Unfiltered DescribeInstances in us-east-1 returns instances launched in other regions; a group name created in three regions resolves to all three sg-ids from any one region; two regions report the identical subnet id set; DescribeInstances(InstanceIds=[id-from-another-region]) and DescribeSecurityGroups(GroupIds=[id-from-another-region]) both SUCCEED where AWS refuses.
Why: any multi-region inventory, cost or compliance tool double-counts: it sees each resource in every region, and an id lookup that must fail silently succeeds, so per-region attribution is fabricated.
Fix: EC2 state ownership — key every resource store by region and resolve ids only within the caller's region, in the ec2 service's state layer and the describe handlers.
Traces: ec2.ec2mr-inv-instances-attribute-to-their-own-region-and-are-absent-from-the-others, ec2.ec2mr-xrg-instance-counts-are-per-region-not-merged, ec2.ec2mr-usg-same-name-groups-are-region-scoped, ec2.ec2mr-pub-public-subnet-derivation-is-attributed-per-region, ec2.ec2mr-inv-describe-instances-rejects-an-instance-id-from-another-region, ec2.ec2mr-inv2-describe-security-groups-rejects-a-group-id-from-another-region-while-the-filter-form-returns-empty.

Cause 2 of 3: the account's default VPC in each region has no 0.0.0.0/0 route to an internet gateway, so no public route table and no public subnet can be derived.
AWS: a default VPC ships with an internet gateway attached and its main route table carries 0.0.0.0/0 -> igw-*; a subnet associated with that table is therefore public. Recorded behavior only (the model does not describe default-VPC contents).
Emulator: DescribeRouteTables for the pre-existing default VPC yields no table with a 0.0.0.0/0 -> igw-* entry, so the derivation returns [] in every region, and the deploy phase of the lifecycle check resolves the public subnet to [] instead of its route table id.
Why: a client that classifies subnets as public/private (CDK vpc.fromLookup, cost and exposure reports) concludes the account has no public subnets at all.
Fix: default-VPC seeding in EC2 state — attach an internet gateway and install the default route in the main route table when the default VPC is seeded.
Traces: ec2.ec2mr-pub-cli-and-sdk-agree-on-public-route-tables, ec2.ec2mr-pub-e2e-public-subnets-lifecycle.

Cause 3 of 3: an instance launched into a subnet with MapPublicIpOnLaunch=true gets no PublicIpAddress.
AWS: RunInstances into a subnet whose MapPublicIpOnLaunch is true assigns a public IPv4 address, and DescribeInstances reports it in PublicIpAddress (the key is present for such an instance and absent for an instance without one). Recorded behavior only.
Emulator: the public-subnet instance reports PublicIpAddress None while the private-subnet instance also reports none, so the two cases are indistinguishable.
Why: a caller cannot tell a public host from a private one, and any reachability or exposure derivation over DescribeInstances is empty.
Fix: RunInstances handler / instance state — honour the subnet's MapPublicIpOnLaunch (and the interface-level AssociatePublicIpAddress override) and populate PublicIpAddress.
Traces: ec2.ec2mr-xrg-private-subnet-instance-omits-public-ip.

## Evidence
Cause 1: "us-east-1's unfiltered DescribeInstances() returned ['i-00000000000000002', 'i-00000000000000003'], launched in another region ... the three regions are sharing one instance store"; "us-east-1 resolved group name '<name>' to ['sg-...035','sg-...055','sg-...075'], expected exactly ['sg-...035']"; "regions 'us-east-1' and 'us-west-1' both report subnets [...]"; for the ById forms: "the call succeeded; real AWS rejects it, so there is no error code". Expected: only the caller region's resources; a foreign id refused InvalidInstanceID.NotFound / InvalidGroup.NotFound while the equivalent filter form returns an empty list.
Cause 2: "'us-east-1' derived no public route table via the SDK; every scenario region's default VPC is expected to carry 0.0.0.0/0 -> igw-*"; "[phase 1/4 deploy] the public subnet resolves to [], expected ['rtb-...']".
Cause 3: "the public-subnet instance 'i-00000000000000002' has PublicIpAddress None; a subnet with MapPublicIpOnLaunch=True must assign one".

## Steps to Reproduce
Replay the behavioral suite checks listed in trace_ids against the emulator. For cause 1: create the same-named security group through two regional endpoints and call DescribeSecurityGroups(Filters=[{'Name':'group-name',...}]) on one of them, then DescribeSecurityGroups(GroupIds=[<other region's sg id>]). For cause 2: DescribeRouteTables filtered on the default VPC and look for 0.0.0.0/0 -> igw-*. For cause 3: CreateSubnet, ModifySubnetAttribute(MapPublicIpOnLaunch=true), RunInstances into it, then DescribeInstances and read PublicIpAddress.
