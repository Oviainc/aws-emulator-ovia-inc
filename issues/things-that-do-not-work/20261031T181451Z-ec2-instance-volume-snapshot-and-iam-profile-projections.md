Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-conflict-volume-attachment
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-conflict-volume-attachment ec2-t12-ebs-volume-attach-detach ec2-create-snapshots-from-instance ec2-instance-monitoring-toggle ec2-instance-reboot-and-status-report ec2-run-instances-validation-and-diagnostic-interrupt ec2-t12-iam-profile-replace-and-stop-echo
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeVolumes|2xx->2xx|$.DescribeVolumesResponse.volumeSet.item.attachmentSet.item.availabilityZone
Issue-Key: ec2|DescribeVolumes|2xx->2xx|$.DescribeVolumesResponse.volumeSet.item.attachmentSet.item.availabilityZoneId
Issue-Key: ec2|DescribeSnapshots|2xx->2xx|$.DescribeSnapshotsResponse.snapshotSet.item.fullSnapshotSizeInBytes
Issue-Key: ec2|DescribeSnapshots|2xx->2xx|$.DescribeSnapshotsResponse.snapshotSet.item.transferType
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.metadataOptions.httpProtocolIpv4
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.metadataOptions.httpPutResponseHopLimit
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.metadataOptions.httpTokens
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.networkInterfaceSet.item.association
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.networkInterfaceSet.item.privateDnsName
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.networkInterfaceSet.item.privateIpAddressesSet.item.association
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.networkInterfaceSet.item.privateIpAddressesSet.item.privateDnsName
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.networkInterfaceSet.item.publicDnsName
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.networkInterfaceSet.item.publicIpDnsNameOptions
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.metadataOptions.httpProtocolIpv4
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.metadataOptions.httpPutResponseHopLimit
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.metadataOptions.httpTokens
Issue-Key: ec2|ReplaceIamInstanceProfileAssociation|2xx->2xx|$.ReplaceIamInstanceProfileAssociationResponse.iamInstanceProfileAssociation.iamInstanceProfile.id
Issue-Key: ec2|DescribeIamInstanceProfileAssociations|2xx->2xx|$.DescribeIamInstanceProfileAssociationsResponse.iamInstanceProfileAssociationSet.item.iamInstanceProfile.id
Issue-Related: 20261023T102725Z-ec2-response-bodies-omit-modeled-members-and-report.md
Issue-Related: 20261030T063325Z-ec2-instance-eni-and-volume-projections-omit-modeled.md
Issue-Commit: 9737b41b9bb889e100c3bf9348295e20ff42401a

# EC2 instance, volume, snapshot and IAM-profile projections omit modeled output members (6 causes)

> _Note: this issue was identified against the code base at commit `9737b41b9bb889e100c3bf9348295e20ff42401a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVolumes

## Description
Six net-new root causes share one fault: EC2 read/create projections drop members the response is required to carry, and in two of them the emulator substitutes its own default for a value the request or IAM state supplies. Each cause must be closed before this issue is submitted.

Cause 1 of 6: DescribeVolumes attachment projection drops the attachment's Availability Zone.
AWS: recorded behavior only. The on-disk Smithy model's VolumeAttachment structure (members deleteOnTermination, device, instanceId, status, attachTime, volumeId, associatedResource, instanceOwningService, ebsCardIndex) models no Availability Zone member, so the requirement is the recording's: for an attached volume, DescribeVolumesResponse.volumeSet.item.attachmentSet.item carries availabilityZone (the zone name of the instance the volume is attached to) and availabilityZoneId.
Emulator: both members are absent from every attachment element.
Why: a client that locates the attached instance's zone from the volume's attachment (zone-affinity checks, snapshot/volume placement) reads nothing where AWS reports the zone.
Fix: the DescribeVolumes projection/serializer for VolumeAttachment, plus the attachment record in volume state that must retain the instance's zone.
Traces: ec2-conflict-volume-attachment#task#9 (also ec2-t12-ebs-volume-attach-detach#task#8).

Cause 2 of 6: DescribeSnapshots omits the modeled transferType and fullSnapshotSizeInBytes.
AWS: Smithy: Snapshot.TransferType (xmlName transferType, enum TransferType with values standard | time-based) and Snapshot.FullSnapshotSizeInBytes (xmlName fullSnapshotSizeInBytes, long, documented as "The full size of the snapshot, in bytes"). The recording shows transferType 'standard' and a long full size for a snapshot created from an instance.
Emulator: snapshotSet.item carries neither member.
Why: a client sizing restores or distinguishing standard from time-based copies sees the fields as unset for every snapshot.
Fix: the DescribeSnapshots projection/serializer and the snapshot state that must record the full size and transfer type at creation.
Traces: ec2-create-snapshots-from-instance#task#6.

Cause 3 of 6: DescribeInstances omits the public-address projection of a running instance and its network interfaces.
AWS: Smithy models the omitted members (Instance.ProductCodes xmlName productCodes, and on each interface InstanceNetworkInterface.Association xmlName association); the public IPv4 DNS members (dnsName/ipAddress on the instance, publicDnsName and publicIpDnsNameOptions on the interface) are recorded behavior only in this model revision. The recording shows a running instance reporting productCodes, ipAddress, dnsName, publicIpDnsNameOptions, and per-interface publicDnsName, publicIpDnsNameOptions, association (publicIp, publicDnsName, ipOwnerId 'amazon') and privateIpAddressesSet.item.association.
Emulator: all of those members are absent (dnsName is served as an empty string), and privateDnsName does not match the private DNS name the launch response bound for the same instance.
Why: a client that reaches an instance over its public address, or that reads the auto-assigned public IP association, finds the instance has no public addressing at all even though it is running with one.
Fix: the DescribeInstances projection/serializer and the instance/ENI state that must record the public IP association and DNS names assigned at launch.
Traces: ec2-instance-monitoring-toggle#task#3 (also #task#5, ec2-instance-reboot-and-status-report#task#2, #task#3, #task#4).

Cause 4 of 6: RunInstances ignores the request's MetadataOptions and mis-shapes the launch response.
AWS: recorded behavior only for the per-launch shape; Smithy: InstanceMetadataOptionsResponse.HttpTokens (enum optional | required) and HttpPutResponseHopLimit (integer 1..64) are modeled and are echoed from the request. The recording shows the launch response carrying stateReason {code 'pending', message 'pending'}, privateDnsName, productCodes, no blockDeviceMapping entries, networkInterfaceSet.item.attachment.status 'attaching', bootMode, and metadataOptions echoing the requested httpTokens 'required' and httpPutResponseHopLimit '2'.
Emulator: stateReason, privateDnsName, productCodes and bootMode are omitted; blockDeviceMapping is populated at launch where AWS reports none; attachment.status is already 'attached'; metadataOptions reports the service defaults httpTokens 'optional' and httpPutResponseHopLimit '1', discarding the request's values.
Why: a launch that requires IMDSv2 appears to come back with IMDSv2 optional, so a client that verifies its hardening request silently accepts an instance it did not ask for; the missing stateReason/bootMode also break launch-progress reads.
Fix: the RunInstances handler (echo MetadataOptions into the created instance's state, set the attachment to attaching, populate stateReason) and its response serializer.
Traces: ec2-instance-reboot-and-status-report#setup#1 (also ec2-run-instances-validation-and-diagnostic-interrupt#task#7).

Cause 5 of 6: ReplaceIamInstanceProfileAssociation omits the modeled IAM instance-profile id.
AWS: Smithy: IamInstanceProfile has exactly two members, Arn (xmlName arn) and Id (xmlName id, "The ID of the instance profile"). The recorded response carries both; the id is the AIPA-prefixed unique id of the profile named in the request, read from IAM state (the particular id value in the recording is that run's, not a requirement).
Emulator: iamInstanceProfileAssociation.iamInstanceProfile.id is absent; only the arn is serialized.
Why: a client that keys its profile bookkeeping on the profile's unique id (which survives renames, unlike the ARN) gets nothing back from the replace call.
Fix: the ReplaceIamInstanceProfileAssociation handler's IAM instance-profile resolution and the IamInstanceProfile serializer.
Traces: ec2-t12-iam-profile-replace-and-stop-echo#task#12.

Cause 6 of 6: DescribeIamInstanceProfileAssociations omits the same modeled id on the read path.
AWS: Smithy: DescribeIamInstanceProfileAssociations returns IamInstanceProfileAssociations (xmlName iamInstanceProfileAssociationSet) whose iamInstanceProfile carries Arn and Id (xmlName id).
Emulator: iamInstanceProfileAssociationSet.item.iamInstanceProfile.id is absent for the existing association.
Why: the association cannot be correlated to the IAM profile by id on either the write or the read path, so neither call can be used to verify a profile swap.
Fix: the DescribeIamInstanceProfileAssociations projection and the association state that must store the resolved profile id.
Traces: ec2-t12-iam-profile-replace-and-stop-echo#task#13.

Related open issues that do NOT carry these replay keys (the replay reports all six clusters unowned): 20261030T063325Z-ec2-instance-eni-and-volume-projections-omit-modeled.md, 20261030T000411Z-ec2-create-and-describe-responses-omit-modeled-output.md, 20261030T210852Z-ec2-describesnapshots-diverges-on-inventory-shape-pagination-token.md.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200), all six clusters verdict=mismatch, status 200 expected and 200 returned, stub_path empty, owned_by empty.

Cause 1 — ec2-conflict-volume-attachment#task#9, DescribeVolumes:
  $.DescribeVolumesResponse.volumeSet.item.attachmentSet.item.availabilityZone: missing (expected 'us-west-2a', got None)
  $.DescribeVolumesResponse.volumeSet.item.attachmentSet.item.availabilityZoneId: missing (expected '<shaped-not-asserted>', got None)

Cause 2 — ec2-create-snapshots-from-instance#task#6, DescribeSnapshots:
  $.DescribeSnapshotsResponse.snapshotSet.item.transferType: missing (expected 'standard', got None)
  $.DescribeSnapshotsResponse.snapshotSet.item.fullSnapshotSizeInBytes: missing (expected '<shaped-not-asserted>', got None)

Cause 3 — ec2-instance-monitoring-toggle#task#3, DescribeInstances:
  ...instancesSet.item.privateDnsName: value_mismatch (expected the launch-bound private DNS name, got 'ip-172-31-0-4.us-west-2.compute.internal')
  ...instancesSet.item.publicIpDnsNameOptions: missing (expected {'dnsHostnameType': 'public-ipv4-dns-name', 'publicIpv4DnsName': <shaped>}, got None)
  ...instancesSet.item.dnsName: value_mismatch (expected a shaped public DNS name, got '')
  ...instancesSet.item.productCodes: missing (expected '', got None)
  ...instancesSet.item.ipAddress: missing (expected a shaped public IPv4, got None)
  ...networkInterfaceSet.item.publicDnsName / publicIpDnsNameOptions / association: missing (got None)
  ...networkInterfaceSet.item.privateIpAddressesSet.item.association: missing (got None)

Cause 4 — ec2-instance-reboot-and-status-report#setup#1, RunInstances:
  $.RunInstancesResponse.instancesSet.item.stateReason: missing (expected {'code': 'pending', 'message': 'pending'}, got None)
  $.RunInstancesResponse.instancesSet.item.privateDnsName / productCodes / bootMode: missing (bootMode expected 'uefi-preferred')
  $.RunInstancesResponse.instancesSet.item.blockDeviceMapping: type_mismatch (expected '', got a populated /dev/xvda item)
  $.RunInstancesResponse.instancesSet.item.networkInterfaceSet.item.attachment.status: value_mismatch (expected 'attaching', got 'attached')
  $.RunInstancesResponse.instancesSet.item.metadataOptions.httpTokens: value_mismatch (expected 'required', got 'optional')
  $.RunInstancesResponse.instancesSet.item.metadataOptions.httpPutResponseHopLimit: value_mismatch (expected '2', got '1')

Cause 5 — ec2-t12-iam-profile-replace-and-stop-echo#task#12, ReplaceIamInstanceProfileAssociation:
  $.ReplaceIamInstanceProfileAssociationResponse.iamInstanceProfileAssociation.iamInstanceProfile.id: missing (got None)

Cause 6 — ec2-t12-iam-profile-replace-and-stop-echo#task#13, DescribeIamInstanceProfileAssociations:
  $.DescribeIamInstanceProfileAssociationsResponse.iamInstanceProfileAssociationSet.item.iamInstanceProfile.id: missing (got None)

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay fixture ec2-conflict-volume-attachment and inspect step task#9 (DescribeVolumes after the volume is attached) — the attachment element has no availabilityZone/availabilityZoneId.
3. Cause 2: replay ec2-create-snapshots-from-instance, step task#6 (DescribeSnapshots) — no transferType/fullSnapshotSizeInBytes.
4. Cause 3: replay ec2-instance-monitoring-toggle, step task#3 (DescribeInstances for the running instance) — no ipAddress/dnsName/productCodes/publicIpDnsNameOptions and no per-interface association.
5. Cause 4: replay ec2-instance-reboot-and-status-report, step setup#1 (RunInstances requesting MetadataOptions HttpTokens=required, HttpPutResponseHopLimit=2) — response reports optional/1 and omits stateReason, privateDnsName, productCodes, bootMode while adding a blockDeviceMapping item.
6. Causes 5 and 6: replay ec2-t12-iam-profile-replace-and-stop-echo, steps task#12 and task#13 — iamInstanceProfile carries arn but no id.
