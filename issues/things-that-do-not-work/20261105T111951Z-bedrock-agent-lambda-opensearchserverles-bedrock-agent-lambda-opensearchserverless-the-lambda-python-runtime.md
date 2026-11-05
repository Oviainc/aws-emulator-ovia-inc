Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent__lambda__opensearchserverless.cli-bedrock-agent-invoke-missing-permission-negative bedrock-agent__lambda__opensearchserverless.cli-create-opensearchserverless-collection-lambda-list-positive bedrock-agent__lambda__opensearchserverless.cli-opensearchserverless-collection-missing-permission-negative bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-get-agent-missing-permission-negative bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-invoke-agent-missing-permission-negative bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-invoke-agent-wrong-agent-id-negative bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-retrieve-missing-permission-negative bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-batch-get-collection-positive bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-create-collection-missing-permission-negative bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-create-collection-positive bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-data-plane-access-denied-negative bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-data-plane-search-positive bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-list-collections-missing-permission-negative bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-list-collections-positive bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-tag-resource-positive
Issue-Key: bedrock-agent__lambda__opensearchserverless.cli-bedrock-agent-invoke-missing-permission-negative|run#13|AssertionError|assert "FunctionError" not in neg_meta, f"unexpected function error: {neg_body}"
Issue-Key: bedrock-agent__lambda__opensearchserverless.cli-create-opensearchserverless-collection-lambda-list-positive|run#18|AssertionError|assert "FunctionError" not in meta, f"unexpected function error: {body}"
Issue-Key: bedrock-agent__lambda__opensearchserverless.cli-opensearchserverless-collection-missing-permission-negative|run#16|AssertionError|assert code_str == "AccessDeniedException", f"expected AccessDeniedException, got {code_str} ({neg_body})"
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-get-agent-missing-permission-negative|_invoke#4|AssertionError|raise AssertionError(f"lambda invocation errored: {raw!r}")
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-invoke-agent-missing-permission-negative|_invoke#5|AssertionError|raise AssertionError(f"lambda invocation errored: {raw!r}")
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-invoke-agent-wrong-agent-id-negative|_invoke#3|AssertionError|raise AssertionError(f"lambda invocation errored: {raw!r}")
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-retrieve-missing-permission-negative|_invoke#4|AssertionError|raise AssertionError(f"lambda invocation errored: {raw!r}")
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-batch-get-collection-positive|_invoke#6|AssertionError|raise AssertionError(f"lambda invocation raised an unhandled error: {body}")
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-create-collection-missing-permission-negative|_invoke#6|AssertionError|raise AssertionError(f"lambda invocation raised an unhandled error: {body}")
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-create-collection-positive|_invoke#4|AssertionError|raise AssertionError(f"lambda invocation raised an unhandled error: {body}")
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-data-plane-access-denied-negative|_invoke#15|AssertionError|raise AssertionError(f"lambda invocation raised an unhandled error: {body}")
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-data-plane-search-positive|_invoke#12|AssertionError|raise AssertionError(f"lambda invocation raised an unhandled error: {body}")
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-list-collections-missing-permission-negative|_invoke#3|AssertionError|raise AssertionError(f"lambda invocation raised an unhandled error: {body}")
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-list-collections-positive|_invoke#5|AssertionError|raise AssertionError(f"lambda invocation raised an unhandled error: {body}")
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-tag-resource-positive|_invoke#7|AssertionError|raise AssertionError(f"lambda invocation raised an unhandled error: {body}")
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# bedrock-agent__lambda__opensearchserverless: the Lambda python runtime has no AWS SDK, so no handler can reach Bedrock Agent or OpenSearch Serverless

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1 — the managed python runtime ships no boto3/botocore, so every handler in this group faults at init instead of calling bedrock-agent or opensearchserverless.
AWS: the python3.x managed runtimes bundle boto3/botocore; a handler may `import boto3` and call bedrock-agent:GetAgent/InvokeAgent/Retrieve/RetrieveAndGenerate and opensearchserverless:CreateCollection/BatchGetCollection/ListCollections/TagResource, plus the collection's data-plane endpoint, under its execution role.
Emulator: Invoke answers HTTP 200 with FunctionError='Unhandled' and payload {"errorMessage":"Unable to import module '<module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError"} — the fixed text is "Unable to import module '<module>': No module named 'boto3'" with the handler module substituted.
Why: no in-function call to Bedrock Agent or OpenSearch Serverless can be observed, and the IAM/data-access-policy negatives prove nothing because every leg fails for the same unrelated reason. The open issues 20261029T191222Z-... and 20261102T091854Z-... own other checks in this group, not these.
Fix: the python runtime's bundled site-packages / module search path in the lambda runtime component.
Traces: all check ids in trace_ids.

## Evidence
AssertionError: unexpected function error: {'errorMessage': "Unable to import module 'index': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError'}; AssertionError: expected AccessDeniedException, got None ({...same payload...}). Expected: StatusCode 200 with no FunctionError and the handler's result, or the handler's caught AccessDeniedException.

## Steps to Reproduce
CreateFunction (python3.12) whose handler does `import boto3` and calls opensearchserverless.list_collections / bedrock-agent.get_agent, then Invoke. The emulator answers 200 with FunctionError=Unhandled and Runtime.ImportModuleError naming boto3.
