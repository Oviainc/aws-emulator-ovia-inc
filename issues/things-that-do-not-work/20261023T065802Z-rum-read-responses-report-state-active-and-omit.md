Issue-Source: trace-sweep-divergence
Issue-Trace: rum-app-monitor-data-query
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rum-app-monitor-data-query rum-app-monitor-domain-list-to-single-domain rum-app-monitor-lifecycle-with-tags rum-basic-app-monitor-lifecycle rum-basic-lifecycle-cw-logs rum-custom-events-and-deobfuscation rum-domain-list-configuration rum-error-duplicate-create-app-monitor rum-error-paths rum-get-app-monitor-data-workflow rum-multi-platform-monitors rum-multiple-app-monitors-isolation rum-update-app-monitor-replaces-domain-list rum-app-monitor-full-configuration rum-metrics-destination-cloudwatch rum-metrics-destination-idempotent-put rum-metrics-destination-lifecycle rum-multiple-monitors-destination-isolation
Issue-Shape-Id: com.amazonaws.rum#RUM
Issue-Key: rum|GetAppMonitor|2xx->2xx|$.AppMonitor.DeobfuscationConfiguration
Issue-Key: rum|GetAppMonitor|2xx->2xx|$.AppMonitor.LastModified
Issue-Key: rum|GetAppMonitor|2xx->2xx|$.AppMonitor.State
Issue-Key: rum|GetAppMonitor|2xx->2xx|$headers.content-type
Issue-Key: rum|GetAppMonitor|2xx->2xx|$.AppMonitor.DataStorage.CwLog.CwLogGroup
Issue-Key: rum|ListAppMonitors|2xx->2xx|$.AppMonitorSummaries[Name="sim2real-probe-rum-rum-monitor-c98bc9bd"].State
Issue-Key: rum|ListAppMonitors|2xx->2xx|$headers.content-type
Issue-Key: rum|ListAppMonitors|2xx->2xx|$.AppMonitorSummaries[Name="sim2real-probe-rum-rum-iso-a-11f15606"].LastModified
Issue-Key: rum|ListAppMonitors|2xx->2xx|$.AppMonitorSummaries[Name="sim2real-probe-rum-rum-iso-a-11f15606"].State
Issue-Key: rum|ListAppMonitors|2xx->2xx|$.AppMonitorSummaries[Name="sim2real-probe-rum-rum-iso-b-11f15606"].LastModified
Issue-Key: rum|ListAppMonitors|2xx->2xx|$.AppMonitorSummaries[Name="sim2real-probe-rum-rum-iso-b-11f15606"].State
Issue-Key: rum|ListRumMetricsDestinations|2xx->2xx|$.Destinations[*]
Issue-Related: 20261023T065802Z-rum-responses-set-content-type-application-json-instead.md
Issue-Commit: c036e6d6272f3d2084b4ffba5fd23a45ba05e284

# RUM read responses report State ACTIVE and omit modeled members

> _Note: this issue was identified against the code base at commit `c036e6d6272f3d2084b4ffba5fd23a45ba05e284`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAppMonitor

## Description
Four root causes in how the rum emulator projects stored resources into read responses. All four are in service rum; a fixer must close ALL FOUR before submitting. (Several of these clusters also report $headers.content-type; that is a separate, service-wide cause filed as its own issue.)

=== Cause 1 of 4: AppMonitor.State is reported as ACTIVE where RUM reports CREATED ===
AWS: Smithy: AppMonitor.State and AppMonitorSummary.State target StateEnum with values [CREATED, DELETING, ACTIVE]. Recorded behavior: an app monitor returned by GetAppMonitor and by ListAppMonitors reports State 'CREATED' — on a freshly created monitor, on a monitor that has been updated (UpdateAppMonitor), and on monitors created with CwLogEnabled, DomainList, custom events and tags. ACTIVE is never observed for these monitors in the corpus.
Emulator: returns State 'ACTIVE' for every app monitor, in both GetAppMonitor and ListAppMonitors.
Why: State is the field clients poll to decide a monitor is usable or being torn down. Code that waits for or asserts on CREATED (the state RUM reports for a monitor that is not deleting) never sees it against the emulator, and code that treats ACTIVE as a distinct, later lifecycle stage draws a conclusion AWS never supports.
Fix: the app-monitor state model / its projection in the GetAppMonitor and ListAppMonitors handlers — the stored lifecycle state must be the one RUM reports (CREATED), and the single value must be shared by the detail and summary projections.
Traces: rum-app-monitor-data-query#task#2, rum-app-monitor-domain-list-to-single-domain#task#4, rum-app-monitor-full-configuration#task#2, rum-basic-app-monitor-lifecycle#task#4 (GetAppMonitor); rum-basic-lifecycle-cw-logs#task#5, rum-multiple-app-monitors-isolation#task#3 (ListAppMonitors).

=== Cause 2 of 4: AppMonitor.DeobfuscationConfiguration is omitted where RUM always returns it ===
AWS: Smithy: AppMonitor.DeobfuscationConfiguration targets DeobfuscationConfiguration { JavaScriptSourceMaps }. Recorded behavior: GetAppMonitor always includes DeobfuscationConfiguration, and for a monitor created without any deobfuscation input it is returned as JavaScriptSourceMaps with Status 'DISABLED' and no S3Uri — i.e. RUM materializes the default sub-structure rather than leaving the member absent.
Emulator: omits AppMonitor.DeobfuscationConfiguration entirely.
Why: a client that reads AppMonitor.DeobfuscationConfiguration.JavaScriptSourceMaps.Status to decide whether source-map deobfuscation is on dereferences a missing member and either crashes or silently falls back, instead of reading the DISABLED default AWS returns.
Fix: the GetAppMonitor response projection (and the app-monitor record's default for deobfuscation configuration) — emit the member with its default JavaScriptSourceMaps/Status when the monitor has no explicit deobfuscation configuration.
Traces: rum-app-monitor-data-query#task#2, rum-app-monitor-domain-list-to-single-domain#task#4, rum-app-monitor-full-configuration#task#2, rum-basic-app-monitor-lifecycle#task#4.

=== Cause 3 of 4: LastModified is populated on a monitor that was never modified ===
AWS: Smithy: AppMonitor.LastModified and AppMonitorSummary.LastModified target ISOTimestampString and are documented as 'the date and time of the most recent changes to this app monitor's configuration'. Recorded behavior: for a monitor that has only ever been created, RUM returns no LastModified member at all (Created is present); the member appears only after a configuration change. The requirement is the presence rule, not any timestamp value — the recorded timestamps are environment-specific examples.
Emulator: always emits LastModified (e.g. '2026-10-23T06:49:15') on create-only monitors, in GetAppMonitor and in ListAppMonitors summaries.
Why: 'has this monitor been modified since creation?' is answered by the presence of LastModified; drift/change-detection tooling that keys on it sees every emulator monitor as modified.
Fix: the app-monitor record plus the Get/ListAppMonitors projections — set LastModified only on mutation (UpdateAppMonitor and the other configuration-changing operations) and omit it otherwise.
Traces: rum-app-monitor-data-query#task#2, rum-app-monitor-full-configuration#task#2 (GetAppMonitor); rum-multiple-app-monitors-isolation#task#3 (ListAppMonitors).

=== Cause 4 of 4: ListRumMetricsDestinations entries omit DestinationArn and IamRoleArn ===
AWS: Smithy: ListRumMetricsDestinations returns Destinations: MetricDestinationSummaryList of MetricDestinationSummary { Destination, DestinationArn, IamRoleArn }. Recorded behavior: for a CloudWatch destination RUM returns all three members, with DestinationArn and IamRoleArn present as empty strings (they carry values only for an Evidently destination).
Emulator: returns the summary with Destination only; DestinationArn and IamRoleArn are absent.
Why: a client that reads summary['DestinationArn']/['IamRoleArn'] (to decide whether a destination is Evidently-backed, or to display the role) hits a missing key against the emulator where AWS gives it a defined empty value.
Fix: the ListRumMetricsDestinations response serializer / the metrics-destination record — emit DestinationArn and IamRoleArn for every summary, empty for CloudWatch destinations.
Traces: rum-metrics-destination-cloudwatch#task#4 (cluster also covers rum-metrics-destination-idempotent-put, rum-metrics-destination-lifecycle, rum-multiple-monitors-destination-isolation).

## Evidence
Cause 1+2+3 — cluster service=rum|GetAppMonitor|status=200->200 (14 affected steps, representative rum-app-monitor-data-query#task#2):
  $.AppMonitor.DeobfuscationConfiguration: missing (expected {'JavaScriptSourceMaps': {'S3Uri': None, 'Status': 'DISABLED'}}, got None)
  $.AppMonitor.LastModified: type_mismatch (expected None, got '2026-10-23T06:49:15')
  $.AppMonitor.State: value_mismatch (expected 'CREATED', got 'ACTIVE')

Cause 1+2 — cluster rum-app-monitor-domain-list-to-single-domain#task#4 (5 affected steps): DeobfuscationConfiguration missing; State expected 'CREATED', got 'ACTIVE'.
Cause 1+2+3 — rum-app-monitor-full-configuration#task#2 and rum-basic-app-monitor-lifecycle#task#4: same keys. (These two clusters additionally report $.AppMonitor.DataStorage.CwLog.CwLogGroup against a captured variable {{cw_log_group}}; that is a generated log-group name and is NOT claimed as a requirement here.)

Cause 1+3 — cluster service=rum|ListAppMonitors|status=200->200, rum-basic-lifecycle-cw-logs#task#5:
  $.AppMonitorSummaries[Name="...-rum-monitor-c98bc9bd"].State: expected 'CREATED', got 'ACTIVE'
and rum-multiple-app-monitors-isolation#task#3 (two monitors):
  ...iso-a.LastModified: expected None, got '2026-10-23T06:50:30'; ...iso-a.State: expected 'CREATED', got 'ACTIVE' (same for iso-b).

Cause 4 — cluster service=rum|ListRumMetricsDestinations|status=200->200 (9 affected steps, representative rum-metrics-destination-cloudwatch#task#4):
  $.Destinations[0]: missing (expected {'Destination': 'CloudWatch', 'DestinationArn': '', 'IamRoleArn': ''}, got {'Destination': 'CloudWatch'})

## Steps to Reproduce
Boot the emulator and point a botocore rum client at it.
1) Causes 1-3: rum.create_app_monitor(Name='probe', Domain='example.com'); rum.get_app_monitor(Name='probe') -> AppMonitor.State == 'ACTIVE' (expected 'CREATED'), no 'DeobfuscationConfiguration' key (expected {'JavaScriptSourceMaps': {'Status': 'DISABLED'}}), and a 'LastModified' value present although the monitor was never updated.
2) Cause 1/3 in the summary projection: rum.list_app_monitors() -> the summary for 'probe' reports State 'ACTIVE' and a LastModified timestamp.
3) Cause 4: rum.put_rum_metrics_destination(AppMonitorName='probe', Destination='CloudWatch'); rum.list_rum_metrics_destinations(AppMonitorName='probe') -> Destinations == [{'Destination': 'CloudWatch'}] instead of [{'Destination': 'CloudWatch', 'DestinationArn': '', 'IamRoleArn': ''}].
Or replay the fixtures/steps listed in trace_ids.
