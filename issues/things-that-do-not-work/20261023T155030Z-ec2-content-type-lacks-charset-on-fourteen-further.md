Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-allowed-images-settings-get-contract
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-allowed-images-settings-get-contract ec2-describe-application-status-contract ec2-describe-fast-launch-images-contract ec2-vm-migration-and-image-performance-task-tracking ec2-describe-vpc-endpoint-connections-contract ec2-fast-snapshot-restore-and-public-access-lockdown ec2-image-describes ec2-instance-reboot-and-status-report ec2-local-gateway-vif-group-association-describe-filter-and-dry-run ec2-placement-group-and-tagging ec2-t12-post-delete-not-found-probes ec2-security-group-rule-lifecycle ec2-spot-placement-scores-queries ec2-t12-ebs-volume-attach-detach ec2-vpc-attributes-and-account-settings
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|GetAllowedImagesSettings|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeApplicationStatus|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeFastLaunchImages|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeVpcEndpointConnections|2xx->2xx|$headers.content-type
Issue-Key: ec2|DisableSnapshotBlockPublicAccess|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeImages|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeImportImageTasks|2xx->2xx|$headers.content-type
Issue-Key: ec2|TerminateInstances|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociations|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribePlacementGroups|2xx->2xx|$headers.content-type
Issue-Key: ec2|DeleteSecurityGroup|2xx->2xx|$headers.content-type
Issue-Key: ec2|GetSpotPlacementScores|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeVolumesModifications|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeStaleSecurityGroups|2xx->2xx|$headers.content-type
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# EC2 Content-Type lacks charset on fourteen further operations

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAllowedImagesSettings

## Description
ONE cause, replayed on fourteen separate operations. The emulator's ec2Query response serializer emits the Content-Type response header without the charset parameter.

Cause 1 of 1: Content-Type response header omits ';charset=UTF-8'.

AWS: Smithy model services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json declares the awsQuery/ec2Query family for sdkId 'ec2'; the model itself does not state a Content-Type literal, so this is recorded behavior only. Every recorded 200 and every recorded 4xx in the corpus carries 'content-type: text/xml;charset=UTF-8'.

Emulator: every response carries 'content-type: text/xml' with the charset parameter dropped. The status code and the XML body are otherwise correct for the fourteen steps listed under Traces (all fourteen clusters report '$headers.content-type' as their ONLY diverging key).

Why: a client or proxy that decodes the body using the declared charset has to guess the encoding instead of being told it. XML parsers that honour the HTTP charset over the XML declaration can mis-decode non-ASCII tag values (tag keys, descriptions, instance names), and HTTP caches/recorders keyed on the exact Content-Type will not match a real AWS recording.

Fix: the ec2Query response serializer / the shared response-writing layer that sets the Content-Type header for this service crate — it is one header-construction site, not fourteen handlers.

Related open issue: 20261023T102725Z-ec2-responses-send-content-type-text-xml-without.md describes the same fault but carries the replay key for AllocateAddress only; none of the fourteen steps below is bound to it (the replay reports owned_by: [] for all of them), so they are filed here and must be re-replayed to confirm the fix covers the whole surface.

Traces: ec2-allowed-images-settings-get-contract#task#1 (GetAllowedImagesSettings, 200); ec2-describe-application-status-contract#task#1 (DescribeApplicationStatus, 200); ec2-describe-fast-launch-images-contract#task#1 (DescribeFastLaunchImages, 200); ec2-describe-vpc-endpoint-connections-contract#task#1 (DescribeVpcEndpointConnections, 200); ec2-fast-snapshot-restore-and-public-access-lockdown#task#5 (DisableSnapshotBlockPublicAccess, 200); ec2-image-describes#task#1 (DescribeImages, 200); ec2-image-describes#task#2 (DescribeImportImageTasks, 200); ec2-instance-reboot-and-status-report#teardown#9 (TerminateInstances, 200); ec2-local-gateway-vif-group-association-describe-filter-and-dry-run#task#1 (DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociations, 200); ec2-placement-group-and-tagging#task#2 (DescribePlacementGroups, 200); ec2-security-group-rule-lifecycle#teardown#15 (DeleteSecurityGroup, 200); ec2-spot-placement-scores-queries#task#1 (GetSpotPlacementScores, 200); ec2-t12-ebs-volume-attach-detach#task#10 (DescribeVolumesModifications, 200); ec2-vpc-attributes-and-account-settings#task#7 (DescribeStaleSecurityGroups, 200).

## Evidence
Replay of simparity-wire-aws-v1, 14 divergence clusters whose only diverging key is $headers.content-type.

Expected (recorded AWS): header 'content-type: text/xml;charset=UTF-8'.
Actual (emulator):      header 'content-type: text/xml'.

Example cluster: divergence_key 'service=ec2|GetAllowedImagesSettings|status=200->200|keys=$headers.content-type', fixture ec2-allowed-images-settings-get-contract step task#1, expected_status 200, actual_status 200, detail "$headers.content-type: value_mismatch (expected 'text/xml;charset=UTF-8', got 'text/xml')". The same single-key detail is reported verbatim for the other thirteen steps listed in trace_ids.

## Steps to Reproduce
1. git_clone() then replay_wire_corpus() against the built emulator.
2. Replay fixture ec2-allowed-images-settings-get-contract, step task#1 (GetAllowedImagesSettings over the ec2Query protocol).
3. Inspect the Content-Type response header: it is 'text/xml' where the recording has 'text/xml;charset=UTF-8'.
4. Repeat for each fixture#step in trace_ids; the same header divergence appears on every one, across both 200 responses and error responses.
