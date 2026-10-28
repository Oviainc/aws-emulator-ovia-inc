Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7e4efc65cc687607a3c1322cd06c2fb91d881b32

# A partially specified AppMonitorConfiguration/CustomEvents is stored verbatim, so the nested members' documented defaults (SessionSampleRate 0.1, Status DISABLED) are never settled

_Filed: 2026-10-28T15:20:43Z_
_Operation: UpdateAppMonitor | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7e4efc65cc687607a3c1322cd06c2fb91d881b32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate settles the documented defaults of `AppMonitorConfiguration`, `CustomEvents` and `DeobfuscationConfiguration` ONLY when the request omits the whole structure. When a request DOES carry the structure but leaves a member out, the structure is stored exactly as it arrived, so the member lands on the raw Smithy `@default` (0 for `SessionSampleRate`) or on nothing at all (`CustomEvents$Status` = None) instead of on the value the model documents for an omitted parameter.

Two concrete consequences:

* `CreateAppMonitor` with `AppMonitorConfiguration={"AllowCookies":true}` persists `session_sample_rate = 0.0`, and `GetAppMonitor`/`ListAppMonitors` then report a monitor that samples 0% of user sessions, where the member documents 0.1. `CreateAppMonitor` with `CustomEvents={}` persists `status = None`, so the read reports a `CustomEvents` structure with no `Status` where the member documents `DISABLED`.
* `UpdateAppMonitor` replaces the stored configuration wholesale (`record.app_monitor_configuration = configuration`), so an update that only means to flip `AllowCookies` or `EnableXRay` ALSO resets the monitor's configured sample rate to 0 — state the monitor was created with is lost, and the monitor is left reporting that it samples nothing. The same wholesale replacement zeroes `CustomEvents$Status` back to absent.

The crate already owns the correct constants and the projection that reports them (`DEFAULT_SESSION_SAMPLE_RATE`, `DEFAULT_CUSTOM_EVENTS_STATUS`, `default_app_monitor_configuration`, `default_custom_events`); they are simply not reached on the "structure present, member absent" path, and no handler fills the gap. The gap is admitted in a comment beside the constant, which cites the member's `@default` of 0 as the authority — the member's own documentation says the opposite.

## Evidence

Smithy, `AppMonitorConfiguration$SessionSampleRate` (`smithy_query(query="doc", shape="AppMonitorConfiguration", member="SessionSampleRate")`):
> "If you omit this parameter, the default of 0.1 is used, and 10% of sessions will be sampled."

Smithy, `CustomEvents$Status` (`smithy_query(query="shape", shape="CustomEvents")`):
> "Specifies whether this app monitor allows the web client to define and send custom events. The default is for custom events to be DISABLED."

The crate's own constant and the comment that admits the gap, `services/service-com-amazonaws-rum-rum/src/state.rs:100-107`:
```
/// `SessionSampleRate` of an app monitor whose create request named no
/// `AppMonitorConfiguration`.
///
/// From the model: `CreateAppMonitorRequest$AppMonitorConfiguration` documents "If you
/// omit this argument, the sample rate used for RUM is set to 10% of the user sessions."
/// (The member's own `@default` is `0`, which applies when the request DOES carry a
/// configuration but leaves the rate out.)
pub const DEFAULT_SESSION_SAMPLE_RATE: f64 = 0.1;
```

The defaults are reached only for a wholly absent structure — `services/service-com-amazonaws-rum-rum/src/state.rs:1225-1237`:
```
pub fn default_app_monitor_configuration() -> AppMonitorConfiguration {
    AppMonitorConfiguration {
        session_sample_rate: DEFAULT_SESSION_SAMPLE_RATE,
        ..AppMonitorConfiguration::default()
    }
}
...
pub fn default_custom_events() -> CustomEvents {
    CustomEvents {
        status: Some(DEFAULT_CUSTOM_EVENTS_STATUS.to_string()),
    }
}
```

Create path, `services/service-com-amazonaws-rum-rum/src/ops/create_app_monitor.rs:413-418` — the helper runs only on `None`, so a structure that IS present is stored unmodified:
```
            app_monitor_configuration: input
                .app_monitor_configuration
                .unwrap_or_else(crate::state::default_app_monitor_configuration),
            custom_events: input
                .custom_events
                .unwrap_or_else(crate::state::default_custom_events),
```

Update path, `services/service-com-amazonaws-rum-rum/src/ops/update_app_monitor.rs:332-343` — wholesale replacement, with no default settled on the new value:
```
        if let Some(configuration) = input.app_monitor_configuration {
            record.app_monitor_configuration = configuration;
            changed = true;
        }
...
        if let Some(custom_events) = input.custom_events {
            record.custom_events = custom_events;
            changed = true;
        }
```

Why an omitted member arrives as 0/None rather than as "absent" the handler could detect — `services/service-com-amazonaws-rum-rum/src/ops/types/shared_types.rs:61-77`:
```
pub struct AppMonitorConfiguration {
    ...
    pub session_sample_rate: f64,
```
```
pub struct CustomEvents {
    pub status: Option<String>,
}
```

What a read then reports, `services/service-com-amazonaws-rum-rum/src/state.rs:734-741` — the stored structure is projected verbatim:
```
            app_monitor_configuration: Some(self.app_monitor_configuration.clone()),
...
            custom_events: Some(self.custom_events.clone()),
```

Test coverage: both tests that pin the 0.1 default do so ONLY for a create that named no `AppMonitorConfiguration` at all — `tests/all/app_monitor_lifecycle.rs:292-294` (`// "If you omit this argument, the sample rate used for RUM is set to 10% of the user sessions."` / `("AppMonitorConfiguration.SessionSampleRate", json!(0.1))`) and `tests/all/platform_domain_authority.rs:204-210` (`assert_eq!(monitor["AppMonitorConfiguration"]["SessionSampleRate"].as_f64(), Some(DEFAULT_SESSION_SAMPLE_RATE), ...)`). Every other `SessionSampleRate` row sends the member explicitly (`app_monitor_lifecycle.rs:91`, `:546`, `:607-609`, `announced_media_type.rs:519`), so no existing test states a reason for the behaviour above and none would change if the nested defaults were settled.

## Steps to Reproduce

1. CreateAppMonitor(Name="m1", Domain="example.com", AppMonitorConfiguration={"AllowCookies": true}) — 200
2. GetAppMonitor(Name="m1") — observed AppMonitor.AppMonitorConfiguration.SessionSampleRate = 0 (the member's raw @default, reported as 0 or omitted); expected 0.1, because the member documents "If you omit this parameter, the default of 0.1 is used"
3. CreateAppMonitor(Name="m2", Domain="example.com", CustomEvents={}) then GetAppMonitor(Name="m2") — observed CustomEvents reported with no Status; expected Status="DISABLED", because the member documents "The default is for custom events to be DISABLED"
4. CreateAppMonitor(Name="m3", Domain="example.com", AppMonitorConfiguration={"SessionSampleRate": 0.5})
5. UpdateAppMonitor(Name="m3", AppMonitorConfiguration={"AllowCookies": true}) — 200
6. GetAppMonitor(Name="m3") — observed SessionSampleRate = 0: the 0.5 the monitor was created with is gone and the monitor now reports that it samples no sessions; expected the documented 0.1 for the omitted parameter (and in no case 0)
