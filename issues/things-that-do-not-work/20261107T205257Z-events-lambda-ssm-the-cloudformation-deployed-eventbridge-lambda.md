Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__ssm.cfn-deploy-eventbridge-lambda-ssm-pipeline
Issue-Key: events__lambda__ssm.cfn-deploy-eventbridge-lambda-ssm-pipeline|_poll_until#17|AssertionError|raise AssertionError(
Issue-Commit: 959ef406fada22402fcbc8404625fdf48db790f8

# events__lambda__ssm: the CloudFormation-deployed EventBridge->Lambda->SSM pipeline never updates the parameter (the runtime SDK has no 'ssm' client)

> _Note: this issue was identified against the code base at commit `959ef406fada22402fcbc8404625fdf48db790f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutParameter

## Description
Cause 1 of 1: a CloudFormation-deployed pipeline (EventBridge rule -> Lambda -> SSM PutParameter) leaves the parameter at its initial value forever.

AWS: the deployed stack's rule invokes the function for a matching event and the function's boto3.client('ssm').put_parameter(Name=..., Value=..., Type="String", Overwrite=True) writes a new version, so GetParameter returns the value the function wrote within seconds.

Emulator: the parameter stays at 'initial' until the check's budget expires — "effect not met within 120s. Last result: 'initial'". The sibling checks in the same group show the mechanism verbatim: the handler cannot even construct the client — errorType "UnknownServiceError", errorMessage "Unknown service: 'ssm'. Valid service names are: sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager" (fixed text; the valid-name list is the emulator's own).

Why: a client that deploys the standard "config value maintained by an event-driven function" template sees no error anywhere — the stack creates, the rule exists, the function returns 200 — yet the parameter is never updated, so downstream consumers read a stale value indefinitely.

Fix: the Lambda runtime's bundled SDK client registry (it must expose an 'ssm' client and route the call through the normal authorization path). Related open issue 20261105T081225Z-events-lambda-ssm-the-lambda-python-runtime-has.md covers the sibling checks; the suite reports this CFN-deployed check as un-owned.

Traces: events__lambda__ssm.cfn-deploy-eventbridge-lambda-ssm-pipeline.

## Evidence
Expected: GetParameter returns the value written by the event-triggered function.
Actual: AssertionError: effect not met within 120s. Last result: 'initial'. Sibling checks in the same group report {'errorMessage': "Unknown service: 'ssm'. Valid service names are: sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager", 'errorType': 'UnknownServiceError'} from the same handler shape.

## Steps to Reproduce
1. CreateStack with an AWS::Events::Rule, an AWS::Lambda::Function whose handler calls ssm.put_parameter(..., Overwrite=True), the permission/role wiring, and an AWS::SSM::Parameter seeded with 'initial'.
2. PutEvents a matching event.
3. Poll GetParameter for the new value.
Observed: the parameter stays 'initial'; the function faults with UnknownServiceError for 'ssm'.
