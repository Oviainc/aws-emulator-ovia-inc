Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-accept-transitgateway-and-vpcendpoint-connection-errors
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-accept-transitgateway-and-vpcendpoint-connection-errors ec2-allocate-hosts-error ec2-allocate-hosts-invalid-instance-type-error ec2-allocate-hosts-invalid-zone-error ec2-dedicated-host-lifecycle ec2-conflict-subnet-cidr-overlap ec2-copy-volumes-error-handling ec2-custom-ip-provisioning-and-legacy-network-feature-discovery ec2-describe-volumes-attributes-and-unknown-id-rejection ec2-page-describe-volumes-tag-filtered-two-pages
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|AcceptTransitGatewayMulticastDomainAssociations|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|AcceptTransitGatewayMulticastDomainAssociations|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|AcceptTransitGatewayMulticastDomainAssociations|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|AcceptTransitGatewayMulticastDomainAssociations|4xx->4xx|$headers.content-type
Issue-Key: ec2|AllocateHosts|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|AllocateHosts|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|AllocateHosts|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|AllocateHosts|4xx->4xx|$headers.content-type
Issue-Key: ec2|CreateVpc|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateVpc|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateVpc|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|CreateVpc|4xx->4xx|$headers.content-type
Issue-Key: ec2|CopyVolumes|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CopyVolumes|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CopyVolumes|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|CopyVolumes|4xx->4xx|$headers.content-type
Issue-Key: ec2|DescribeCoipPools|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeCoipPools|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeCoipPools|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|DescribeCoipPools|4xx->4xx|$headers.content-type
Issue-Key: ec2|DescribeVolumes|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeVolumes|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeVolumes|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|DescribeVolumes|4xx->4xx|$headers.content-type
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# EC2 picks the wrong resource-specific error code on not-found and range rejections

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CopyVolumes

## Description
Six operations reject with HTTP 400 but an EC2 error Code that names the wrong condition - or, in one case, the wrong parameter entirely. All six must be closed before submitting. Background: EC2 models no error shapes (Smithy com-amazonaws-ec2-amazonec2: errors: [] on every operation queried), so the Code is the whole branchable contract; the recorded messages are examples. Related open issue 20261022T235108Z-ec2-misclassifies-rejections-and-skips-validation-malformed-ids.md reports the same family of misclassification from the BEHAVIORAL corpus (its causes 4 and 7 overlap causes 3 and 6 here) but carries no wire replay key for any of these fixture steps, so a fix verified against it would leave these unexercised.

Cause 1 of 6: AcceptTransitGatewayMulticastDomainAssociations validates the wrong member.
AWS: Code InvalidTransitGatewayAttachmentID.NotFound for the unknown attachment id supplied in the request (message example "The transit-gateway-attachment ID 'tgw-attach-00000000000000002' does not exist").
Emulator: Code InvalidTransitGatewayMulticastDomainId.NotFound, complaining about a multicast-domain id ('tgw-mcast-domain-00000000000000001') instead of the attachment id the caller got wrong.
Why: the client is told to fix a parameter that was fine and is never told about the one that was not.
Fix: validation order / member selection in the AcceptTransitGatewayMulticastDomainAssociations handler.
Traces: ec2-accept-transitgateway-and-vpcendpoint-connection-errors#task#2.

Cause 2 of 6: AllocateHosts with both InstanceType and InstanceFamily set.
AWS: Code InvalidRequest (message example "You cannot set both 'InstanceType' and 'InstanceFamily'.").
Emulator: Code InvalidParameterCombination.
Why: AWS reserves InvalidParameterCombination for other forms here; a client branching on InvalidRequest to fall back to a single dimension never matches. This cluster covers four fixtures.
Fix: the mutual-exclusion check in the AllocateHosts handler.
Traces: ec2-allocate-hosts-error#task#1.

Cause 3 of 6: CreateVpc with a CIDR outside the permitted VPC range.
AWS: Code InvalidVpc.Range for 10.0.0.0/8 (message example "The CIDR '10.0.0.0/8' is invalid.").
Emulator: Code InvalidParameterValue.
Why: a client that distinguishes a range violation (retry with a /16) from a syntax error cannot.
Fix: CIDR range validation in the CreateVpc handler.
Traces: ec2-conflict-subnet-cidr-overlap#task#4.

Cause 4 of 6: CopyVolumes with a malformed volume id.
AWS: Code InvalidVolumeID.Malformed - the id syntax is checked before lookup (message example "The volume ID 'vol-0123456789abcdef0' is malformed").
Emulator: Code InvalidVolume.NotFound - it goes straight to lookup.
Why: a typo is reported as a missing resource, so repair logic waits for a volume that can never appear.
Fix: volume-id syntax validation before lookup in the CopyVolumes handler.
Traces: ec2-copy-volumes-error-handling#task#1 (cluster also covers task#2).

Cause 5 of 6: DescribeCoipPools with a malformed pool id.
AWS: Code InvalidPoolID.Malformed (message example "The pool ID 'coip-pool-00000000000000000' is invalid.").
Emulator: Code InvalidPoolID.NotFound.
Why and Fix: as cause 4, in the DescribeCoipPools handler.
Traces: ec2-custom-ip-provisioning-and-legacy-network-feature-discovery#task#8.

Cause 6 of 6: DescribeVolumes with an id that fails parameter validation.
AWS: Code InvalidParameterValue (message example "Value (vol-1234567890abcdef0) for parameter volumes is invalid. Expected: 'vol-...'.").
Emulator: Code InvalidVolume.NotFound.
Why: a parameter-level rejection is reported as a resource-level one, so a client retries a lookup that can never succeed.
Fix: request validation ordering in the DescribeVolumes handler.
Traces: ec2-describe-volumes-attributes-and-unknown-id-rejection#task#3 (cluster also covers ec2-page-describe-volumes-tag-filtered-two-pages#task#12).

## Evidence
All six clusters are status 400 -> 400 with '$.Response.Errors.Error.Code: value_mismatch'. Cause 1 expected 'InvalidTransitGatewayAttachmentID.NotFound', got 'InvalidTransitGatewayMulticastDomainId.NotFound'. Cause 2 expected 'InvalidRequest', got 'InvalidParameterCombination' (affected_count 4). Cause 3 expected 'InvalidVpc.Range', got 'InvalidParameterValue'. Cause 4 expected 'InvalidVolumeID.Malformed', got 'InvalidVolume.NotFound' (affected_count 2). Cause 5 expected 'InvalidPoolID.Malformed', got 'InvalidPoolID.NotFound'. Cause 6 expected 'InvalidParameterValue', got 'InvalidVolume.NotFound' (affected_count 2).

## Steps to Reproduce
Replay the fixture steps named under Traces. Minimal repros: (1) ec2:AcceptTransitGatewayMulticastDomainAssociations TransitGatewayAttachmentId='tgw-attach-00000000000000002'. (2) ec2:AllocateHosts with both InstanceType and InstanceFamily set. (3) ec2:CreateVpc CidrBlock='10.0.0.0/8'. (4) ec2:CopyVolumes with SourceVolumeId='vol-0123456789abcdef0'. (5) ec2:DescribeCoipPools PoolIds=['coip-pool-00000000000000000']. (6) ec2:DescribeVolumes VolumeIds=['vol-1234567890abcdef0'].
