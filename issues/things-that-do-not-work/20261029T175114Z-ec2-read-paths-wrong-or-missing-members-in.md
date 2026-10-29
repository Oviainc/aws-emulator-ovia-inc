Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-describe-vpc-block-public-access-exclusions ec2.op-contract-deleteVpcBlockPublicAccessExclusion ec2.op-contract-create-vpc-block-public-access-exclusion ec2.diops-io2-provisioned-iops-outrun-gp3-baseline-measured-by-fio-in-one-guest ec2.ec2-iot-data-interface-endpoint-pins-ipv4-and-zonal-dns-entries ec2.op-contract-start-network-insights-access-scope-analysis ec2.op-contract-delete-network-insights-access-scope-analysis ec2.op-contract-get-network-insights-access-scope-analysis-findings ec2.op-contract-deregister-instance-event-notification-attributes ec2.op-contract-describe-addresses-attribute ec2.vscan-unencrypted-gp2-volume-defaults-iops-and-omits-throughput-and-kms-key ec2.asvpce-streaming-service-catalogue-declares-interface-only-full-access ec2.get-instance-tpm-ek-pub-format-contract ec2.op-contract-get-instance-tpm-ek-pub ec2.op-contract-describe-spot-fleet-request-history ec2.op-contract-modify-availability-zone-group ec2.disassociate-security-group-vpc-dependency-rejected
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-describe-vpc-block-public-access-exclusions|run#12|AssertionError|assert vpc_id in entry.get("ResourceArn", ""), (
Issue-Key: ec2.op-contract-deleteVpcBlockPublicAccessExclusion|run#12|AssertionError|assert vpc_id in exclusion.get("ResourceArn", ""), (
Issue-Key: ec2.op-contract-create-vpc-block-public-access-exclusion|run#12|AssertionError|assert excl["ResourceArn"] == subnet_arn, f"expected ResourceArn {subnet_arn}, got {excl['ResourceArn']}"
Issue-Key: ec2.diops-io2-provisioned-iops-outrun-gp3-baseline-measured-by-fio-in-one-guest|_host_ebs_ceiling#5|KeyError|ebs = info["EbsInfo"]["EbsOptimizedInfo"]
Issue-Key: ec2.ec2-iot-data-interface-endpoint-pins-ipv4-and-zonal-dns-entries|run#18|KeyError|assert described["RouteTableIds"] == [], (
Issue-Key: ec2.op-contract-start-network-insights-access-scope-analysis|run#10|AssertionError|assert status == "succeeded", f"expected the unconstrained scope analysis to succeed, got {status!r}"
Issue-Key: ec2.op-contract-delete-network-insights-access-scope-analysis|run#9|AssertionError|assert finished["Status"] == "succeeded", f"analysis did not succeed: {finished}"
Issue-Key: ec2.op-contract-get-network-insights-access-scope-analysis-findings|run#9|AssertionError|assert terminal_status == "succeeded", f"analysis did not succeed: {terminal_status!r}"
Issue-Key: ec2.op-contract-deregister-instance-event-notification-attributes|_poll_until#18|AssertionError|raise AssertionError(f"timed out after {timeout_s}s waiting for predicate; last value was {last!r}")
Issue-Key: ec2.op-contract-describe-addresses-attribute|run#7|AssertionError|assert first == [], f"expected no address-attribute entry for a never-customized EIP, got {first}"
Issue-Key: ec2.vscan-unencrypted-gp2-volume-defaults-iops-and-omits-throughput-and-kms-key|run#10|AssertionError|assert got == (8, "gp2", default, 100, az, False), (
Issue-Key: ec2.asvpce-streaming-service-catalogue-declares-interface-only-full-access|run#9|AssertionError|assert d["VpcEndpointPolicySupported"] is False, f"VpcEndpointPolicySupported {d['VpcEndpointPolicySupported']!r}"
Issue-Key: ec2.get-instance-tpm-ek-pub-format-contract|run#3|AssertionError|raise AssertionError(
Issue-Key: ec2.op-contract-get-instance-tpm-ek-pub|_latest_nitrotpm_windows_ami#2|AssertionError|assert images, (
Issue-Key: ec2.op-contract-describe-spot-fleet-request-history|_latest_amazon_linux_ami#4|AssertionError|raise AssertionError("no Amazon Linux 2 AMI found to build a Spot Fleet launch spec")
Issue-Key: ec2.op-contract-modify-availability-zone-group|run#4|AssertionError|raise AssertionError("no opt-in-eligible Availability/Local Zone group found in this account/region")
Issue-Key: ec2|CreateNetworkInterface|InvalidGroup.NotFound
Issue-Commit: abd75e982d55cd9e91541cb3d125feb2d8edb60f

# EC2 read paths: wrong or missing members in DescribeInstanceTypes, DescribeVpcEndpoints, the VPC-block-public-access exclusion projection, the AMI catalogue and the Network Access Scope analyzer (9 causes)

> _Note: this issue was identified against the code base at commit `abd75e982d55cd9e91541cb3d125feb2d8edb60f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcBlockPublicAccessExclusions

## Description
Nine root causes in EC2's read/serialization layer and seeded catalogues. Each makes a response unusable by a client that reads a documented member.

Cause 1 of 9: VpcBlockPublicAccessExclusion.ResourceArn names the exclusion instead of the excluded resource.
AWS: Smithy documents VpcBlockPublicAccessExclusion.ResourceArn only as 'The ARN of the exclusion.'; the recording shows AWS puts the ARN of the VPC or subnet the exclusion applies to in that member (e.g. arn:aws:ec2:<region>:<account>:subnet/<subnetId> or .../vpc/<vpcId>).
Emulator: serializes arn:aws:ec2:<region>:<account>:vpc-block-public-access-exclusion/<exclusionId> — the exclusion's own ARN — on create, describe and delete.
Why: a client cannot tell which VPC/subnet an exclusion covers, so it cannot reconcile exclusions against its network.
Fix: the VPC-block-public-access exclusion projection shared by Create/Describe/Delete.
Traces: ec2.op-contract-create-vpc-block-public-access-exclusion, ec2.op-contract-describe-vpc-block-public-access-exclusions, ec2.op-contract-deleteVpcBlockPublicAccessExclusion.

Cause 2 of 9: DescribeInstanceTypes omits EbsOptimizedInfo. AWS returns EbsInfo.EbsOptimizedInfo (baseline/maximum bandwidth, IOPS and throughput) for EBS-optimized types. Emulator: the key is absent, so any client reading it raises KeyError. Fix: the instance-type catalogue and DescribeInstanceTypes' serializer. (The open issue 20261024T015148Z-ec2-read-paths-diverge-in-9-ways-wrong.md owns a sibling check with the same KeyError; this check is not covered by it.) Traces: ec2.diops-io2-provisioned-iops-outrun-gp3-baseline-measured-by-fio-in-one-guest.

Cause 3 of 9: the VPC-endpoint projection omits RouteTableIds. AWS always serializes RouteTableIds on a VpcEndpoint (empty list when none). Emulator: the key is absent on the read path used after creating an interface endpoint, so a client indexing it raises KeyError. Fix: the VpcEndpoint serializer. Traces: ec2.ec2-iot-data-interface-endpoint-pins-ipv4-and-zonal-dns-entries.

Cause 4 of 9: Network Access Scope analysis always terminates 'failed'. AWS runs the analysis and reports Status='succeeded' with FindingsFound. Emulator: Status='failed' with StatusMessage 'This emulator does not perform Network Access Scope reachability analysis, so no findings were produced for this scope.' and FindingsFound='unknown'. Why: every downstream read (findings, delete-after-analysis) is unreachable. Fix: the Network Insights access-scope analyzer. Traces: ec2.op-contract-start-network-insights-access-scope-analysis, ec2.op-contract-delete-network-insights-access-scope-analysis, ec2.op-contract-get-network-insights-access-scope-analysis-findings.

Cause 5 of 9: DeregisterInstanceEventNotificationAttributes does not apply. AWS removes the named InstanceTagKeys from the account's notification attributes, so a subsequent DescribeInstanceEventNotificationAttributes no longer lists them. Emulator: the attribute keeps reporting InstanceTagKeys with the deregistered key and IncludeAllTagsOfInstance=False until the check's 30s predicate wait expires. Fix: the deregister handler's state mutation. Traces: ec2.op-contract-deregister-instance-event-notification-attributes.

Cause 6 of 9: DescribeAddressesAttribute invents entries. AWS returns no address-attribute entry for an EIP whose attributes were never customized. Emulator: returns an entry carrying PublicIp and AllocationId. Fix: the address-attribute read path's filter. Traces: ec2.op-contract-describe-addresses-attribute.

Cause 7 of 9: volume projection defaults. AWS describes a fresh unencrypted gp2 volume with its default Iops populated (100 for an 8 GiB gp2), Encrypted per the account's encryption-by-default setting, no Throughput and no KmsKeyId. Emulator: returns Iops absent (None) and MultiAttachEnabled False where the recording expects the documented default tuple. Fix: CreateVolume/DescribeVolumes defaulting. Traces: ec2.vscan-unencrypted-gp2-volume-defaults-iops-and-omits-throughput-and-kms-key.

Cause 8 of 9: the service/AMI/AZ catalogues the emulator seeds do not match AWS's. DescribeVpcEndpointServices reports VpcEndpointPolicySupported=True for a streaming service AWS publishes as interface-only full-access (policy NOT supported); DescribeImages contains no NitroTPM-preconfigured Windows AMI (name pattern 'TPM-Windows_Server-*', owner amazon) and no Amazon Linux 2 AMI to build a Spot Fleet launch spec; and no opt-in-eligible Availability/Local Zone group exists for ModifyAvailabilityZoneGroup. Why: a whole family of operations is unreachable because the catalogue the account is seeded with is missing public AWS-owned entries. Fix: the seeded service/AMI/AZ-group catalogues in EC2 state. Traces: ec2.asvpce-streaming-service-catalogue-declares-interface-only-full-access, ec2.get-instance-tpm-ek-pub-format-contract, ec2.op-contract-get-instance-tpm-ek-pub, ec2.op-contract-describe-spot-fleet-request-history, ec2.op-contract-modify-availability-zone-group.

Cause 9 of 9: a security group created moments earlier is not visible to CreateNetworkInterface. AWS resolves a security group by id as soon as CreateSecurityGroup returns. Emulator: CreateNetworkInterface refuses with `InvalidGroup.NotFound` / `The security group '<sgId>' does not exist` for a group the same session just created, so the dependency scenario cannot be set up at all. Fix: the security-group lookup CreateNetworkInterface uses (it is not reading the same store CreateSecurityGroup writes, or is scoping it differently). Traces: ec2.disassociate-security-group-vpc-dependency-rejected.

## Evidence
Verbatim replay diagnosis lines. Cause 1: `expected ResourceArn arn:aws:ec2:us-west-2:000000000000:subnet/subnet-00000000000000035, got arn:aws:ec2:us-west-2:000000000000:vpc-block-public-access-exclusion/vpcbpa-exclude-00000000000000039`; `expected exclusion ResourceArn to reference vpc-00000000000000022, got {'ExclusionId': 'vpcbpa-exclude-...', 'InternetGatewayExclusionMode': 'allow-bidirectional', 'ResourceArn': 'arn:aws:ec2:us-west-2:000000000000:vpc-block-public-access-exclusion/vpcbpa-exclude-...', 'State': 'create-complete', ...}` (twice). Cause 2: `KeyError: 'EbsOptimizedInfo'`. Cause 3: `KeyError: 'RouteTableIds'`. Cause 4: `AssertionError: analysis did not succeed: {... 'Status': 'failed', 'StatusMessage': 'This emulator does not perform Network Access Scope reachability analysis, so no findings were produced for this scope.', ... 'FindingsFound': 'unknown' ...}`; `analysis did not succeed: 'failed'`; `expected the unconstrained scope analysis to succeed, got 'failed'`. Cause 5: `AssertionError: timed out after 30s waiting for predicate; last value was {'InstanceTagKeys': ['sim2real-behav-notify-tag-...'], 'IncludeAllTagsOfInstance': False}`. Cause 6: `expected no address-attribute entry for a never-customized EIP, got [{'PublicIp': '54.0.0.2', 'AllocationId': 'eipalloc-...'}]`. Cause 7: `the volume must describe as (8, gp2, Encrypted=False per encryption-by-default, 100 IOPS, us-west-2a, single-attach), got (8, 'gp2', False, None, 'us-west-2a', False)`. Cause 8: `AssertionError: VpcEndpointPolicySupported True`; `no NitroTPM-preconfigured Windows AMI (name pattern 'TPM-Windows_Server-*', owned by amazon) is available in this region/account`; `no NitroTPM-preconfigured Windows AMI ... found in this region to test against`; `no Amazon Linux 2 AMI found to build a Spot Fleet launch spec`; `no opt-in-eligible Availability/Local Zone group found in this account/region`. Cause 9: `botocore.exceptions.ClientError: An error occurred (InvalidGroup.NotFound) when calling the CreateNetworkInterface operation: The security group 'sg-00000000000000031' does not exist`. Ids, IPs and tag names above are per-run values; the members, statuses and defaults are the requirement.

## Steps to Reproduce
1. Boot the emulator; point an EC2 client at it.
2. Cause 1: CreateVpcBlockPublicAccessExclusion for a subnet, then DescribeVpcBlockPublicAccessExclusions — read ResourceArn.
3. Cause 2: DescribeInstanceTypes(InstanceTypes=['io2'-capable type, e.g. m5.large]) and read EbsInfo.EbsOptimizedInfo.
4. Cause 3: CreateVpcEndpoint(Interface) then DescribeVpcEndpoints — read RouteTableIds.
5. Cause 4: CreateNetworkInsightsAccessScope then StartNetworkInsightsAccessScopeAnalysis and poll Status.
6. Cause 5: RegisterInstanceEventNotificationAttributes with a tag key, DeregisterInstanceEventNotificationAttributes with the same key, then DescribeInstanceEventNotificationAttributes.
7. Cause 6: AllocateAddress then DescribeAddressesAttribute(Attribute='domain-name').
8. Cause 8: DescribeImages(Owners=['amazon'], Filters=[{name:['TPM-Windows_Server-*']}]) and DescribeAvailabilityZones for opt-in groups.
9. Cause 9: CreateSecurityGroup then immediately CreateNetworkInterface with that group id.
10. Re-replay the check ids in trace_ids.
