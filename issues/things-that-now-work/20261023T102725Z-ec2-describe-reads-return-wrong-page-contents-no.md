Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-conflict-subnet-cidr-overlap
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-conflict-subnet-cidr-overlap ec2-network-interface-create-and-delete ec2-page-describe-subnets-vpc-scoped-two-pages ec2-default-vpc-and-subnet-bootstrap ec2-page-describe-volumes-tag-filtered-two-pages ec2-page-describe-security-groups-vpc-scoped-trailing-empty-page ec2-page-describe-instances-tag-filtered-two-pages ec2-catalog-describe-instance-type-offerings
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateSubnet|2xx->2xx|$.CreateSubnetResponse.subnet.state
Issue-Key: ec2|CreateSubnet|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item.state
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item[*]
Issue-Key: ec2|DescribeVolumes|2xx->2xx|$.DescribeVolumesResponse.volumeSet.item[*]
Issue-Key: ec2|DescribeSecurityGroups|2xx->2xx|$.DescribeSecurityGroupsResponse.nextToken
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.nextToken
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="c8gb.metal-24xl"]
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$.DescribeInstanceTypeOfferingsResponse.instanceTypeOfferingSet.item[instanceType="c8gb.metal-48xl"]
Issue-Related: 20261023T102725Z-ec2-response-bodies-omit-modeled-members-and-report.md
Issue-Related: 20261023T102725Z-ec2-responses-send-content-type-text-xml-without.md
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# EC2 describe reads return wrong page contents, no nextToken and stuck resource state

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSubnets

## Description
Eight defects in what EC2 describe reads return: resources that never leave their initial state, an empty baseline, pagination that returns the wrong members and drops nextToken, and a region offerings catalog that advertises types AWS does not offer. All eight must be closed before submitting.

Cause 1 of 8: CreateSubnet reports state 'pending' and the subnet never becomes available.
AWS: the CreateSubnetResponse subnet reports state 'available' - subnet creation is synchronous. Smithy: SubnetState enum = [pending, available, unavailable, failed, failed-insufficient-capacity], so 'pending' is a legal value but not the one AWS returns here.
Emulator: state 'pending'. Cluster covers 2 steps.
Why: every client that waits for a subnet to become available before launching into it blocks until its own timeout.
Fix: subnet state transition in the CreateSubnet handler / subnet state.
Traces: ec2-conflict-subnet-cidr-overlap#task#2.

Cause 2 of 8: DescribeSubnets keeps reporting 'pending' afterwards.
AWS: state 'available'.
Emulator: 'pending' - confirming the state is never advanced, not merely reported late in the create response.
Fix: same state as cause 1, observed through DescribeSubnets.
Traces: ec2-page-describe-subnets-vpc-scoped-two-pages#task#10.

Cause 3 of 8: the default VPC has no subnets.
AWS: DescribeSubnets for the default VPC returns one default subnet per Availability Zone, each with defaultForAz 'true', mapPublicIpOnLaunch 'true', subnetArn, privateDnsNameOptionsOnLaunch and blockPublicAccessStates.
Emulator: subnetSet is serialized as the empty string - there is no default subnet anywhere in the baseline.
Why: any tutorial-shaped client that launches into the default VPC without creating network state cannot run.
Fix: default-VPC/default-subnet seeding in EC2 state.
Traces: ec2-default-vpc-and-subnet-bootstrap#task#2. Related open issue 20261022T235108Z-ec2-misclassifies-rejections-and-skips-validation-malformed-ids.md reports the same empty baseline from the behavioral corpus (its cause 9) but carries no wire replay key for this fixture step.

Cause 4 of 8: DescribeSubnets paging returns the wrong subnets on a page.
AWS: page 1 of a vpc-scoped, MaxResults-limited read returns subnet-...024, ...023 and their siblings, ordered as recorded.
Emulator: returns a different set beginning at subnet-...020, so the page membership (not just the order) differs and a two-page walk visits the wrong rows.
Why: a paginating client sees some subnets twice and others never.
Fix: pagination ordering/token handling in the DescribeSubnets handler.
Traces: ec2-page-describe-subnets-vpc-scoped-two-pages#task#9.

Cause 5 of 8: DescribeVolumes paging returns the wrong volumes on a page.
AWS: page 1 returns vol-...01e, ...021 and siblings with status as recorded.
Emulator: returns vol-...01c, ...01d and reports status 'creating' for volumes AWS reports as created.
Why: as cause 4, plus a volume stuck in 'creating' makes every wait-for-available waiter hang.
Fix: pagination ordering in the DescribeVolumes handler and the volume state transition out of 'creating'.
Traces: ec2-page-describe-volumes-tag-filtered-two-pages#task#8.

Cause 6 of 8: DescribeSecurityGroups omits nextToken on a truncated page.
AWS: when a page is truncated the response carries nextToken.
Emulator: nextToken is absent.
Why: a client cannot know there is a further page, so it silently processes a partial result set - the most dangerous pagination failure mode because nothing errors.
Fix: pagination token emission in the DescribeSecurityGroups handler.
Traces: ec2-page-describe-security-groups-vpc-scoped-trailing-empty-page#task#8.

Cause 7 of 8: DescribeInstances paging returns the wrong reservationSet and nextToken.
AWS: the page's reservationSet is a list of the reservations for that page, with nextToken set.
Emulator: reservationSet has the wrong shape/membership and nextToken diverges.
Fix: pagination in the DescribeInstances handler (its body projection is covered separately in the response-members issue).
Traces: ec2-page-describe-instances-tag-filtered-two-pages#task#12.

Cause 8 of 8: DescribeInstanceTypeOfferings advertises instance types AWS does not offer in us-west-2.
AWS: the region's instanceTypeOfferingSet does not contain the metal sizes the emulator adds. Smithy: DescribeInstanceTypeOfferings output InstanceTypeOfferings xmlName 'instanceTypeOfferingSet', MaxResults range 5-1000.
Emulator: returns extra offerings - c8gb.metal-24xl/48xl, c8gn.metal-24xl/48xl, c8ib.metal-48xl/96xl, i3.metal, m8gb/m8gn/m8ib/m8idb/m8in metal sizes, m9g/m9gd.metal-24xl, r8gb/r8gn/r8ib/r8idb/r8idn/r8in metal sizes and u-6tb1.metal - across 18 separate filter queries.
Why: a client that picks an instance type by querying regional offerings chooses a type that RunInstances in real us-west-2 would refuse, so the emulator green-lights an unlaunchable plan.
Fix: the regional instance-type offerings catalog data this operation reads.
Traces: ec2-catalog-describe-instance-type-offerings#task#25 plus 17 sibling clusters in the same fixture (task#27, #30, #53, #91, #93, #96, #98, #100, #102, #103, #144, #146, #149, #151, #152, #153, #165).

## Evidence
Cause 1 - '$.CreateSubnetResponse.subnet.state: value_mismatch (expected available, got pending)' (2 steps). Cause 2 - '$.DescribeSubnetsResponse.subnetSet.item.state: value_mismatch (expected available, got pending)'. Cause 3 - '$.DescribeSubnetsResponse.subnetSet: type_mismatch (expected {item: {...defaultForAz: true...}}, got \'\')'. Cause 4 - subnetSet.item[0..4] mismatch: expected subnet-00000000000000024/023..., got subnet-00000000000000020... with state 'pending'. Cause 5 - volumeSet.item[0..4] mismatch: expected vol-...01e/021..., got vol-...01c/01d with status 'creating'. Cause 6 - '$.DescribeSecurityGroupsResponse.nextToken: missing (expected <shaped-not-asserted>, got None)'. Cause 7 - '$.DescribeInstancesResponse.reservationSet: type_mismatch' plus a divergence on nextToken. Cause 8 - 'instanceTypeOfferingSet.item[instanceType="c8gb.metal-24xl"]: extra (expected None, got {...location: us-west-2})' and 17 further clusters of the same kind.

## Steps to Reproduce
Replay the fixture steps named under Traces. Minimal repros: (1) ec2:CreateSubnet in a VPC and read subnet.state. (2) ec2:DescribeSubnets on it afterwards. (3) ec2:DescribeSubnets Filters=[{'Name':'vpc-id','Values':[default vpc]}]. (4) create 6 subnets in one VPC, then ec2:DescribeSubnets Filters=[vpc-id] MaxResults=5 and walk both pages. (5) create 6 tagged volumes, then ec2:DescribeVolumes Filters=[tag] MaxResults=5. (6) ec2:DescribeSecurityGroups Filters=[vpc-id] MaxResults=5 with more groups than fit and look for <nextToken>. (7) ec2:DescribeInstances Filters=[tag] MaxResults=N. (8) ec2:DescribeInstanceTypeOfferings LocationType='region' Filters=[{'Name':'instance-type','Values':['c8gb.*']}].
