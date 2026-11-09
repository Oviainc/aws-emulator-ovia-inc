Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent.deploy-knowledge-base-cfn-behaves
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent|StartIngestionJob|ValidationException
Issue-Commit: 19c62db1539bfab1a25075fa0f8ae1a77a215876

# bedrock-agent: a CloudFormation AWS::Bedrock::DataSource hands back the template Name as its DataSourceId, so StartIngestionJob refuses the CFN-deployed data source

> _Note: this issue was identified against the code base at commit `19c62db1539bfab1a25075fa0f8ae1a77a215876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartIngestionJob

## Description
Cause 1 of 1: GetAtt <DataSource>.DataSourceId on a CloudFormation-provisioned AWS::Bedrock::DataSource resolves to the data source's template Name instead of the data-source id bedrock-agent issued, so every bedrock-agent call that takes a dataSourceId is refused for anything created through CloudFormation.

AWS: Smithy (service bedrock-agent, com.amazonaws.bedrockagent): CreateDataSource.name has pattern ^([0-9a-zA-Z][_-]?){1,100}$ while every dataSourceId member targets Id with pattern ^[0-9a-zA-Z]{10}$ (StartIngestionJob.dataSourceId, GetIngestionJob.dataSourceId, GetDataSource.dataSourceId are all httpLabel, required, pattern ^[0-9a-zA-Z]{10}$). A data source therefore has a 10-character alphanumeric id distinct from its name, and that id is what AWS::Bedrock::DataSource's DataSourceId attribute returns; StartIngestionJob on it answers 202 with the ingestionJob.
Emulator: the stack Output declared as {"Fn::GetAtt": ["DataSourceResource", "DataSourceId"]} comes back as the Name the template set (a long name of the form <prefix>-<suffix>, e.g. the run's 'sim2real-ds-<hex>'), so the following StartIngestionJob is refused 400 ValidationException with message "1 validation error detected: Value '<the value GetAtt returned>' at 'dataSourceId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-zA-Z]{10}$". Fixed text of that message is "1 validation error detected: Value '" ... "' at 'dataSourceId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-zA-Z]{10}$"; the quoted value is whatever the request carried. The refusal itself is correct for that input — the defect is the value CloudFormation handed out.
Why: a client that deploys a knowledge base + data source with CloudFormation and then drives ingestion through the API (StartIngestionJob / GetIngestionJob / GetDataSource, the documented pattern) can never address the resource it just created: every call is refused with a validation error, and the id it exported is unusable for the whole lifetime of the stack.
Fix: the AWS::Bedrock::DataSource resource provisioning in the CloudFormation resource-type handler — mint/record the data source's bedrock-agent id and resolve the DataSourceId attribute (and the resource's PhysicalResourceId, which AWS forms as '<knowledgeBaseId>|<dataSourceId>') from it rather than from the template Name.
Traces: bedrock-agent.deploy-knowledge-base-cfn-behaves.

## Evidence
Failing check bedrock-agent.deploy-knowledge-base-cfn-behaves [error]:
botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the StartIngestionJob operation: 1 validation error detected: Value 'sim2real-ds-03bdf9b581cc' at 'dataSourceId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-zA-Z]{10}$

'sim2real-ds-03bdf9b581cc' is the per-run DS Name the template set via Properties.Name, read back from the stack Output DataSourceId = !GetAtt DataSourceResource.DataSourceId. Expected: a 10-character alphanumeric data-source id, and StartIngestionJob answering 202 with ingestionJob. The name value is per-run; the requirement is that GetAtt DataSourceId yield an id matching ^[0-9a-zA-Z]{10}$ that bedrock-agent resolves.

## Steps to Reproduce
1. CreateStack with an AWS::Bedrock::KnowledgeBase (S3_VECTORS) plus an AWS::Bedrock::DataSource whose Properties.Name is a long name, and an Output DataSourceId = !GetAtt <DataSource>.DataSourceId.
2. Wait for CREATE_COMPLETE and read the stack Outputs.
3. Call bedrock-agent StartIngestionJob(knowledgeBaseId=<KnowledgeBaseId output>, dataSourceId=<DataSourceId output>).
Observed: 400 ValidationException on the dataSourceId pattern. Expected: 202 with the ingestion job.
