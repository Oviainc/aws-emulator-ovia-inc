Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6e8c3e9c5f69577c8704496d0849780bb1468b0d

# The per-subnet probe bound groups probes by the raw sourceArn string instead of the subnet the ARN names, so one subnet can hold twice the bound

_Filed: 2026-10-28T21:00:05Z_
_Operation: CreateMonitor | Class: validation_

> _Note: this issue was identified against the code base at commit `6e8c3e9c5f69577c8704496d0849780bb1468b0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::check_probes_per_subnet` — the one helper both create paths use (`CreateMonitor`'s inline `probes` list and `CreateProbe`) — groups probes by the FULL `sourceArn` string. Its own doc, and the doc of the bound it enforces, state the rule is per (monitor, source SUBNET) pair.

The crate already has the function that says which subnet an ARN names, `crate::state::subnet_id_of`, and the validation path calls it (`validate_probe_spec`). That function deliberately ignores the ARN's partition and region segments (`let _partition = segments.next()?;`, `let _region = segments.next()?;`), so two different ARN strings that differ only in those segments both validate and both name the SAME subnet id — yet they land in two different buckets of the per-subnet counter. One monitor can therefore be given 4 probes through `arn:aws:ec2:us-east-1:<acct>:subnet/subnet-0123456789abcdef0` and 4 more through `arn:aws:ec2:eu-west-1:<acct>:subnet/subnet-0123456789abcdef0`: 8 probes in one source subnet, double what the crate says one subnet of a monitor may hold, and a `CreateMonitor` whose inline list does this is accepted in a single call. The counting path skips the helper the validating path uses for exactly this question.

## Evidence

The grouping key, services/service-com-amazonaws-networkmonitor-networkmonitor/src/state.rs:787-808:
```
pub fn check_probes_per_subnet<'a>(
    existing: Option<&'a Monitor>,
    requested: impl IntoIterator<Item = &'a str>,
) -> Result<(), String> {
    let mut per_subnet: BTreeMap<&str, usize> = BTreeMap::new();
    ...
        for probe in monitor.probes.values() {
            *per_subnet.entry(probe.source_arn.as_str()).or_default() += 1;
        }
    for source_arn in requested {
        let count = per_subnet.entry(source_arn).or_default();
```

The rule it is supposed to apply, its own doc at src/state.rs:776-777: "The limit is counted per (monitor, source subnet) pair", and the constant's doc at src/state.rs:212-216:
```
/// The most probes one monitor holds in ONE source subnet: AWS's Network
/// Monitor quota "Number of probes per subnet for each monitor" ... The limit is
/// per (monitor, source subnet) pair, so probes spread over separate subnets of
/// the same monitor do not share it.
pub const MAX_PROBES_PER_SUBNET: usize = 4;
```

The helper the crate already has for naming the subnet, src/state.rs:333-367:
```
/// The subnet id a probe's `sourceArn` names, when it names a VPC subnet of this
/// account at all; `None` for anything else.
pub fn subnet_id_of(source_arn: &str) -> Option<&str> {
    let mut segments = source_arn.splitn(6, ':');
    if segments.next()? != "arn" { return None; }
    let _partition = segments.next()?;
    if segments.next()? != SUBNET_ARN_SERVICE { return None; }
    let _region = segments.next()?;
```
and the path that DOES call it, src/state.rs:740-746 (`validate_probe_spec`: `if subnet_id_of(&spec.source_arn).is_none() { return Err(...)}`) — so both region-variant ARNs pass validation as the same subnet id while the counter treats them as two subnets.

Callers of the counter: src/ops/create_monitor.rs:345-349 (`check_probes_per_subnet(None, specs.iter().map(|spec| spec.source_arn.as_str()))`) and src/ops/create_probe.rs:307-311.

Test coverage: tests/all/probes_per_subnet_limit.rs:7-13 states the rule under test as "a monitor holds at most [`MAX_PROBES_PER_SUBNET`] probes whose `sourceArn` names one subnet, counted per (monitor, subnet) pair", and every row builds its ARNs with tests/all/monitor_and_probe.rs:75-77 `source_arn_of(index)` = `arn:aws:ec2:{REGION}:{ACCOUNT}:subnet/subnet-0123456789abcd{index:03}` — one ARN per subnet id and always the same region, so no case names one subnet through two different ARNs. No comment or test justifies keying the count by the ARN string.

## Steps to Reproduce

1. POST /monitors with {"monitorName":"onesubnet","aggregationPeriod":30}.
2. POST /monitors/onesubnet/probes four times with probe.sourceArn = "arn:aws:ec2:us-east-1:<account>:subnet/subnet-0123456789abcdef0" (all accepted: the subnet is now at the bound of 4).
3. POST a fifth probe with that same ARN: refused 400 ValidationException "Maximum probes per subnet limit exceeded." — as expected.
4. POST four more probes with probe.sourceArn = "arn:aws:ec2:eu-west-1:<account>:subnet/subnet-0123456789abcdef0" (same subnet id, different region segment; subnet_id_of accepts it): all four are accepted.
5. GET /monitors/onesubnet: the monitor now lists 8 probes whose sourceArn names subnet-0123456789abcdef0, twice MAX_PROBES_PER_SUBNET. The same can be done in ONE call via CreateMonitor's inline `probes` list (4 entries of each ARN form).
