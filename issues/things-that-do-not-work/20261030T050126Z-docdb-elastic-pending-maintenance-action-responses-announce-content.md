Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-elastic-apply-maintenance-action-missing-resource-not-found
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-elastic-apply-maintenance-action-missing-resource-not-found docdb-elastic-pending-maintenance-action-missing-and-create-cluster-invalid-shard-capacity
Issue-Shape-Id: com.amazonaws.docdbelastic#ChimeraDbLionfishServiceLambda
Issue-Key: docdb-elastic|ListPendingMaintenanceActions|2xx->2xx|$headers.content-type
Issue-Key: docdb-elastic|GetPendingMaintenanceAction|4xx->4xx|$headers.content-type
Issue-Commit: 2d55507234edce158ad24cb25218df0b3fd7399f

# DocDB Elastic pending-maintenance-action responses announce Content-Type application/json instead of application/x-amz-json-1.1

> _Note: this issue was identified against the code base at commit `2d55507234edce158ad24cb25218df0b3fd7399f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPendingMaintenanceActions

## Description
The DocDB Elastic response serializer stamps the wrong media type on the JSON bodies it returns from the pending-maintenance-action paths, on both the success and the refusal path. Two net-new causes; both are header-only divergences (status and body matched byte-for-byte).

Smithy: ChimeraDbLionfishServiceLambda (sdkId "DocDB Elastic"); ListPendingMaintenanceActions is GET /pending-actions (http code 200), GetPendingMaintenanceAction is GET /pending-action/{resourceArn} with ResourceNotFoundException at httpError 404. The exact media type is recorded behavior only — the model pins no mediaType trait — and every recorded AWS response for this service carries Content-Type: application/x-amz-json-1.1 on its JSON payload.

Note on existing issues: the open issue 20261029T172712Z-docdb-elastic-responses-announce-content-type-application-json.md reports the same serializer defect, but its replay keys cover only ListClusters (2xx) and ListClusterSnapshots (4xx) and it does not carry either binding below, so these two steps would be left unexercised by a fix validated against it. They are filed here so a fixer re-replays them.

Cause 1 of 2: ListPendingMaintenanceActions success (fixture docdb-elastic-apply-maintenance-action-missing-resource-not-found, step task#2).
AWS: 200 with header content-type: application/x-amz-json-1.1 and the ListPendingMaintenanceActionsOutput JSON body (required member resourcePendingMaintenanceActions).
Emulator: 200 with content-type: application/json; the body itself matched.
Why: a client, proxy or middleware that dispatches on the response media type (content negotiation, cassette matching, strict SDK media-type checks, logging pipelines keyed on the AWS JSON media type) sees a media type AWS never serves for this service, so a payload that is otherwise identical is handled differently than against AWS.
Fix: the DocDB Elastic crate's restJson1 response serializer — the single place the outgoing content-type header is set for this service, so the fix must cover success and error responses alike rather than being patched per operation.

Cause 2 of 2: GetPendingMaintenanceAction refusal (fixture docdb-elastic-pending-maintenance-action-missing-and-create-cluster-invalid-shard-capacity, step task#1).
AWS: 404 with header content-type: application/x-amz-json-1.1 carrying the ResourceNotFoundException body; status and body matched the recording.
Emulator: 404 with content-type: application/json.
Why: same as cause 1 on the refusal path — an error body a client cannot classify by media type, from a response that is otherwise correct.
Fix: the same serializer; the error path must inherit the same content-type as the success path.

Traces: docdb-elastic-apply-maintenance-action-missing-resource-not-found#task#2 (ListPendingMaintenanceActions, 200) and docdb-elastic-pending-maintenance-action-missing-and-create-cluster-invalid-shard-capacity#task#1 (GetPendingMaintenanceAction, 404).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200), verdict mismatch on both steps:
- ListPendingMaintenanceActions, docdb-elastic-apply-maintenance-action-missing-resource-not-found#task#2: expected_status 200, actual_status 200; $headers.content-type: value_mismatch (expected 'application/x-amz-json-1.1', got 'application/json'). No other key diverged. Neither stub_path nor owned_by was set for this cluster.
- GetPendingMaintenanceAction, docdb-elastic-pending-maintenance-action-missing-and-create-cluster-invalid-shard-capacity#task#1: expected_status 404, actual_status 404; $headers.content-type: value_mismatch (expected 'application/x-amz-json-1.1', got 'application/json'). No other key diverged. Neither stub_path nor owned_by was set for this cluster.
The requirement is the media-type string itself ('application/x-amz-json-1.1'); no host, port, generated id or timestamp from the recordings is part of it.

## Steps to Reproduce
1. Boot the emulator and target the DocDB Elastic (docdb-elastic) endpoint.
2. GET /pending-actions (ListPendingMaintenanceActions) and inspect the response headers -> content-type is application/json where AWS answers application/x-amz-json-1.1; the 200 body is otherwise correct.
3. GET /pending-action/{resourceArn} (GetPendingMaintenanceAction) with a resourceArn no resource carries -> the 404 ResourceNotFoundException body also carries content-type application/json.
4. Or replay fixtures docdb-elastic-apply-maintenance-action-missing-resource-not-found (task#2) and docdb-elastic-pending-maintenance-action-missing-and-create-cluster-invalid-shard-capacity (task#1).
