Issue-Source: trace-sweep-divergence
Issue-Trace: rum-app-monitor-and-metrics-error-paths
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rum-app-monitor-and-metrics-error-paths rum-app-monitor-error-paths rum-error-duplicate-create-app-monitor rum-error-paths rum-metrics-destination-error-paths rum-error-notfound-operations rum-delete-then-verify-gone rum-error-invalid-excluded-page-pattern rum-multiple-app-monitors-isolation rum-resource-policy-nonexistent-app-monitor rum-resource-policy-operations-on-missing-appmonitor
Issue-Shape-Id: com.amazonaws.rum#RUM
Issue-Key: rum|CreateAppMonitor|4xx->4xx|$.message
Issue-Key: rum|CreateAppMonitor|4xx->4xx|$.resourceType
Issue-Key: rum|PutRumMetricsDestination|4xx->4xx|$.message
Issue-Key: rum|PutRumMetricsDestination|4xx->4xx|$.resourceName
Issue-Key: rum|PutRumMetricsDestination|4xx->4xx|$.resourceType
Issue-Key: rum|ListRumMetricsDestinations|4xx->4xx|$.message
Issue-Key: rum|ListRumMetricsDestinations|4xx->4xx|$.resourceName
Issue-Key: rum|ListRumMetricsDestinations|4xx->4xx|$.resourceType
Issue-Key: rum|ListRumMetricsDestinations|4xx->4xx|$headers.content-type
Issue-Key: rum|DeleteAppMonitor|4xx->4xx|$.message
Issue-Key: rum|DeleteAppMonitor|4xx->4xx|$.resourceName
Issue-Key: rum|DeleteAppMonitor|4xx->4xx|$.resourceType
Issue-Key: rum|DeleteAppMonitor|4xx->4xx|$headers.content-type
Issue-Key: rum|GetAppMonitor|4xx->4xx|$.message
Issue-Key: rum|GetAppMonitor|4xx->4xx|$.resourceName
Issue-Key: rum|GetAppMonitor|4xx->4xx|$.resourceType
Issue-Key: rum|GetAppMonitor|4xx->4xx|$headers.content-type
Issue-Key: rum|UpdateAppMonitor|4xx->4xx|$.message
Issue-Key: rum|UpdateAppMonitor|4xx->4xx|$.resourceName
Issue-Key: rum|UpdateAppMonitor|4xx->4xx|$.resourceType
Issue-Key: rum|PutResourcePolicy|4xx->4xx|$.message
Issue-Key: rum|PutResourcePolicy|4xx->4xx|$.resourceName
Issue-Key: rum|PutResourcePolicy|4xx->4xx|$.resourceType
Issue-Key: rum|GetResourcePolicy|4xx->4xx|$.message
Issue-Key: rum|GetResourcePolicy|4xx->4xx|$.resourceName
Issue-Key: rum|GetResourcePolicy|4xx->4xx|$.resourceType
Issue-Key: rum|GetResourcePolicy|4xx->4xx|$headers.content-type
Issue-Key: rum|DeleteResourcePolicy|4xx->4xx|$.message
Issue-Key: rum|DeleteResourcePolicy|4xx->4xx|$.resourceName
Issue-Key: rum|DeleteResourcePolicy|4xx->4xx|$.resourceType
Issue-Key: rum|DeleteResourcePolicy|4xx->4xx|$headers.content-type
Issue-Commit: c036e6d6272f3d2084b4ffba5fd23a45ba05e284

# RUM not-found and conflict errors use wrong message and add resourceName

> _Note: this issue was identified against the code base at commit `c036e6d6272f3d2084b4ffba5fd23a45ba05e284`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAppMonitor

## Description
Nine clusters, two shared faults in the rum emulator's app-monitor error payloads: the request-derived message text, and two members RUM leaves off the wire. Causes are grouped per operation below because each is a separate handler path reported by its own cluster; a fixer must close ALL NINE before submitting. HTTP status and error code are already correct everywhere here (409 ConflictException, 404 ResourceNotFoundException). Related open issue: '20261022T205048Z-rum-error-representation-and-input-validation-diverge-conflict.md' describes the same message rules from the BEHAVIORAL suite, but it carries no wire replay key, so these wire clusters are filed here; close them together.

Shared fault A (message rule): RUM builds these messages from the request. Recorded behavior, agreeing across 9 clusters, many operations and several name prefixes: a conflict is 'Resource <Name> already exists' and a not-found is 'Resource not found <Name>' — literal prefix, the request's Name member verbatim, no trailing period. The emulator instead builds 'App monitor <Name> already exists.' / 'App monitor <Name> does not exist.'. Because the construction rule (and not one recorded string) is what the recordings establish, the message is a requirement here.
Shared fault B (extra members): Smithy models ResourceNotFoundException and ConflictException as { message (required), resourceName (required), resourceType }, but RUM does NOT put resourceName/resourceType on the wire for these errors — every recorded error body contains `message` only. The emulator additionally serializes resourceName=<Name> and resourceType='AppMonitor' (and resourceType='RUM application' is what the recording shows for the one conflict case where RUM does emit it). An emulator response therefore has members the AWS response does not.

=== Cause 1 of 9: CreateAppMonitor duplicate name — ConflictException message and resourceType ===
AWS: Smithy: CreateAppMonitor errors include ConflictException (httpError 409); input Name is @required AppMonitorName. Recorded: 409, message 'Resource <Name> already exists', resourceType 'RUM application'.
Emulator: 409 with message 'App monitor <Name> already exists.' and resourceType 'AppMonitor'.
Why: a client matching the documented conflict message (or parsing the resource name out of it, or switching on resourceType) misclassifies a real name collision.
Fix: the CreateAppMonitor handler's conflict path — message construction and the resourceType value.
Traces: rum-app-monitor-and-metrics-error-paths#task#2 (cluster also covers rum-app-monitor-error-paths#task#2, rum-error-duplicate-create-app-monitor#task#2, rum-error-paths#task#2).

=== Cause 2 of 9: PutRumMetricsDestination on a missing monitor ===
AWS: Smithy: PutRumMetricsDestination models ResourceNotFoundException (404). Recorded: 404, body carries message 'Resource not found <AppMonitorName>' only.
Emulator: 404 with 'App monitor <name> does not exist.' plus resourceName and resourceType members.
Why: see shared faults A and B.
Fix: the shared app-monitor lookup helper used by the metrics-destination handlers (message construction + not emitting resourceName/resourceType).
Traces: rum-app-monitor-and-metrics-error-paths#task#3 (also rum-metrics-destination-error-paths#task#1).

=== Cause 3 of 9: ListRumMetricsDestinations on a missing monitor ===
AWS: Smithy: ListRumMetricsDestinations models ResourceNotFoundException (404). Recorded: 404, message 'Resource not found <AppMonitorName>' only.
Emulator: 404 with the 'App monitor ... does not exist.' message plus resourceName/resourceType.
Why/Fix: as cause 2, in the ListRumMetricsDestinations handler's lookup path.
Traces: rum-app-monitor-and-metrics-error-paths#task#4 (also rum-metrics-destination-error-paths#task#2).

=== Cause 4 of 9: DeleteAppMonitor on a missing monitor ===
AWS: Smithy: DeleteAppMonitor models ResourceNotFoundException (404); Name is an @httpLabel. Recorded: 404, message 'Resource not found <Name>' only.
Emulator: 404 with 'App monitor <Name> does not exist.' plus resourceName/resourceType.
Why/Fix: as cause 2, in the DeleteAppMonitor handler's lookup path.
Traces: rum-app-monitor-and-metrics-error-paths#task#6 (also rum-app-monitor-error-paths#task#3, rum-error-notfound-operations#task#3, rum-error-paths#task#6).

=== Cause 5 of 9: GetAppMonitor on a missing or already-deleted monitor ===
AWS: Smithy: GetAppMonitor models ResourceNotFoundException (404). Recorded: 404, message 'Resource not found <Name>' only.
Emulator: 404 with 'App monitor <Name> does not exist.' plus resourceName/resourceType.
Why/Fix: as cause 2, in the GetAppMonitor handler's lookup path.
Traces: rum-app-monitor-error-paths#task#4 (cluster covers 8 steps, incl. rum-delete-then-verify-gone#task#3, rum-error-notfound-operations#task#1, rum-error-paths#task#4/#teardown#9, rum-multiple-app-monitors-isolation#teardown#8/#9).

=== Cause 6 of 9: UpdateAppMonitor on a missing monitor ===
AWS: Smithy: UpdateAppMonitor models ResourceNotFoundException (404). Recorded: 404, message 'Resource not found <Name>' only.
Emulator: 404 with 'App monitor <Name> does not exist.' plus resourceName/resourceType.
Why/Fix: as cause 2, in the UpdateAppMonitor handler's lookup path.
Traces: rum-error-notfound-operations#task#2 (also rum-error-paths#task#5).

=== Cause 7 of 9: PutResourcePolicy on a missing monitor ===
AWS: Smithy: PutResourcePolicy (PUT /appmonitor/{Name}/policy) models ResourceNotFoundException (404). Recorded: 404, message 'Resource not found <Name>' only.
Emulator: 404 with 'App monitor <Name> does not exist.' plus resourceName/resourceType.
Why/Fix: as cause 2, in the PutResourcePolicy handler's app-monitor resolution path.
Traces: rum-resource-policy-nonexistent-app-monitor#task#1 (also rum-resource-policy-operations-on-missing-appmonitor#task#2).

=== Cause 8 of 9: GetResourcePolicy on a missing monitor ===
AWS: Smithy: GetResourcePolicy models ResourceNotFoundException (404) distinctly from PolicyNotFoundException (404). Recorded: 404, message 'Resource not found <Name>' only.
Emulator: 404 with 'App monitor <Name> does not exist.' plus resourceName/resourceType.
Why/Fix: as cause 2, in the GetResourcePolicy handler's app-monitor resolution path.
Traces: rum-resource-policy-nonexistent-app-monitor#task#2 (also rum-resource-policy-operations-on-missing-appmonitor#task#1).

=== Cause 9 of 9: DeleteResourcePolicy on a missing monitor ===
AWS: Smithy: DeleteResourcePolicy models ResourceNotFoundException (404). Recorded: 404, message 'Resource not found <Name>' only.
Emulator: 404 with 'App monitor <Name> does not exist.' plus resourceName/resourceType.
Why/Fix: as cause 2, in the DeleteResourcePolicy handler's app-monitor resolution path.
Traces: rum-resource-policy-nonexistent-app-monitor#task#3 (also rum-resource-policy-operations-on-missing-appmonitor#task#3).

## Evidence
Cause 1 — service=rum|CreateAppMonitor|status=409->409 (4 steps, rep rum-app-monitor-and-metrics-error-paths#task#2):
  $.message: expected 'Resource sim2real-probe-rum-rum-err-7b040ac8 already exists', got 'App monitor sim2real-probe-rum-rum-err-7b040ac8 already exists.'
  $.resourceType: expected 'RUM application', got 'AppMonitor'

Cause 2 — service=rum|PutRumMetricsDestination|status=404->404 (2 steps):
  $.message: expected 'Resource not found sim2real-probe-rum-rum-missing-7b040ac8', got 'App monitor ... does not exist.'
  $.resourceName: type_mismatch (expected None, got 'sim2real-probe-rum-rum-missing-7b040ac8')
  $.resourceType: type_mismatch (expected None, got 'AppMonitor')

Cause 3 — service=rum|ListRumMetricsDestinations|status=404->404 (2 steps): same three keys (+content-type, filed separately).
Cause 4 — service=rum|DeleteAppMonitor|status=404->404 (4 steps): same three keys.
Cause 5 — service=rum|GetAppMonitor|status=404->404 (8 steps): expected 'Resource not found sim2real-probe-rum-rum-missing-app-1ac8a841', got 'App monitor ... does not exist.'; resourceName/resourceType expected absent.
Cause 6 — service=rum|UpdateAppMonitor|status=404->404 (2 steps): same three keys.
Causes 7/8/9 — service=rum|PutResourcePolicy|GetResourcePolicy|DeleteResourcePolicy|status=404->404 (2 steps each): expected 'Resource not found sim2real-probe-rum-rum-missing-monitor-0ccb29b1', got 'App monitor ... does not exist.'; resourceName expected absent, got the name; resourceType expected absent, got 'AppMonitor'.

In all nine clusters the HTTP status matched (409 / 404) and only the body keys above diverged.

## Steps to Reproduce
Boot the emulator and point a botocore rum client at it.
1) Cause 1: rum.create_app_monitor(Name='dup', Domain='example.com') twice -> second call ConflictException/409 with Message 'App monitor dup already exists.' and resourceType 'AppMonitor'; AWS: 'Resource dup already exists' with resourceType 'RUM application'.
2) Causes 2-9: against a never-created name 'missing', call rum.get_app_monitor(Name='missing'), rum.update_app_monitor(Name='missing'), rum.delete_app_monitor(Name='missing'), rum.put_rum_metrics_destination(AppMonitorName='missing', Destination='CloudWatch'), rum.list_rum_metrics_destinations(AppMonitorName='missing'), rum.put_resource_policy(Name='missing', PolicyDocument='{}'), rum.get_resource_policy(Name='missing'), rum.delete_resource_policy(Name='missing').
   Each returns 404 ResourceNotFoundException with message 'App monitor missing does not exist.' and extra resourceName/resourceType members; AWS returns message 'Resource not found missing' and no resourceName/resourceType.
Or replay the fixtures/steps listed in trace_ids.
