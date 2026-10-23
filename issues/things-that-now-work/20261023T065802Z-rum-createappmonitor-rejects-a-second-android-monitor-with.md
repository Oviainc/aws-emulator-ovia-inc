Issue-Source: trace-sweep-divergence
Issue-Trace: rum-multi-platform-monitors
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rum-multi-platform-monitors
Issue-Shape-Id: com.amazonaws.rum#RUM
Issue-Key: rum|CreateAppMonitor|2xx->4xx|$.Id
Issue-Key: rum|CreateAppMonitor|2xx->4xx|status
Issue-Key: rum|GetAppMonitor|2xx->4xx|$.AppMonitor
Issue-Key: rum|GetAppMonitor|2xx->4xx|$headers.content-type
Issue-Key: rum|GetAppMonitor|2xx->4xx|status
Issue-Key: rum|ListAppMonitors|2xx->2xx|$.AppMonitorSummaries[Name="sim2real-probe-rum-rum-android-dd432c0c"]
Issue-Key: rum|ListAppMonitors|2xx->2xx|$.AppMonitorSummaries[Name="sim2real-probe-rum-rum-web-dd432c0c"].LastModified
Issue-Key: rum|ListAppMonitors|2xx->2xx|$.AppMonitorSummaries[Name="sim2real-probe-rum-rum-web-dd432c0c"].State
Issue-Key: rum|DeleteAppMonitor|2xx->4xx|status
Issue-Related: 20261023T065802Z-rum-read-responses-report-state-active-and-omit.md
Issue-Commit: c036e6d6272f3d2084b4ffba5fd23a45ba05e284

# RUM CreateAppMonitor rejects a second Android monitor with 400

> _Note: this issue was identified against the code base at commit `c036e6d6272f3d2084b4ffba5fd23a45ba05e284`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAppMonitor

## Description
Four clusters from one fixture: a CreateAppMonitor call AWS accepts is refused with HTTP 400, and the three downstream steps that depend on the created monitor then diverge too. All four are in service rum; a fixer must close ALL FOUR before submitting — fixing cause 1 is necessary but the three dependent clusters must be re-replayed to confirm they pass.

=== Cause 1 of 4: CreateAppMonitor for a non-Web Platform is rejected with HTTP 400 instead of creating the monitor ===
AWS: Smithy: CreateAppMonitor (POST /appmonitor) input member Platform targets AppMonitorPlatform, an enum with values [Web, Android, iOS], documented 'Web for web applications, Android for Android applications, iOS for IOS applications'; AppMonitor.Platform and AppMonitorSummary.Platform carry it back. ValidationException (400) is modeled only for constraint violations. Recorded behavior: creating a second app monitor on the same account with a distinct Name and Platform 'Android' succeeds with HTTP 200 and an Id, and the monitor is subsequently returned by GetAppMonitor with Platform 'Android', State 'CREATED' and the default DataStorage/CustomEvents/Deobfuscation structure.
Emulator: answers HTTP 400 with no Id, and no monitor is created.
Why: a client cannot register Android or iOS app monitors at all — the whole mobile-RUM path is unreachable — and it receives a 400 for an input the model declares valid. Hypothesis: the CreateAppMonitor handler accepts only Platform 'Web' (or rejects the Platform member outright) rather than validating it against the AppMonitorPlatform enum; the fixture's preceding Web monitor in the same account was created successfully, so neither the name nor an account-level limit explains the refusal.
Fix: the CreateAppMonitor handler's input validation and app-monitor record — accept every AppMonitorPlatform enum value, store it, and reject only values outside the enum (with ValidationException/400).
Traces: rum-multi-platform-monitors#task#2.

=== Cause 2 of 4: GetAppMonitor on the Android monitor returns 404 instead of the monitor ===
AWS: Smithy: GetAppMonitor (GET /appmonitor/{Name}) returns AppMonitor; ResourceNotFoundException (404) is for a name that does not exist. Recorded: the Android monitor is returned with Platform 'Android', State 'CREATED', Created set, LastModified absent, CustomEvents.Status 'DISABLED', DataStorage.CwLog.CwLogEnabled false and the default DeobfuscationConfiguration.
Emulator: HTTP 404 with no AppMonitor, because cause 1 never created the monitor.
Why: a client that creates a mobile app monitor and then reads it back sees a missing resource; this is the observable consequence of cause 1 and is how a caller first notices the create silently did nothing.
Fix: resolved by cause 1 (the CreateAppMonitor handler); re-replay this step to confirm the monitor is readable with Platform 'Android'.
Traces: rum-multi-platform-monitors#task#4.

=== Cause 3 of 4: ListAppMonitors omits the Android monitor ===
AWS: Smithy: ListAppMonitors (POST /appmonitors) returns AppMonitorSummaries of AppMonitorSummary { Name, Id, Created, LastModified, State, Platform }. Recorded: both monitors are listed, the Android one with Platform 'Android' and State 'CREATED'.
Emulator: the Android summary is absent entirely (the Web one is listed, with the State/LastModified divergences filed under the separate read-projection issue).
Why: inventory/enumeration code does not see mobile app monitors at all against the emulator.
Fix: resolved by cause 1; re-replay to confirm the summary appears.
Traces: rum-multi-platform-monitors#task#5.

=== Cause 4 of 4: DeleteAppMonitor of the Android monitor returns 404 instead of 200 ===
AWS: Smithy: DeleteAppMonitor (DELETE /appmonitor/{Name}) answers 200 for an existing monitor. Recorded: the teardown delete of the Android monitor succeeds with 200.
Emulator: HTTP 404 — nothing to delete, again because of cause 1.
Why: teardown/cleanup logic that treats a 404 on delete as an error aborts, and tests that assert a successful delete fail.
Fix: resolved by cause 1; re-replay to confirm the delete answers 200.
Traces: rum-multi-platform-monitors#teardown#7.

## Evidence
Cause 1 — service=rum|CreateAppMonitor|status=200->400|keys=$.Id,status (rum-multi-platform-monitors#task#2):
  status: expected 200, got 400
  $.Id: missing (expected the captured {{android_monitor_id}}, got None)
  The preceding CreateAppMonitor in the same fixture (the Web monitor, task#1) passed, so only the Android create is refused.

Cause 2 — service=rum|GetAppMonitor|status=200->404|keys=$.AppMonitor,status (rum-multi-platform-monitors#task#4):
  status: expected 200, got 404
  $.AppMonitor: missing (expected {'AppMonitorConfiguration': {'SessionSampleRate': 0.1, ...}, 'CustomEvents': {'Status': 'DISABLED'}, 'DataStorage': {'CwLog': {'CwLogEnabled': False}}, 'DeobfuscationConfiguration': {'JavaScriptSourceMaps': {'Status': 'DISABLED'}}, 'Name': 'sim2real-probe-rum-rum-android-dd432c0c', 'Platform': 'Android', 'State': 'CREATED', 'Tags': {}}, got None)

Cause 3 — service=rum|ListAppMonitors|status=200->200 (rum-multi-platform-monitors#task#5):
  $.AppMonitorSummaries[Name="sim2real-probe-rum-rum-android-dd432c0c"]: missing (expected {'Id': ..., 'Name': ..., 'Platform': 'Android', 'State': 'CREATED'}, got None)

Cause 4 — service=rum|DeleteAppMonitor|status=200->404|keys=status (rum-multi-platform-monitors#teardown#7):
  status: expected 200, got 404

## Steps to Reproduce
Boot the emulator and point a botocore rum client at it.
1) rum.create_app_monitor(Name='probe-web', Domain='example.com') -> succeeds (200).
2) Cause 1: rum.create_app_monitor(Name='probe-android', Domain='example.com', Platform='Android') -> HTTP 400 with no Id; AWS returns 200 with an Id. (Try Platform='iOS' as well.)
3) Cause 2: rum.get_app_monitor(Name='probe-android') -> ResourceNotFoundException/404; AWS returns the monitor with Platform 'Android'.
4) Cause 3: rum.list_app_monitors() -> no summary for 'probe-android'.
5) Cause 4: rum.delete_app_monitor(Name='probe-android') -> 404; AWS returns 200.
Or replay fixture rum-multi-platform-monitors (task#2, task#4, task#5, teardown#7).
