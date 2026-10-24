Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# CreateMonitor does not enforce the 4-probes-per-subnet limit for probes created inline

_Filed: 2026-10-24T07:15:54Z_
_Operation: CreateMonitor | Class: validation_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: CreateMonitor can create probes inline (`probes: [CreateMonitorProbeInput]`), and those probes are subject to the same limit AWS applies to CreateProbe — "Number of probes per subnet for each monitor" (default 4). A CreateMonitor request whose `probes` list contains 5 or more entries that share one `sourceArn` (one subnet) is refused by AWS with HTTP 400 / `ValidationException` (recorded AWS classification for this limit — see open issue 20261023T004257Z cause 3, message example "Maximum probes per subnet limit exceeded."), and no monitor is created (a subsequent GetMonitor 404s).

Actual (emulator): `src/ops/create_monitor.rs` counts nothing per subnet. The only ceiling applied to the inline probe list is `crate::state::MAX_PROBES_PER_MONITOR` (100, an emulator-chosen value) at handler lines ~281-292, and the per-probe checks in `crate::state::validate_probe_spec` only look at `sourceArn` shape, `destination`, port and protocol — never at how many stored probes already name the same subnet. So CreateMonitor with 10 probes all sourced from `arn:aws:ec2:<region>:<account>:subnet/subnet-0123456789abcdef0` returns HTTP 200, stores all 10, and GetMonitor lists 10 probes.

Secondary (same handler, same rule): when a ceiling IS hit, the handler answers `ServiceQuotaExceededException` (HTTP 402, lines ~270-292). The recorded AWS behavior for this service's probe-count limit is `ValidationException` (HTTP 400) even though `ServiceQuotaExceededException` is modeled on the operation, so a client catching ValidationException around a bulk CreateMonitor sees a 402 here instead.

The already-filed per-subnet issue scopes its fix explicitly to "CreateProbe's validation/state must count existing probes grouped by (monitorName, probe.sourceArn)"; the CreateMonitor inline-probe path is a second, independent way to exceed the same limit and is not named there.

## Evidence

Spec/grounding: the Smithy model documents CreateMonitor as "You can also create a monitor with probes using this command" and binds `probes` (list<CreateMonitorProbeInput>, each with a required `sourceArn` = "The ARN of the subnet"), so inline probes are real probes attached to one monitor and subject to the same per-subnet quota. The quota and its recorded refusal class (HTTP 400 ValidationException, "Maximum probes per subnet limit exceeded.") are documented in open issue 20261023T004257Z (trace networkmonitor.probes-per-subnet-quota-exceeded-rejected), which scopes the fix to CreateProbe only.

Handler: `services/service-com-amazonaws-networkmonitor-networkmonitor/src/ops/create_monitor.rs` lines ~224-300 build a `ProbeSpec` per entry and insert each one after `crate::state::build_probe`, with only `MAX_PROBES_PER_MONITOR` (100) and the per-probe rules applied. `grep` for `per_subnet|source_arn ==|probes_per_subnet` across the crate returns no matches, confirming no subnet grouping exists anywhere in the service.

Tests: `tests/all/monitor_and_probe.rs` covers probe storage, address-family derivation, probe tags and client-token replay, but no test creates several probes sharing one `sourceArn`; nothing asserts (or accepts) the current unlimited behavior.

## Steps to Reproduce

1. POST /monitors with body {"monitorName":"m-subnet-quota","aggregationPeriod":30,"probes":[ ...five entries... ]} where all five entries use the SAME "sourceArn":"arn:aws:ec2:<region>:<account>:subnet/subnet-0123456789abcdef0", distinct destinations (e.g. 203.0.113.1 .. 203.0.113.5), "protocol":"ICMP","packetSize":56.
2. Observe HTTP 200 and a stored monitor; expected HTTP 400 ValidationException ("Maximum probes per subnet limit exceeded.") with no monitor created.
3. GET /monitors/m-subnet-quota — observe 5 probes listed in `probes`; expected ResourceNotFoundException (404) because the create was refused.
4. Control: the same request with the five probes spread over five DIFFERENT subnet ARNs must still be accepted (the limit is per subnet, not per monitor).
5. Control for the refusal class: when any probe-count limit is hit, the error code must be ValidationException (HTTP 400), not ServiceQuotaExceededException (HTTP 402) as `src/ops/create_monitor.rs` currently returns.
