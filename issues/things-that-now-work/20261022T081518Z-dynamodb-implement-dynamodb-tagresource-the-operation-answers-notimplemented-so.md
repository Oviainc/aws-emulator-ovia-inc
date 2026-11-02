Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__resourcegroupstaggingapi.table-native-tags-visible-through-get-resources
Issue-Key: dynamodb__resourcegroupstaggingapi|TagResource|NotImplemented
Issue-Commit: 5ef273b5ec187d9b55db2ccfdad00816d8c5c39a

# Implement dynamodb TagResource: the operation answers NotImplemented, so no table can be tagged natively

> _Note: this issue was identified against the code base at commit `5ef273b5ec187d9b55db2ccfdad00816d8c5c39a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Cause 1 of 1: DynamoDB's own TagResource is a scaffold stub.

AWS: TagResource is a bound DynamoDB control-plane operation (awsJson1.0, X-Amz-Target DynamoDB_20120810.TagResource) that applies a tag list to a table ARN and returns an empty 200 body; its modeled failures are the service's own client errors (ResourceNotFoundException, LimitExceededException, ResourceInUseException, InternalServerError). NotImplemented is not a DynamoDB error code and no DynamoDB operation can answer it. The tag written this way is also what the cross-service tagging index must subsequently report.
Emulator: "An error occurred (NotImplemented) when calling the TagResource operation: operation TagResource is scaffolded but not implemented in this emulator" — a 501-class refusal from the generated scaffold rather than a tag write.
Why: a caller cannot tag a DynamoDB table at all, so neither DynamoDB's own ListTagsForResource nor Resource Groups Tagging API discovery can ever see a table tag; any cost-allocation, ownership or discovery workflow built on table tags is dead on the emulator, and the refusal is a server-class error no client retry policy can resolve.
Fix: the TagResource handler in the dynamodb service crate (the scaffolded ops/tag_resource stub) plus the table tag state it must write, so ListTagsForResource and the tagging index both observe it. The report did not mark this failure as already filed, and no open issue names dynamodb TagResource.
Traces: dynamodb__resource-groups-tagging-api.table-native-tags-visible-through-get-resources (dies on the native TagResource call before any tagging-API assertion runs).

## Evidence
Expected: TagResource(ResourceArn=<table arn>, Tags=[{Key,Value}]) returns an empty 200 and the tag becomes visible through DynamoDB's ListTagsForResource and through ResourceGroupsTaggingAPI GetResources.
Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TagResource operation: operation TagResource is scaffolded but not implemented in this emulator.

## Steps to Reproduce
1. Boot the emulator and point a botocore 'dynamodb' client at it.
2. Create a table, then call tag_resource(ResourceArn=<table arn>, Tags=[{'Key':'k','Value':'v'}]).
3. Observe the NotImplemented refusal instead of an empty 200.
