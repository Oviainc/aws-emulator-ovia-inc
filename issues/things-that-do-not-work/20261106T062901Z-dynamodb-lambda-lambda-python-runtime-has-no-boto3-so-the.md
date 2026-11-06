Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda.e2e-lambda-env-var-repoint-denied-by-stale-role-scoping-lifecycle
Issue-Key: dynamodb__lambda.e2e-lambda-env-var-repoint-denied-by-stale-role-scoping-lifecycle|phase#5|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 5b7d66e77a060af6cd000fd4d7e09192c88c4502

# Lambda python runtime has no boto3, so the DynamoDB env-var repoint lifecycle fails at its first phase

> _Note: this issue was identified against the code base at commit `5b7d66e77a060af6cd000fd4d7e09192c88c4502`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: a python3.12 handler that imports boto3 cannot be imported in the emulator's managed runtime.
AWS: Smithy: Invoke output FunctionError is bound to header X-Amz-Function-Error. The managed python3.x runtimes ship boto3/botocore, so `import boto3` at module scope succeeds and the invoke returns 200 with no X-Amz-Function-Error. Recorded behavior only for the bundled-package list.
Emulator: the invoke returns FunctionError='Unhandled' with payload {"errorMessage":"Unable to import module 'index': No module named 'boto3'","errorType":"Runtime.ImportModuleError","requestId":"...","stackTrace":[]}, so phase 1 of the lifecycle (deploy + first write) already fails.
Why: every handler that calls another AWS service - the normal case - reports a packaging error that does not exist on AWS, and multi-phase scenarios cannot start.
Fix: the python runtime behind the lambda service's Invoke handler must provide boto3/botocore in the module search path. Open issues report this symptom for other service headings (for example 20261105T094343Z-dynamodb-events-lambda-the-lambda-python-runtime-provides.md); this check is listed under `dynamodb__lambda` and is not carried by any of them.
Traces: dynamodb__lambda.e2e-lambda-env-var-repoint-denied-by-stale-role-scoping-lifecycle.

## Evidence
Expected (AWS): the deploy phase's invoke returns 200 with no FunctionError and the handler writes the item to the table named by its environment variable.
Actual (emulator): AssertionError: [phase 1/5 deploy] the handler raised (Unhandled): b'{"errorMessage":"Unable to import module \'index\': No module named \'boto3\'","errorType":"Runtime.ImportModuleError","requestId":"<uuid>","stackTrace":[]}'.

## Steps to Reproduce
1. Replay the behavioral suite and re-run dynamodb__lambda.e2e-lambda-env-var-repoint-denied-by-stale-role-scoping-lifecycle.
2. CreateFunction (python3.12) whose index.py starts with `import boto3`, then Invoke it and read X-Amz-Function-Error and the payload.
