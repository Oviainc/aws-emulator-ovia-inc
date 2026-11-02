Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__resourcegroupstaggingapi.table-native-tags-visible-through-get-resources
Issue-Key: dynamodb__resourcegroupstaggingapi.table-native-tags-visible-through-get-resources|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# DynamoDB native TagResource/UntagResource writes never reach the Resource Groups Tagging index, so GetResources(dynamodb:table) never finds a tagged table

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
One cause.

Cause 1 of 1: a tag written with DynamoDB's own TagResource is not discoverable through ResourceGroupsTaggingAPI GetResources, and the index never converges.

AWS: DynamoDB TagResource (Smithy: dynamodb#TagResource, input ResourceArn (required, length 1..1283) + Tags (required TagList); output empty; errors ResourceNotFoundException/ResourceInUseException/LimitExceededException, all 400) writes into the account's tag store that Resource Groups Tagging API reads. ResourceGroupsTaggingAPI GetResources (Smithy: resourcegroupstaggingapi#GetResources, input TagFilters (list, max 50), ResourceTypeFilters, ResourceARNList (1..100); output ResourceTagMappingList of ResourceTagMapping{ResourceARN, Tags}) therefore returns exactly one ResourceTagMapping whose ResourceARN equals the table's own TableDescription.TableArn and whose Tags carry the natively written key/value, within a short eventual-consistency window. DynamoDB UntagResource then withdraws the entry so the same filtered query returns an empty ResourceTagMappingList. Recorded behavior shows the index trailing the write by only a few seconds.

Emulator: the check creates a table, waits for table_exists, calls dynamodb TagResource, then polls GetResources with ResourceTypeFilters=["dynamodb:table"] and a TagFilters key/value filter. The predicate len(mappings)==1 is never satisfied — GetResources keeps answering with no mapping for the table — and the check is killed at the 60 s harness budget without ever reaching the untag half. The emulator's per-service tag stores and its tagging-API index are not the same store: tags written through DynamoDB are invisible to GetResources.

Why: an agent or client that discovers its estate through the tagging API (the normal way to answer "which tables carry tag X") gets an empty answer for resources that really are tagged, and any cleanup/cost/compliance workflow driven by GetResources silently skips every DynamoDB table tagged through DynamoDB's own API. The inverse is equally wrong: an untag through DynamoDB cannot be observed to retract the index entry.

Fix: the DynamoDB TagResource/UntagResource handlers and their tag state must publish into the shared cross-service tagging index that resourcegroupstaggingapi GetResources reads (and GetResources must resolve the dynamodb:table resource type against it), rather than keeping a DynamoDB-private tag map. Related open issues report the same missing-index symptom for other resource types: 20261024T043150Z-resource-groups-tagging-api-getresources-never-answers-for.md (Step Functions) and 20261024T043741Z-apigateway-lambda-logs-resource-groups-t-apigateway-lambda-logs-resource-groups-tagging-api-sqs.md (API Gateway api keys); neither covers dynamodb:table.

Traces: dynamodb__resource-groups-tagging-api.table-native-tags-visible-through-get-resources (mismatch, timed out at 60 s while polling GetResources).

## Evidence
Expected: after ddb.tag_resource(ResourceArn=<TableArn>, Tags=[{Key:k,Value:v}]), rgt.get_resources(ResourceTypeFilters=["dynamodb:table"], TagFilters=[{Key:k,Values:[v]}]) returns exactly 1 ResourceTagMapping with ResourceARN == TableArn and Tags containing k=v; after ddb.untag_resource(TagKeys=[k]) the same query returns [].
Actual: GetResources never returns a mapping for the table; the polling loop (predicate len(mappings)==1) never converges and the check is terminated: "Check timed out after 60.0s". No assertion on the untag half was ever reached.

## Steps to Reproduce
1. CreateTable (PAY_PER_REQUEST, single HASH key) and wait for ACTIVE.
2. dynamodb TagResource on the table's TableArn with one key/value pair.
3. Poll resourcegroupstaggingapi GetResources with ResourceTypeFilters=["dynamodb:table"] and TagFilters=[{Key:k,Values:[v]}].
4. Observe the ResourceTagMappingList stays empty instead of returning the tagged table.
