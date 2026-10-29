Issue-Source: trace-sweep-divergence
Issue-Trace: events-endpoint-create-validation-error
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: events-endpoint-create-validation-error events-endpoint-create-validation-errors events-endpoint-errors
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|CreateEndpoint|4xx->2xx|$.__type
Issue-Key: events|CreateEndpoint|4xx->2xx|$.message
Issue-Key: events|CreateEndpoint|4xx->2xx|status
Issue-Key: events|ListEndpoints|2xx->2xx|$.Endpoints[*]
Issue-Key: events|DescribeEndpoint|4xx->2xx|$.__type
Issue-Key: events|DescribeEndpoint|4xx->2xx|$.message
Issue-Key: events|DescribeEndpoint|4xx->2xx|status
Issue-Key: events|UpdateEndpoint|4xx->2xx|$.__type
Issue-Key: events|UpdateEndpoint|4xx->2xx|$.message
Issue-Key: events|UpdateEndpoint|4xx->2xx|status
Issue-Commit: 10534c2dd099d7fcc20a17236779012152da4f21

# events global endpoints skip validation and existence checks: CreateEndpoint accepts a nonexistent health check and Describe/UpdateEndpoint answer 200 for an absent endpoint

> _Note: this issue was identified against the code base at commit `10534c2dd099d7fcc20a17236779012152da4f21`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEndpoint

## Description
The global-endpoint surface performs neither the referenced-resource validation on create nor the existence check on read/update: three requests AWS refuses with 400 are answered 200, and the endpoint AWS never created is left in the store. Four causes.

Cause 1 of 4 — CreateEndpoint accepts a RoutingConfig health check that does not exist.
AWS: Recorded behavior only (the Smithy model binds CreateEndpoint errors InternalException, LimitExceededException and ResourceAlreadyExistsException; the refusal below is not modelled). Recorded response: HTTP 400 with __type 'ValidationException' and message 'A health check with ARN <healthCheckArn> does not exist.' — fixed text 'A health check with ARN ' + the Route 53 health-check ARN taken verbatim from the request's RoutingConfig.FailoverConfig.Primary.HealthCheck + ' does not exist.' The ARN is a request value, not part of the template.
Emulator: answers HTTP 200 and creates the endpoint; no __type and no message in the body.
Why: a client's failover endpoint comes up referencing a health check that cannot exist, so the primary/secondary routing it was configured for can never be evaluated — and the error a caller is written to catch never arrives, so a misconfiguration is silently promoted to a live resource.
Fix: CreateEndpoint validation in the events crate — resolve RoutingConfig.FailoverConfig.Primary.HealthCheck before any state is written, and refuse with the ValidationException above.
Traces: events-endpoint-create-validation-error#task#2 (cluster also covers events-endpoint-create-validation-errors#task#2 and events-endpoint-errors#task#1).

Cause 2 of 4 — the endpoint that should never have been created is visible in ListEndpoints.
AWS: Recorded behavior: because the CreateEndpoint above is refused, ListEndpoints answers 200 with no Endpoints entry for that name.
Emulator: answers 200 with an extra Endpoints[0] element describing the rejected endpoint (name, routing config carrying the nonexistent health check, replication config, both event buses, endpoint id/url, State 'CREATING').
Why: this is the state leak of cause 1 — the refusal must leave no resource behind. A caller that retries after handling the expected 400 then hits ResourceAlreadyExistsException for an endpoint it was told was not created. It also shows State never settles for endpoints (reported 'CREATING').
Fix: CreateEndpoint must validate before mutating state (same handler as cause 1); ListEndpoints itself is reading the store correctly.
Traces: events-endpoint-create-validation-error#task#3.

Cause 3 of 4 — DescribeEndpoint answers 200 for an endpoint that does not exist.
AWS: Smithy: DescribeEndpoint binds ResourceNotFoundException (client error, HTTP 400) — 'An entity that you specified does not exist.' Recorded response: HTTP 400, __type 'ResourceNotFoundException', message 'Endpoint <name> does not exist in region <region>.' — fixed text 'Endpoint ' + the Name from the request + ' does not exist in region ' + the region the call was made in + '.'
Emulator: answers HTTP 200 with neither __type nor message.
Why: a caller using DescribeEndpoint as an existence probe (the standard idiom: catch ResourceNotFoundException to decide create-vs-update) concludes the endpoint exists and skips creating it.
Fix: the DescribeEndpoint handler must look the endpoint up and raise ResourceNotFoundException with the message above instead of serializing an empty/absent record as 200.
Traces: events-endpoint-errors#task#2.

Cause 4 of 4 — UpdateEndpoint answers 200 for an endpoint that does not exist.
AWS: Smithy: UpdateEndpoint binds ResourceNotFoundException (client error, HTTP 400). Recorded response: HTTP 400, __type 'ResourceNotFoundException', message 'Endpoint <name> does not exist in region <region>.' (same template as cause 3, values from the request and the call's region).
Emulator: answers HTTP 200 with no __type/message.
Why: an update against a missing endpoint reports success, so a client believes it applied routing/replication changes that were applied to nothing.
Fix: the UpdateEndpoint handler's existence check, sharing the lookup/refusal path with DescribeEndpoint.
Traces: events-endpoint-errors#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1:
- events|CreateEndpoint|status=400->200: status expected 400, got 200; $.__type expected 'ValidationException', got None; $.message expected 'A health check with ARN arn:aws:route53:::healthcheck/<id> does not exist.', got None. Cluster of 3 steps (events-endpoint-create-validation-error#task#2, events-endpoint-create-validation-errors#task#2, events-endpoint-errors#task#1).
- events|ListEndpoints|status=200->200: $.Endpoints[0] expected absent, got the rejected endpoint record (RoutingConfig.FailoverConfig.Primary.HealthCheck = the nonexistent health-check ARN, State 'CREATING').
- events|DescribeEndpoint|status=400->200: status expected 400, got 200; $.__type expected 'ResourceNotFoundException', got None; $.message expected 'Endpoint <name> does not exist in region us-west-2.', got None.
- events|UpdateEndpoint|status=400->200: identical shape to DescribeEndpoint above.

## Steps to Reproduce
1. Replay fixtures events-endpoint-create-validation-error and events-endpoint-errors (replay_wire_corpus).
2. task#2 of events-endpoint-create-validation-error: CreateEndpoint whose RoutingConfig.FailoverConfig.Primary.HealthCheck names a Route 53 health check that does not exist -> expect 400 ValidationException 'A health check with ARN <arn> does not exist.'; emulator answers 200.
3. task#3 ListEndpoints -> expect no entry for that endpoint name; emulator lists it.
4. events-endpoint-errors task#2 DescribeEndpoint and task#3 UpdateEndpoint on a never-created endpoint name -> expect 400 ResourceNotFoundException 'Endpoint <name> does not exist in region <region>.'; emulator answers 200 both times.
