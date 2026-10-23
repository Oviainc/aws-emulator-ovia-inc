Issue-Source: trace-sweep-divergence
Issue-Trace: rum-delete-then-verify-gone
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rum-delete-then-verify-gone rum-error-paths rum-metrics-destination-cloudwatch rum-metrics-destination-lifecycle rum-multiple-monitors-destination-isolation
Issue-Shape-Id: com.amazonaws.rum#RUM
Issue-Key: rum|DeleteAppMonitor|2xx->2xx|$headers.content-type
Issue-Key: rum|ListRumMetricsDestinations|2xx->2xx|$headers.content-type
Issue-Commit: c036e6d6272f3d2084b4ffba5fd23a45ba05e284

# RUM responses set content-type application/json instead of application/x-amz-json-1.1

> _Note: this issue was identified against the code base at commit `c036e6d6272f3d2084b4ffba5fd23a45ba05e284`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAppMonitor

## Description
One root cause, reported by 2 clusters whose ONLY divergence is the response content-type, and additionally present as an extra key in ~20 other RUM clusters (those are filed under their own root causes). A fixer must close this single cause.

=== Cause 1 of 1: every RUM response carries content-type application/json instead of application/x-amz-json-1.1 ===
AWS: Smithy: service com.amazonaws.rum#RUM, protocol restJson1, operations DeleteAppMonitor (DELETE /appmonitor/{Name}) and ListRumMetricsDestinations (GET /rummetrics/{AppMonitorName}/metricsdestination). The model does not pin a response content-type, so this is recorded behavior only: across the whole corpus every recorded RUM response — success and error, every operation — is sent with `content-type: application/x-amz-json-1.1`. The requirement is the header value RUM actually puts on the wire for its JSON bodies, not the payload (the bodies themselves match on these two steps).
Emulator: answers with `content-type: application/json` on every response.
Why: a client that selects a parser or validates the media type from the response content-type (strict HTTP intermediaries, recorders/proxies, cassette-replay harnesses, anything asserting on the AWS media type) sees a different media type against the emulator than against AWS, and any recorded-traffic comparison of an emulator response against an AWS response fails on the header even when the body is byte-identical.
Why it is one cause and not many: the value is uniform across all operations and both success and error paths, so it is emitted by the shared response serializer, not per handler.
Fix: the service's shared JSON response serializer / HTTP response builder for the rum crate (the place that sets the content-type header for both success and error responses), not the individual operation handlers.
Traces: rum-delete-then-verify-gone#task#2 (DeleteAppMonitor 200, content-type the only diverging key), rum-metrics-destination-cloudwatch#task#2 (ListRumMetricsDestinations 200, content-type the only diverging key).

## Evidence
Cluster service=rum|DeleteAppMonitor|status=200->200|keys=$headers.content-type (2 affected steps: rum-delete-then-verify-gone#task#2, rum-error-paths#teardown#8):
  $headers.content-type: value_mismatch (expected 'application/x-amz-json-1.1', got 'application/json')
  status matched (200), body matched.

Cluster service=rum|ListRumMetricsDestinations|status=200->200|keys=$headers.content-type (3 affected steps: rum-metrics-destination-cloudwatch#task#2, rum-metrics-destination-lifecycle#task#2, rum-multiple-monitors-destination-isolation#task#5):
  $headers.content-type: value_mismatch (expected 'application/x-amz-json-1.1', got 'application/json')

The same $headers.content-type key appears in roughly 20 further rum clusters (GetAppMonitor, ListAppMonitors, Get/Delete/PutResourcePolicy, 404 error bodies), confirming it is emitted service-wide.

## Steps to Reproduce
1) Boot the emulator and create an app monitor: POST /appmonitor {"Name":"probe","Domain":"example.com"}.
2) curl -i -X DELETE 'http://<emulator>/appmonitor/probe' and inspect the response headers -> `content-type: application/json`; AWS RUM answers `content-type: application/x-amz-json-1.1`.
3) curl -i 'http://<emulator>/rummetrics/<name>/metricsdestination' -> same divergence.
Or replay fixtures rum-delete-then-verify-gone (task#2) and rum-metrics-destination-cloudwatch (task#2).
