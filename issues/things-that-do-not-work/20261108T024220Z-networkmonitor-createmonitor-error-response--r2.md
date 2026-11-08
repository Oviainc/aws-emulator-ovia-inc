Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: b8cfa81034a84e4fdc6867803324616d4ab48fbf

# CreateMonitor's monitor-ceiling ServiceQuotaExceededException states a free-form "this emulator keeps at most N monitors" sentence, which the same handler and the crate's own refusal rule forbid

_Filed: 2026-11-08T02:42:20Z_
_Operation: CreateMonitor | Class: error-response_

> _Note: this issue was identified against the code base at commit `b8cfa81034a84e4fdc6867803324616d4ab48fbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When the store already holds `MAX_PROBES`… i.e. `crate::state::MAX_MONITORS` monitors, `CreateMonitor` answers the modelled `ServiceQuotaExceededException` with a message built from the emulator's own chosen ceiling: `"this emulator keeps at most {MAX_MONITORS} monitors"`.

That contradicts two rules this crate states about its own refusals, and the way its SIBLING emulator-chosen ceiling is stated:

1. Fourteen lines earlier, in the SAME handler, the conflict refusal carries a comment stating the principle the handler follows: a refusal "names nothing this emulator generated". The quota refusal names exactly that — the word "emulator" and the emulator-chosen number 500.
2. `state.rs`'s module doc owns "How a refusal this service authors is stated": either AWS's constraint-violation construction through the shared clause renderer (`stated_violation` / `stated_requirement`), or, for a refusal whose AWS sentence is known as a whole, that sentence. The quota message is neither.
3. The probe-count ceilings are EMULATOR-CHOSEN too (`MAX_PROBES_PER_MONITOR`'s doc says so explicitly, "for the reason given on [`MAX_MONITORS`]"), yet their refusal is a rule-determined sentence that "names neither the member nor the count" (`PROBES_PER_MONITOR_MESSAGE` = "Maximum probes per monitor limit exceeded."). So one of the two emulator-chosen ceilings leaks the emulator and its constant into the wire response and the other does not.

The status (402) and the error class are justified by `MAX_MONITORS`' own doc, so only the message text is the defect: a client that reads the refusal is told about the emulator rather than about the quota it exceeded.

## Evidence

The free-form message (services/service-com-amazonaws-networkmonitor-networkmonitor/src/ops/create_monitor.rs:366-376):
```
        if store.monitor_count() >= crate::state::MAX_MONITORS {
            return Err(CreateMonitorErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: Some(format!(
                        "this emulator keeps at most {} monitors",
                        crate::state::MAX_MONITORS
                    )),
```

The principle the SAME handler states for its other authored refusal (create_monitor.rs:353-356):
```
        if store.monitor(&region, &input.monitor_name).is_some() {
            // The conflict is stated from the request and the caller identity
            // alone (see `crate::state::monitor_conflict_message`), so it names
            // nothing this emulator generated.
```

The crate's rule for stating an authored refusal (src/state.rs:39-51):
```
//! ## How a refusal this service authors is stated
//!
//! A rule the model states in prose but its constraints cannot express is
//! applied by the service, so the service also has to STATE the refusal. It
//! states it the way AWS states a constraint failure — the
//! `N validation error(s) detected: Value '<submitted>' at '<member path>'
//! failed to satisfy constraint: <requirement>` construction, built from the
//! request — ... A refusal whose AWS sentence is known as a whole
//! ([`TCP_PORT_REQUIRED_MESSAGE`], [`aggregation_period_message`],
//! [`monitor_not_found_message`]) keeps that sentence instead.
```

The sibling emulator-chosen ceiling, stated WITHOUT naming the emulator or the count (src/state.rs:219-223 and 231-245):
```
/// The most probes this emulator keeps in ONE monitor. An EMULATOR-CHOSEN
/// ceiling, for the reason given on [`MAX_MONITORS`]; ...
pub const MAX_PROBES_PER_MONITOR: usize = 100;
...
/// A whole sentence rather than a constraint clause: the refusal is the one AWS
/// states for this limit, and it names neither the member nor the count, so it
/// reads the same whichever operation added the probes.
pub const PROBES_PER_SUBNET_MESSAGE: &str = "Maximum probes per subnet limit exceeded.";
...
pub const PROBES_PER_MONITOR_MESSAGE: &str = "Maximum probes per monitor limit exceeded.";
```

Nothing in the crate justifies or pins the divergence: `grep -n 'keeps at most|MAX_MONITORS|ServiceQuotaExceeded' tests/` returns no match, and the suite that owns authored-refusal wording enumerates its own exceptions and does not include this one (tests/all/authored_refusal_messages.rs:28-32):
```
//! What this suite does NOT cover is a refusal whose AWS sentence is known as a
//! whole rather than as a construction (`Destination port must be provided for
//! TCP protocol.`, `The aggregation period must be one of [30, 60].`, `<state>
//! is not a valid probe status.`): those keep their recorded sentence and are
//! pinned by `validation_message_wording` and `update_probe`.
```

## Steps to Reproduce

1. Start the emulator and create `crate::state::MAX_MONITORS` (500) monitors through `POST /monitors` (any distinct `monitorName`s; a single region is enough, since `monitor_count()` sums every region).
2. `POST /monitors` once more with a fresh `monitorName`.
3. Observe HTTP 402 `ServiceQuotaExceededException` whose `message` is `"this emulator keeps at most 500 monitors"` — a sentence naming the emulator and its own constant, which create_monitor.rs:354-356 says a refusal of this handler must not do.
4. Compare with the sibling emulator-chosen ceiling: add 101 probes to one monitor through `POST /monitors/{monitorName}/probes` and observe the refusal `"Maximum probes per monitor limit exceeded."`, which names neither the emulator nor the count.
