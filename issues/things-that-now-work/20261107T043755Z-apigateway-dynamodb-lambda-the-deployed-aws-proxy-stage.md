Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda.apigw-lambda-ddb-pipeline-positive apigateway__dynamodb__lambda.apigw-repeat-visits-land-as-distinct-items-and-stream-one-insert-each
Issue-Key: apigateway__dynamodb__lambda.apigw-lambda-ddb-pipeline-positive|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda.apigw-repeat-visits-land-as-distinct-items-and-stream-one-insert-each|run#12|AssertionError|assert len(ips) == 1 and "0.0.0.0" not in ips, f"the visits reported caller IPs {sorted(ips)!r}"
Issue-Commit: 12bd58142e134c788a6e6e722018c3787d666c1f

# apigateway__dynamodb__lambda: the deployed AWS_PROXY stage never answers a POST within the budget, and the proxy event carries no requestContext.identity.sourceIp

> _Note: this issue was identified against the code base at commit `12bd58142e134c788a6e6e722018c3787d666c1f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api deployed stage, AWS_PROXY integration)

## Description
This issue carries 2 net-new root causes.

Cause 1 of 2: a POST to a deployed REST stage wired to an AWS_PROXY integration never completes, so the HTTP -> API Gateway -> Lambda -> DynamoDB pipeline (the PutItem and the function's log lines) is unobservable.
Failing check ids for this cause: apigateway__dynamodb__lambda.apigw-lambda-ddb-pipeline-positive.
AWS: recorded behavior only — the execute-api data plane is not in the API Gateway control-plane Smithy model; the modelled setup is CreateRestApi / CreateResource / PutMethod / PutIntegration type=AWS_PROXY / CreateDeployment, lambda AddPermission for apigateway.amazonaws.com. The recording shows the POST of {"id": <id>, "data": <value>} to <stage-invoke-url>/items answered HTTP 200 with the handler's body, the item then readable by a strongly consistent GetItem, and /aws/lambda/<function> holding the handler's own printed line plus a REPORT line.
Emulator: the request never terminates — the check is killed by the harness and the suite reports exactly 'Error: Check timed out after 60.0s'. No status, headers or body are observed, so none of the downstream assertions (persisted item, log lines) is ever reached.
Why: a client of a proxy-integrated REST API hangs instead of receiving a status; an end-to-end ingest path built this way cannot be exercised at all, and callers cannot distinguish success from backend failure.
Fix: the execute-api data-plane request path of the API Gateway service — deployed stage/route resolution and the AWS_PROXY invoke of the target function, mapping the handler's statusCode/headers/body back to the HTTP response. Every request that reaches a deployed stage must terminate in a response.
Traces: apigateway__dynamodb__lambda.apigw-lambda-ddb-pipeline-positive.

Cause 2 of 2: the REST AWS_PROXY event handed to the backend function carries no caller IP, so the handler reads requestContext.identity.sourceIp as absent.
Failing check ids for this cause: apigateway__dynamodb__lambda.apigw-repeat-visits-land-as-distinct-items-and-stream-one-insert-each.
AWS: recorded behavior only — the proxy event envelope is not described by the Smithy model. The recording shows that for a REST API with an AWS_PROXY (Lambda proxy, payload format 1.0) integration, event["requestContext"]["identity"]["sourceIp"] is the viewer's IP address for every invoke; a handler reading it gets a real address, never a placeholder, and two successive visits from the same client report the same address.
Emulator: the key is missing/empty, so the handler's `or "0.0.0.0"` fallback is taken and both visits report the IP "0.0.0.0"; the check fails with AssertionError: the visits reported caller IPs ['0.0.0.0'].
Why: any handler that logs, partitions or rate-limits by requestContext.identity.sourceIp (here the DynamoDB hash key is the caller IP) collapses every caller onto one synthetic key, so per-client data separation, geo/abuse logic and audit records are all wrong.
Fix: the AWS_PROXY event builder in the API Gateway execute-api integration path — populate requestContext.identity (sourceIp, and the sibling identity fields it fills from the request) from the inbound connection.
Traces: apigateway__dynamodb__lambda.apigw-repeat-visits-land-as-distinct-items-and-stream-one-insert-each.

Note: the open issue 20261105T111951Z-apigateway-dynamodb-lambda-every-scenario-s-bootstrap-lambda.md covers the python runtime's missing boto3 for this group's bootstrap functions; it carries neither of these two checks' replay keys and neither cause above is that one.

## Evidence
Cause 1 — Expected: POST <stage-invoke-url>/items -> HTTP 200; item readable with ConsistentRead; log group /aws/lambda/<fn> holds the handler line and a REPORT line. Actual: 'Error: Check timed out after 60.0s' with no response ever observed.

Cause 2 — Expected: the two signed GETs both report the viewer's IP address, so the set of reported IPs has one element and does not contain the handler's "0.0.0.0" fallback. Actual: AssertionError: the visits reported caller IPs ['0.0.0.0'] — event["requestContext"]["identity"]["sourceIp"] was absent/empty on both invokes.

## Steps to Reproduce
Cause 1: create a table, a function that PutItems it, a REST API with POST /items wired to an AWS_PROXY integration on that function, AddPermission for apigateway.amazonaws.com, CreateDeployment to stage prod, then POST {"id":...,"data":...} to <stage-invoke-url>/items — no response arrives.
Cause 2: deploy a REST API with an AWS_PROXY integration whose handler returns ((event.get("requestContext") or {}).get("identity") or {}).get("sourceIp"), invoke the stage twice and observe the value is missing (the handler's fallback is used) instead of the caller's IP.
Or replay the two check ids in trace_ids.
