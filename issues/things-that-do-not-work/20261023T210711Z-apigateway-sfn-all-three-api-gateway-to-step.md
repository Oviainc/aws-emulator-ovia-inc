Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__stepfunctions.xsvc-apigateway-2-sfn-role-missing-start-execution-denied-surfaces-access-denied apigateway__stepfunctions.xsvc-apigateway-2-sfn-start-execution-input-visible-in-describe-execution apigateway__stepfunctions.xsvc-apigateway-2-sfn-start-sync-execution-maps-output-and-failed-shape
Issue-Key: apigateway__stepfunctions.xsvc-apigateway-2-sfn-role-missing-start-execution-denied-surfaces-access-denied|_poll_http#16|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__stepfunctions.xsvc-apigateway-2-sfn-start-execution-input-visible-in-describe-execution|_poll_http#18|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__stepfunctions.xsvc-apigateway-2-sfn-start-sync-execution-maps-output-and-failed-shape|_poll_http#16|Timeout|time.sleep(interval_s)
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# apigateway__sfn: all three API Gateway-to-Step Functions checks hang to the 60 s cap at the deployed stage

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: a request to a deployed REST API stage whose AWS-service integration calls Step Functions never returns.
AWS: the stage answers its invoke URL — the integration's StartExecution input is visible in DescribeExecution; StartSyncExecution's output and failed shape are mapped into the HTTP response; an integration credentials role without states:StartExecution surfaces an access-denied error through the stage. Recorded behavior only for the per-case statuses; what is structural is that every request to a deployed stage produces an HTTP response.
Emulator: no response arrives; all three checks exhaust the harness budget and are reported as "Check timed out after 60.0s".
Why: the API Gateway-to-Step Functions integration — including how an authorization failure in the integration role is surfaced to the caller — is unobservable, and the caller stalls instead of getting an answer.
Fix: the execute-api data-plane request path — stage/route resolution and AWS-service integration dispatch must always produce a response.
Traces: apigateway__sfn.xsvc-apigateway-2-sfn-role-missing-start-execution-denied-surfaces-access-denied, apigateway__sfn.xsvc-apigateway-2-sfn-start-execution-input-visible-in-describe-execution, apigateway__sfn.xsvc-apigateway-2-sfn-start-sync-execution-maps-output-and-failed-shape.

## Evidence
Detail for all three checks is exactly "Error: Check timed out after 60.0s".

## Steps to Reproduce
1. Create a state machine and a REST API with an AWS-service integration to states:StartExecution / StartSyncExecution, deploy to a stage.
2. POST to the stage invoke URL and observe that no response arrives within 60 s.
3. Or replay the check ids in trace_ids.
