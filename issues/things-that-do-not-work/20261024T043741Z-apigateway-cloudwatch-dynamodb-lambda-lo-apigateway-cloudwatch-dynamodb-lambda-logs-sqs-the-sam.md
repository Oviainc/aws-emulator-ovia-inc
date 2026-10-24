Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-sam
Issue-Key: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-sam|_post_order#5|Timeout|time.sleep(5.0)
Issue-Commit: 3ecafc116f2b44ac9fb7ddf17b83f092c778a529

# apigateway__cloudwatch__dynamodb__lambda__logs__sqs: the SAM order-service cross-flavor check never completes — it hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `3ecafc116f2b44ac9fb7ddf17b83f092c778a529`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: the SAM-deployed order-intake application never answers at its REST stage, so the check hangs instead of failing.
AWS: a stack deployed with CAPABILITY_AUTO_EXPAND from an AWS::Serverless-2016-10-31 template converges, and the nine legs the check asserts each terminate: the API method and AWS_PROXY integration synthesised from the Events block serve HTTPS at the stage invoke URL, the lambda:InvokeFunction permission is in place, and the SQS event-source mapping delivers. Recorded behavior only for the leg bodies; what is structural is that every stage request and every polled stack/resource status returns.
Emulator: the check reports exactly 'Check timed out after 60.0s' — no HTTP status line and no terminal status arrives, so none of the legs is ever graded. Hypothesis: the same dead execute-api data plane observed for the sibling flavors of this application (see cause 1 of 20261023T210711Z-apigateway-api-gateway-data-plane-is-dead-30-checks.md, which does not carry this key) plus the untransformed AWS::Serverless::* storage described in 20261023T151100Z-apigateway-cloudwatch-dynamodb-lambda-lo-cloudformation-stores-aws-serverless-resources-untransformed-a-deployed.md.
Why: a hang stalls the caller rather than failing it; a client deploying this reference application from SAM gets neither a working endpoint nor an error it can branch on.
Fix: the execute-api data-plane request path for a deployed stage (it must always produce a response) and the SAM transform expansion that must materialise the API/integration/permission/event-source resources the template only declares as Events.
Traces: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-sam.

## Evidence
Error: Check timed out after 60.0s — no HTTP status, no body, no terminal stack/resource status for the SAM flavor of the order-service application (the cfn/cdk/terraform flavors assert the same nine legs with the same helpers).

## Steps to Reproduce
1. create_stack/update_stack the order-intake SAM template with CAPABILITY_AUTO_EXPAND.
2. Drive the nine legs: POST an order at the REST stage invoke URL, then observe the SQS/DynamoDB/logs effects.
3. Observe that the stage request never returns and the check exhausts the 60 s budget.
4. Or replay apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-sam.
