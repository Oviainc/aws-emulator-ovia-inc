Issue-Source: trace-sweep-divergence
Issue-Trace: lambdacore-get-network-connector-by-name-id-and-arn-equivalent
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambdacore-get-network-connector-by-name-id-and-arn-equivalent
Issue-Shape-Id: com.amazonaws.lambdacore#LambdaCoreApiService
Issue-Key: lambda-core|GetNetworkConnector|2xx->2xx|$.LastModified
Issue-Key: lambda-core|GetNetworkConnector|2xx->2xx|$.StateReason
Issue-Commit: 946f42f6a143eaa9f4f23faa899ea5f11b0d1603

# lambda-core GetNetworkConnector response omits StateReason and serializes LastModified as an epoch number

> _Note: this issue was identified against the code base at commit `946f42f6a143eaa9f4f23faa899ea5f11b0d1603`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetNetworkConnector

## Description
Cause 1 of 1 — the successful GetNetworkConnector body diverges from the modeled output shape in two ways, on every one of the three equivalent lookup forms (by name, by id, by ARN).
AWS: Smithy (com.amazonaws.lambdacore) GetNetworkConnectorResponse models LastModified -> CoreTimestamp with @timestampFormat "date-time", so under this service's REST-JSON protocol it is an ISO-8601 string (the fixture asserts the member's shape, not a fixed instant). The same shape models StateReason -> String alongside State / StateReasonCode; the recording shows AWS populating StateReason on a freshly created connector (recorded example: 'Initial creation'), so a client always receives a reason for the reported State.
Emulator: answers HTTP 200 but (a) serializes LastModified as a bare JSON number (replay example 1792721744.753684, epoch seconds with a fraction) instead of a date-time string, and (b) omits StateReason from the body entirely.
Why: a generated SDK deserializer bound to a date-time timestamp expects a string, so a JSON number either fails deserialization or decodes to a wrong instant — every consumer of GetNetworkConnector sees a broken LastModified. The missing StateReason leaves callers that render or branch on why a connector is in its current state (PENDING/FAILED diagnostics) with a null where AWS always supplies text, and it is absent even in the simple post-create case.
Fix: the GetNetworkConnector response serializer and the connector state record it projects — honour @timestampFormat date-time for LastModified, and populate/emit StateReason from the stored connector state (set at creation, updated on state transitions).
Traces: lambdacore-get-network-connector-by-name-id-and-arn-equivalent#task#7, with the identical divergence at #task#8 and #task#9 (the by-id and by-ARN lookups of the same connector), confirming it is the shared serializer and not one lookup form.

Note: the same epoch-timestamp defect appears on the list projection recorded under the `lambda` service token and is filed with that service's issue; this issue owns the lambda-core GetNetworkConnector surface.

## Evidence
fixture lambdacore-get-network-connector-by-name-id-and-arn-equivalent, step task#7, GetNetworkConnector, expected 200 / actual 200:
  $.LastModified: value_mismatch — expected a date-time-shaped value ('<shaped-not-asserted>'), actual 1792721744.753684 (bare JSON number)
  $.StateReason: missing — expected 'Initial creation', actual None
Same divergence reported for the cluster's other members task#8 and task#9 (lookup by id and by ARN).

## Steps to Reproduce
1. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
2. POST /2026-04-04/network-connectors to create a connector, then GET /2026-04-04/network-connectors/{Name} (fixture lambdacore-get-network-connector-by-name-id-and-arn-equivalent, step task#7).
3. Inspect the 200 body: LastModified is a JSON number rather than an ISO-8601 string, and StateReason is absent.
4. Repeat the GET with the connector's Id and with its ARN (steps task#8, task#9) — the same two divergences appear.
