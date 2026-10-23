Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-instance-monitoring-toggle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-instance-monitoring-toggle ec2-instance-reboot-and-status-report ec2-page-describe-instances-tag-filtered-two-pages
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstances|2xx->2xx|$
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.monitoring.state
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.metadataOptions.state
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.networkInterfaceSet.item.attachment.status
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item[*].instancesSet.item.instanceState.name
Issue-Related: 20261023T102725Z-ec2-response-bodies-omit-modeled-members-and-report.md
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# EC2 DescribeInstances omits instance detail and never advances state

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstances

## Description
Four causes with one root: the instance record the emulator keeps is far thinner than AWS's and is never updated by the mutations that act on it, so DescribeInstances both under-reports the instance and never reflects a state change. Every one of the four steps ends with the fixture's poll capping out at 60s.

AWS: DescribeInstances reports, per instance, a large fixed field set — productCodes, groupSet, operator, placement.availabilityZoneId/groupName/tenancy, monitoring.state, subnetId, vpcId, ipAddress, dnsName, privateDnsName, privateDnsNameOptions, publicIpDnsNameOptions, sourceDestCheck, architecture, rootDeviceType, rootDeviceName, blockDeviceMapping, virtualizationType, hypervisor, enaSupport, enclaveOptions, hibernationOptions, cpuOptions, bootMode, currentInstanceBootMode, capacityReservationSpecification, maintenanceOptions, metadataOptions, networkPerformanceOptions, networkInterfaceSet, secondaryInterfaceSet, platformDetails, usageOperation, usageOperationUpdateTime — and it reflects mutations: MonitorInstances flips monitoring.state to 'enabled', a launched instance progresses to instanceState 'running', an attached root volume's blockDeviceMapping entry progresses to 'attached' and its networkInterfaceSet attachment to 'attached'. Smithy: DescribeInstances' output models only NextToken + Reservations (target ReservationList) and binds no errors, so the member list above is recorded behavior only, read off the recorded responses.

Emulator: the reservation/instance items are returned with most of those members absent, and the state-bearing ones do not change.

Cause 1 of 4 — monitoring.state does not follow MonitorInstances, and the instance item omits the member set above.
Trace: ec2-instance-monitoring-toggle#task#3. Poll for monitoring.state 'enabled' caps at 60s.

Cause 2 of 4 — after RebootInstances the instance item still omits the member set and metadataOptions.state never settles.
Trace: ec2-instance-reboot-and-status-report#task#3.

Cause 3 of 4 — the instance's networkInterfaceSet attachment status never reaches 'attached'.
Trace: ec2-instance-reboot-and-status-report#task#4.

Cause 4 of 4 — tag-filtered DescribeInstances returns an EMPTY reservationSet and the launched instances never reach instanceState 'running': the poll reports "not in ['running'] after 13 attempt(s)" while the response body carries reservationSet ''.
Trace: ec2-page-describe-instances-tag-filtered-two-pages#setup#11.

Why: DescribeInstances is the read every EC2 client depends on. With instanceState stuck short of 'running' the AWS InstanceRunning waiter (and the fixtures) never unblock, so nothing can be done with a launched instance. With the member set absent, clients that read subnetId/vpcId/networkInterfaceSet to wire up networking, rootDeviceName/blockDeviceMapping to find the root volume, or platformDetails/usageOperation for billing attribution all get null where AWS gives a value — and a tag filter that returns an empty reservationSet makes the instances invisible to any tag-scoped automation even though they exist.

Fix: the ec2 service crate's instance state and the DescribeInstances serializer — the instance record must carry the full member set and be mutated by MonitorInstances/RebootInstances/RunInstances (including the instanceState, blockDeviceMapping and network-interface attachment transitions), and the tag-filter path of DescribeInstances must match the instances it stored.

Related open issue: 20261023T102725Z-ec2-describe-reads-return-wrong-page-contents-no.md covers a similar read defect but carries none of these four replay keys (owned_by: [] on all four clusters), so they are filed here.

Traces: the four fixture#step bindings named under each cause above.

## Evidence
Replay of simparity-wire-aws-v1, 4 divergence clusters, all status 200 expected and returned.

Cause 1 — divergence_key 'service=ec2|DescribeInstances|status=200->200|keys=$,...groupSet,...blockDeviceMapping,...bootMode,...capacityReservationSpecification,...cpuOptions,...currentInstanceBootMode,...dnsName,...enaSupport,...enclaveOptions,...groupSet,...hibernationOptions,...hypervisor,...ipAddress,...maintenanceOptions,...metadataOptions,...monitoring.state,...networkInterfaceSet,...networkPerformanceOptions,...operator,...placement.availabilityZoneId,...placement.groupName,...placement.tenancy,...platformDetails,...privateDnsName,...privateDnsNameOptions,...productCodes,...publicIpDnsNameOptions,...rootDeviceName,...rootDeviceType,...secondaryInterfaceSet,...sourceDestCheck,...subnetId,...usageOperation,...usageOperationUpdateTime,...virtualizationType,...vpcId,$headers.content-type', fixture ec2-instance-monitoring-toggle step task#3; detail begins '$: poll capped at 60s'; recording expects monitoring {state 'enabled'}.

Cause 2 — same operation/key family, fixture ec2-instance-reboot-and-status-report step task#3, additionally keyed on ...metadataOptions.state; '$: poll capped at 60s'; recording expects instanceState {code '16', name 'running'} and blockDeviceMapping ebs status 'attached'.

Cause 3 — same family, fixture ec2-instance-reboot-and-status-report step task#4, additionally keyed on ...networkInterfaceSet.item.attachment.status; '$: poll capped at 60s'.

Cause 4 — divergence_key 'service=ec2|DescribeInstances|status=200->200|keys=$,$.DescribeInstancesResponse.reservationSet.item[5].instancesSet.item.instanceState.name', fixture ec2-page-describe-instances-tag-filtered-two-pages step setup#11: actual body {'DescribeInstancesResponse': {'reservationSet': '', 'requestId': ...}}; detail "poll: not in ['running'] after 13 attempt(s)".

(Request ids, host names, generated instance/eni/subnet ids and timestamps in these bodies differ per environment and are not asserted here.)

## Steps to Reproduce
1. git_clone() then replay_wire_corpus().
2. Replay fixture ec2-instance-monitoring-toggle: it launches an instance, calls MonitorInstances, then polls DescribeInstances (task#3) for monitoring.state 'enabled'. The emulator's instance item has no monitoring state change and is missing most members; the poll caps at 60s.
3. Replay fixture ec2-instance-reboot-and-status-report to task#3 and task#4 — same thin item, and the network-interface attachment status never reaches 'attached'.
4. Replay fixture ec2-page-describe-instances-tag-filtered-two-pages to setup#11: the tag-filtered DescribeInstances returns reservationSet '' and the instances never report instanceState 'running'.
