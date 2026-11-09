Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__opensearch__s3.lambda-sdk-opensearch-bulk-index-positive lambda__opensearch__s3.lambda-sdk-opensearch-update-document-positive
Issue-Key: lambda__opensearch__s3.lambda-sdk-opensearch-bulk-index-positive|run#16|AssertionError|assert bulk["statusCode"] == 200, bulk
Issue-Key: lambda__opensearch__s3.lambda-sdk-opensearch-update-document-positive|run#14|AssertionError|assert update["statusCode"] == 200, update
Issue-Commit: dd6dc626874166bf5c09c57665a1e26f5c7b5618

# lambda__opensearch__s3: the OpenSearch domain data plane refuses both document-write helper APIs — POST /_bulk answers 405 with the index route's method list and POST /{index}/_update/{id} is unrouted (2 causes)

> _Note: this issue was identified against the code base at commit `dd6dc626874166bf5c09c57665a1e26f5c7b5618`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DomainRestDataPlane

## Description
Two net-new root causes, both reached from a Lambda function that SigV4-signs requests (service name `es`) to the endpoint DescribeDomain published for the domain. Each cause has its own failing check id. NOTE: open issue 20261101T140814Z-opensearch-the-domain-rest-data-plane-serves-only.md describes the same `/_bulk` routing defect as its Cause 1, but under the `opensearch` service and without this check's replay key, so this check is not covered by it; a fixer that fixes that cause closes Cause 1 here too, but Cause 2 is not in any open issue.

Cause 1 of 2 — POST /_bulk is not served; the path is resolved as an INDEX name.
AWS: Smithy: DescribeDomain's `DomainStatus$Endpoint` is "Domain-specific endpoint used to submit index, search, and data upload requests to the domain" — the bulk API is that data-upload path. `POST /_bulk` (and `POST /{index}/_bulk`) takes a newline-delimited action/source body and answers HTTP 200 with `{"took":<n>,"errors":<bool>,"items":[...]}`, 200 even when individual items failed, each item carrying its own `status` and `error`. The REST data plane itself is not in the Smithy model, so recorded behavior only for the emulator's status and wording.
Emulator: HTTP 405 `{"error":"Incorrect HTTP method for uri [/_bulk] and method [POST], allowed: [PUT, GET, HEAD, DELETE]"}`. Fixed text of that message: "Incorrect HTTP method for uri [", "] and method [", "], allowed: ["; its variable parts are the request URI (`/_bulk`), the request method (`POST`) and the allowed-method list — and the list returned here is the `/{index}` route's, which is the tell that the reserved `_bulk` segment was matched as an index name.
Why: the bulk API is the normal ingest path for every OpenSearch SDK and for log shippers; a client that loads documents with it cannot write a single document, and because a `_`-prefixed reserved top-level endpoint is reinterpreted as an index name the refusal misdirects the caller to a method list that has nothing to do with bulk.
Fix: the domain REST data-plane router in services/service-com-amazonaws-opensearch-amazonopensearchservice/src/service/domain_rest.rs — reserve `_`-prefixed first path segments from index-name resolution and register the bulk handler against the document/ingest state.
Traces: lambda__opensearch__s3.lambda-sdk-opensearch-bulk-index-positive

Cause 2 of 2 — POST /{index}/_update/{id} (the partial-document update API) is unrouted.
AWS: `POST /{index}/_update/{id}` with `{"doc":{...}}` MERGES the named fields into the stored document and answers HTTP 200 with `{"_index":"<index>","_id":"<id>","_version":<n+1>,"result":"updated","_shards":{...},"_seq_no":<n>,"_primary_term":<n>}`; fields not named in the partial body survive, and `?refresh=true` makes the new value immediately searchable. Recorded behavior only for the emulator's status and wording.
Emulator: HTTP 400 `{"error":"no handler found for uri [/<index>/_update/<id>] and method [POST]"}` — fixed text "no handler found for uri [", "] and method ["; variable parts the request URI (carrying the caller's index name and document id) and the method. The body also lacks the engine's error envelope (`{"error":{"type":...,"reason":...},"status":400}`), so a client branching on `error.type` reads nothing.
Why: partial update is how a client advances a field of an existing document without re-sending it (status transitions, counters, enrichment). With the route absent a caller must read-modify-write the whole document — and, because the refusal carries no error type, cannot even classify the failure.
Fix: the same data-plane router plus the per-document update path in that crate's ingest state (merge the `doc` object into the stored source, bump `_version`, report `result: updated`, honour `refresh`).
Traces: lambda__opensearch__s3.lambda-sdk-opensearch-update-document-positive

## Evidence
Cause 1: expected HTTP 200 and a bulk response with errors=false and 5 items; the Lambda's signed POST <endpoint>/_bulk?refresh=true returned {'statusCode': 405, 'body': '{"error":"Incorrect HTTP method for uri [/_bulk] and method [POST], allowed: [PUT, GET, HEAD, DELETE]"}'} (check lambda-sdk-opensearch-bulk-index-positive, assertion `assert bulk["statusCode"] == 200`).
Cause 2: expected HTTP 200 with result='updated' and a following GET showing the untouched field preserved; the Lambda's signed POST <endpoint>/<index>/_update/1?refresh=true returned {'statusCode': 400, 'body': '{"error":"no handler found for uri [/sim2real-update/_update/1] and method [POST]"}'} (check lambda-sdk-opensearch-update-document-positive, assertion `assert update["statusCode"] == 200`). The preceding PUT <endpoint>/<index>/_doc/1 succeeded, so the index and document exist.

## Steps to Reproduce
1. CreateDomain (EngineVersion OpenSearch_2.11+, ClusterConfig {InstanceType t3.small.search, InstanceCount 1}, EBSOptions gp3/10) with an AccessPolicies document allowing es:ESHttp* for the Lambda role's ARN on arn:aws:es:<region>:<account>:domain/<domain>/*; wait for the domain to be active and read DomainStatus.Endpoint.
2. Create an IAM role trusting lambda.amazonaws.com with es:ESHttp* on that resource and a Python function that SigV4-signs ('es') and sends an arbitrary method/path/body to the endpoint.
3. Cause 1: invoke the function with method POST, path /_bulk?refresh=true and an NDJSON body of {"index":{"_index":"<idx>","_id":"<n>"}} / source pairs -> 405 'Incorrect HTTP method for uri [/_bulk] ... allowed: [PUT, GET, HEAD, DELETE]' instead of 200 with items[].
4. Cause 2: invoke the function with method PUT, path /<idx>/_doc/1?refresh=true, body {"status":"pending","attempts":0,"owner":"x"} (succeeds), then with method POST, path /<idx>/_update/1?refresh=true, body {"doc":{"status":"done","attempts":1}} -> 400 'no handler found for uri [/<idx>/_update/1] and method [POST]' instead of 200 result=updated; a following GET /<idx>/_doc/1 should have shown {"status":"done","attempts":1,"owner":"x"}.
