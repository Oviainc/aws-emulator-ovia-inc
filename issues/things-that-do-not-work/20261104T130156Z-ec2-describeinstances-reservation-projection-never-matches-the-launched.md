Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-sshr-traj-instances-in-the-open-group-split-on-public-ip
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-sshr-traj-instances-in-the-open-group-split-on-public-ip
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item
Issue-Related: 20261030T063325Z-ec2-instance-eni-and-volume-projections-omit-modeled.md
Issue-Related: 20261101T141251Z-ec2-peering-id-format-prefix-list-and-capacity.md
Issue-Commit: 30059ba7efbea1c9406793a7f2ccf8806d86986b

# EC2 DescribeInstances reservation projection never matches the launched instances

> _Note: this issue was identified against the code base at commit `30059ba7efbea1c9406793a7f2ccf8806d86986b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstances

## Description
Cause 1 of 1: a DescribeInstances poll over instances launched earlier in the trajectory never converges to the recorded reservation projection (fixture ec2-sshr-traj-instances-in-the-open-group-split-on-public-ip, step task#10).
AWS: recorded behavior only. The recording shows DescribeInstances answering 200 with reservationSet containing one item per RunInstances reservation made earlier in the trajectory (the private-only instance and the public-IP instance launched into the same subnet and security group), each instancesSet.item reaching instanceState {code '16', name 'running'} and carrying the instance's placement, monitoring, subnetId/vpcId, groupSet, blockDeviceMapping and networkInterfaceSet (with its attachment) populated. AWS reaches that state within the step's poll window.
Emulator: answers 200 throughout, but the reservationSet it serves never matches the recorded projection - the diverging keys are $, $.DescribeInstancesResponse.reservationSet and $.DescribeInstancesResponse.reservationSet.item, i.e. the set of reservations/instances returned (its cardinality and contents) is not the one the earlier RunInstances calls in this trajectory created. The step's poll ran until it was capped at 60s without ever matching. Hypothesis: the reservation grouping or the describe-time filtering drops or merges the instances launched with and without a public IP, or the instances never advance to the running state.
Why: DescribeInstances is the convergence point of every launch workflow - a client that waits for its instances to appear as running hangs until its own timeout and then cannot tell whether the launch failed or the read is wrong, and anything that enumerates reservations (SSH/bastion tooling, autoscaling glue, inventory) sees the wrong instance set while the call still returns 200.
Fix: the DescribeInstances handler's reservation projection in the ec2 service crate together with the instance state machine that must take a launched instance to running (code 16) and keep one reservation per RunInstances call.
Traces: ec2-sshr-traj-instances-in-the-open-group-split-on-public-ip#task#10.

No open issue carries this replay key.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 57.3%): cluster ec2-sshr-traj-instances-in-the-open-group-split-on-public-ip#task#10, DescribeInstances, expected 200 / actual 200, unowned and not stub-blocked.
Detail: "$: poll capped at 60s" with diverging keys $, $.DescribeInstancesResponse.reservationSet and $.DescribeInstancesResponse.reservationSet.item. The expected body is a reservationSet whose item is a list of reservations, the first holding the trajectory's private instance (instanceId {{inst_priv}}, imageId {{ami}}, instanceState {'code': '16', 'name': 'running'}, instanceType 't3.micro', placement availabilityZone 'us-west-2a' tenancy 'default', monitoring 'disabled', sourceDestCheck 'true', groupSet with the trajectory's security group, rootDeviceType 'ebs' with /dev/xvda blockDeviceMapping attached, and a networkInterfaceSet item in status 'in-use' with its attachment at deviceIndex 0); the emulator's response never satisfied that projection within the poll window.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture ec2-sshr-traj-instances-in-the-open-group-split-on-public-ip from the start so the trajectory's RunInstances calls create the private-only and public-IP instances.
3. Run step task#10 (DescribeInstances poll): the emulator returns 200 but reservationSet/reservationSet.item never match the recorded two-reservation, running-state projection, and the poll is capped at 60s.
