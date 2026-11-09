Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent__cloudformation__iam__s3__s3vectors.deploy-data-source-cfn-rejects-unknown-knowledge-base
Issue-Key: bedrock-agent__cloudformation__iam__s3__s3vectors.deploy-data-source-cfn-rejects-unknown-knowledge-base|run#10|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Commit: 19c62db1539bfab1a25075fa0f8ae1a77a215876

# bedrock-agent__cloudformation__s3: an AWS::Bedrock::DataSource naming a knowledge-base id Bedrock never issued reaches CREATE_COMPLETE instead of failing NotFound and rolling the stack back

> _Note: this issue was identified against the code base at commit `19c62db1539bfab1a25075fa0f8ae1a77a215876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataSource

## Description
Cause 1 of 1: the AWS::Bedrock::DataSource resource handler does not resolve its KnowledgeBaseId against bedrock-agent, so a stack that declares a data source on a non-existent knowledge base is reported as successfully created.

AWS: Smithy (com.amazonaws.bedrockagent#CreateDataSource): knowledgeBaseId is httpLabel, required, pattern ^[0-9a-zA-Z]{10}$ and the operation models ResourceNotFoundException (HTTP 404) — "The specified resource Amazon Resource Name (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again." A create against a well-formed but unissued id is therefore refused not-found, and CloudFormation turns that into a CREATE_FAILED resource whose reason is 'Resource handler returned message: "Resource of type \'AWS::Bedrock::KnowledgeBase\' with identifier \'<the KnowledgeBaseId from the template>\' was not found." (RequestToken: <token>, HandlerErrorCode: NotFound)' and the stack ends ROLLBACK_COMPLETE, the rollback removing the other resources the stack had created (the stack's S3 bucket is gone, HeadBucket answering 404 NoSuchBucket). The fixed text is "Resource of type 'AWS::Bedrock::KnowledgeBase' with identifier '" ... "' was not found." with the identifier taken from the template, and HandlerErrorCode NotFound.
Emulator: the stack reaches CREATE_COMPLETE — the data source is created against an id no knowledge base has, no CREATE_FAILED event for the resource is recorded, and no rollback happens.
Why: a client gets a green stack that owns a data source attached to nothing: no ingestion can ever run against it, the mis-typed/stale knowledge-base id is never reported, and the resources the failed stack should have rolled back (here its S3 bucket) are left behind. Deployment pipelines that gate on stack status conclude the deployment succeeded.
Fix: the AWS::Bedrock::DataSource resource handler's create path in cloudformation — call through to bedrock-agent CreateDataSource (which must refuse an unknown knowledgeBaseId with ResourceNotFoundException) and map the refusal to a CREATE_FAILED resource with HandlerErrorCode NotFound so the stack rolls back.
Traces: bedrock-agent__cloudformation__iam__s3__s3vectors.deploy-data-source-cfn-rejects-unknown-knowledge-base.

## Evidence
Failing check bedrock-agent__cloudformation__iam__s3__s3vectors.deploy-data-source-cfn-rejects-unknown-knowledge-base [mismatch]:
AssertionError: expected ROLLBACK_COMPLETE for an unknown knowledge base id, got 'CREATE_COMPLETE'

The template declares an AWS::S3::Bucket plus an AWS::Bedrock::DataSource whose KnowledgeBaseId is a well-formed 10-character id that was never issued. Expected: stack ROLLBACK_COMPLETE; the 'Ds' resource's single CREATE_FAILED reason parsing to message "Resource of type 'AWS::Bedrock::KnowledgeBase' with identifier '<fake kb id>' was not found." with HandlerErrorCode NotFound; and the stack's bucket gone afterwards. The fake id is a per-run value.

## Steps to Reproduce
1. CreateStack with {"DsBucket": AWS::S3::Bucket, "Ds": AWS::Bedrock::DataSource with KnowledgeBaseId set to a 10-character id no knowledge base has, Name set, DataSourceConfiguration Type S3 with S3Configuration.BucketArn = GetAtt DsBucket.Arn}.
2. Poll DescribeStacks until a terminal status.
Observed: CREATE_COMPLETE. Expected: ROLLBACK_COMPLETE, a CREATE_FAILED event on 'Ds' with the NotFound handler reason above, and the stack's bucket removed (HeadBucket refused).
