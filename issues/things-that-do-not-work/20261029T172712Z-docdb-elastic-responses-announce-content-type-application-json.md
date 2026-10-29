Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-elastic-get-cluster-nonexistent-not-found
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-elastic-get-cluster-nonexistent-not-found docdb-elastic-cluster-snapshot-ops-missing-resource-not-found
Issue-Shape-Id: com.amazonaws.docdbelastic#ChimeraDbLionfishServiceLambda
Issue-Key: docdb-elastic|ListClusters|2xx->2xx|$headers.content-type
Issue-Key: docdb-elastic|ListClusterSnapshots|4xx->4xx|$headers.content-type
Issue-Commit: b1cd28f1ca277d6a032df605f83af5017e934043

# DocDB Elastic responses announce Content-Type application/json instead of application/x-amz-json-1.1

> _Note: this issue was identified against the code base at commit `b1cd28f1ca277d6a032df605f83af5017e934043`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListClusters

## Description
The DocDB Elastic response serializer stamps the wrong media type on its JSON bodies, on both success and refusal paths. Two net-new causes are header-only divergences; the same wrong header also rides along on the not-found steps filed under the separate ResourceNotFoundException-wording issue, so fixing the serializer is what closes all of them.

Smithy: ChimeraDbLionfishServiceLambda (sdkId "DocDB Elastic") is restJson1 over sigv4; the recorded AWS responses for this service carry Content-Type: application/x-amz-json-1.1 on every JSON payload (recorded behavior only for the exact media type — the model does not pin a mediaType trait).

Cause 1 of 2: ListClusters success (fixture docdb-elastic-get-cluster-nonexistent-not-found, step task#2).
AWS: 200 with header content-type: application/x-amz-json-1.1 and the ListClustersOutput JSON body.
Emulator: 200 with content-type: application/json; the body itself matches.
Why: a client or proxy that dispatches on the response media type (content negotiation, recorded-cassette matching, strict SDK/middleware checks, logging pipelines keyed on the AWS JSON media type) sees a media type AWS never serves for this service, so a payload that is byte-identical is still handled differently than against AWS.
Fix: the DocDB Elastic crate's restJson1 response serializer — the single place the outgoing content-type header is set for this service, so it applies to success and error responses alike.

Cause 2 of 2: ListClusterSnapshots refusal (fixture docdb-elastic-cluster-snapshot-ops-missing-resource-not-found, step task#4).
AWS: 400 with header content-type: application/x-amz-json-1.1; status and error body match the recording.
Emulator: 400 with content-type: application/json.
Why: same as cause 1 on the refusal path — an error body a client cannot classify by media type, from a response that is otherwise correct.
Fix: same serializer; the error path must inherit the same content-type as the success path.

Same-header collateral (closed by the same fix, bodies filed elsewhere): DeleteCluster (docdb-elastic-cluster-lifecycle-ops-missing-cluster-not-found#task#3), GetClusterSnapshot and DeleteClusterSnapshot (docdb-elastic-cluster-snapshot-ops-missing-resource-not-found#task#2, #task#3) and GetCluster (docdb-elastic-get-cluster-nonexistent-not-found#task#1) also diverge on $headers.content-type.

## Evidence
Replay of trace set simparity-wire-aws-v1, verdict mismatch:
- ListClusters, docdb-elastic-get-cluster-nonexistent-not-found#task#2: expected_status 200, actual_status 200; $headers.content-type: value_mismatch (expected 'application/x-amz-json-1.1', got 'application/json'). No other key diverged.
- ListClusterSnapshots, docdb-elastic-cluster-snapshot-ops-missing-resource-not-found#task#4: expected_status 400, actual_status 400; $headers.content-type: value_mismatch (expected 'application/x-amz-json-1.1', got 'application/json'). No other key diverged.
The same header divergence is reported on four further steps (GetCluster, DeleteCluster, GetClusterSnapshot, DeleteClusterSnapshot) whose body divergences are filed as the ResourceNotFoundException-wording issue.

## Steps to Reproduce
1. Boot the emulator and target the DocDB Elastic (docdb-elastic) endpoint.
2. GET /clusters (ListClusters) and inspect the response headers -> content-type is application/json, where AWS answers application/x-amz-json-1.1.
3. GET /cluster-snapshots (ListClusterSnapshots) with a request AWS refuses with 400 -> the 400 body also carries content-type application/json.
4. Or replay fixtures docdb-elastic-get-cluster-nonexistent-not-found (task#2) and docdb-elastic-cluster-snapshot-ops-missing-resource-not-found (task#4).
