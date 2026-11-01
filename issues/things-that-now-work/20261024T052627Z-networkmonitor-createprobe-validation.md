Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# CreateProbe accepts a sourceArn naming a subnet that does not exist (ARN is only shape-checked, EC2 state never consulted)

_Filed: 2026-10-24T05:26:27Z_
_Operation: CreateProbe | Class: validation_

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected (AWS): `probe.sourceArn` is resolved against the caller's actual VPC subnets. A well-formed subnet ARN in the caller's account that names a subnet which does not exist is refused with ValidationException (HTTP 400) — the recorded AWS wording for this family is "The specified subnet <subnetId> does not exist or could not be found." — and no probe is attached to the monitor.

Actual (emulator): the check is purely LEXICAL. `crate::state::subnet_id_of` only parses the ARN (`arn:<partition>:ec2:<region>:<emulator account>:subnet/subnet-<8 or 17 alnum>`); any string of that shape passes. `CreateProbe` with `sourceArn = arn:aws:ec2:us-east-1:<account>:subnet/subnet-00000000000000000` (never created anywhere) returns HTTP 200 with a fully populated probe in state PENDING, and GetMonitor then lists a probe whose source subnet does not exist. The same code path is reached by `CreateMonitor`'s `probes` list and by `UpdateProbe` (both call `validate_probe_spec`), so all three write paths are over-permissive.

This repo contains an EC2 emulator whose state holds real subnets (`services/service-com-amazonaws-ec2-amazonec2/src/state.rs`: `subnets: BTreeMap<String, Subnet>`, `SubnetSpec { vpc_id, cidr_block, availability_zone, region, ... }`, created via CreateSubnet), so the existence lookup the real service performs is implementable here: resolve the subnet id out of the ARN against that store and refuse when it is absent.

## Evidence

Spec: ProbeInput$sourceArn is documented as "The ARN of the subnet" with only `@pattern(^arn:.*$)`, so the resource must be resolved by the service; CreateProbe declares ValidationException (HTTP 400). Handler: `services/service-com-amazonaws-networkmonitor-networkmonitor/src/state.rs` `subnet_id_of()` (ARN parsing only — partition/service/account/resource-type/id-shape, no store lookup) called from `validate_probe_spec()`, used by `build_probe()` which `create_probe.rs` (handler lines ~227-291) and `create_monitor.rs` invoke. Tests: `tests/all/monitor_lifecycle_and_front_door.rs::a_probes_source_must_name_a_subnet_of_the_callers_account` only asserts the ARN-shape refusal (wrong service / wrong account / wrong resource type); no test asserts that a shape-valid but nonexistent subnet id is refused. Related existing issue 20261022T021102Z (cause 5, fixed) covered only a non-subnet ARN (an SNS topic ARN), not a nonexistent subnet, so this is not the same gap.

## Steps to Reproduce

1. Boot the emulator and point a networkmonitor client at it.
2. CreateMonitor(monitorName="m1", aggregationPeriod=60).
3. CreateProbe(monitorName="m1", probe={sourceArn: "arn:aws:ec2:<region>:<emulator account>:subnet/subnet-00000000000000000", destination: "203.0.113.10", protocol: "ICMP", packetSize: 56}) — no such subnet was ever created in the EC2 emulator.
4. Observe HTTP 200 and a created probe (state PENDING); AWS answers ValidationException/400 ("The specified subnet subnet-00000000000000000 does not exist or could not be found.") and attaches nothing.
5. GetMonitor(monitorName="m1") shows the bogus probe in `probes` (AWS would show an empty list).
6. Control: create a subnet in the EC2 emulator (ec2:CreateVpc + ec2:CreateSubnet) and pass that subnet's ARN — this must still be accepted.
