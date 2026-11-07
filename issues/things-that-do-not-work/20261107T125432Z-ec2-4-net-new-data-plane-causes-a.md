Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.cpuopts-guest-topology-differs-from-the-type-default ec2.cpuopts-topology-survives-a-stop-start-cycle ec2.ec2-attach-volume-to-a-device-name-already-in-use-rejected ec2.idvdoc-document-reminted-and-resigned-after-a-stop-start ec2.dsnapinc-one-changed-block-in-guest-makes-the-second-snapshot-incremental-and-the-restore-carries-it ec2.dvolgrow-shrink-rejected-leaves-the-guest-block-device-and-its-bytes-untouched ec2.placement-cluster-group-members-read-one-shared-zone-and-their-group-in-guest ec2.ebsio-online-volume-grow-preserves-the-bytes-and-the-guest-sees-the-new-capacity ec2.ebsio-multi-attach-shares-one-volume-with-two-instances-while-a-plain-volume-is-refused ec2.ec2-elastic-ip-association-moves-which-address-serves-the-instance ec2.delete-on-termination-decides-which-attached-volume-survives-the-terminate ec2.diops-gp3-iops-to-size-ratio-above-the-limit-rejected-and-no-iops-reach-the-guest
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.cpuopts-guest-topology-differs-from-the-type-default|_poll#18|TimeoutError|raise TimeoutError(f"{what or 'condition'}: the run budget ended first; not graded")
Issue-Key: ec2.cpuopts-topology-survives-a-stop-start-cycle|_poll#19|TimeoutError|raise TimeoutError(f"{what or 'condition'}: the run budget ended first; not graded")
Issue-Key: ec2.ec2-attach-volume-to-a-device-name-already-in-use-rejected|_poll#18|TimeoutError|raise TimeoutError(f"{what or 'condition'}: the run budget ended first; not graded")
Issue-Key: ec2.idvdoc-document-reminted-and-resigned-after-a-stop-start|_poll_until#13|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: ssm|SendCommand|InvalidInstanceId
Issue-Key: ec2.ebsio-online-volume-grow-preserves-the-bytes-and-the-guest-sees-the-new-capacity|_poll_until#18|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: ec2.ebsio-multi-attach-shares-one-volume-with-two-instances-while-a-plain-volume-is-refused|_poll_until#37|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: ec2.ec2-elastic-ip-association-moves-which-address-serves-the-instance|_poll_until#11|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: ec2.delete-on-termination-decides-which-attached-volume-survives-the-terminate|_retry_dependency#3|Timeout|time.sleep(delay)
Issue-Key: ec2.diops-gp3-iops-to-size-ratio-above-the-limit-rejected-and-no-iops-reach-the-guest|_poll#19|Timeout|time.sleep(interval_s)
Issue-Commit: 49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4

# EC2: 4 net-new data-plane causes — a launched instance never becomes an Online SSM managed node, GetConsoleOutput never carries guest output, no auto-assigned public IPv4 ever appears, and waited-on EBS state never converges

> _Note: this issue was identified against the code base at commit `49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
Four independent net-new root causes behind twelve failing `### ec2` checks. All four are about an instance the control plane reports as running that has no observable guest or address behind it; they are distinct defects and all must be closed. Related issues already filed cover OTHER service groups or other symptoms and do NOT cover these ec2 checks: 20261107T075601Z-ec2-ssm-windows-amis-report-platform-windows-instead.md (ec2__ssm), 20261107T081213Z-ssm-3-net-new-causes-a-launched-ec2.md (ssm), 20261107T075601Z-ec2-createvpc-from-an-ipam-pool-is-a.md, 20261104T142958Z-ec2-state-never-converges-for-six-waited-on.md.

Cause 1 of 4 — an instance launched from the AL2023 AMI with an AmazonSSMManagedInstanceCore instance profile never becomes an Online SSM managed node, so no command can be run in the guest.
AWS: recorded behavior only for the timing; AWS behavior is that such an instance registers with Systems Manager shortly after boot — DescribeInstanceInformation reports the instance with PingStatus `Online`, and SendCommand against it is accepted instead of being refused `InvalidInstanceId`. Until it registers, SendCommand is refused with `InvalidInstanceId`.
Emulator: the instance reaches `running` but never appears Online; every poll for PingStatus=='Online' and every poll for SendCommand being accepted exhausts the run budget (`TimeoutError: the SSM agent on <instance-id> coming Online: the run budget ended first; not graded`, `TimeoutError: <what>: SendCommand to <instance-id> being accepted: the run budget ended first; not graded`).
Why: no client can observe anything inside an instance — CPU topology, block devices, IMDS facts, filesystem payloads — so the guest half of every EC2 contract is unverifiable and automation that waits for a managed node hangs forever.
Fix: the instance-launch path must register a managed node for an instance whose profile carries the SSM core policy (the RunInstances handler's post-launch state plus the SSM managed-node state the DescribeInstanceInformation/SendCommand handlers read).
Traces: ec2.cpuopts-guest-topology-differs-from-the-type-default, ec2.cpuopts-topology-survives-a-stop-start-cycle, ec2.ec2-attach-volume-to-a-device-name-already-in-use-rejected, ec2.idvdoc-document-reminted-and-resigned-after-a-stop-start, ec2.dsnapinc-one-changed-block-in-guest-makes-the-second-snapshot-incremental-and-the-restore-carries-it, ec2.dvolgrow-shrink-rejected-leaves-the-guest-block-device-and-its-bytes-untouched, ec2.placement-cluster-group-members-read-one-shared-zone-and-their-group-in-guest.

Cause 2 of 4 — GetConsoleOutput never returns the output the guest wrote.
AWS: Smithy: GetConsoleOutput returns `Output` (base64 of the instance's console log). An instance whose UserData writes to the console has that text appear in GetConsoleOutput's Output within a few minutes of boot.
Emulator: the polls for the guest's own marker in the console output never see it (`TimeoutError: guest <instance-id> reports the payload it wrote: the run budget ended first; not graded`, `TimeoutError: guest A <instance-id> reports what it wrote to <vol-id>: the run budget ended first; not graded`), so UserData output is never reflected in the console.
Why: console output is the one channel that needs no agent and no network; with it empty, boot diagnosis and any UserData-driven verification is impossible.
Fix: the GetConsoleOutput handler and the instance state that should accumulate the guest's console/UserData output.
Traces: ec2.ebsio-online-volume-grow-preserves-the-bytes-and-the-guest-sees-the-new-capacity, ec2.ebsio-multi-attach-shares-one-volume-with-two-instances-while-a-plain-volume-is-refused.

Cause 3 of 4 — an instance launched in a public subnet never gets an auto-assigned public IPv4 address.
AWS: Smithy: Instance carries `PublicIpAddress` / `PublicDnsName`; an instance launched into a subnet with MapPublicIpOnLaunch (or with AssociatePublicIpAddress) is reported by DescribeInstances with a PublicIpAddress once it is running.
Emulator: the poll for the instance getting an auto-assigned public IPv4 address never succeeds (`TimeoutError: instance <instance-id> gets an auto-assigned public IPv4 address: the run budget ended first; not graded`), so the Elastic IP association contract (which address serves the instance) can never be exercised.
Why: a client cannot learn how to reach the instance, and EIP association/disassociation — whose whole observable effect is which address serves the instance — is untestable.
Fix: public-IPv4 assignment in the RunInstances handler/instance state and its projection in DescribeInstances.
Traces: ec2.ec2-elastic-ip-association-moves-which-address-serves-the-instance.

Cause 4 of 4 — EBS lifecycle state waited on by these checks never converges, so the check spends its whole budget without reaching a verdict.
AWS: recorded behavior only: a terminate honours each attachment's DeleteOnTermination (the True volume disappears — describe and snapshot both refused `InvalidVolume.NotFound` — while the False one becomes `available`), and a gp3 volume modified past its IOPS-per-GiB ceiling is refused while the legal target takes effect. Both settle in seconds to a couple of minutes.
Emulator: both checks end `Check timed out after 60.0s` — no state transition the check waits on ever arrives.
Why: callers polling EBS/instance state (every waiter in every SDK) never finish; the operations appear accepted but their effect never materialises.
Fix: the volume/instance state machines behind terminate-with-DeleteOnTermination and ModifyVolume — transitions must be applied rather than left pending.
Traces: ec2.delete-on-termination-decides-which-attached-volume-survives-the-terminate, ec2.diops-gp3-iops-to-size-ratio-above-the-limit-rejected-and-no-iops-reach-the-guest.

## Evidence
Cause 1 (representative): `TimeoutError: the SSM agent on <instance-id> coming Online: the run budget ended first; not graded` (ec2.cpuopts-guest-topology-differs-from-the-type-default, ec2.cpuopts-topology-survives-a-stop-start-cycle, ec2.ec2-attach-volume-to-a-device-name-already-in-use-rejected, ec2.idvdoc-document-reminted-and-resigned-after-a-stop-start) and `TimeoutError: <what>: SendCommand to <instance-id> being accepted: the run budget ended first; not graded` (ec2.dsnapinc-…, ec2.dvolgrow-shrink-…, ec2.placement-cluster-group-members-…). Expected: DescribeInstanceInformation reports PingStatus `Online` and SendCommand is accepted.
Cause 2: `TimeoutError: guest <instance-id> reports the payload it wrote: the run budget ended first; not graded` and `TimeoutError: guest A <instance-id> reports what it wrote to <vol-id>: the run budget ended first; not graded`. Expected: GetConsoleOutput.Output contains the marker the guest's UserData printed.
Cause 3: `TimeoutError: instance <instance-id> gets an auto-assigned public IPv4 address: the run budget ended first; not graded`. Expected: DescribeInstances reports Instance.PublicIpAddress for the running instance.
Cause 4: `Error: Check timed out after 60.0s` for both ec2.delete-on-termination-decides-which-attached-volume-survives-the-terminate and ec2.diops-gp3-iops-to-size-ratio-above-the-limit-rejected-and-no-iops-reach-the-guest. Expected: the DeleteOnTermination=True volume refused `InvalidVolume.NotFound` after terminate and the DeleteOnTermination=False volume `available`; the over-ratio gp3 ModifyVolume refused and the legal one applied.

## Steps to Reproduce
Replay the behavioral suite and re-run the twelve checks listed in trace_ids. Minimal repro per cause: (1) RunInstances with the AL2023 AMI and an instance profile holding arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore, then poll ssm DescribeInstanceInformation for PingStatus=='Online' and call ssm SendCommand against the instance id; (2) RunInstances with UserData that echoes a marker to the console, then poll ec2 GetConsoleOutput for the marker; (3) RunInstances into a subnet with MapPublicIpOnLaunch=true and poll DescribeInstances for PublicIpAddress; (4) attach two volumes with DeleteOnTermination true/false, TerminateInstances, then DescribeVolumes/CreateSnapshot on each, and separately ModifyVolume a gp3 volume to size*500+1 IOPS and then to a legal target.
