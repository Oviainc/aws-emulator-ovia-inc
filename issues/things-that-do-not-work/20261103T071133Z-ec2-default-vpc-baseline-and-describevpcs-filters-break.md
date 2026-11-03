Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-xrg-traj-isdefault-filter-value-is-lowercase-literal
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-xrg-traj-isdefault-filter-value-is-lowercase-literal ec2-dvpc-traj-describe-vpcs-page-size-and-token-validation ec2-dvpc-traj-dry-run-probe-then-real-call ec2-dvpc-traj-isdefault-value-casing-answers-empty ec2-dvpc-traj-reference-path-instance-in-created-vpc ec2-dvpc-traj-vpc-id-is-region-scoped ec2-ec2mr-byvpc-vpc-create-round-trips-isdefault-and-cidr-associations ec2-ec2mr-dvpc-created-vpc-reports-isdefault-false-not-absent ec2-ec2mr-dvpc-describe-vpcs-rejects-misspelled-isdefault-filter ec2-madl-traj-vpc-subnet-discovery-filters ec2-vvd-traj-vpc-tag-spec-mistakes-and-filter-lookup
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeVpcs|2xx->4xx|$
Issue-Key: ec2|DescribeVpcs|2xx->4xx|$.DescribeVpcsResponse
Issue-Key: ec2|DescribeVpcs|2xx->4xx|status
Issue-Key: ec2|DescribeVpcs|2xx->2xx|$.DescribeVpcsResponse.vpcSet
Issue-Commit: ce4f28f15b4841beefbad6c596b9d2158570753e

# EC2 default-VPC baseline and DescribeVpcs filters break account-shape reads

> _Note: this issue was identified against the code base at commit `ce4f28f15b4841beefbad6c596b9d2158570753e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcs

## Description
DescribeVpcs cannot answer the three queries a client uses to learn the shape of the account: the isDefault filter is rejected outright, the default VPC itself is not describable, and a tag filter over an existing VPC returns an empty set. All three are reads AWS answers 200 with content.

Cause 1 of 3: the isDefault filter is refused.
AWS: Smithy: DescribeVpcs.Filters is a FilterList; AWS supports the filter name 'isDefault' (value the lowercase literal 'true'/'false') and answers 200 with the matching vpcSet — for the recorded request, a single VPC with state 'available', its cidrBlock, tagSet and isDefault 'false'.
Emulator: 400 Code 'InvalidParameterValue', Message "The filter 'isDefault' is invalid", and the step never settles (polled to the 10-attempt cap).
Why: a supported filter is reported as invalid, so the caller cannot enumerate default/non-default VPCs at all and a poll on that read hangs to its budget.
Fix: the filter table behind services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_vpcs.rs must accept isDefault and evaluate it against the VPC's default flag.
Traces: ec2-xrg-traj-isdefault-filter-value-is-lowercase-literal#task#2.

Cause 2 of 3: the account's default VPC is not describable.
AWS: recorded behavior only — DescribeVpcs for the default VPC answers 200 with vpcSet containing the default VPC: state 'available', cidrBlock 172.31.0.0/16, a cidrBlockAssociationSet entry whose cidrBlockState.state is 'associated', dhcpOptionsId, dhcpOptionsSource 'vpc', instanceTenancy 'default', isDefault 'true' and blockPublicAccessStates.internetGatewayBlockMode 'off'.
Emulator: 400 — no DescribeVpcsResponse body at all.
Why: the default-VPC baseline every region has in a real account is missing (or unreadable), so bootstrap flows that discover the default VPC and its 172.31/16 range cannot start.
Fix: the account/region baseline in the EC2 state store plus describe_vpcs.rs projection (isDefault, dhcpOptionsSource, blockPublicAccessStates).
Traces: ec2-dvpc-traj-describe-vpcs-page-size-and-token-validation#task#5 (cluster also covers 18 further steps across ec2-dvpc-*, ec2-ec2mr-*, ec2-madl-* and ec2-xrg-* fixtures).

Cause 3 of 3: a tag filter over an existing VPC returns an empty vpcSet.
AWS: recorded behavior only — 200 with vpcSet.item describing the tagged VPC (vpcId, ownerId, state 'available', cidrBlock, cidrBlockAssociationSet, dhcpOptionsId, dhcpOptionsSource 'vpc', tagSet carrying the Name tag from the create request, instanceTenancy 'default', isDefault 'false', blockPublicAccessStates).
Emulator: 200 with vpcSet serialized as an empty value.
Why: tags set in CreateVpc's TagSpecifications are not indexed for filtering, so tag-based discovery — the normal way these trajectories find their own resources — silently finds nothing.
Fix: tag indexing in create_vpc.rs / the VPC state store and the tag:<key> filter evaluation in describe_vpcs.rs.
Traces: ec2-vvd-traj-vpc-tag-spec-mistakes-and-filter-lookup#task#6.

## Evidence
Replayed wire corpus simparity-wire-aws-v1. ec2-xrg-traj-isdefault-filter-value-is-lowercase-literal task#2 — expected 200 with vpcSet.item (isDefault 'false'); actual 400 'InvalidParameterValue' / "The filter 'isDefault' is invalid", and the poll reports 'status 400, not the graded 200 after 10 attempt(s)'. ec2-dvpc-traj-describe-vpcs-page-size-and-token-validation task#5 — expected 200 with the default VPC (cidrBlock 172.31.0.0/16, isDefault 'true'); actual 400 with no response body (19 steps affected). ec2-vvd-traj-vpc-tag-spec-mistakes-and-filter-lookup task#6 — expected vpcSet.item for the tagged VPC; actual vpcSet ''.

## Steps to Reproduce
1. Call DescribeVpcs with Filter.1.Name=isDefault, Filter.1.Value.1=false (fixture ec2-xrg-traj-isdefault-filter-value-is-lowercase-literal, task#2): expect 200 with a vpcSet, observe 400 InvalidParameterValue.
2. Call DescribeVpcs with no arguments in a fresh region (ec2-dvpc-traj-describe-vpcs-page-size-and-token-validation, task#5): expect the default VPC, observe 400.
3. CreateVpc with TagSpecifications Name=<n>, then DescribeVpcs with Filter tag:Name=<n> (ec2-vvd-traj-vpc-tag-spec-mistakes-and-filter-lookup, task#6): observe an empty vpcSet.
