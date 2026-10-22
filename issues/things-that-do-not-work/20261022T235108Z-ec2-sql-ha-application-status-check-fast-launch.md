Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-disable-instance-sql-ha-standby-detections ec2.op-contract-enableInstanceSqlHaStandbyDetections ec2.op-contract-disable-application-status-check-suppression ec2.op-contract-disable-fast-launch ec2.op-contract-enableReachabilityAnalyzerOrganizationSharing ec2.op-contract-modify-address-attribute ec2.op-contract-assignIpv6Addresses-b ec2.op-contract-delete-public-ipv4-pool-standalone-rejection ec2.ec2-run-from-launch-template
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-disable-instance-sql-ha-standby-detections|run#4|AssertionError|assert code == "InvalidInstanceID.Malformed", (
Issue-Key: ec2.op-contract-enableInstanceSqlHaStandbyDetections|run#5|AssertionError|assert dry_run_code == "InvalidInstanceID.Malformed", (
Issue-Key: ec2.op-contract-disable-application-status-check-suppression|run#4|AssertionError|assert code == "InvalidInstanceID.Malformed", f"expected InvalidInstanceID.Malformed, got {code}"
Issue-Key: ec2.op-contract-disable-fast-launch|run#4|AssertionError|assert err["Code"] == "DryRunOperation", f"expected DryRunOperation, got {err['Code']}"
Issue-Key: ec2.op-contract-enableReachabilityAnalyzerOrganizationSharing|run#3|AssertionError|assert dry_run_code == "DryRunOperation", f"expected DryRunOperation, got {dry_run_code}"
Issue-Key: ec2.op-contract-modify-address-attribute|run#3|AssertionError|assert code == "InvalidElasticIpID.NotFound", f"unexpected error code: {code!r}"
Issue-Key: ec2.op-contract-assignIpv6Addresses-b|run#5|AssertionError|assert info["code"] == "InvalidNetworkInterfaceID.NotFound", (
Issue-Key: ec2.op-contract-delete-public-ipv4-pool-standalone-rejection|run#2|AssertionError|"NotFound" in code or code == "InvalidParameterValue"
Issue-Key: ec2|RunInstances|NotImplemented
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# EC2 SQL-HA, application-status-check, fast-launch, reachability-analyzer, address/IPv6 and RunInstances-with-LaunchTemplate paths answer 501 NotImplemented (9 causes)

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
Nine unimplemented EC2 code paths, each its own cause. All nine must be closed before submitting. Common shape: these operations model a DryRun member and no error shapes (Smithy com-amazonaws-ec2-amazonec2; EC2 error codes are ec2Query wire codes, documented not modeled). AWS answers an authorized DryRun=true call with 400 DryRunOperation 'Request would have succeeded, but DryRun flag is set.' and validates resource ids BEFORE the permission check, so a malformed instance id gives InvalidInstanceID.Malformed even under DryRun. The emulator answers NotImplemented ('operation <Op> is scaffolded but not implemented in this emulator'). Why, for all nine: each named surface is unusable, and for the RunInstances case the single most common launch form in real client code (launch template) cannot be used at all. Fix, for all nine: implement the named handler under services/service-com-amazonaws-ec2-amazonec2/src/ops/.

Cause 1 of 9: DisableInstanceSqlHaStandbyDetections — expected InvalidInstanceID.Malformed even under DryRun, got NotImplemented. Trace: ec2.op-contract-disable-instance-sql-ha-standby-detections.
Cause 2 of 9: EnableInstanceSqlHaStandbyDetections — same expectation, got NotImplemented. Trace: ec2.op-contract-enableInstanceSqlHaStandbyDetections.
Cause 3 of 9: DisableApplicationStatusCheckSuppression — expected InvalidInstanceID.Malformed, got NotImplemented. Trace: ec2.op-contract-disable-application-status-check-suppression.
Cause 4 of 9: DisableFastLaunch — expected DryRunOperation, got NotImplemented. Trace: ec2.op-contract-disable-fast-launch.
Cause 5 of 9: EnableReachabilityAnalyzerOrganizationSharing — expected DryRunOperation, got NotImplemented. Trace: ec2.op-contract-enableReachabilityAnalyzerOrganizationSharing.
Cause 6 of 9: ModifyAddressAttribute — 'unexpected error code: NotImplemented' where AWS gives the address not-found/parameter code. Trace: ec2.op-contract-modify-address-attribute.
Cause 7 of 9: AssignIpv6Addresses — expected InvalidNetworkInterfaceID.NotFound, got NotImplemented. Trace: ec2.op-contract-assignIpv6Addresses-b.
Cause 8 of 9: DeletePublicIpv4Pool — expected a not-found/invalid-parameter rejection for ipv4pool-ec2-00000000000000000, got NotImplemented. Trace: ec2.op-contract-delete-public-ipv4-pool-standalone-rejection.
Cause 9 of 9: RunInstances with a LaunchTemplate — AWS launches the instance described by LaunchTemplate{LaunchTemplateId|Name, Version} and returns Reservation.Instances; the emulator refuses with NotImplemented and the body 'operation RunInstances with a LaunchTemplate is scaffolded but not implemented in this emulator', i.e. RunInstances exists but its launch-template branch does not. Why: every launch-template-driven client (ASG, CDK, CLI) cannot start an instance. Fix: the LaunchTemplate resolution branch of services/service-com-amazonaws-ec2-amazonec2/src/ops/run_instances.rs. Trace: ec2.ec2-run-from-launch-template.

## Evidence
Replay output: AssertionError: expected InvalidInstanceID.Malformed even under DryRun (instance-ID validation runs before the permission check), got NotImplemented; AssertionError: expected InvalidInstanceID.Malformed, got NotImplemented; AssertionError: expected DryRunOperation, got NotImplemented; AssertionError: unexpected error code: 'NotImplemented'; AssertionError: expected InvalidNetworkInterfaceID.NotFound, got 'NotImplemented'; AssertionError: expected a not-found/invalid-parameter rejection for ipv4pool-ec2-00000000000000000, got NotImplemented; botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the RunInstances operation: operation RunInstances with a LaunchTemplate is scaffolded but not implemented in this emulator.

## Steps to Reproduce
Replay the check ids in trace_ids. Minimal repros: (1) ec2:DisableInstanceSqlHaStandbyDetections with InstanceIds=['i-00000000000000000'], DryRun=True (expect InvalidInstanceID.Malformed). (2) ec2:AssignIpv6Addresses with NetworkInterfaceId='eni-00000000000000000'. (3) ec2:CreateLaunchTemplate then ec2:RunInstances with LaunchTemplate={'LaunchTemplateId': ...}, MinCount=1, MaxCount=1.
