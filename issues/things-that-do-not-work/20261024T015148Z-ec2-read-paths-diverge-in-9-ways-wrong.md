Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-modify-volume-attribute ec2.op-contract-allocateAddress-b ec2.ec2mr-inv-describe-instances-page-boundary-follows-the-token-to-the-full-set ec2.ec2mr-byvpc-per-region-vpc-groups-are-disjoint-across-three-regions ec2.network-interface-attach-detach-lifecycle ec2.diops-gp3-iops-to-size-ratio-above-the-limit-rejected-and-no-iops-reach-the-guest ec2.nvme-instance-store-namespace-is-nvme-while-the-xen-ebs-root-stays-xvd ec2.persist-hibernation-on-unencrypted-root-rejected ec2.vscan-deprecated-al2-ami-hidden-from-name-filter-unless-include-deprecated
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-modify-volume-attribute|run#12|AssertionError|assert code == "InvalidVolumeID.Malformed", f"unexpected error code: {code}"
Issue-Key: ec2|AllocateAddress|InvalidAddress.NotFound
Issue-Key: ec2.ec2mr-inv-describe-instances-page-boundary-follows-the-token-to-the-full-set|run#12|AssertionError|assert len(page1) == page_size, (
Issue-Key: ec2|DescribeInstances|InvalidParameterValue
Issue-Key: ec2.network-interface-attach-detach-lifecycle|run#15|KeyError|primary_nics = [n for n in running["NetworkInterfaces"] if n["Attachment"]["DeviceIndex"] == 0]
Issue-Key: ec2.diops-gp3-iops-to-size-ratio-above-the-limit-rejected-and-no-iops-reach-the-guest|_host_ebs_ceiling#5|KeyError|ebs = info["EbsInfo"]["EbsOptimizedInfo"]
Issue-Key: ec2.nvme-instance-store-namespace-is-nvme-while-the-xen-ebs-root-stays-xvd|run#8|AssertionError|assert facts["EbsInfo"].get("NvmeSupport") == "supported", (
Issue-Key: ec2.persist-hibernation-on-unencrypted-root-rejected|run#4|AssertionError|assert info[0].get("HibernationSupported") is True, (
Issue-Key: ec2.vscan-deprecated-al2-ami-hidden-from-name-filter-unless-include-deprecated|_deprecated_al2_ami#2|AssertionError|assert picked, (
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# EC2 read paths diverge in 9 ways: wrong malformed-id code, no RequestId on error responses, MaxResults/vpc-id filter ignored on DescribeInstances, missing NetworkInterfaces, and three wrong instance-type capability fields (9 causes)

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstances

## Description
Nine independent fidelity defects in EC2 operations that DO answer (no scaffold stub involved). All nine must be closed before submitting. Background: EC2's Smithy model (com-amazonaws-ec2-amazonec2) declares no error shapes, so the ec2Query error CODE on the wire and the shape of the success body are the whole contract a client can branch on.

Cause 1 of 9: ModifyVolumeAttribute answers a MALFORMED volume id with a resource-level not-found code.
AWS: InvalidVolumeID.Malformed (HTTP 400) — id syntax is validated before any lookup. Smithy: ModifyVolumeAttribute.VolumeId targets VolumeId; the model declares no errors, so the code is the contract; recorded behavior only for the exact code.
Emulator: InvalidVolume.NotFound.
Why: a client cannot tell a typo from a deleted volume, so repair/retry logic takes the wrong branch. Related open issue 20261022T235108Z-ec2-misclassifies-rejections-and-skips-validation-malformed-ids.md files the same family for DescribeImages/TerminateInstances/DescribeVolumes but does NOT carry ModifyVolumeAttribute.
Fix: volume-id parsing/validation before lookup in the ModifyVolumeAttribute handler.
Traces: ec2.op-contract-modify-volume-attribute.

Cause 2 of 9: an EC2 ERROR response carries no request id, so ResponseMetadata.RequestId is absent.
AWS: every ec2Query response, success or error, carries <requestId> in the envelope; botocore surfaces it as ResponseMetadata.RequestId. Recorded behavior only (not modelled).
Emulator: AllocateAddress(Domain='vpc', Address=<address not allocated to the account>) is refused and the parsed error carries no RequestId at all.
Why: clients and audit tooling that log or correlate on the request id get nothing, and SDK retry/debug plumbing that asserts its presence breaks. Related open issue 20261023T135552Z-cloudtrail-ec2-ec2-error-responses-carry-no-request-id-so.md names this family but does not carry this check.
Fix: request-id emission in the ec2Query error serializer.
Traces: ec2.op-contract-allocateAddress-b.

Cause 3 of 9: DescribeInstances ignores MaxResults and never paginates.
AWS: MaxResults caps the first page and a NextToken is returned when more instances exist. Smithy: DescribeInstances is paginated (inputToken NextToken / outputToken NextToken / pageSize MaxResults).
Emulator: with 6 instances and MaxResults=5 the first page returned all 6.
Why: any client that walks pages sees a page larger than it asked for and no token boundary; page-bounded clients silently over-read.
Fix: page-size clamping and token minting in the DescribeInstances handler.
Traces: ec2.ec2mr-inv-describe-instances-page-boundary-follows-the-token-to-the-full-set.

Cause 4 of 9: DescribeInstances rejects the 'vpc-id' filter outright.
AWS: 'vpc-id' is a supported DescribeInstances filter and returns the instances in that VPC.
Emulator: InvalidParameterValue, message "The filter 'vpc-id' is not one this emulator resolves" (an emulator-authored message that no AWS client can be written against).
Why: the standard way to inventory a VPC's instances fails, and the failure is a parameter rejection rather than an empty result, so callers cannot degrade gracefully.
Fix: filter resolution in the DescribeInstances handler.
Traces: ec2.ec2mr-byvpc-per-region-vpc-groups-are-disjoint-across-three-regions.

Cause 5 of 9: DescribeInstances omits the NetworkInterfaces list on a running instance.
AWS: Instance.NetworkInterfaces (xmlName networkInterfaceSet) lists every attached ENI with its Attachment.DeviceIndex; the primary interface is always present at DeviceIndex 0.
Emulator: the instance structure has no NetworkInterfaces member at all (KeyError 'NetworkInterfaces' on the parsed response).
Why: ENI attach/detach, secondary-IP and multi-interface workflows are unobservable — a client cannot find the primary interface it must modify.
Fix: NetworkInterfaces population in the DescribeInstances serializer/state projection.
Traces: ec2.network-interface-attach-detach-lifecycle.

Cause 6 of 9: DescribeInstanceTypes omits EbsInfo.EbsOptimizedInfo.
AWS: Smithy EbsInfo carries EbsOptimizedInfo (xmlName ebsOptimizedInfo) describing baseline/maximum EBS bandwidth, throughput and IOPS for the type.
Emulator: the member is absent (KeyError 'EbsOptimizedInfo'); the EbsInfo returned for the type under test holds only EbsOptimizedSupport, EncryptionSupport, NvmeSupport, MaximumEbsAttachments and AttachmentLimitType.
Why: clients that size gp3 IOPS/throughput against the instance's EBS ceiling cannot read the ceiling.
Fix: instance-type catalog data behind DescribeInstanceTypes.
Traces: ec2.diops-gp3-iops-to-size-ratio-above-the-limit-rejected-and-no-iops-reach-the-guest.

Cause 7 of 9: DescribeInstanceTypes reports the wrong EbsInfo.NvmeSupport for an NVMe-instance-store type.
AWS: i3.large reports EbsInfo.NvmeSupport 'supported' (EBS attaches as NVMe optionally, not 'unsupported'). Smithy: EbsInfo.NvmeSupport targets EbsNvmeSupport.
Emulator: {'EbsOptimizedSupport': 'supported', 'EncryptionSupport': 'supported', 'NvmeSupport': 'unsupported', 'MaximumEbsAttachments': 27, 'AttachmentLimitType': 'dedicated'} for i3.large.
Why: a client choosing device naming (nvme* vs xvd*) from this field picks the wrong guest device paths.
Fix: instance-type catalog data behind DescribeInstanceTypes.
Traces: ec2.nvme-instance-store-namespace-is-nvme-while-the-xen-ebs-root-stays-xvd.

Cause 8 of 9: DescribeInstanceTypes reports HibernationSupported=false for a type that supports hibernation.
AWS: m5d.large reports HibernationSupported true. Smithy: InstanceTypeInfo.HibernationSupported (xmlName hibernationSupported) 'Indicates whether On-Demand hibernation is supported.'
Emulator: HibernationSupported False for m5d.large.
Why: clients that gate the hibernation option on this flag never offer hibernation on a supported type, and cannot distinguish an unsupported type from an unsupported configuration.
Fix: instance-type catalog data behind DescribeInstanceTypes.
Traces: ec2.persist-hibernation-on-unencrypted-root-rejected.

Cause 9 of 9: the seeded AMI catalog holds no deprecated Amazon Linux 2 image of a realistic age.
AWS: a region always holds deprecated amzn2-ami-hvm-2.0.*-x86_64-gp2 images whose CreationDate is roughly a year old, hidden from a name-filtered DescribeImages unless IncludeDeprecated=true. Recorded behavior only.
Emulator: DescribeImages(IncludeDeprecated=true) listed exactly 1 image matching that name pattern, and no deprecated AL2 image with a CreationDate in the 300-540-day window exists, so the include/exclude contrast cannot be exercised.
Why: clients that pin or audit AMI deprecation see a catalog whose dates do not age, so deprecation-aware selection is untestable against the emulator.
Fix: AMI catalog seeding (creation/deprecation timestamps relative to now) behind DescribeImages.
Traces: ec2.vscan-deprecated-al2-ami-hidden-from-name-filter-unless-include-deprecated.

## Evidence
Cause 1 — AssertionError: unexpected error code: InvalidVolume.NotFound (assert code == "InvalidVolumeID.Malformed").
Cause 2 — AssertionError: expected a RequestId in ResponseMetadata, raised from allocate_address(Domain='vpc', Address='192.0.2.88') which was refused InvalidAddress.NotFound.
Cause 3 — AssertionError: MaxResults=5 over 6 instances must return exactly 5 on the first page, it returned 6: ['i-00000000000000001' ... 'i-00000000000000006'].
Cause 4 — botocore.exceptions.ClientError: An error occurred (InvalidParameterValue) when calling the DescribeInstances operation: The filter 'vpc-id' is not one this emulator resolves.
Cause 5 — KeyError: 'NetworkInterfaces' at `primary_nics = [n for n in running["NetworkInterfaces"] if n["Attachment"]["DeviceIndex"] == 0]`.
Cause 6 — KeyError: 'EbsOptimizedInfo' while reading DescribeInstanceTypes EbsInfo.
Cause 7 — AssertionError: i3.large must declare EbsInfo.NvmeSupport 'supported' ... got {'EbsOptimizedSupport': 'supported', 'EncryptionSupport': 'supported', 'NvmeSupport': 'unsupported', 'MaximumEbsAttachments': 27, 'AttachmentLimitType': 'dedicated'}.
Cause 8 — AssertionError: m5d.large reports HibernationSupported False; the rejection under test must be about encryption, not about the instance type.
Cause 9 — AssertionError: no deprecated Amazon Linux 2 AMI created 300-540 days ago in this region; IncludeDeprecated listed 1 'amzn2-ami-hvm-2.0.*-x86_64-gp2' images.

## Steps to Reproduce
Replay the check ids in trace_ids. Minimal repros: (1) ec2:ModifyVolumeAttribute VolumeId='vol-zzzz' AutoEnableIO={'Value':True}. (2) ec2:AllocateAddress Domain='vpc' Address='192.0.2.88' and read ResponseMetadata.RequestId off the ClientError. (3) run 6 instances, then ec2:DescribeInstances MaxResults=5. (4) ec2:DescribeInstances Filters=[{'Name':'vpc-id','Values':[<vpc>]}]. (5) run an instance to 'running', then ec2:DescribeInstances and read Reservations[].Instances[].NetworkInterfaces. (6) ec2:DescribeInstanceTypes InstanceTypes=['t3.micro'] and read InstanceTypes[0].EbsInfo.EbsOptimizedInfo. (7) same call for i3.large and read EbsInfo.NvmeSupport. (8) same call for m5d.large and read HibernationSupported. (9) ec2:DescribeImages Owners=['amazon'] Filters=[{'Name':'name','Values':['amzn2-ami-hvm-2.0.*-x86_64-gp2']}] IncludeDeprecated=True and inspect CreationDate/DeprecationTime.
