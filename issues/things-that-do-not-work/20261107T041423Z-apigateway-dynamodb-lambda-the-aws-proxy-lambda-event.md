Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda.apigw-repeat-visits-land-as-distinct-items-and-stream-one-insert-each
Issue-Key: apigateway__dynamodb__lambda.apigw-repeat-visits-land-as-distinct-items-and-stream-one-insert-each|run#12|AssertionError|assert len(ips) == 1 and "0.0.0.0" not in ips, f"the visits reported caller IPs {sorted(ips)!r}"
Issue-Commit: 12bd58142e134c788a6e6e722018c3787d666c1f

# apigateway__dynamodb__lambda: the AWS_PROXY Lambda event carries no requestContext.identity.sourceIp, so a handler reading the caller IP sees nothing

> _Note: this issue was identified against the code base at commit `12bd58142e134c788a6e6e722018c3787d666c1f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause. Progress note: the deployed stage now ANSWERS (HTTP 200 with the handler's JSON body), so the non-response cause carried by open issue 20261103T022510Z-apigateway-dynamodb-lambda-repeated-requests-to-a-deployed-rest-stage-never.md (and the related 20261103T021944Z-apigateway-dynamodb-lambda-a-signed-get-on-a.md) is no longer what this check fails on. The replay reports this check as owned by neither issue, so the new cause is filed here.

Cause 1 of 1: the Lambda proxy-integration event built by the execute-api data plane omits (or blanks) requestContext.identity.sourceIp.
AWS: recorded/documented behavior only — the API Gateway Smithy model describes the control plane (PutIntegration type=AWS_PROXY) and says nothing about the invocation payload, so the requirement is AWS's documented Lambda proxy integration (payload format 1.0) input format: every event carries a requestContext object whose identity object carries sourceIp, the source IP address of the TCP connection that made the request to API Gateway. It is always present and always a real client address; it is never the unspecified address 0.0.0.0. Its value is a request-derived variable (the connecting peer's address), not a fixed string, and the sibling identity members (userAgent, and for an AWS_IAM-authorized method caller/user/userArn/accountId/accessKey) are populated from the same request/authorization context.
Emulator: the stage answers 200 and invokes the function, but the event the function receives has no usable requestContext.identity.sourceIp — the handler's `((event.get("requestContext") or {}).get("identity") or {}).get("sourceIp") or "0.0.0.0"` falls through to its literal fallback, and both visits report the IP "0.0.0.0". The check asserts the reported IP set is a single real address and fails: `AssertionError: the visits reported caller IPs ['0.0.0.0']`. (The assertion is on presence of a real caller address, not on any particular address, so it is not an environment-specific value.)
Why: a client that relies on AWS's proxy event cannot see who called it. Handlers that rate-limit, geo-route, audit, de-duplicate or authorize per caller IP all silently collapse onto one bogus key (0.0.0.0): a per-IP visitor table keys every caller the same, a per-IP throttle throttles everyone together, and an access/audit record attributes every request to the unspecified address. The defect is invisible at the HTTP level because the stage still returns 200, so it surfaces only as wrong data.
Fix: the execute-api data plane's AWS_PROXY event builder — the component that assembles the proxy integration payload (payload format 1.0) must populate requestContext.identity from the inbound connection and the request's authorization context: sourceIp from the connecting peer, userAgent from the request's User-Agent header, and the caller identity members from the SigV4 principal for an AWS_IAM-authorized method.
Traces: apigateway__dynamodb__lambda.apigw-repeat-visits-land-as-distinct-items-and-stream-one-insert-each.

## Evidence
apigateway__dynamodb__lambda.apigw-repeat-visits-land-as-distinct-items-and-stream-one-insert-each — verdict: mismatch.

Expected (AWS): two signed GETs to a deployed REGIONAL stage with an AWS_IAM method and an AWS_PROXY Lambda integration each deliver an event whose requestContext.identity.sourceIp is the caller's address; the handler echoes it, so the two responses report one and the same real IP and the assertion `len(ips) == 1 and "0.0.0.0" not in ips` holds.

Actual (emulator): both GETs answer 200 and the handler runs, but sourceIp is absent/empty in the event, so the handler returns its fallback literal. Replay output:
  File "check_apigateway__dynamodb__lambda_apigw_repeat_visits_land_as_distinct_items_and_stream_one_insert_each.py", line 72, in run
    assert len(ips) == 1 and "0.0.0.0" not in ips, f"the visits reported caller IPs {sorted(ips)!r}"
  AssertionError: the visits reported caller IPs ['0.0.0.0']

Fixed text vs. variable parts: the required behavior is the PRESENCE of requestContext.identity.sourceIp populated from the request's connecting peer — the address itself is a per-request/per-environment value and is not part of the requirement. "0.0.0.0" is the check handler's own literal fallback and is therefore proof the member was not supplied by the emulator. The check never reached its later stream assertions, so the per-visit INSERT/no-OldImage contract in this scenario remains ungraded behind this cause.

## Steps to Reproduce
1. Replay the behavioral suite and re-run apigateway__dynamodb__lambda.apigw-repeat-visits-land-as-distinct-items-and-stream-one-insert-each.
2. Minimal reproduction: create a DynamoDB table with StreamSpecification NEW_AND_OLD_IMAGES; create a python Lambda whose handler returns `json.dumps({"ip": ((event.get("requestContext") or {}).get("identity") or {}).get("sourceIp")})`; create a REST API, add resource /visit with a GET method (authorizationType AWS_IAM) and PutIntegration type=AWS_PROXY / integrationHttpMethod=POST pointing at the function's invocations URI; AddPermission for apigateway.amazonaws.com; CreateDeployment stageName=prod.
3. Issue a SigV4-signed (service execute-api) GET to <stage-url>/visit.
4. Expect: 200 whose body reports the caller's IP address. Observe: 200 whose body reports no sourceIp at all (null/absent), so any handler-side fallback value is returned instead.
