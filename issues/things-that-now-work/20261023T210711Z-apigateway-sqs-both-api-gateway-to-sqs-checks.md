Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__sqs.xsvc-apigateway-2-sqs-form-template-sendmessage-delivers-body-and-md5 apigateway__sqs.xsvc-apigateway-2-sqs-role-missing-sendmessage-denied-surfaces-access-denied
Issue-Key: apigateway__sqs.xsvc-apigateway-2-sqs-form-template-sendmessage-delivers-body-and-md5|_poll_http#20|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__sqs.xsvc-apigateway-2-sqs-role-missing-sendmessage-denied-surfaces-access-denied|_poll_http#19|Timeout|time.sleep(interval_s)
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# apigateway__sqs: both API Gateway-to-SQS checks hang to the 60 s cap at the deployed stage

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: a request to a deployed REST API stage whose AWS-service integration calls SQS SendMessage never returns.
AWS: the stage answers its invoke URL — a form-encoded request mapping template drives sqs:SendMessage and the response carries the message body and its MD5; an integration credentials role without sqs:SendMessage surfaces an access-denied error through the stage. Recorded behavior only for the per-case statuses; what is structural is that every request to a deployed stage produces an HTTP response.
Emulator: no response arrives; both checks exhaust the harness budget and are reported as "Check timed out after 60.0s".
Why: the API Gateway-to-SQS integration, its form-template mapping and the surfacing of an integration-role denial are unobservable, and the caller stalls rather than being answered.
Fix: the execute-api data-plane request path — stage/route resolution, request-template evaluation and AWS-service integration dispatch must always produce a response.
Traces: apigateway__sqs.xsvc-apigateway-2-sqs-form-template-sendmessage-delivers-body-and-md5, apigateway__sqs.xsvc-apigateway-2-sqs-role-missing-sendmessage-denied-surfaces-access-denied.

## Evidence
Detail for both checks is exactly "Error: Check timed out after 60.0s".

## Steps to Reproduce
1. Create an SQS queue and a REST API with an AWS-service integration to sqs:SendMessage driven by a form-encoded request template; deploy to a stage.
2. POST to the stage invoke URL and observe that no response arrives within 60 s.
3. Or replay the check ids in trace_ids.
