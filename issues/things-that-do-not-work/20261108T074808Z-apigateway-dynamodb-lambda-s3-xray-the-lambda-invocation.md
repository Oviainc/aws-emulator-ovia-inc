Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__s3__xray.apigw-lambda-ddb-s3-trace-tree-shape apigateway__dynamodb__lambda__s3__xray.apigw-trace-header-continuation-and-sampled-zero-rejects-recording
Issue-Key: apigateway__dynamodb__lambda__s3__xray.apigw-lambda-ddb-s3-trace-tree-shape|run#12|AssertionError|assert obs["raw_trace_header"] and _HEADER_RE.match(obs["raw_trace_header"]), (
Issue-Key: apigateway__dynamodb__lambda__s3__xray.apigw-trace-header-continuation-and-sampled-zero-rejects-recording|run#16|AssertionError|assert obs_a["root"] == a_id, (
Issue-Commit: 3f1b01ed3b5d1c2056caeeb85ce9541e8ea3070e

# apigateway__dynamodb__lambda__s3__xray: the Lambda invocation environment carries no _X_AMZN_TRACE_ID, so a traced REST stage starts no trace and never continues a caller's trace context (1 cause)

> _Note: this issue was identified against the code base at commit `3f1b01ed3b5d1c2056caeeb85ce9541e8ea3070e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: a Lambda invoked behind a tracing-enabled REST API stage sees no _X_AMZN_TRACE_ID environment variable at all, so neither a stage-authored trace id nor a caller-supplied trace context is visible to the handler. Failing checks: apigateway__dynamodb__lambda__s3__xray.apigw-lambda-ddb-s3-trace-tree-shape, apigateway__dynamodb__lambda__s3__xray.apigw-trace-header-continuation-and-sampled-zero-rejects-recording.

AWS: recorded behavior only — the X-Ray trace context is not part of any Smithy model; it is the documented Lambda execution-environment contract. When a function is invoked through an API Gateway stage with TracingEnabled, AWS sets _X_AMZN_TRACE_ID in the invocation environment to a header of the fixed form `Root=1-<8 hex>-<24 hex>;Parent=<16 hex>;Sampled=<0|1>`. The fixed text is the field names `Root=`, `Parent=`, `Sampled=` and the `1-` version prefix of the root id; the variable parts are the epoch-seconds hex and the 24-hex unique part of the root id, the 16-hex parent (segment) id, and the sampling decision. When the caller supplies X-Amzn-Trace-Id with a Root= (and optionally Sampled=0), AWS continues that context: the Root the handler observes is byte-identical to the Root the caller sent, and Sampled=0 propagates so nothing is recorded.

Emulator: the handler observes _X_AMZN_TRACE_ID as absent. Replay reports `AssertionError: _X_AMZN_TRACE_ID was None; expected Root=1-<8hex>-<24hex>;Parent=<16hex>;Sampled=1` for the trace-tree check, and for the continuation check `AssertionError: the handler observed Root=None; we supplied '1-<8hex>-<24hex>', so the caller's trace context was not continued (whole header None)`. The whole header is None, not a wrong value, so both generation and continuation are unobservable.

Why: a client that instruments with the AWS X-Ray SDK (or reads the header directly) gets no trace id, no parent linkage and no sampling decision. Every segment the handler would emit is either dropped or orphaned, so a distributed trace across stage -> Lambda -> DynamoDB/S3 can never be assembled and Sampled=0 suppression cannot be honoured.

Fix: the Lambda invoke path's execution-environment construction — it must populate _X_AMZN_TRACE_ID for every invoke, authored by the invoking surface (API Gateway stage) when the caller sent no X-Amzn-Trace-Id, and derived from the caller's header (preserving Root and Sampled) when it did.

Note: several open issues cover earlier stages of this scenario where the deployed stage itself answered 404 or never answered at all (20261024T043741Z-apigateway-dynamodb-lambda-s3-xray-both-x-ray-9c23a5.md, 20261027T085253Z-apigateway-dynamodb-lambda-s3-xray-caller-trace-context.md, 20261030T003023Z-apigateway-dynamodb-lambda-s3-xray-x-ray-trace-header-continuation-through-a-rest.md, 20261101T204543Z-apigateway-dynamodb-lambda-s3-xray-both-distributed-tracing.md, 20261102T085952Z-apigateway-dynamodb-lambda-s3-xray-the-traced-rest-872f8c.md). None of them covers this failure: the stage now answers and the handler now runs, and the remaining defect is the missing trace-context environment variable.

Traces: both checks listed above.

## Evidence
Expected (AWS): the handler's os.environ['_X_AMZN_TRACE_ID'] matches `Root=1-<8 hex>-<24 hex>;Parent=<16 hex>;Sampled=1`; and when the caller sends X-Amzn-Trace-Id with Root=1-<8hex>-<24hex>, the Root the handler observes equals the Root the caller sent (Sampled=0 propagating as 0).

Actual (emulator): _X_AMZN_TRACE_ID is absent from the invocation environment.
  - apigw-lambda-ddb-s3-trace-tree-shape: `AssertionError: _X_AMZN_TRACE_ID was None; expected Root=1-<8hex>-<24hex>;Parent=<16hex>;Sampled=1`
  - apigw-trace-header-continuation-and-sampled-zero-rejects-recording: `AssertionError: the handler observed Root=None; we supplied '1-6af01ea8-016847f60da6e6a712217dc6', so the caller's trace context was not continued (whole header None)` (the `1-6af01ea8-...` value is one run's generated root id, not part of the requirement — the requirement is that the observed Root equals whatever Root the caller sent).

## Steps to Reproduce
1. Create a REST API with an AWS_PROXY integration to a Python Lambda and deploy a stage with TracingEnabled=true.
2. GET the stage path with no X-Amzn-Trace-Id header; have the handler return os.environ.get('_X_AMZN_TRACE_ID'). Observe None instead of `Root=1-<8hex>-<24hex>;Parent=<16hex>;Sampled=1`.
3. GET the stage path again sending X-Amzn-Trace-Id: Root=1-<8hex>-<24hex>;Sampled=0. Observe the handler still sees None, so the caller's Root is not continued and Sampled=0 cannot be honoured.
4. Replay apigateway__dynamodb__lambda__s3__xray.apigw-lambda-ddb-s3-trace-tree-shape and apigateway__dynamodb__lambda__s3__xray.apigw-trace-header-continuation-and-sampled-zero-rejects-recording.
