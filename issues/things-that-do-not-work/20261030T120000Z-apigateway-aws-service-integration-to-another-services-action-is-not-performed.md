Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb.apigw-aws-integration-mapping-template-writes-ddb
Issue-Key: apigateway__dynamodb.apigw-aws-integration-mapping-template-writes-ddb|_poll_http#12|Timeout|time.sleep(interval_s)
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# apigateway: an AWS-service integration to another service's action is not performed, so a deployed stage answers it `API_CONFIGURATION_ERROR`

> _Split out of `20261023T210711Z-apigateway-dynamodb-all-three-aws-integration-method-setting.md`, whose other two traces (the stage response cache) are fixed. This report carries the one key that is still open, so the defect stays tracked rather than disappearing with the report it was filed in._

## Operation
CreateDeployment (the `execute-api` data plane of the stage it deploys)

## Description
AWS: a deployed stage whose method carries an `AWS`-type integration — `uri`
`arn:aws:apigateway:{region}:dynamodb:action/PutItem`, driven by a request mapping template —
evaluates that template, calls the named action of the named service, and answers with the
integration response the call maps to (200 for the `PutItem` / `GetItem` legs of the trace,
with the item observable through DynamoDB afterwards).

Emulator: the request IS answered — the api's `API_CONFIGURATION_ERROR` gateway response,
`500 {"message":"Internal server error"}` — but no call is made and no template is
evaluated, so a check that polls the stage for the integration's 200 polls until its budget
runs out. `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/service/invoke.rs`
states this posture in its header: `perform_integration` serves `MOCK` in full from the
records, passes an `HTTP_PROXY` naming an endpoint of this emulator through, and refuses
every other type rather than inventing a backend answer.

Why it is not just a missing branch — two capabilities are missing:

1. **No mapping-template evaluation.** The request body the action is called with is
   produced by a Velocity (VTL) template (`Integration$requestTemplates`, "a map of Velocity
   templates"). This tree evaluates none: `is_velocity_template` DETECTS a template and the
   call is refused, because a hand-written subset of the language would silently produce the
   wrong body. No maintained Rust VTL engine crate exists to drive it from — `cargo info
   velocity` resolves to an unrelated TUI crate and `velocity-template` to a 0.0.2
   experiment — so this needs either such a crate appearing or a bounded reference resolver
   (`$input.path`/`$input.json` over a maintained JSONPath crate, `$stageVariables`,
   `$context`) that refuses every directive, argued and reviewed on its own.
2. **No in-process call to another service from this path.** The data plane is not a
   modelled operation. A request carrying no credential is served from
   `Service::override_routing_error` (`vendor/smithy-server-runtime/src/service.rs`), which
   is SYNCHRONOUS and is handed no `OperationContext`, so the `ctx.callback()` route a
   modelled handler uses to call another emulated service in-process is not reachable from
   it. What IS reachable is the shape this crate already uses for wafv2
   (`service::invoke::web_acl_decision`): the other service's own crate exposes a function
   over its own store. Serving this integration that way needs the dynamodb crate to expose
   the item operations' semantics as such a seam, keyed by the action the integration's
   `uri` names, so the request is performed by DynamoDB's own code rather than by a second
   implementation of it in this crate.

## Evidence
The trace's detail is "Error: Check timed out after 60.0s": its helper polls the stage until
the expected status arrives, and the stage answers `500` every time. The refusal itself is
correct for an integration the emulator cannot perform — what is missing is performing it.

## Steps to Reproduce
1. Create a DynamoDB table; create a REST API with a `POST` method whose integration is
   `type: AWS`, `uri: arn:aws:apigateway:{region}:dynamodb:action/PutItem`, with an
   `application/json` request template that maps the method request body to a `PutItem`
   request; deploy to a stage.
2. POST to the stage invoke URL: the stage answers `500 {"message":"Internal server error"}`
   instead of the integration's `200`, and nothing is written to the table.
3. Or replay the check id in Issue-Trace-Ids.
