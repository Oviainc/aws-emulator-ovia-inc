Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.create-route-server-endpoint-subnet-outside-vpc-rejected ec2.create-route-server-peer-invalid-bgp-asn-rejected ec2.create-route-server-peer-malformed-asn-rejected ec2.op-contract-create-route-server ec2.describe-transit-gateway-id-not-found-rejected ec2.op-contract-create-transit-gateway-vpc-attachment ec2.op-contract-create-transit-gateway-prefix-list-reference ec2.op-contract-associate-enclave-certificate-iam-role ec2.op-contract-delete-instance-connect-endpoint ec2.op-contract-cancel-export-task ec2.op-contract-cancel-reserved-instances-listing ec2.op-contract-create-vpc-block-public-access-exclusion ec2.op-contract-deleteVpcBlockPublicAccessExclusion ec2.multiattach-modify-of-an-io2-volume-to-gp3-is-refused-until-the-flag-is-cleared
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.create-route-server-endpoint-subnet-outside-vpc-rejected|_error_code#15|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: ec2.create-route-server-peer-invalid-bgp-asn-rejected|run#18|AssertionError|assert message == "Value (PeerAsn) for 4294967296 is invalid.", message
Issue-Key: ec2.create-route-server-peer-malformed-asn-rejected|run#18|AssertionError|assert message == "Value (PeerAsn) for -1 is invalid.", message
Issue-Key: ec2.op-contract-create-route-server|run#13|AssertionError|assert message == "Value (AmazonSideAsn) for 4294967296 is invalid.", message
Issue-Key: ec2.describe-transit-gateway-id-not-found-rejected|run#12|AssertionError|assert code_malformed == "InvalidTransitGatewayID.Malformed", (
Issue-Key: ec2.op-contract-create-transit-gateway-vpc-attachment|run#13|AssertionError|assert foreign == want_foreign, (
Issue-Key: ec2.op-contract-create-transit-gateway-prefix-list-reference|_error_of#6|AssertionError|raise AssertionError("the call real AWS rejects was accepted")
Issue-Key: ec2.op-contract-associate-enclave-certificate-iam-role|_error_of#23|AssertionError|raise AssertionError(f"expected rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: ec2.op-contract-delete-instance-connect-endpoint|_error_of#24|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: ec2.op-contract-cancel-export-task|run#4|AssertionError|assert message == "The export-instance-task ID 'export-i-0123456789abcdef0' does not exist", message
Issue-Key: ec2.op-contract-cancel-reserved-instances-listing|run#7|AssertionError|assert code_message == expected_code_message, f"expected message {expected_code_message!r}, got {code_message!r}"
Issue-Key: ec2.op-contract-create-vpc-block-public-access-exclusion|run#13|AssertionError|assert excl["ResourceArn"] == subnet_arn, f"expected ResourceArn {subnet_arn}, got {excl['ResourceArn']}"
Issue-Key: ec2.multiattach-modify-of-an-io2-volume-to-gp3-is-refused-until-the-flag-is-cleared|run#11|AssertionError|assert message == "The parameter multi-attach-enabled is not supported for gp3 volumes.", (
Issue-Related: 20261029T175114Z-ec2-read-paths-wrong-or-missing-members-in.md
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# EC2 route-server, transit-gateway and resource-ARN operations accept requests AWS refuses and author their own refusal messages (10 root causes)

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRouteServerEndpoint

## Description
Ten net-new root causes across EC2's route-server, transit-gateway, enclave, instance-connect, cancel-family and VPC-block-public-access operations. The EC2 model is ec2Query and declares no per-operation errors (smithy_query operation=... -> "errors": []), so the codes and messages below are the recorded AWS behavior.

Cause 1 of 10: CreateRouteServerEndpoint accepts a subnet outside the route server's VPC.
AWS: recorded behavior only. A subnet that does not belong to the route server's VPC is refused; there is no success path.
Emulator: returns HTTP 200 and creates the endpoint (replay shows a RouteServerEndpoint with State='pending' and an EniAddress allocated).
Why: the endpoint cannot exist on AWS, so a topology that the emulator provisions is unbuildable in production.
Fix: the subnet/VPC consistency check in the CreateRouteServerEndpoint handler's validation.
Traces: ec2.create-route-server-endpoint-subnet-outside-vpc-rejected

Cause 2 of 10: route-server ASN refusals use emulator prose instead of AWS's parameter-value template.
AWS: recorded behavior only. An ASN outside 1..4294967295 is refused with AWS's `Value (<value>) for parameter <member> is invalid. Valid values are from 1 to 4294967295.` template - fixed text with the submitted value and the member path (`bgpOptions.peerAsn`, `amazonSideAsn`) as variables.
Emulator: emits its own sentences, e.g. `Invalid value '4294967296' for amazonSideAsn. It must be from 1 to 4294967295`, and the same divergence for peerAsn on both the above-range and the negative case.
Why: callers that parse the parameter path out of the message to point a user at the offending field cannot find it.
Fix: the ASN range validation message templates in the CreateRouteServer / CreateRouteServerPeer handlers.
Traces: ec2.create-route-server-peer-invalid-bgp-asn-rejected, ec2.create-route-server-peer-malformed-asn-rejected, ec2.op-contract-create-route-server

Cause 3 of 10: a mismatched-prefix transit-gateway id is classified NotFound instead of Malformed.
AWS: recorded behavior only. An id whose prefix does not match the resource type is refused InvalidTransitGatewayID.Malformed.
Emulator: answers InvalidTransitGatewayID.NotFound.
Why: Malformed is terminal (fix the input) while NotFound invites retry/eventual-consistency handling; the swap makes clients retry an unfixable request.
Fix: id shape validation ahead of lookup in the DescribeTransitGateways handler.
Traces: ec2.describe-transit-gateway-id-not-found-rejected

Cause 4 of 10: CreateTransitGatewayVpcAttachment's unknown-subnet message diverges.
AWS: recorded behavior only. A subnet that does not exist (or belongs to another VPC) is refused ('InvalidSubnetID.NotFound', 'Subnet <subnet-id> was deleted or does not exist.') - fixed text `Subnet ` / ` was deleted or does not exist.` with the id from the request.
Emulator: answers the same code with `The subnet ID '<subnet-id>' does not exist`.
Why: AWS's wording distinguishes "deleted or never existed"; log-based and message-keyed handling of attachment failures does not match.
Fix: the subnet-resolution message template in the CreateTransitGatewayVpcAttachment handler.
Traces: ec2.op-contract-create-transit-gateway-vpc-attachment

Cause 5 of 10: CreateTransitGatewayPrefixListReference accepts a request AWS refuses.
AWS: recorded behavior only. The request this check issues is refused; there is no success path.
Emulator: accepts it.
Why: an invalid prefix-list reference is stored, so route propagation behaves differently here than on AWS.
Fix: the validation in the CreateTransitGatewayPrefixListReference handler.
Traces: ec2.op-contract-create-transit-gateway-prefix-list-reference

Cause 6 of 10: AssociateEnclaveCertificateIamRole accepts a request AWS refuses.
AWS: recorded behavior only. The request is refused; the emulator's success body also composes CertificateS3ObjectKey by concatenating the role ARN and the certificate ARN, which is not an S3 object key shape.
Emulator: returns HTTP 200 with `CertificateS3ObjectKey: 'arn:aws:iam::<acct>:role/<role>/arn:aws:acm:<region>:<acct>:certificate/<id>'`.
Why: a client that fetches the object at the returned key gets a path that cannot exist, and the refusal it should have handled never arrives.
Fix: the validation and the object-key composition in the AssociateEnclaveCertificateIamRole handler.
Traces: ec2.op-contract-associate-enclave-certificate-iam-role

Cause 7 of 10: DeleteInstanceConnectEndpoint accepts a delete AWS refuses and reports a terminal state.
AWS: recorded behavior only. The delete this check issues is refused; there is no success path.
Emulator: returns HTTP 200 with the endpoint already in State='delete-complete'.
Why: the caller believes the (refused) delete succeeded and skips the remediation AWS requires; the instantaneous terminal state also hides the `delete-in-progress` transition clients poll for.
Fix: the state/precondition guard in the DeleteInstanceConnectEndpoint handler.
Traces: ec2.op-contract-delete-instance-connect-endpoint

Cause 8 of 10: cancel-family refusal messages diverge from AWS's templates.
AWS: recorded behavior only. CancelExportTask on an unknown task and CancelReservedInstancesListing with a bad listing id are refused with AWS's exact text - for the listing case the message is ` Invalid value for 'listingId'` (AWS's text begins with a space; that leading space is part of the fixed text, not a formatting artefact).
Emulator: emits `The export task id '<id>' does not exist` for the first and drops the leading space for the second (`Invalid value for 'listingId'`).
Why: byte-for-byte message assertions and message-keyed routing in client code fail on both.
Fix: the message templates in the CancelExportTask and CancelReservedInstancesListing handlers.
Traces: ec2.op-contract-cancel-export-task, ec2.op-contract-cancel-reserved-instances-listing

Cause 9 of 10: VPC block-public-access exclusions report their own ARN instead of the excluded resource's.
AWS: recorded behavior only. A VpcBlockPublicAccessExclusion's ResourceArn is the ARN of the EXCLUDED resource - e.g. `arn:aws:ec2:<region>:<acct>:subnet/<subnet-id>` or `.../vpc/<vpc-id>` - taken from the CreateVpcBlockPublicAccessExclusion request.
Emulator: returns the exclusion's own ARN, `arn:aws:ec2:<region>:<acct>:vpc-block-public-access-exclusion/<exclusion-id>`, on create, describe and delete.
Why: ResourceArn is the only field that says WHICH resource is excluded; a client auditing exclusions cannot map them back to VPCs or subnets at all.
Fix: the exclusion serializer / the ResourceArn field in block-public-access state.
Traces: ec2.op-contract-create-vpc-block-public-access-exclusion, ec2.op-contract-describe-vpc-block-public-access-exclusions, ec2.op-contract-deleteVpcBlockPublicAccessExclusion

Cause 10 of 10: multi-attach volume-type refusals use emulator prose and the wrong class.
AWS: recorded behavior only. CreateVolume with MultiAttachEnabled=true on a gp3 volume is refused with exactly 'InvalidParameterCombination', and ModifyVolume changing an io2 multi-attach volume to gp3 carries AWS's own wording.
Emulator: answers 'InvalidParameterValue' with `multiAttachEnabled is supported with io1 and io2 volumes only, not with a gp3 volume`, and for ModifyVolume `multiAttachEnabled is supported with io1 and io2 volumes only, so it cannot be enabled on a gp3 volume`.
Why: the request is a bad combination of two valid members; clients that branch on InvalidParameterCombination to clear the flag before retrying never do so.
Fix: the multi-attach/volume-type validation in the CreateVolume and ModifyVolume handlers.
Traces: ec2.multiattach-modify-of-an-io2-volume-to-gp3-is-refused-until-the-flag-is-cleared

## Evidence
Replayed failing checks under `### ec2` (none marked ALREADY FILED / OWNED by, except where noted):

1. ec2.create-route-server-endpoint-subnet-outside-vpc-rejected -> AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'RouteServerEndpoint': {'RouteServerId': 'rs-...', 'RouteServerEndpointId': 'rse-...', 'VpcId': 'vpc-...', 'SubnetId': 'subnet-...', 'EniId': 'eni-...', 'EniAddress': '10.76.1.4', 'State': 'pending', ...}, ... 'HTTPStatusCode': 200 ...}
2. ec2.create-route-server-peer-invalid-bgp-asn-rejected -> AssertionError: Value (4294967296) for parameter bgpOptions.peerAsn is invalid. Valid values are from 1 to 4294967295.
   ec2.create-route-server-peer-malformed-asn-rejected -> AssertionError: Value (-1) for parameter bgpOptions.peerAsn is invalid. Valid values are from 1 to 4294967295.
   ec2.op-contract-create-route-server -> AssertionError: Invalid value '4294967296' for amazonSideAsn. It must be from 1 to 4294967295
3. ec2.describe-transit-gateway-id-not-found-rejected -> AssertionError: expected InvalidTransitGatewayID.Malformed for mismatched-prefix id, got 'InvalidTransitGatewayID.NotFound'
4. ec2.op-contract-create-transit-gateway-vpc-attachment -> AssertionError: a subnet of vpc-... attached for vpc-...: expected ('InvalidSubnetID.NotFound', 'Subnet subnet-... was deleted or does not exist.'), got ('InvalidSubnetID.NotFound', "The subnet ID 'subnet-...' does not exist")
5. ec2.op-contract-create-transit-gateway-prefix-list-reference -> AssertionError: the call real AWS rejects was accepted
6. ec2.op-contract-associate-enclave-certificate-iam-role -> AssertionError: expected rejection, but the call SUCCEEDED and returned {'CertificateS3BucketName': 'aws-ec2-enclave-certificate-us-west-2-000000000000', 'CertificateS3ObjectKey': 'arn:aws:iam::000000000000:role/<role>/arn:aws:acm:us-west-2:000000000000:certificate/<id>', ...}
7. ec2.op-contract-delete-instance-connect-endpoint -> AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'InstanceConnectEndpoint': {... 'State': 'delete-complete' ...}, ... 'HTTPStatusCode': 200 ...}
8. ec2.op-contract-cancel-export-task -> AssertionError: The export task id 'export-i-0123456789abcdef0' does not exist
   ec2.op-contract-cancel-reserved-instances-listing -> AssertionError: expected message " Invalid value for 'listingId'", got "Invalid value for 'listingId'"
9. ec2.op-contract-create-vpc-block-public-access-exclusion -> AssertionError: expected ResourceArn arn:aws:ec2:us-west-2:000000000000:subnet/subnet-00000000000000037, got arn:aws:ec2:us-west-2:000000000000:vpc-block-public-access-exclusion/vpcbpa-exclude-0000000000000003b
   ec2.op-contract-deleteVpcBlockPublicAccessExclusion -> AssertionError: expected exclusion ResourceArn to reference vpc-00000000000000024, got {... 'ResourceArn': 'arn:aws:ec2:us-west-2:000000000000:vpc-block-public-access-exclusion/vpcbpa-exclude-...' ...}
   ec2.op-contract-describe-vpc-block-public-access-exclusions -> AssertionError: expected exclusion ResourceArn to reference vpc-00000000000000024, got 'arn:aws:ec2:...:vpc-block-public-access-exclusion/vpcbpa-exclude-...' (this one is listed as OWNED by 20261029T175114Z-ec2-read-paths-wrong-or-missing-members-in.md; it is included here only as corroborating evidence that the create and delete paths share the same defect)
10. ec2.multiattach-modify-of-an-io2-volume-to-gp3-is-refused-until-the-flag-is-cleared -> AssertionError: ModifyVolume(VolumeType='gp3') refusal message on vol-...: 'multiAttachEnabled is supported with io1 and io2 volumes only, so it cannot be enabled on a gp3 volume'

## Steps to Reproduce
1. git_clone, then run_behavioral_suite; the checks in trace_ids appear under `### ec2`.
2. Standalone repros:
   - CreateRouteServer in VPC A, CreateSubnet in VPC B, CreateRouteServerEndpoint with that subnet: expect a refusal.
   - CreateRouteServer(AmazonSideAsn=4294967296) and CreateRouteServerPeer(BgpOptions={PeerAsn:4294967296}) and PeerAsn=-1: expect AWS's `Value (<v>) for parameter <path> is invalid. Valid values are from 1 to 4294967295.`
   - DescribeTransitGateways(TransitGatewayIds=['tgw-attach-0123456789abcdef0']): expect InvalidTransitGatewayID.Malformed.
   - CreateTransitGatewayVpcAttachment naming a subnet of another VPC: expect `Subnet <id> was deleted or does not exist.`
   - CreateTransitGatewayPrefixListReference with the check's invalid input: expect a refusal.
   - AssociateEnclaveCertificateIamRole with the check's input: expect a refusal.
   - CreateInstanceConnectEndpoint, then DeleteInstanceConnectEndpoint twice / with the check's precondition: expect a refusal, and never an immediate 'delete-complete'.
   - CancelExportTask('export-i-0123456789abcdef0') and CancelReservedInstancesListing with a bad listing id: compare the messages byte-for-byte.
   - CreateVpcBlockPublicAccessExclusion(SubnetId=<subnet>) then DescribeVpcBlockPublicAccessExclusions: ResourceArn must be the subnet ARN.
   - CreateVolume(VolumeType='gp3', MultiAttachEnabled=True): expect InvalidParameterCombination; ModifyVolume an io2 multi-attach volume to gp3: expect AWS's wording.
