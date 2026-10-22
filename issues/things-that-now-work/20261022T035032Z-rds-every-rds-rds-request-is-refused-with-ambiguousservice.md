Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__rds__scheduler.xsvc-rds-3-scheduler-stop-db-instance-denied-without-permission-then-stops-with-it
Issue-Key: events__rds__scheduler.xsvc-rds-3-scheduler-stop-db-instance-denied-without-permission-then-stops-with-it|run#4|ResponseParserError|version = rds.describe_db_engine_versions(Engine=_ENGINE, DefaultOnly=True)[
Issue-Commit: 37e4b54b7f2f231778aa0090f22742466dbbad1b

# Every RDS ('rds') request is refused with AmbiguousService — three emulated services claim the 'rds' signing name — and the refusal is sent as a JSON body to a query-protocol client, so botocore cannot even parse it

> _Note: this issue was identified against the code base at commit `37e4b54b7f2f231778aa0090f22742466dbbad1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBEngineVersions

## Description
Two tightly-coupled defects in the front door, both proven by one check. Both must be closed.

=== Cause 1 of 2: the router refuses every 'rds' request as AmbiguousService ===
AWS: recorded behavior only for the routing layer. Amazon RDS is reached with sigv4 signing name / endpoint prefix 'rds', and a request so signed is dispatched to RDS; DescribeDBEngineVersions (Smithy: com-amazonaws-rds-amazonrdsv19, awsQuery protocol) answers 200 with a DBEngineVersions list, or a modeled query-protocol error. 'AmbiguousService' is not an AWS error code and appears in no AWS model.
Emulator: the router answers the body {"__type":"AmbiguousService","message":"3 emulated services all claim requests for 'rds': rds, rds, rds; the request is rejected rather than served by an arbitrary one"} for every 'rds' request. The collision is between the three registered services that share the RDS wire identity — service-com-amazonaws-rds-amazonrdsv19, service-com-amazonaws-docdb-amazonrdsv19 and service-com-amazonaws-neptune-amazonrdsv19 — all three of which are stood up, so the bootstrapped RDS service is unreachable.
Why: every RDS operation is 100% unavailable; a client receives an error code that exists in no AWS SDK and cannot be mapped to a modeled exception. Any cross-service scenario that touches an RDS instance (e.g. EventBridge Scheduler's universal aws-sdk:rds:stopDBInstance target) cannot run at all.
Fix: the request router's service-resolution table — the signing name 'rds' must resolve to exactly one registered service (DocumentDB and Neptune share the amazonrdsv19 model but have their own endpoint prefixes, docdb/neptune, and must be registered under those) instead of rejecting. Same root defect as open issues 20261021T234042Z-osis-router-refuses-every-sigv4-iam-request-as-ambiguousservice, 20261022T031220Z-wisdom-all-wisdom-requests-rejected-with-ambiguousservice-two-emulated and 20261022T032542Z-application-autoscaling-lambda-lambda-is-unreachable-four-emulated-services-all-claim; none covers the 'rds' collision.

=== Cause 2 of 2: the router's error body ignores the target service's wire protocol ===
AWS: RDS uses the awsQuery protocol (Smithy: com-amazonaws-rds-amazonrdsv19), so every RDS error — including a front-door refusal — is an XML <ErrorResponse> document with Error/Code, Error/Message and a RequestId, and botocore's query error parser reads exactly that.
Emulator: the router emits a JSON body ({"__type":...,"message":...}) regardless of the target service's protocol. Against a query-protocol client this is unparseable: botocore raises ResponseParserError ("Unable to parse response (not well-formed (invalid token): line 1, column 0), invalid XML received") instead of a ClientError, so the caller never even sees an error code and cannot handle the failure.
Why: a client gets a parser crash rather than a typed error; the refusal is invisible to any error-handling path, and the same bug will garble every router-level refusal (throttling, unknown operation, auth) for all query/XML-protocol services.
Fix: the router's error serializer — select the error encoding from the resolved/target service's protocol (XML ErrorResponse for awsQuery and restXml, JSON __type for awsJson/restJson) rather than always emitting JSON.

Traces (both causes): events__rds__scheduler.xsvc-rds-3-scheduler-stop-db-instance-denied-without-permission-then-stops-with-it

## Evidence
events__rds__scheduler.xsvc-rds-3-scheduler-stop-db-instance-denied-without-permission-then-stops-with-it [error], on the check's first RDS call (rds.describe_db_engine_versions(Engine="mysql", DefaultOnly=True)):

  botocore.parsers.ResponseParserError: Unable to parse response (not well-formed (invalid token): line 1, column 0), invalid XML received. Further retries may succeed:
  b'{"__type":"AmbiguousService","message":"3 emulated services all claim requests for \'rds\': rds, rds, rds; the request is rejected rather than served by an arbitrary one"}'

Expected (Cause 1): the request is routed to RDS and DescribeDBEngineVersions answers 200 with DBEngineVersions.
Expected (Cause 2): any refusal is an awsQuery XML <ErrorResponse> with Error/Code and Error/Message, which botocore surfaces as a ClientError.
Actual: a JSON AmbiguousService body that the query-protocol parser cannot read at all.

## Steps to Reproduce
1. Boot the emulator.
2. rds = boto3.client("rds", endpoint_url=<emulator>)
3. rds.describe_db_engine_versions(Engine="mysql", DefaultOnly=True)  ->  botocore.parsers.ResponseParserError; the raw body is the JSON AmbiguousService document above. Every other RDS operation (create_db_instance, stop_db_instance, describe_db_instances) fails identically.
