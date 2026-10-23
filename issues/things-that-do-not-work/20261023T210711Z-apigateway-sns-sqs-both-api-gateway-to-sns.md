Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__sns__sqs.xsvc-apigateway-2-sns-publish-message-attributes-reach-sqs-subscriber apigateway__sns__sqs.xsvc-apigateway-2-sns-role-missing-publish-denied-surfaces-authorization-error
Issue-Key: apigateway__sns__sqs.xsvc-apigateway-2-sns-publish-message-attributes-reach-sqs-subscriber|_poll_http#19|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__sns__sqs.xsvc-apigateway-2-sns-role-missing-publish-denied-surfaces-authorization-error|_poll_http#18|Timeout|time.sleep(interval_s)
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# apigateway__sns__sqs: both API Gateway-to-SNS checks hang to the 60 s cap at the deployed stage

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: a request to a deployed REST API stage whose AWS-service integration publishes to SNS never returns.
AWS: the stage answers its invoke URL — the message attributes the integration sets on sns:Publish arrive at the SQS subscriber, and an integration credentials role without sns:Publish surfaces an authorization error through the stage. Recorded behavior only for the per-case statuses; what is structural is that every request to a deployed stage produces an HTTP response.
Emulator: no response arrives; both checks exhaust the harness budget and are reported as "Check timed out after 60.0s".
Why: the API Gateway-to-SNS-to-SQS fan-out and the way an integration-role authorization failure is surfaced to the caller are unobservable, and the caller stalls rather than being answered.
Fix: the execute-api data-plane request path — stage/route resolution and AWS-service integration dispatch must always produce a response.
Traces: apigateway__sns__sqs.xsvc-apigateway-2-sns-publish-message-attributes-reach-sqs-subscriber, apigateway__sns__sqs.xsvc-apigateway-2-sns-role-missing-publish-denied-surfaces-authorization-error.

## Evidence
Detail for both checks is exactly "Error: Check timed out after 60.0s".

## Steps to Reproduce
1. Create an SNS topic with an SQS subscriber, then a REST API with an AWS-service integration to sns:Publish; deploy to a stage.
2. POST to the stage invoke URL and observe that no response arrives within 60 s.
3. Or replay the check ids in trace_ids.
