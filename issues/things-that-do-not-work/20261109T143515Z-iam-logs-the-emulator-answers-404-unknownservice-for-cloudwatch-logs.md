Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__logs.hlc-endpoint-event-shapes-accepted-missing-event-rejected iam__logs.structured-json-endpoint-partial-success-reports-refused-record-count
Issue-Key: iam__logs.hlc-endpoint-event-shapes-accepted-missing-event-rejected|run#24|AssertionError|assert status1 == 200, (
Issue-Key: iam__logs.structured-json-endpoint-partial-success-reports-refused-record-count|run#22|AssertionError|assert status1 == 200, (
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# The emulator answers 404 UnknownService for CloudWatch Logs' HTTP ingestion endpoints, so no event-shaped request can be posted

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLogEvents

## Description
Cause 1 of 1: requests to the CloudWatch Logs HTTP ingestion endpoints are not routed to the logs service at all.
AWS: the endpoint accepts a JSON body whose top-level object carries an 'event' field (and the structured-JSON form carrying multiple records), answering 200 for a request whose events are all valid and reporting the refused-record count for a partially valid one.
Emulator: every such request is answered HTTP 404 with body '{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}' — the 404, the __type 'UnknownService' and that message are the divergence; the request is never reaching the logs service.
Why: a client posting log events over the HTTP ingestion endpoints (the agent-style path) gets a 404 from the router instead of an accept/partial-success response, so no event can be ingested that way.
Fix: request routing for the CloudWatch Logs HTTP ingestion endpoints (the router must resolve these paths to the logs service), and the endpoint's accept/partial-success handler in the logs crate.
Traces: both checks in trace_ids.

## Evidence
Expected: 200 for an all-valid batch, and a refused-record count for a partially valid one. Actual: 'AssertionError: a top-level object carrying an "event" field was never accepted (last status=404, body='{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}')' and 'AssertionError: a request whose events are all valid should return 200 (last status=404, body=... same ...)'.

## Steps to Reproduce
Replay iam__logs.hlc-endpoint-event-shapes-accepted-missing-event-rejected and .structured-json-endpoint-partial-success-reports-refused-record-count: POST the event-shaped bodies to the logs ingestion endpoints.
