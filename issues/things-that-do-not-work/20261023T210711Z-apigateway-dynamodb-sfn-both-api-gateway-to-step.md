Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__stepfunctions.apigw-sfn-jsonata-execution-input-size-limit-rejected apigateway__dynamodb__stepfunctions.apigw-sfn-jsonata-item-crud-lifecycle
Issue-Key: apigateway__dynamodb__stepfunctions.apigw-sfn-jsonata-execution-input-size-limit-rejected|_poll_http#16|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__stepfunctions.apigw-sfn-jsonata-item-crud-lifecycle|_poll_http#19|Timeout|time.sleep(interval_s)
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# apigateway__dynamodb__sfn: both API Gateway-to-Step Functions checks hang to the 60 s cap at the deployed stage

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: a request to a deployed REST API stage whose integration starts a Step Functions execution never returns.
AWS: a deployed stage with an AWS-service integration to states:StartExecution answers the invoke URL with the integration's response (200 for the CRUD lifecycle legs) and refuses an execution input over the Step Functions input-size limit with the modeled refusal. Recorded behavior only for the per-case statuses; what is structural is that every request to a deployed stage produces an HTTP response.
Emulator: no response arrives; both checks exhaust the harness budget and are reported as "Check timed out after 60.0s".
Why: the API Gateway-to-Step Functions path is unobservable — neither the successful CRUD lifecycle nor the input-size refusal can be seen — and the hang stalls the caller instead of answering it.
Fix: the execute-api data-plane request path — stage/route resolution and integration dispatch must always produce a response.
Traces: apigateway__dynamodb__sfn.apigw-sfn-jsonata-execution-input-size-limit-rejected, apigateway__dynamodb__sfn.apigw-sfn-jsonata-item-crud-lifecycle.

## Evidence
Detail for both checks is exactly "Error: Check timed out after 60.0s".

## Steps to Reproduce
1. Create a state machine, then a REST API with an AWS-service integration to states:StartExecution; deploy it to a stage.
2. POST to the stage invoke URL and observe that no response arrives within 60 s.
3. Or replay the check ids in trace_ids.
