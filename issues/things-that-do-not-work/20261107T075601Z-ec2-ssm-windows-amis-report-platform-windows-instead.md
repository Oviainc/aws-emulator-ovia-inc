Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ssm.dwin-windows-guest-ebs-drive-letter-reads-blocked-after-detach ec2__ssm.dwin-windows-guest-runs-powershell-and-reads-imds ec2__ssm.dwin-windows-password-data-decrypts-with-its-key-pair ec2__ssm.w22n-cli-per-page-jmespath-count-sums-to-the-sdk-paginators-cross-boundary-derivation ec2__ssm.w22n-count-derivation-selects-only-windows-server-2022-managed-nodes ec2__ssm.w22n-instance-properties-join-ec2-name-type-and-profile-onto-the-2022-node ec2__ssm.w22n-inventory-aggregate-counts-2022-nodes-and-drops-a-terminated-node ec2__ssm.w22n-server-side-platform-and-tag-filters-narrow-before-the-2022-predicate ec2__ssm.w22n-stopped-2022-node-leaves-the-count-while-inventory-marks-it-stopped ec2__ssm.w22n-windows-2022-instance-without-an-ssm-role-blocked-from-the-count-until-repaired ec2__ssm.w22n-windows-2022-managed-node-is-region-scoped ec2__ssm.run-instances-empty-placement-availability-zone-treated-as-unset ec2__ssm.vscan-root-volume-smaller-than-the-ami-snapshot-rejected-while-a-larger-one-launches ec2__ssm.vscan-deprecated-ami-agent-trails-the-current-ami-agent ec2__ssm.guest-imdsv1-read-rejected-once-http-tokens-is-required ec2__ssm.guest-user-data-does-not-rerun-after-a-stop-start-cycle ec2__ssm.guest-user-data-output-read-back-through-run-command ec2__ssm.imds-token-ttl-above-21600-seconds-rejected-while-the-maximum-is-served ec2__ssm.run-command-shows-sshd-listening-while-no-group-allows-port-22-from-the-internet ec2__ssm.security-group-no-ingress-keeps-empty-list-and-default-egress ec2__ssm.ssm-run-command-executes-on-a-real-managed-instance-and-returns-its-stdout ec2__ssm.ssm-send-command-to-a-stopped-instance-rejected-and-no-invocation-is-created ec2__ssm.vscan-deprecated-ami-guest-has-more-pending-security-updates-than-the-current-ami ec2__ssm.vscan-inventory-omits-a-host-whose-profile-role-lacks-the-ssm-core-policy-until-it-is-attached ec2__ssm.vscan-inventory-platform-and-address-agree-with-ec2 ec2__ssm.vscan-run-command-to-an-instance-without-a-profile-rejected-invalid-instance-id
Issue-Key: ec2__ssm.dwin-windows-guest-ebs-drive-letter-reads-blocked-after-detach|run#7|AssertionError|assert image["Platform"] == "windows", (
Issue-Key: ec2__ssm.dwin-windows-guest-runs-powershell-and-reads-imds|run#7|AssertionError|assert image["Platform"] == "windows", (
Issue-Key: ec2__ssm.dwin-windows-password-data-decrypts-with-its-key-pair|run#10|AssertionError|assert windows_image["Platform"] == "windows", (
Issue-Key: ec2__ssm.w22n-cli-per-page-jmespath-count-sums-to-the-sdk-paginators-cross-boundary-derivation|_launch_fleet#4|AssertionError|assert image.get("Platform") == want, (
Issue-Key: ec2__ssm.w22n-count-derivation-selects-only-windows-server-2022-managed-nodes|_launch_fleet#4|AssertionError|assert image.get("Platform") == want, (
Issue-Key: ec2__ssm.w22n-instance-properties-join-ec2-name-type-and-profile-onto-the-2022-node|_launch_fleet#4|AssertionError|assert image.get("Platform") == want, (
Issue-Key: ec2__ssm.w22n-inventory-aggregate-counts-2022-nodes-and-drops-a-terminated-node|_launch_fleet#4|AssertionError|assert image.get("Platform") == want, (
Issue-Key: ec2__ssm.w22n-server-side-platform-and-tag-filters-narrow-before-the-2022-predicate|_launch_fleet#3|AssertionError|assert image.get("Platform") == want, (
Issue-Key: ec2__ssm.w22n-stopped-2022-node-leaves-the-count-while-inventory-marks-it-stopped|_launch_fleet#4|AssertionError|assert image.get("Platform") == want, (
Issue-Key: ec2__ssm.w22n-windows-2022-instance-without-an-ssm-role-blocked-from-the-count-until-repaired|_launch_fleet#4|AssertionError|assert image.get("Platform") == want, (
Issue-Key: ec2__ssm.w22n-windows-2022-managed-node-is-region-scoped|_launch_fleet#4|AssertionError|assert image.get("Platform") == want, (
Issue-Key: ec2__ssm.run-instances-empty-placement-availability-zone-treated-as-unset|run#9|AssertionError|assert empty == _DRY_RUN_OK, (
Issue-Key: ec2__ssm.vscan-root-volume-smaller-than-the-ami-snapshot-rejected-while-a-larger-one-launches|_refusal#12|AssertionError|raise AssertionError("RunInstances launched; real AWS refuses a root volume below the snapshot size")
Issue-Key: ec2__ssm.vscan-deprecated-ami-agent-trails-the-current-ami-agent|run#9|AssertionError|assert new_image["CreationDate"] > old_image["CreationDate"], (
Issue-Key: ec2__ssm.guest-imdsv1-read-rejected-once-http-tokens-is-required|_poll#10|Timeout|time.sleep(interval_s)
Issue-Key: ec2__ssm.guest-user-data-does-not-rerun-after-a-stop-start-cycle|_poll#11|Timeout|time.sleep(interval_s)
Issue-Key: ec2__ssm.guest-user-data-output-read-back-through-run-command|_poll#14|Timeout|time.sleep(interval_s)
Issue-Key: ec2__ssm.imds-token-ttl-above-21600-seconds-rejected-while-the-maximum-is-served|_poll#14|Timeout|time.sleep(interval_s)
Issue-Key: ec2__ssm.run-command-shows-sshd-listening-while-no-group-allows-port-22-from-the-internet|_wait_online#9|Timeout|time.sleep(10)
Issue-Key: ec2__ssm.security-group-no-ingress-keeps-empty-list-and-default-egress|_poll#22|Timeout|time.sleep(interval_s)
Issue-Key: ec2__ssm.ssm-run-command-executes-on-a-real-managed-instance-and-returns-its-stdout|_poll#19|Timeout|time.sleep(interval_s)
Issue-Key: ec2__ssm.ssm-send-command-to-a-stopped-instance-rejected-and-no-invocation-is-created|_poll#21|Timeout|time.sleep(interval_s)
Issue-Key: ec2__ssm.vscan-deprecated-ami-guest-has-more-pending-security-updates-than-the-current-ami|_poll#11|Timeout|time.sleep(interval_s)
Issue-Key: ec2__ssm.vscan-inventory-omits-a-host-whose-profile-role-lacks-the-ssm-core-policy-until-it-is-attached|_poll#15|Timeout|time.sleep(interval_s)
Issue-Key: ec2__ssm.vscan-inventory-platform-and-address-agree-with-ec2|_poll#10|Timeout|time.sleep(interval_s)
Issue-Key: ec2__ssm.vscan-run-command-to-an-instance-without-a-profile-rejected-invalid-instance-id|_poll#13|Timeout|time.sleep(interval_s)
Issue-Commit: 620c90c23a321396910fd66a692ef2fcecf33f7f

# EC2+SSM: Windows AMIs report Platform 'Windows' instead of 'windows' (10 checks), AMI dates invert, launch validation gaps, and managed nodes never register (12 checks hang)

> _Note: this issue was identified against the code base at commit `620c90c23a321396910fd66a692ef2fcecf33f7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeImages

## Description
Five net-new root causes behind the surviving ec2__ssm failures.

Cause 1 of 5: a Windows image's Platform is reported capitalised, so every Windows-node derivation fails at its first predicate.
AWS: EC2 reports the platform of a Windows image/instance as the lowercase string "windows" on the wire (the ec2 Smithy PlatformValues enum nominally lists "Windows"; the recorded value AWS returns, and the value every Windows-selection filter and client compares against, is "windows"), and the value is absent for Linux images.
Emulator: the AMI resolved from the SSM public parameter /aws/service/ami-windows-latest/Windows_Server-2022-English-Full-Base reports Platform='Windows'.
Why: every "is this a Windows node" derivation — the documented platform discriminator — evaluates false, so all ten Windows scenarios (drive letters, PowerShell/IMDS, password data, 2022-node counting, inventory aggregation, region scoping) stop before they start.
Fix: the DescribeImages/DescribeInstances serializer for Platform (and the public-AMI seed data behind the ami-windows-latest parameters).
Traces: ec2__ssm.dwin-windows-guest-ebs-drive-letter-reads-blocked-after-detach, .dwin-windows-guest-runs-powershell-and-reads-imds, .dwin-windows-password-data-decrypts-with-its-key-pair, .w22n-cli-per-page-jmespath-count-sums-to-the-sdk-paginators-cross-boundary-derivation, .w22n-count-derivation-selects-only-windows-server-2022-managed-nodes, .w22n-instance-properties-join-ec2-name-type-and-profile-onto-the-2022-node, .w22n-inventory-aggregate-counts-2022-nodes-and-drops-a-terminated-node, .w22n-server-side-platform-and-tag-filters-narrow-before-the-2022-predicate, .w22n-stopped-2022-node-leaves-the-count-while-inventory-marks-it-stopped, .w22n-windows-2022-instance-without-an-ssm-role-blocked-from-the-count-until-repaired, .w22n-windows-2022-managed-node-is-region-scoped.

Cause 2 of 5: an empty Placement.AvailabilityZone is refused instead of being treated as unset.
AWS: Placement={"AvailabilityZone": ""} is equivalent to omitting it — the launch proceeds (under DryRun, DryRunOperation "Request would have succeeded, but DryRun flag is set.") and EC2 picks a zone.
Emulator: refused InvalidParameterValue, "Invalid value '' for availabilityZone. Region 'us-west-2' has no such Availability Zone".
Why: SDKs and IaC templates routinely send empty strings for unset optional fields; those launches now fail outright.
Fix: RunInstances placement parsing — treat an empty AvailabilityZone as absent.
Traces: ec2__ssm.run-instances-empty-placement-availability-zone-treated-as-unset.

Cause 3 of 5: a root volume smaller than the AMI snapshot is accepted.
AWS: a block-device mapping whose VolumeSize is below the AMI snapshot's size is refused InvalidBlockDeviceMapping (the volume cannot be smaller than the snapshot it restores), while a larger size launches.
Emulator: RunInstances launches the undersized root volume.
Why: an impossible instance is created, and the negative branch of any image-sizing validation is unreachable.
Fix: RunInstances block-device-mapping validation against the AMI snapshot size.
Traces: ec2__ssm.vscan-root-volume-smaller-than-the-ami-snapshot-rejected-while-a-larger-one-launches.

Cause 4 of 5: the deprecated AMI in the seeded catalogue post-dates the current one.
AWS: the current Amazon Linux 2 AMI resolved from the SSM public parameter always has a CreationDate newer than the deprecated AMI it replaces, and a deprecated AMI carries a DeprecationTime in the past.
Emulator: the current AMI reads 2024-11-13T18:17:12.000Z while the "deprecated" one reads 2025-10-13T00:00:00Z — the ordering is inverted.
Why: any freshness derivation ("is this instance running an outdated image?") concludes the opposite of AWS.
Fix: the seeded AMI catalogue's CreationDate/DeprecationTime for the public images and the DescribeImages projection of them.
Traces: ec2__ssm.vscan-deprecated-ami-agent-trails-the-current-ami-agent.

Cause 5 of 5: an instance never becomes an SSM managed node and never presents a guest, so 12 checks spin to the 60s budget.
AWS: an instance with an SSM-core profile registers with SSM, SendCommand's invocation reaches Success with the command's stdout, a stopped instance's SendCommand is refused InvalidInstanceId, inventory reports the node's platform/address/agent version, and in-guest IMDS honours HttpTokens=required and the 21600s token TTL maximum (recorded behavior).
Emulator: the node never registers and no in-guest effect is observable; each check below ends "Check timed out after 60.0s".
Why: the EC2+SSM operating surface (Run Command, inventory, IMDS enforcement, UserData behaviour across stop/start) is unusable, and a missing registration cannot be told apart from a wrong answer.
Fix: the SSM agent registration the EC2 instance lifecycle must produce, the Run Command invocation state machine, and the instance runtime behind the in-guest reads.
Traces: ec2__ssm.guest-imdsv1-read-rejected-once-http-tokens-is-required, .guest-user-data-does-not-rerun-after-a-stop-start-cycle, .guest-user-data-output-read-back-through-run-command, .imds-token-ttl-above-21600-seconds-rejected-while-the-maximum-is-served, .run-command-shows-sshd-listening-while-no-group-allows-port-22-from-the-internet, .security-group-no-ingress-keeps-empty-list-and-default-egress, .ssm-run-command-executes-on-a-real-managed-instance-and-returns-its-stdout, .ssm-send-command-to-a-stopped-instance-rejected-and-no-invocation-is-created, .vscan-deprecated-ami-guest-has-more-pending-security-updates-than-the-current-ami, .vscan-inventory-omits-a-host-whose-profile-role-lacks-the-ssm-core-policy-until-it-is-attached, .vscan-inventory-platform-and-address-agree-with-ec2, .vscan-run-command-to-an-instance-without-a-profile-rejected-invalid-instance-id.

## Evidence
From the behavioral replay:
- the ten dwin-*/w22n-* checks [mismatch]: AssertionError at `assert image.get("Platform") == want` — "/aws/service/ami-windows-latest/Windows_Server-2022-English-Full-Base must resolve to a windows image; ami-00000000000000028 reports Platform='Windows' Name='Windows_Server-2022-English-Full-Base-2024.11.13'".
- run-instances-empty-placement-availability-zone-treated-as-unset [mismatch]: Placement.AvailabilityZone '': expected ('DryRunOperation', 'Request would have succeeded, but DryRun flag is set.'), got ('InvalidParameterValue', "Invalid value '' for availabilityZone. Region 'us-west-2' has no such Availability Zone").
- vscan-root-volume-smaller-than-the-ami-snapshot-rejected-while-a-larger-one-launches [mismatch]: RunInstances launched; real AWS refuses a root volume below the snapshot size.
- vscan-deprecated-ami-agent-trails-the-current-ami-agent [mismatch]: the current AL2 AMI 'ami-...' (2024-11-13T18:17:12.000Z) must post-date the deprecated one 'ami-...' (2025-10-13T00:00:00Z).
- the 12 checks in Cause 5 [mismatch]: Error: Check timed out after 60.0s.

## Steps to Reproduce
Replay the behavioral suite and re-run every check in trace_ids: resolve /aws/service/ami-windows-latest/... and read DescribeImages Platform; RunInstances with Placement.AvailabilityZone="" under DryRun; RunInstances with a root VolumeSize below the AMI snapshot; compare CreationDate of the current and deprecated AL2 AMIs; launch an SSM-profiled instance and poll DescribeInstanceInformation / SendCommand / inventory.
