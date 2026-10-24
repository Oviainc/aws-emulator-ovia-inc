Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# Probe records never carry vpcId, so CreateProbe/GetProbe/GetMonitor/UpdateProbe always omit the modeled vpcId

_Filed: 2026-10-24T05:26:27Z_
_Operation: CreateProbe | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected (AWS): a probe's `vpcId` is derived by the service from the source subnet named in `sourceArn` (model doc for `Probe$vpcId` / CreateProbe output `vpcId`: "The ID of the source VPC subnet", constrained to `^vpc-[a-zA-Z0-9]{8,32}$`). Every read path that reports a probe (CreateProbe, GetProbe, UpdateProbe, and each entry of GetMonitor's `probes`) carries it once the probe's source subnet is known.

Actual (emulator): the stored `Probe` record hard-codes `vpc_id: None` at construction time and nothing ever writes it, so `vpcId` is absent from EVERY probe response the emulator produces. A client reading `probe["vpcId"]` (or an SDK model expecting it) sees nothing where AWS returns a vpc id, on all four probe-reporting operations.

This is implementable here: the EC2 emulator's subnet store (`services/service-com-amazonaws-ec2-amazonec2/src/state.rs`, `SubnetSpec { vpc_id, ... }`) holds the VPC of each subnet, so resolving `sourceArn` against it yields the `vpcId` the real service reports (and is the same lookup needed to validate subnet existence).

Secondary symptom of the same class worth checking while fixing: `packetSize` is likewise stored as `Option` and reported absent when the request omitted it, whereas probes always have a concrete packet size (AWS's documented/console default is 56) — the emulator already applies the analogous documented default for `aggregationPeriod` but not for `packetSize`.

## Evidence

Spec: `com.amazonaws.networkmonitor#Probe$vpcId` (target VpcId, pattern `^vpc-[a-zA-Z0-9]{8,32}$`, doc "The ID of the source VPC subnet") is a member of the Probe shape returned by GetProbe/CreateProbe/UpdateProbe/GetMonitor. Handler: `src/state.rs` `build_probe()` sets `vpc_id: None` unconditionally (and the `Probe::vpc_id` doc comment states the omission is deliberate: "this emulator holds no EC2 subnets"); `src/ops/create_probe.rs::output_for`, `src/ops/get_probe.rs` handler, `src/ops/get_monitor.rs::probe_of` and `src/ops/update_probe.rs::output_for` all copy that `None` straight through. Tests: `tests/all/monitor_and_probe.rs::a_probe_is_stored_and_its_address_family_follows_the_destination` and `monitor_lifecycle_and_front_door.rs::get_probe_reports_the_stored_probe` assert the other probe members; no test asserts `vpcId`, and no existing issue mentions it.

## Steps to Reproduce

1. CreateMonitor(monitorName="m1", aggregationPeriod=60).
2. CreateProbe(monitorName="m1", probe={sourceArn: <an ec2 subnet ARN>, destination: "203.0.113.10", protocol: "ICMP", packetSize: 56}).
3. Inspect the CreateProbe response body: no `vpcId` key (AWS returns the source subnet's VPC id, e.g. "vpc-0123456789abcdef0").
4. GetProbe(monitorName="m1", probeId=<id>) and GetMonitor(monitorName="m1") — `vpcId` is absent from the probe payload in both.
5. UpdateProbe(monitorName="m1", probeId=<id>, state="INACTIVE") — the returned probe is still missing `vpcId`.
