Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__kinesis__lambda.check-cfn-ddb-stream-lambda-deployment
Issue-Key: dynamodb__kinesis__lambda.check-cfn-ddb-stream-lambda-deployment|_deploy_cfn#7|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Commit: 5b7d66e77a060af6cd000fd4d7e09192c88c4502

# CloudFormation stack wiring a DynamoDB stream to a Lambda event source mapping ends ROLLBACK_COMPLETE instead of CREATE_COMPLETE

> _Note: this issue was identified against the code base at commit `5b7d66e77a060af6cd000fd4d7e09192c88c4502`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a template declaring a DynamoDB table with a stream, a function and an AWS::Lambda::EventSourceMapping cannot be created - the stack rolls back.
AWS: CreateStack with that template settles in CREATE_COMPLETE: the table is created with StreamSpecification, the function is created, and the event source mapping is created against the table's LatestStreamArn; DescribeStacks then reports CREATE_COMPLETE and the resources exist.
Emulator: the stack settles in ROLLBACK_COMPLETE ("Stack <name> ended in ROLLBACK_COMPLETE"), so none of the wiring survives. Recorded behavior only for which resource failed - the check only observes the terminal stack status.
Why: the whole deployment path for stream-driven functions is unusable through CloudFormation, and a client gets a rolled-back stack with no working pipeline.
Fix: the CloudFormation provisioning of AWS::Lambda::EventSourceMapping / AWS::DynamoDB::Table (stream ARN resolution between them) - the resource whose create fails must be identified from the stack events and made to succeed.
Traces: dynamodb__kinesis__lambda.check-cfn-ddb-stream-lambda-deployment.

## Evidence
Expected (AWS): the stack reaches CREATE_COMPLETE and the event source mapping exists against the table's stream.
Actual (emulator): AssertionError: Stack <stack-name> ended in ROLLBACK_COMPLETE.

## Steps to Reproduce
1. Replay the behavioral suite and re-run dynamodb__kinesis__lambda.check-cfn-ddb-stream-lambda-deployment.
2. CreateStack with a template containing AWS::DynamoDB::Table (StreamSpecification), AWS::IAM::Role, AWS::Lambda::Function and AWS::Lambda::EventSourceMapping whose EventSourceArn is Fn::GetAtt Table.StreamArn, then poll DescribeStacks and read the stack events.
