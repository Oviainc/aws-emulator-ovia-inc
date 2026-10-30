Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-conflict-flow-log-duplicate
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-conflict-flow-log-duplicate ec2-customer-gateway-crud-lifecycle ec2-customer-gateway-lifecycle ec2-instance-event-window-and-application-status-check-lifecycle ec2-instance-event-window-tag-association-lifecycle ec2-ipam-consolidated-delete-and-deprovision-lifecycle ec2-modify-verified-access-instance-contract ec2-network-insights-access-scope-analysis-lifecycle ec2-replace-root-volume-task ec2-subnet-route-table-lifecycle ec2-t12-post-delete-not-found-probes
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateFlowLogs|2xx->2xx|$.CreateFlowLogsResponse.clientToken
Issue-Key: ec2|DescribeCustomerGateways|2xx->2xx|$.DescribeCustomerGatewaysResponse.customerGatewaySet.item.certificatePinning
Issue-Key: ec2|DescribeInstanceEventWindows|2xx->2xx|$.DescribeInstanceEventWindowsResponse.instanceEventWindowSet.item.associationTarget
Issue-Key: ec2|CreateIpamPool|2xx->2xx|$.CreateIpamPoolResponse.ipamPool.locale
Issue-Key: ec2|ModifyVerifiedAccessInstance|2xx->2xx|$.ModifyVerifiedAccessInstanceResponse.verifiedAccessInstance.verifiedAccessInstanceTransitGatewayAttachmentSet
Issue-Key: ec2|StartNetworkInsightsAccessScopeAnalysis|2xx->2xx|$.StartNetworkInsightsAccessScopeAnalysisResponse.networkInsightsAccessScopeAnalysis.analyzedEniCount
Issue-Key: ec2|CreateReplaceRootVolumeTask|2xx->2xx|$.CreateReplaceRootVolumeTaskResponse.replaceRootVolumeTask.snapshotId
Issue-Key: ec2|DescribeReplaceRootVolumeTasks|2xx->2xx|$.DescribeReplaceRootVolumeTasksResponse.replaceRootVolumeTaskSet.item.snapshotId
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item.dhcpOptionsId
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item.dhcpOptionsSource
Issue-Key: ec2|DeletePlacementGroup|2xx->2xx|$.DeletePlacementGroupResponse.groupId
Issue-Key: ec2|DeletePlacementGroup|2xx->2xx|$.DeletePlacementGroupResponse.groupName
Issue-Commit: 7c2160283d3342f6e10783a5c4b2d926b7b7bab3

# EC2 create and describe responses omit modeled output members

> _Note: this issue was identified against the code base at commit `7c2160283d3342f6e10783a5c4b2d926b7b7bab3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFlowLogs

## Description
The emulator's serializers drop output members that AWS always emits for these operations. Status and error class are correct; the bodies are incomplete. This bundle carries 10 causes. It is the same family of defect as the open issues 20261023T102725Z-ec2-response-bodies-omit-modeled-members-and-report.md and 20261029T215142Z-ec2-describe-projections-omit-modeled-members-and-seed.md, but none of the causes below is carried by either of those issues' replay keys.

Cause 1 of 10: CreateFlowLogs omits clientToken.
AWS: echoes the client token in the response (Smithy: CreateFlowLogsResult.ClientToken, xmlName clientToken) - the service-generated token when the request did not supply one.
Emulator: member absent.
Why: a client that records the token for idempotent retries has nothing to record.
Fix: the CreateFlowLogs response serializer.
Traces: ec2-conflict-flow-log-duplicate#task#3.

Cause 2 of 10: DescribeCustomerGateways omits certificatePinning.
AWS: each customerGatewaySet item carries certificatePinning ('false' when not enabled). Recorded behavior only - this repo's Smithy CustomerGateway shape predates the member (it models certificateArn/deviceName/bgpAsn/bgpAsnExtended/state/type/ipAddress/tagSet), so the recording is the requirement.
Emulator: member absent.
Why: clients that read the pinning flag see an undefined value.
Fix: the CustomerGateway projection in describe_customer_gateways.rs.
Traces: ec2-customer-gateway-crud-lifecycle#task#2.

Cause 3 of 10: DescribeInstanceEventWindows omits associationTarget.
AWS: each instanceEventWindowSet item carries associationTarget with its three member sets present (dedicatedHostIdSet, instanceIdSet, tagSet), empty when nothing is associated.
Emulator: the whole associationTarget element is absent.
Why: a client inspecting what an event window is attached to cannot distinguish 'nothing attached' from 'field unsupported'.
Fix: the event-window projection in describe_instance_event_windows.rs.
Traces: ec2-instance-event-window-and-application-status-check-lifecycle#task#2.

Cause 4 of 10: CreateIpamPool omits ipamPool.locale.
AWS: the created pool always reports its locale member (the value 'None' when the pool is not locale-scoped).
Emulator: member absent.
Why: a client that keys allocation decisions on the pool locale reads nothing.
Fix: the IPAM pool serializer in create_ipam_pool.rs.
Traces: ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#11.

Cause 5 of 10: ModifyVerifiedAccessInstance omits verifiedAccessInstanceTransitGatewayAttachmentSet.
AWS: the echoed instance carries the attachment set element, empty when there are no transit gateway attachments.
Emulator: member absent.
Why: the modify receipt is not shape-complete, so clients deserializing into the modeled struct see a missing collection rather than an empty one.
Fix: the VerifiedAccessInstance serializer shared by the modify handler.
Traces: ec2-modify-verified-access-instance-contract#task#2.

Cause 6 of 10: StartNetworkInsightsAccessScopeAnalysis omits analyzedEniCount.
AWS: the returned analysis carries analyzedEniCount ('0' at start).
Emulator: member absent.
Why: progress reporting on the analysis has no counter to read.
Fix: the analysis serializer in start_network_insights_access_scope_analysis.rs.
Traces: ec2-network-insights-access-scope-analysis-lifecycle#task#3.

Cause 7 of 10: CreateReplaceRootVolumeTask omits replaceRootVolumeTask.snapshotId.
AWS: when the task is created from a snapshot, the task echoes that snapshot id (the snap-* id supplied in the request).
Emulator: member absent.
Why: the caller cannot confirm which snapshot the root volume will be replaced from.
Fix: the task serializer in create_replace_root_volume_task.rs.
Traces: ec2-replace-root-volume-task#task#6.

Cause 8 of 10: DescribeReplaceRootVolumeTasks omits the same snapshotId on the read path.
AWS: each replaceRootVolumeTaskSet item carries snapshotId for snapshot-sourced tasks.
Emulator: member absent.
Why: the information is unavailable on the read path too, so it cannot be recovered after the create response.
Fix: the task projection in describe_replace_root_volume_tasks.rs (share the serializer with Cause 7).
Traces: ec2-replace-root-volume-task#task#7.

Cause 9 of 10: DescribeSubnets omits dhcpOptionsId and dhcpOptionsSource.
AWS: each subnetSet item reports the dopt-* id in effect for the subnet and the source of those options ('vpc' when inherited from the VPC).
Emulator: both members absent.
Why: a client resolving which DHCP option set applies to a subnet gets nothing.
Fix: the Subnet projection in describe_subnets.rs, reading the VPC's associated DHCP options.
Traces: ec2-subnet-route-table-lifecycle#task#5 (+2 affected steps).

Cause 10 of 10: DeletePlacementGroup omits groupId and groupName.
AWS: the delete response echoes the deleted group's groupName (the name from the request) and its generated groupId. Recorded behavior only - this repo's Smithy models DeletePlacementGroup's output as Unit, so the recording is the requirement.
Emulator: both members absent.
Why: a client that logs or asserts on the delete receipt has an empty body to work with.
Fix: the DeletePlacementGroup response serializer.
Traces: ec2-t12-post-delete-not-found-probes#task#18.

## Evidence
All ten causes are HTTP 200 responses whose bodies lack members the recording always carries:
$.CreateFlowLogsResponse.clientToken missing; $.DescribeCustomerGatewaysResponse.customerGatewaySet.item.certificatePinning missing (expected 'false'); $.DescribeInstanceEventWindowsResponse.instanceEventWindowSet.item.associationTarget missing (expected {dedicatedHostIdSet, instanceIdSet, tagSet}); $.CreateIpamPoolResponse.ipamPool.locale missing (expected 'None'); $.ModifyVerifiedAccessInstanceResponse.verifiedAccessInstance.verifiedAccessInstanceTransitGatewayAttachmentSet missing (expected empty); $.StartNetworkInsightsAccessScopeAnalysisResponse.networkInsightsAccessScopeAnalysis.analyzedEniCount missing (expected '0'); $.CreateReplaceRootVolumeTaskResponse.replaceRootVolumeTask.snapshotId missing; $.DescribeReplaceRootVolumeTasksResponse.replaceRootVolumeTaskSet.item.snapshotId missing; $.DescribeSubnetsResponse.subnetSet.item.dhcpOptionsId and .dhcpOptionsSource missing (expected the dopt-* id and 'vpc'); $.DeletePlacementGroupResponse.groupId and .groupName missing.

## Steps to Reproduce
Replay simparity-wire-aws-v1 and inspect ec2-conflict-flow-log-duplicate#task#3, ec2-customer-gateway-crud-lifecycle#task#2, ec2-instance-event-window-and-application-status-check-lifecycle#task#2, ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#11, ec2-modify-verified-access-instance-contract#task#2, ec2-network-insights-access-scope-analysis-lifecycle#task#3, ec2-replace-root-volume-task#task#6 and #task#7, ec2-subnet-route-table-lifecycle#task#5, ec2-t12-post-delete-not-found-probes#task#18.
