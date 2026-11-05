Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__kinesis__lambda.check-kinesis-to-dynamodb-pipeline dynamodb__kinesis__lambda.check-lambda-dynamodb-getitem-without-permission dynamodb__kinesis__lambda.check-lambda-dynamodb-putitem-without-permission dynamodb__kinesis__lambda.check-lambda-kinesis-putrecord-without-permission dynamodb__kinesis__lambda.check-lambda-puts-kinesis-record dynamodb__kinesis__lambda.check-lambda-reads-dynamodb dynamodb__kinesis__lambda.check-lambda-writes-dynamodb
Issue-Key: dynamodb__kinesis__lambda.check-kinesis-to-dynamodb-pipeline|run#10|KeyError|assert result["payload"]["ok"] is True, (
Issue-Key: dynamodb__kinesis__lambda.check-lambda-dynamodb-getitem-without-permission|<lambda>#4|KeyError|lambda p: p["put_ok"] is True,
Issue-Key: dynamodb__kinesis__lambda.check-lambda-dynamodb-putitem-without-permission|run#7|KeyError|assert payload["get_ok"] is True, (
Issue-Key: dynamodb__kinesis__lambda.check-lambda-kinesis-putrecord-without-permission|run#8|KeyError|assert denied["payload"]["ok"] is False, (
Issue-Key: dynamodb__kinesis__lambda.check-lambda-puts-kinesis-record|run#8|KeyError|assert result["payload"]["ok"] is True, f"lambda invocation reported failure: {result}"
Issue-Key: dynamodb__kinesis__lambda.check-lambda-reads-dynamodb|run#7|KeyError|assert result["payload"]["ok"] is True, (
Issue-Key: dynamodb__kinesis__lambda.check-lambda-writes-dynamodb|run#6|KeyError|assert result["payload"]["ok"] is True, f"lambda invoke did not report ok: {result}"
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# dynamodb__kinesis__lambda: the Lambda python runtime has no AWS SDK, so no handler can reach DynamoDB or Kinesis

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1 — the managed python runtime ships no boto3/botocore, so every handler in this group faults at init instead of calling DynamoDB or Kinesis.
AWS: the python3.x managed runtimes bundle boto3/botocore; a handler may `import boto3` and call dynamodb:GetItem/PutItem and kinesis:PutRecord under its execution role, and Invoke returns the handler's payload with no FunctionError — or the handler's caught AccessDeniedException when the role lacks the action.
Emulator: Invoke answers HTTP 200 with FunctionError='Unhandled' and payload {"errorMessage":"Unable to import module '<module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError"}; checks that then read the expected result field raise KeyError or never satisfy their poll predicate.
Why: the Kinesis->Lambda->DynamoDB pipeline and every IAM-boundary control in this group are unobservable; a caller gets an init failure where AWS returns the handler's result. Open issue 20261029T180026Z-dynamodb-kinesis-lambda-event-source-mappings-are-created-for-non-existent.md covers the ESM checks in this group, not these.
Fix: the python runtime's bundled site-packages / module search path in the lambda runtime component.
Traces: all seven check ids in trace_ids.

## Evidence
The checks fail at the point where they read the handler's result; the last observed invoke response is HTTP 200 with FunctionError='Unhandled' and payload {"errorMessage":"Unable to import module '<module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError"}. Expected: StatusCode 200 with no FunctionError and the handler's JSON result.

## Steps to Reproduce
CreateFunction (python3.12) whose handler does `import boto3` and calls dynamodb.put_item / kinesis.put_record, then Invoke it; grant or withhold the action on the execution role for the negative legs.
