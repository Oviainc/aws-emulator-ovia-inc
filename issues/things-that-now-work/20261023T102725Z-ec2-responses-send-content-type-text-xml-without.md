Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-accept-address-transfer-no-pending-transfer-error
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-accept-address-transfer-no-pending-transfer-error ec2-allocate-address-vpc-and-unowned-address-error ec2-eip-allocate-returns-allocation-id-and-public-ip ec2-elastic-ip-lifecycle ec2-account-and-instance-type-metadata ec2-vpc-attributes-and-account-settings ec2-describe-addresses-lifecycle ec2-t12-eip-disassociate-and-domain-attribute ec2-capacity-block-reservation-for-ml-workloads ec2-catalog-describe-instance-type-offerings ec2-client-vpn-endpoint-mutual-cert-auth-attempt ec2-create-security-group-returns-group-id ec2-page-describe-security-groups-vpc-scoped-trailing-empty-page ec2-security-group-rule-lifecycle ec2-coip-pool-mistaken-usage ec2-coip-pool-usage-errors ec2-custom-ip-provisioning-and-legacy-network-feature-discovery ec2-copy-volumes-same-az ec2-create-vpc-availability-and-delete-lifecycle ec2-local-gateway-vpc-association-route-table-not-found ec2-default-vpc-and-subnet-bootstrap
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|AllocateAddress|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeVpcAttribute|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeAddresses|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeCapacityBlocks|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeInstanceTypeOfferings|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeSecurityGroups|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeCoipPools|2xx->2xx|$headers.content-type
Issue-Key: ec2|CopyVolumes|2xx->2xx|$headers.content-type
Issue-Key: ec2|CreateVpc|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeVpcs|2xx->2xx|$headers.content-type
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# EC2 responses send Content-Type text/xml without the charset parameter

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AllocateAddress

## Description
One cause of 1, present on essentially every EC2 response in the corpus - a single defect in the shared response writer, not per-operation behavior. Twenty clusters have this as their ONLY divergence (so the operation is otherwise wire-correct), and it rides along in nearly every other EC2 cluster too.

Cause 1 of 1: the Content-Type response header is emitted as 'text/xml' where AWS sends 'text/xml;charset=UTF-8'.
AWS: every ec2Query response, success or error, carries Content-Type: text/xml;charset=UTF-8 (recorded behavior only - the Smithy model declares the ec2Query protocol but not the header's charset parameter). The charset parameter is part of the header AWS sends on 200, 400 and 412 alike.
Emulator: Content-Type: text/xml, with the charset parameter dropped.
Why: a client or proxy that decodes the body using the charset advertised by the header falls back to its own default instead of UTF-8, so any non-ASCII byte in a tag value, a tag key or an error message decodes differently against the emulator than against AWS; strict HTTP fixtures and recorded-response comparators also mismatch on every single call.
Fix: the shared ec2Query response writer / Content-Type header for this service, not any individual operation handler - one edit fixes all twenty of the clusters below and removes the noise from the rest of the corpus.
Traces: ec2-accept-address-transfer-no-pending-transfer-error#task#1 (AllocateAddress, 200) plus the nine other content-type-only clusters named in trace_ids; see Evidence for the remaining ten.

## Evidence
20 clusters whose detail is exactly "$headers.content-type: value_mismatch (expected 'text/xml;charset=UTF-8', got 'text/xml')" and nothing else. Representative: AllocateAddress 200 in ec2-accept-address-transfer-no-pending-transfer-error#task#1 (cluster covers 4 steps). Others, by operation: DescribeVpcAttribute, DescribeAddresses (7 steps), DescribeCapacityBlocks, DescribeInstanceTypeOfferings (165 steps), DescribeSecurityGroups, DescribeCoipPools, CopyVolumes, CreateVpc, DescribeVpcs, DescribeApplicationStatus, DescribeFastLaunchImages, DescribeVpcEndpointConnections, DisableSnapshotBlockPublicAccess, DescribeImages, DescribeImportImageTasks, TerminateInstances, DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociations, DeleteSecurityGroup, GetSpotPlacementScores. The same header mismatch also appears as a secondary key in nearly every other EC2 divergence cluster in this corpus.

## Steps to Reproduce
Issue any EC2 ec2Query call against the emulator and read the response headers. Minimal repro: ec2:AllocateAddress Domain=vpc, then compare the Content-Type header against the recorded AWS response ('text/xml;charset=UTF-8'). Replaying any fixture step in trace_ids reproduces it.
