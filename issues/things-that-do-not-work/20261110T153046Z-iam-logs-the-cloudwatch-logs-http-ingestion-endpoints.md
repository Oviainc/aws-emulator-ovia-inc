Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__logs.hlc-endpoint-event-shapes-accepted-missing-event-rejected iam__logs.structured-json-endpoint-partial-success-reports-refused-record-count
Issue-Key: iam__logs.hlc-endpoint-event-shapes-accepted-missing-event-rejected|_poll_until_status#23|Timeout|time.sleep(interval_s)
Issue-Key: iam__logs.structured-json-endpoint-partial-success-reports-refused-record-count|_poll_until_status#21|Timeout|time.sleep(interval_s)
Issue-Commit: f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8

# iam__logs: the CloudWatch Logs HTTP ingestion endpoints (HLC and Structured JSON) never answer, so neither acceptance nor partial-success refusal can be graded (1 cause)

> _Note: this issue was identified against the code base at commit `f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLogEvents

## Description
Cause 1 of 1: requests to the Logs HTTP ingestion endpoints produce no response within the run budget, so neither endpoint's accept path nor its refusal path is observable.
AWS: the HLC endpoint accepts a bearer-token-authenticated body carrying an 'event' field — a single object or an array alike — and refuses a body with no 'event' field at all with HTTP 400 and the body {"message": "All events were invalid"} (fixed text, no variable part); GetLogEvents then reads back exactly the accepted bodies. The Structured JSON endpoint answers an empty 200 body when every event in the request is accepted, and a partialSuccess body naming the rejected record count and an error message when an event falls outside the ingestion time window. Reference: AWS docs CWL_HLC_Endpoint and CWL_HTTP_Endpoints_StructuredJSON; recorded behavior only for the partialSuccess field spelling.
Emulator: both checks produce no verdict at all — 'Error: Check timed out after 60.0s'. Neither the 200, the 400 'All events were invalid' body, nor a partialSuccess body is ever returned.
Why: an agent or client that ships logs over the HTTP endpoints gets no acknowledgement and no refusal — it cannot tell accepted from rejected records, and must either hang or retry blindly.
Fix: the cloudwatchlogs HTTP ingestion endpoint handlers (HLC and Structured JSON) and their bearer-token authentication and per-record validation, which must answer 200/partialSuccess/400 rather than nothing.
Traces: iam__logs.hlc-endpoint-event-shapes-accepted-missing-event-rejected, iam__logs.structured-json-endpoint-partial-success-reports-refused-record-count.

## Evidence
Both checks end with 'Error: Check timed out after 60.0s'. Expected: HLC — 200 for an 'event' object and for an array of them, 400 with body {"message": "All events were invalid"} when 'event' is absent, and GetLogEvents returning only the accepted bodies; Structured JSON — empty 200 body when all events are accepted, partialSuccess body with the rejected count and error message when one event is outside the ingestion window. Actual: no HTTP response at all inside the cap.

## Steps to Reproduce
Replay iam__logs.hlc-endpoint-event-shapes-accepted-missing-event-rejected and iam__logs.structured-json-endpoint-partial-success-reports-refused-record-count: create a log group/stream and a bearer token, POST to the HLC endpoint with an 'event' object, an array of them, and a body lacking 'event'; POST to the Structured JSON endpoint with an all-valid batch and with a batch containing one out-of-window event.
