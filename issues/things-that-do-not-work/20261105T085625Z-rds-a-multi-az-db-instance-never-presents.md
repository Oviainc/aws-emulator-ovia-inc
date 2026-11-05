Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.multi-az-standby-auto-failover-on-primary-failure
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.multi-az-standby-auto-failover-on-primary-failure|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Commit: 349fac5dd238950f9954cc41b162705b83d96a37

# rds: a Multi-AZ DB instance never presents the standby-failover state RebootDBInstance(ForceFailover=true) must produce, so the AZ-swap check never reaches a verdict

> _Note: this issue was identified against the code base at commit `349fac5dd238950f9954cc41b162705b83d96a37`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RebootDBInstance

## Description
Cause 1 of 1: the Multi-AZ failover a forced reboot performs is not represented, so a client polling for it polls forever.
AWS: `CreateDBInstance` with `MultiAZ=true` provisions a standby in a second Availability Zone, and `DescribeDBInstances` then reports both `AvailabilityZone` (the primary's) and `SecondaryAvailabilityZone` — Smithy, `DBInstance$SecondaryAvailabilityZone`: "If present, specifies the name of the secondary Availability Zone for a DB instance with multi-AZ support." `RebootDBInstance` with `ForceFailover=true` — Smithy, `RebootDBInstanceMessage$ForceFailover`: "Specifies whether the reboot is conducted through a Multi-AZ failover", with the constraint "You can't enable force failover if the instance isn't configured for Multi-AZ" — makes the SERVICE promote the standby: once the instance is `available` again, the two values have swapped, `AvailabilityZone` being the former `SecondaryAvailabilityZone` and `SecondaryAvailabilityZone` the former primary. The call is accepted with 200 and the reboot completes; the only modeled refusals are `DBInstanceNotFoundFault` (404), `InvalidDBInstanceStateFault` (400) and `KMSKeyNotAccessibleFault` (400).
Emulator: the check is cut off at the harness cap (`Check timed out after 60.0s`) inside one of its polls, having produced no refusal and no error code — the instance never presents the state being waited for. Its waits are, in order: `DBInstanceStatus == "available"` with `MultiAZ is True`, then a non-empty `SecondaryAvailabilityZone`, then, after `RebootDBInstance(ForceFailover=True)`, `DBInstanceStatus == "available" and AvailabilityZone == <former SecondaryAvailabilityZone>`. Hypothesis: `CreateDBInstance(MultiAZ=true)` records no secondary AZ, so `DescribeDBInstances` omits `SecondaryAvailabilityZone` and the second poll can never be satisfied; equivalently, `RebootDBInstance` treats `ForceFailover` as a no-op and the AZ pair never swaps. Either way the emulator reports no state in which the forced failover happened.
Why: a client that uses a forced reboot to exercise or verify its Multi-AZ failover path — the documented way to test standby promotion — gets a call that appears to succeed while the topology never changes, and a readiness/verification poll on `SecondaryAvailabilityZone` or on the swap hangs instead of converging. High-availability behaviour that a real deployment depends on is silently absent.
Fix: the `CreateDBInstance`/`ModifyDBInstance` Multi-AZ state in the rds crate (assign and persist a secondary AZ distinct from the primary whenever `MultiAZ` is true, and serialize it as `DBInstance$SecondaryAvailabilityZone`) plus the `RebootDBInstance` handler (when `ForceFailover` is true on a Multi-AZ instance, swap the primary and secondary AZs as part of the reboot transition back to `available`; when the instance is not Multi-AZ, keep the modeled refusal rather than swapping).
Traces: rds.multi-az-standby-auto-failover-on-primary-failure. Related open issues cover different ground and do not carry this check: `20261104T053605Z-rds-the-published-db-instance-cluster-endpoint-serves.md` is about the SQL wire protocol (this check opens no session and is pure control plane), and `20261104T053605Z-rds-6-net-new-causes-applyimmediately-false-is.md` is about pending-modification representation.

## Evidence
`Error: Check timed out after 60.0s` — no response, no error code. Expected, from the check's own polls: after `CreateDBInstance(MultiAZ=true)` and the instance reaching `available`, `DescribeDBInstances` reports `MultiAZ: true` with a non-empty `SecondaryAvailabilityZone` distinct from `AvailabilityZone`; after `RebootDBInstance(DBInstanceIdentifier=<id>, ForceFailover=true)` returns 200 and the instance is `available` again, `AvailabilityZone` equals the former `SecondaryAvailabilityZone` and `SecondaryAvailabilityZone` equals the former `AvailabilityZone` (the assertions read "primary AZ did not swap to the former standby AZ" and "standby AZ did not become the former primary AZ").

## Steps to Reproduce
Replay the behavioral suite and run rds.multi-az-standby-auto-failover-on-primary-failure. By hand: (1) rds CreateDBSubnetGroup over subnets in at least two AZs; (2) rds CreateDBInstance with Engine=mysql, DBInstanceClass=db.t3.micro, AllocatedStorage=20, MultiAZ=true and that subnet group; (3) poll rds DescribeDBInstances until DBInstanceStatus=available and read MultiAZ, AvailabilityZone and SecondaryAvailabilityZone — SecondaryAvailabilityZone must be present; (4) call rds RebootDBInstance with ForceFailover=true and poll DescribeDBInstances until available — AvailabilityZone and SecondaryAvailabilityZone must have swapped.
