Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-accept-vpc-peering-connection-lifecycle
Issue-Step: teardown#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-accept-vpc-peering-connection-lifecycle ec2-create-vpc-availability-and-delete-lifecycle ec2-account-and-instance-type-metadata ec2-byoip-cidr-advertise-withdraw-lifecycle ec2-hardware-and-byoip-not-found-error-catalog ec2-describe-addresses-lifecycle ec2-elastic-ip-lifecycle ec2-describe-application-status-contract ec2-describe-availability-zones-contract ec2-describe-fast-launch-images-contract ec2-describe-vpc-endpoint-connections-contract ec2-dry-run-permission-checks-across-mutations
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeVpcs|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|DescribeVpcs|4xx->4xx|$headers.content-type
Issue-Key: ec2|DescribeVpcAttribute|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|DescribeVpcAttribute|4xx->4xx|$headers.content-type
Issue-Key: ec2|AdvertiseByoipCidr|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|AdvertiseByoipCidr|4xx->4xx|$headers.content-type
Issue-Key: ec2|DeprovisionByoipCidr|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|DeprovisionByoipCidr|4xx->4xx|$headers.content-type
Issue-Key: ec2|DescribeAddresses|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|DescribeAddresses|4xx->4xx|$headers.content-type
Issue-Key: ec2|DescribeApplicationStatus|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|DescribeApplicationStatus|4xx->4xx|$headers.content-type
Issue-Key: ec2|DescribeAvailabilityZones|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|DescribeAvailabilityZones|4xx->4xx|$headers.content-type
Issue-Key: ec2|DescribeFastLaunchImages|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|DescribeFastLaunchImages|4xx->4xx|$headers.content-type
Issue-Key: ec2|DescribeVpcEndpointConnections|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|DescribeVpcEndpointConnections|4xx->4xx|$headers.content-type
Issue-Key: ec2|DisableSnapshotBlockPublicAccess|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|DisableSnapshotBlockPublicAccess|4xx->4xx|$headers.content-type
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# EC2 ec2Query error envelope omits the RequestID element

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcs

## Description
One cause of 1, reproduced by 16 separate divergence clusters across unrelated operations - a single defect in the shared error serializer, not per-operation behavior.

Cause 1 of 1: every EC2 error response is serialized without its <RequestID> element.
AWS: the ec2Query error envelope is <Response><Errors><Error><Code/><Message/></Error></Errors><RequestID>...</RequestID></Response>; the RequestID child of Response is always present, on 400 rejections and on 412 DryRunOperation responses alike (recorded behavior only - EC2 models no error shapes, so the envelope is defined by the protocol and the recordings, not by a modeled member). The recorded ids themselves (e.g. 'aa1c7dd4-35ac-4b37-a130-9b04c7b4821e') are per-request values and are not a requirement; the presence of the element is.
Emulator: <Response> carries Errors but no RequestID at all ($.Response.RequestID: missing ... got None), on every error path exercised: DescribeVpcs, DescribeVpcAttribute, AdvertiseByoipCidr, DeprovisionByoipCidr, DescribeAddresses, DescribeApplicationStatus (412), DescribeAvailabilityZones, DescribeFastLaunchImages, DescribeVpcEndpointConnections (412), DisableSnapshotBlockPublicAccess (412), DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociations (412), RunInstances (412), DescribeSecurityGroups, DescribeSubnets, CopyVolumes.
Why: the request id is what a client echoes into its own logs and quotes to support when a call fails; XML parsers and SDK error wrappers that read Response/RequestID get a null, and any client-side correlation between a failed call and its id is lost. Successful responses do carry a requestId, so only the failure path is affected - exactly the path where the id matters.
Fix: the shared ec2Query error serializer for this service - it must emit RequestID alongside Errors for every error status (400 and 412 both reproduce).
Traces: ec2-accept-vpc-peering-connection-lifecycle#teardown#10 plus the fifteen other clusters named in trace_ids and in Evidence; each is one operation's error path exhibiting the same omission.

## Evidence
16 clusters whose only body divergence is '$.Response.RequestID: missing (expected <id>, got None)'. Representative: DescribeVpcs 400 in ec2-accept-vpc-peering-connection-lifecycle#teardown#10 (cluster also covers teardown#12 and ec2-create-vpc-availability-and-delete-lifecycle#task#4). Others: DescribeVpcAttribute 400, AdvertiseByoipCidr 400, DeprovisionByoipCidr 400, DescribeAddresses 400, DescribeApplicationStatus 412, DescribeAvailabilityZones 400, DescribeFastLaunchImages 400, DescribeVpcEndpointConnections 412, DisableSnapshotBlockPublicAccess 412, DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociations 412, RunInstances 412, DescribeSecurityGroups 400, DescribeSubnets 400, CopyVolumes 400. The same omission also rides along in every other EC2 error cluster in the corpus.

## Steps to Reproduce
Replay any fixture step named in trace_ids and read the raw XML. Minimal repro: ec2:DescribeVpcs VpcIds=['vpc-00000000000000000'] (a nonexistent id) and inspect the <Response> body - there is no <RequestID> child. Same with any DryRun=true mutation, which returns 412 DryRunOperation, also without RequestID.
