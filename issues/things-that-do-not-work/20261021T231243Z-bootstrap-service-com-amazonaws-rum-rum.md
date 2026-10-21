Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 0761ec9a912f1ad457725ac2bfa14369ca92ad04

# Bootstrap the rum emulator

_Filed: 2026-10-21T23:12:43Z_
_Operation: service-com-amazonaws-rum-rum | Class: missing_

> _Note: this issue was identified against the code base at commit `0761ec9a912f1ad457725ac2bfa14369ca92ad04`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 20 operations in `services/service-com-amazonaws-rum-rum/src/ops/` are unmodified scaffolds (`//! SCAFFOLDED, NOT IMPLEMENTED:` -> HTTP 501) and the crate holds no state. Implement a FIRST SLICE: the app-monitor lifecycle plus one telemetry call, all computed from persisted state in `ComAmazonawsRumRumState` (`src/state.rs`), reached via `crate::state::service_state(&*self.engine)`. No canned, echoed, or hardcoded responses; no random ID without storage.

Happy-path operations, in order:

1. **CreateAppMonitor** — `POST /appmonitor`. Input: `Name` (required, 1..255, `^(?!\.)[\.\-_#A-Za-z0-9]+$`), `Domain`/`DomainList`, `Tags`, `AppMonitorConfiguration`, `CwLogEnabled`, `CustomEvents`, `DeobfuscationConfiguration`, `Platform` (Android|Web|iOS). Output: `Id` — a freshly generated UUIDv4 (36 chars, must match the model's UUID pattern) that is STORED and later returned by GetAppMonitor. Creating a second monitor with an existing `Name` must fail `ConflictException` (409). Record `Created` and `LastModified` as 19-char ISO timestamp strings (`ISOTimestampString`, e.g. `2024-01-01T00:00:00`) and `State` = `ACTIVE`.
2. **GetAppMonitor** — `GET /appmonitor/{Name}`. Output `AppMonitor` must be rebuilt from stored state: `Name`, `Id`, `Domain`/`DomainList`, `Created`, `LastModified`, `Tags`, `State`, `AppMonitorConfiguration`, `CustomEvents`, `DeobfuscationConfiguration`, `Platform`, `DataStorage` (CwLog.CwLogEnabled/CwLogGroup reflecting the created `CwLogEnabled`). Unknown name -> `ResourceNotFoundException` (404).
3. **ListAppMonitors** — `POST /appmonitors`. Output `AppMonitorSummaries` = one `AppMonitorSummary{Name, Id, Created, LastModified, State, Platform}` per stored monitor, honouring `MaxResults` (1..100) with a real opaque `NextToken` that resumes where the previous page ended; omit `NextToken` on the last page.
4. **PutRumEvents** — `POST /appmonitors/{Id}/`. Input: `Id` (the monitor id returned by CreateAppMonitor), `BatchId`, `AppMonitorDetails`, `UserDetails`, `RumEvents`, optional `Alias`. Must look the monitor up BY STORED ID and 404 (`ResourceNotFoundException`) for an unknown id; on success append the events to that monitor's event buffer and return an empty 200 body.
5. **UpdateAppMonitor** — `PATCH /appmonitor/{Name}`. Apply only the supplied fields (`Domain`, `DomainList`, `AppMonitorConfiguration`, `CwLogEnabled`, `CustomEvents`, `DeobfuscationConfiguration`) to the stored monitor, bump `LastModified`, leave `Id`/`Created` unchanged; unknown name -> 404. A following GetAppMonitor must show the new values.
6. **DeleteAppMonitor** — `DELETE /appmonitor/{Name}`. Remove the monitor (and its events/tags) from state; a following GetAppMonitor and a PutRumEvents with its old `Id` must both return `ResourceNotFoundException` (404). Deleting an unknown name -> 404.

## State Model

```
ComAmazonawsRumRumState {
    // primary index: monitor name -> monitor
    app_monitors: HashMap<String /*Name*/, AppMonitorRecord>,
    // secondary index so PutRumEvents can resolve {Id} -> Name
    monitor_ids: HashMap<String /*Id (uuid)*/, String /*Name*/>,
}

AppMonitorRecord {
    name: String,
    id: String,                       // uuid, 36 chars, matches AppMonitorId pattern
    arn: String,                      // arn:aws:rum:<region>:<account>:appmonitor/<name>
    domain: Option<String>,
    domain_list: Option<Vec<String>>,
    created: String,                  // 19-char ISO timestamp
    last_modified: String,            // 19-char ISO timestamp
    state: String,                    // StateEnum: CREATING|DELETING|ACTIVE
    platform: Option<String>,         // Android|Web|iOS
    cw_log_enabled: bool,
    app_monitor_configuration: Option<AppMonitorConfiguration>,
    custom_events: Option<CustomEvents>,
    deobfuscation_configuration: Option<DeobfuscationConfiguration>,
    tags: BTreeMap<String, String>,
    events: Vec<StoredRumEvent { batch_id, alias, event_json }>,  // filled by PutRumEvents
}
```

Pagination tokens for ListAppMonitors should encode a stable offset/last-key into the ordered monitor list, validated on the way back in (bad token -> `ValidationException` 400).

Everything else (BatchCreate/BatchGet/BatchDelete/UpdateRumMetricDefinition, PutRumMetricsDestination, ListRumMetricsDestinations, DeleteRumMetricsDestination, Put/Get/DeleteResourcePolicy, GetAppMonitorData, Tag/Untag/ListTagsForResource) stays 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-rum-rum/OPERATIONS.md` lists all 20 operations with routes, members and errors: CreateAppMonitor (`POST /appmonitor`, output `Id` with UUID pattern, `ConflictException`), GetAppMonitor (`GET /appmonitor/{Name}`, output `AppMonitor`, `ResourceNotFoundException`), ListAppMonitors (`POST /appmonitors`, @paginated items=AppMonitorSummaries, MaxResults 1..100), PutRumEvents (`POST /appmonitors/{Id}/`, required BatchId/AppMonitorDetails/UserDetails/RumEvents, `ResourceNotFoundException`), UpdateAppMonitor (`PATCH /appmonitor/{Name}`), DeleteAppMonitor (`DELETE /appmonitor/{Name}`). `smithy_query shape=AppMonitor,AppMonitorSummary` gives the exact output fields (Name, Id, Created, LastModified, Tags, State, DataStorage, Platform, ...). All 20 handler files under `src/ops/` still carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker (grep, line 4 of each), so every call answers HTTP 501 and no state exists; `README.md` documents the restJson1 protocol and the shared-engine state API the handlers must use.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `rum` client at it.
2. `aws rum create-app-monitor --name demo-monitor --domain example.com --cw-log-enabled --tags Team=web` -> expect 200 with an `Id` that is a valid UUID (currently 501).
3. `aws rum get-app-monitor --name demo-monitor` -> expect the AppMonitor with the SAME `Id`, `Domain=example.com`, `State=ACTIVE`, `Tags={Team:web}`, `DataStorage.CwLog.CwLogEnabled=true`, and a 19-char `Created`/`LastModified`.
4. `aws rum list-app-monitors --max-results 1` -> expect exactly one summary for demo-monitor with matching Name/Id and, if more monitors exist, a usable `NextToken` that returns the next distinct monitor.
5. POST RUM events to `/appmonitors/<Id>/` with BatchId/AppMonitorDetails/UserDetails/RumEvents -> expect 200; the same POST to a random UUID -> expect 404 ResourceNotFoundException.
6. `aws rum update-app-monitor --name demo-monitor --no-cw-log-enabled --domain other.example.com` then get-app-monitor -> expect the updated Domain/CwLogEnabled, unchanged `Id`/`Created`, and a bumped `LastModified`.
7. `aws rum delete-app-monitor --name demo-monitor`, then get-app-monitor and PutRumEvents with the old Id -> expect 404 ResourceNotFoundException from both.
