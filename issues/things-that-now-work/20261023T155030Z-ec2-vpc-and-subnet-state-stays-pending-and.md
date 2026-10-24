Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-subnet-route-table-lifecycle
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-subnet-route-table-lifecycle ec2-t12-post-delete-not-found-probes ec2-vpc-attributes-and-account-settings
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item.dhcpOptionsId
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item.dhcpOptionsSource
Issue-Key: ec2|DescribeVpcs|2xx->2xx|$.DescribeVpcsResponse.vpcSet.item.state
Issue-Related: 20261023T102725Z-ec2-describe-reads-return-wrong-page-contents-no.md
Issue-Related: 20261023T102725Z-ec2-response-bodies-omit-modeled-members-and-report.md
Issue-Related: 20261023T102725Z-ec2-responses-send-content-type-text-xml-without.md
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# EC2 VPC and subnet state stays pending and omits dhcpOptions

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSubnets

## Description
Two causes with one root: newly created VPCs and subnets are left in state 'pending' forever and are not linked to the account's DHCP option set, so the describe reads never match what AWS reports a moment after creation.

Cause 1 of 2 — DescribeSubnets.
AWS: a subnet created by CreateSubnet is reported by DescribeSubnets with state 'available' (the pending state is transient and the recording shows it already settled by the first read), plus dhcpOptionsId naming the option set inherited from the VPC and dhcpOptionsSource 'vpc'. Smithy: no state machine or default is modeled for these members, so this is recorded behavior only, taken from the recorded DescribeSubnets response.
Emulator: state is 'available' never reached — it reports 'pending' — and both dhcpOptionsId and dhcpOptionsSource are absent from the item.
Trace: ec2-subnet-route-table-lifecycle#task#5 (also #task#7 and ec2-t12-post-delete-not-found-probes#task#7).

Cause 2 of 2 — DescribeVpcs.
AWS: a VPC created by CreateVpc is reported with state 'available' and dhcpOptionsSource 'vpc'.
Emulator: state 'pending' and dhcpOptionsSource absent.
Trace: ec2-vpc-attributes-and-account-settings#task#2.

Why: 'available' is the readiness signal for both resources — the AWS SDK VpcAvailable waiter and every hand-rolled wait loop block on it, and the corpus' own fixtures proceed on it. A client that creates a VPC or subnet and waits for 'available' never unblocks, so nothing downstream (launching into the subnet, attaching a gateway) is reachable. The missing dhcpOptionsId/dhcpOptionsSource additionally break any client that reads a subnet's inherited DHCP configuration rather than querying the VPC.

Fix: the ec2 service crate's VPC/subnet state — the create handlers must settle the resource into state 'available' (immediately, or via the same transition the describe path reads) and record the inherited DHCP option set, and the DescribeSubnets / DescribeVpcs serializers must render dhcpOptionsId and dhcpOptionsSource from it.

Related open issue: 20261023T102725Z-ec2-describe-reads-return-wrong-page-contents-no.md mentions stuck resource state but carries neither of these two replay keys (owned_by: [] on both clusters), so they are filed here.

Traces: ec2-subnet-route-table-lifecycle#task#5 (representative), ec2-vpc-attributes-and-account-settings#task#2.

## Evidence
Replay of simparity-wire-aws-v1, 2 divergence clusters, both status 200 expected and returned.

Cause 1 — divergence_key 'service=ec2|DescribeSubnets|status=200->200|keys=$.DescribeSubnetsResponse.subnetSet.item.dhcpOptionsId,$.DescribeSubnetsResponse.subnetSet.item.dhcpOptionsSource,$.DescribeSubnetsResponse.subnetSet.item.state,$headers.content-type', fixture ec2-subnet-route-table-lifecycle step task#5 (affected also task#7, ec2-t12-post-delete-not-found-probes#task#7):
  "state: value_mismatch (expected 'available', got 'pending')"
  "dhcpOptionsId: missing (expected 'dopt-09f6f8711b0848656', got None)"  [the id value itself is environment-specific; the requirement is that the member be present]
  "dhcpOptionsSource: missing (expected 'vpc', got None)"

Cause 2 — divergence_key 'service=ec2|DescribeVpcs|status=200->200|keys=$.DescribeVpcsResponse.vpcSet.item.dhcpOptionsSource,$.DescribeVpcsResponse.vpcSet.item.state,$headers.content-type', fixture ec2-vpc-attributes-and-account-settings step task#2:
  "state: value_mismatch (expected 'available', got 'pending')"
  "dhcpOptionsSource: missing (expected 'vpc', got None)"

## Steps to Reproduce
1. git_clone() then replay_wire_corpus().
2. Replay fixture ec2-subnet-route-table-lifecycle to step task#5: CreateSubnet then DescribeSubnets. The returned subnet item has state 'pending' and no dhcpOptionsId / dhcpOptionsSource; the recording has state 'available' with both present.
3. Replay fixture ec2-vpc-attributes-and-account-settings to step task#2: DescribeVpcs on the created VPC reports state 'pending' and no dhcpOptionsSource.
4. Re-read either resource after any delay — the state never advances.
