Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatch-get-dashboard-contract
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatch-get-dashboard-contract
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch|GetDashboard|2xx->2xx|$.DashboardBody
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CloudWatch GetDashboard re-serializes the stored dashboard body

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDashboard

## Description
1 cause.

Cause 1 of 1: GetDashboard on the 'cloudwatch'-token endpoint returns a re-serialized dashboard body.

AWS: DashboardBody is returned as the EXACT string that was submitted to PutDashboard. Smithy (com.amazonaws.cloudwatch#GraniteServiceVersion20100801): GetDashboard output DashboardBody targets DashboardBody, a plain string; the service stores the document opaquely and does not normalise its whitespace, key order or number formatting.

Emulator: parses the submitted body into a JSON value and re-serializes it on read, inserting a space after every ':' and ',' — a submitted '{"widgets":[{"type":"text",...}]}' comes back as '{"widgets": [{"type": "text", ...}]}'.

Why: a client that round-trips a dashboard (GetDashboard, diff against its source of truth, PutDashboard) sees a change on every single read, so drift detection never converges and IaC reconcilers rewrite the dashboard on every run. Any client hashing or byte-comparing the body to detect a real edit is defeated.

Fix: dashboard state in the CloudWatch crate must retain the submitted body verbatim, and the GetDashboard response serializer must emit it unchanged rather than re-encoding a parsed value.

Traces: cloudwatch-get-dashboard-contract#task#2. This divergence is reported under the 'cloudwatch' service token (the same defect on the 'monitoring' token is filed separately, titled 'CloudWatch dashboard bodies are re-serialized, breaking byte fidelity and Size'); it must be re-replayed on this endpoint to confirm the fix covers it.

## Evidence
Replay (simparity-wire-aws-v1), 1 cluster, service 'cloudwatch'.
GetDashboard, cloudwatch-get-dashboard-contract#task#2 (status 200 expected and returned): $.DashboardBody value_mismatch — expected '{"widgets":[{"type":"text","x":0,"y":0,"width":6,"height":2,"properties":{"markdown":"sim2real behavioral check"}}]}', got '{"widgets": [{"type": "text", "x": 0, "y": 0, "width": 6, "height": 2, "properties": {"markdown": "sim2real behavioral check"}}]}'. The only difference is inserted whitespace after ':' and ',' — the structure is identical, which is what identifies this as a re-serialization rather than a content defect.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the simparity-wire-aws-v1 corpus.
2. PutDashboard a body serialized with no spaces between tokens.
3. GetDashboard the same dashboard name (fixture cloudwatch-get-dashboard-contract, step task#2).
4. Byte-compare the returned DashboardBody with the submitted string — the emulator's differs by inserted whitespace.
