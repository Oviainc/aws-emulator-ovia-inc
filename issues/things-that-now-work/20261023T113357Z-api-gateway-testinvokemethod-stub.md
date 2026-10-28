Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.cli-apigateway-create-deploy-invoke-lambda apigateway__dynamodb__lambda__logs.cli-apigateway-missing-lambda-permission-rejected
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-create-deploy-invoke-lambda|_run_shell#27|AssertionError|raise AssertionError(
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-missing-lambda-permission-rejected|_run_shell#25|AssertionError|raise AssertionError(
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement api-gateway TestInvokeMethod: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Resolution
Fixed. The code base drifted after this report was filed: `TestInvokeMethod` is implemented in `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/test_invoke_method.rs` — it resolves the `RestApi`, the `Resource` and the verb from the live records, refuses a method carrying no integration as `BadRequestException`, performs the integration through `crate::service::invoke::perform_integration` (the very function the `execute-api` data plane serves a real request through) and reports the status, body, headers, execution log and measured latency — and the file carries no `//! SCAFFOLDED, NOT IMPLEMENTED:` header line, so the stub census already records it as implemented. The duplicate report of the same stub, `issues/things-that-now-work/20261023T125454Z-apigateway-testinvokemethod-stub.md`, is where that work landed.

What this change adds is the coverage that pins the implemented behaviour from the side neither existing suite covered: `services/service-com-amazonaws-apigateway-backplanecontrolservice/tests/all/test_invoke_method_data_plane_parity.rs` asserts, over a table of stored configurations (a `MOCK` that answers, one whose template cannot be evaluated, one refusing an unmapped media type, a type naming an uncallable backend; the declared and accepted media types; the verb; a mapped response header), that a simulated invocation reports exactly what the API's deployed stage serves for the same records, that the simulation applies neither `authorizationType` nor `apiKeyRequired` (over `NONE`, `AWS_IAM`, `CUSTOM` and the API-key gate), and that simulating a call changes nothing a later caller sees.

Still open and tracked elsewhere: an `AWS` / `AWS_PROXY` integration naming a Lambda function is reported through the API's `API_CONFIGURATION_ERROR` gateway response, because no deployment package runs in this emulator — `issues/things-that-do-not-work/20261026T054047Z-apigateway-cloudformation-lambda-api-gateway-aws-proxy-integration-pointing-at-a.md`. The two units named below exercise exactly that Lambda-backed path, so they unblock with that issue rather than with this one.

## Operation
TestInvokeMethod

## Description
`TestInvokeMethod` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/test_invoke_method.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `TestInvokeMethod` call, so each one unblocks only when `TestInvokeMethod` works: `apigateway__dynamodb__lambda__logs.cli-apigateway-create-deploy-invoke-lambda`, `apigateway__dynamodb__lambda__logs.cli-apigateway-missing-lambda-permission-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/test_invoke_method.rs`. Replaying the corpus, the emulator refused the `TestInvokeMethod` call: File "check_apigateway__dynamodb__lambda__logs_cli_apigateway_create_deploy_invoke_lambda.py", line 131, in _run_aws
    proc = _run_shell(ctx, ["aws", "--output", "json", *args], timeout_s=timeout_s, check=check)
  File "check_apigateway__dynamodb__lambda__logs_cli_apigateway_create_deploy_invoke_lambda.py", line 123, in _run_shell
    raise AssertionError(
    ...<2 lines>...
    )
AssertionError: Command failed (rc=254):
  cmd: ['aws', '--output', 'json', 'apigateway', 'test-invoke-method', '--rest-api-id', '8sh3tqaaaa', '--resource-id', '6ofuwnbaaa', '--http-method', 'GET']
  stderr: 
aws: [ERROR]: An error occurred (NotImplemented) when calling the TestInvokeMethod operation: operation TestInvokeMethod is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the api-gateway emulator
Call TestInvokeMethod and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
