Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: c61e5d5a111548c41d0df12f114137f026337222

# Per-volume-type size/IOPS/throughput rules are implemented twice (CreateVolume and ModifyVolume)

_Filed: 2026-10-27T06:29:12Z_
_Operation: CreateVolume | Class: maintainability_

> _Note: this issue was identified against the code base at commit `c61e5d5a111548c41d0df12f114137f026337222`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "the size, IOPS and throughput must be ones the named volume type accepts, and a member the type takes no value for must be omitted" is implemented twice in this crate, once in `CreateVolume::validate_configuration` and once in `ModifyVolume::validate_target`, with the same three match blocks in the same order and the same six wordings. Both read the same `crate::state::VolumeTypeSpec` (`size_range()`, `spec.iops`, `spec.iops_required`, `spec.throughput_mib`), so the two copies can drift: a change to the shape of one branch (for example the `iops_required` case, which the two already spell differently) silently leaves the other operation applying the older rule, and the two would then refuse different requests for the same volume type. The two copies should be one helper taking the spec plus the (input value, target value) pair each operation already has, so each response and each existing test stays exactly as it is.

## Evidence

Copy 1 — services/.../src/ops/create_volume.rs:188-255:
```
    let (size_min, size_max) = spec.size_range();
    match input.size_in_gb {
...
        Some(size) if size < size_min || size > size_max => {
            return Err(field_validation_failed(
                "SizeInGB",
                format!(
                    "SizeInGB must be from {size_min} to {size_max} GiB for a \
                     {volume_type} volume"
...
    match (input.iops, spec.iops) {
        (Some(_), None) => { ... "Iops must be omitted for a {volume_type} volume, which does not provision IOPS" ... }
        (Some(iops), Some((min, max))) if iops < min || iops > max => { ... "Iops must be from {min} to {max} for a {volume_type} volume" ... }
        (None, _) if spec.iops_required => { ... "Iops is required for a {volume_type} volume" ... }
...
    match (input.throughput, spec.throughput_mib) {
        (Some(_), None) => { ... "Throughput must be omitted for a {volume_type} volume, whose throughput is not configurable" ... }
        (Some(throughput), Some((min, max))) if throughput < min || throughput > max => { ... "Throughput must be from {min} to {max} MiB/s for a {volume_type} volume" ... }
```
Copy 2 — services/.../src/ops/modify_volume.rs:275-339:
```
    let (size_min, size_max) = spec.size_range();
    if let Some(size) = target.size_in_gb {
        if size < size_min || size > size_max {
            return Err(field_validation_failed(
                "SizeInGB",
                format!(
                    "SizeInGB must be from {size_min} to {size_max} GiB for a \
                     {volume_type} volume"
...
    match (input.iops, spec.iops) {
        (Some(_), None) => { ... "Iops must be omitted for a {volume_type} volume, which does not provision IOPS" ... }
        (_, Some((min, max))) if target.iops.is_some_and(|iops| iops < min || iops > max) => { ... "Iops must be from {min} to {max} for a {volume_type} volume" ... }
        (None, Some(_)) if spec.iops_required && target.iops.is_none() => { ... "Iops is required for a {volume_type} volume" ... }
...
    match (input.throughput, spec.throughput_mib) {
        (Some(_), None) => { ... "Throughput must be omitted for a {volume_type} volume, whose throughput is not configurable" ... }
        (_, Some((min, max))) if target.throughput.is_some_and(|throughput| throughput < min || throughput > max) => { ... "Throughput must be from {min} to {max} MiB/s for a {volume_type} volume" ... }
```
Both read the one table and the one accessor: services/.../src/state.rs:1326-1341 (`VolumeTypeSpec::size_range`) and state.rs:1365-1415 (`VOLUME_TYPE_SPECS`), whose own doc at state.rs:1343-1364 states the rules once.

Existing tests that pin the responses on both sides: tests/all/volume_configuration_validation.rs (the `CreateVolume` per-type size/IOPS/throughput rows, e.g. lines 255-300 and 470-560) and tests/all/volume_modification.rs (the `ModifyVolume` target rows, e.g. lines 136-275 and 500-560), plus tests/all/catalog_fidelity.rs:488.

## Steps to Reproduce

1. Open services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/src/ops/create_volume.rs at lines 188-255 and services/.../src/ops/modify_volume.rs at lines 275-339 side by side: the same three rules, in the same order, with the same six message strings.
2. Change one branch in one file (for example the `Iops is required` condition) and observe nothing forces the other file to change: `cargo test -p service-com-amazonaws-workspacesinstances-eucmifrontendapiservice` still exercises the two copies independently through tests/all/volume_configuration_validation.rs and tests/all/volume_modification.rs.
3. Factor the three rules into one helper over `crate::state::VolumeTypeSpec` that both callers pass their (input, target) values to, and re-run the suite: every response and every test must stay unchanged.
