Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 7959e6ca7cb81d5c32ad32f8e1e29786f97ad065

# CreateMonitor judges the inline probe COUNT before each probe's own rules, contradicting the comment that states the opposite order (and its sibling per-subnet ceiling)

_Filed: 2026-10-28T10:02:52Z_
_Operation: CreateMonitor | Class: error-response_

> _Note: this issue was identified against the code base at commit `7959e6ca7cb81d5c32ad32f8e1e29786f97ad065`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

In CreateMonitor's handler the per-monitor probe-count ceiling (`check_probes_per_monitor`, which judges "how many probes the request asks for") runs BEFORE the loop that validates each requested probe, while the comment sitting directly above that loop states the inverse rule: each probe must satisfy the probe rules on its own BEFORE the request is judged on how many probes it asks for, "so a malformed probe is refused for what it says rather than for the company it keeps".

Consequence: a create whose `probes` list is both too long and contains a malformed probe (e.g. a `destination` that is not an IP literal, or TCP with no `destinationPort`) is refused with the count sentence "Maximum probes per monitor limit exceeded." and the caller is never told which member it actually got wrong — the exact outcome the comment says must not happen.

The same handler also applies the two sibling count ceilings in two different orders: the per-SUBNET ceiling is checked AFTER the validation loop (with a comment saying it is "Checked before any probe id is minted"), the per-MONITOR one before it, so two rules of the same family resolve against a malformed probe in opposite ways.

## Evidence

services/service-com-amazonaws-networkmonitor-networkmonitor/src/ops/create_monitor.rs:324-349 — the count check at line 327 precedes the validation loop whose own comment (lines 328-332) demands the opposite order:
```
324        // The probes this request creates inline are probes of this monitor, so
325        // they are bounded as a whole exactly as `CreateProbe`'s are ...
327        crate::state::check_probes_per_monitor(None, specs.len()).map_err(validation)?;
328        // Each requested probe has to satisfy the probe rules on its own before
329        // the request is judged on how many probes it asks for, so a malformed
330        // probe is refused for what it says rather than for the company it
331        // keeps. `crate::state::build_probe` below applies the same rules when
332        // it writes each record.
333        for (position, spec) in specs.iter().enumerate() {
334            crate::state::validate_probe_spec(
335                spec,
336                &crate::state::element_path("probes", position + 1),
337            )
338            .map_err(validation)?;
339        }
...
345        crate::state::check_probes_per_subnet(
346            None,
347            specs.iter().map(|spec| spec.source_arn.as_str()),
348        )
349        .map_err(validation)?;
```
The refusal the count check emits — src/state.rs:228-232:
```
pub const PROBES_PER_MONITOR_MESSAGE: &str = "Maximum probes per monitor limit exceeded.";
```
versus the per-member clause the validation loop would have emitted, src/state.rs:747-756 (`DESTINATION_REQUIREMENT` / `TCP_PORT_REQUIRED_MESSAGE`).

Test coverage: tests/all/probes_per_monitor_limit.rs pins only well-formed probes against the ceiling ("One probe body for the `position`th probe of a monitor: spread over subnets ... with a destination of its own", lines 39-43), so no test fixes the precedence either way.

## Steps to Reproduce

1. POST /monitors with monitorName="order" and a `probes` list of 101 entries (one past MAX_PROBES_PER_MONITOR) whose FIRST entry is malformed, e.g. {"sourceArn":"arn:aws:ec2:<region>:<account>:subnet/subnet-0123456789abcdef0","destination":"not-an-ip","protocol":"ICMP","packetSize":56}.
2. Observe the 400 ValidationException message "Maximum probes per monitor limit exceeded." — the malformed `probes.1.member.destination` is never named, although the comment at create_monitor.rs:328-332 states a malformed probe is refused "for what it says rather than for the company it keeps".
3. Contrast with the sibling ceiling: send 5 WELL-FORMED probes in one subnet plus one malformed probe — the per-subnet check sits after the validation loop (create_monitor.rs:345), so there the malformed member IS named, i.e. the two count rules order themselves differently.
