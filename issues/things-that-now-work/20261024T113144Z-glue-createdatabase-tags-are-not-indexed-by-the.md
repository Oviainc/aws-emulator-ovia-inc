Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__resourcegroupstaggingapi.glue-database-tags-visible-via-resource-groups-tagging-api
Issue-Key: glue__resourcegroupstaggingapi.glue-database-tags-visible-via-resource-groups-tagging-api|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Glue CreateDatabase Tags are not indexed by the Resource Groups Tagging API: GetResources by the database ARN returns nothing

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDatabase

## Description
One cause.

Cause 1 of 1: tags supplied to Glue CreateDatabase are never visible to ResourceGroupsTaggingAPI GetResources queried by the database's ARN.

AWS: tags passed in Glue CreateDatabase's Tags map are stored in the account-wide tag store, so ResourceGroupsTaggingAPI GetResources (Smithy: resourcegroupstaggingapi#GetResources, input ResourceARNList length 1..100; output ResourceTagMappingList of ResourceTagMapping{ResourceARN, Tags}) called with ResourceARNList=["arn:aws:glue:<region>:<account>:database/<name>"] returns exactly one ResourceTagMapping whose ResourceARN is that ARN and whose Tags contain every key/value given at creation — with no follow-up TagResource call. Recorded behavior shows this converging within seconds.

Emulator: the check creates a Glue database with Tags={Environment: dev, Project: project1}, then pages GetResources with ResourceARNList=[the database ARN] until exactly one mapping appears. It never appears; the poll loop keeps getting an empty ResourceTagMappingList and the check is killed at the 60 s harness budget. Create-time Glue tags are kept (if at all) only in a Glue-private store that the tagging API does not read, and the glue:database ARN is not resolvable by the index.

Why: a client that enumerates its Glue estate through the tagging API — the documented cross-service way to find resources by tag — sees no Glue databases at all, so tag-driven inventory, cost allocation and cleanup silently omit them even though Glue's own GetTags would echo the tags back.

Fix: the Glue CreateDatabase handler (and the Glue tag state behind GetTags/TagResource/UntagResource) must register the database ARN and its tags in the shared tagging index that resourcegroupstaggingapi GetResources reads, and GetResources must resolve glue:database ARNs against that index. Related open issues describe the same missing index for other resource types (20261024T043150Z-resource-groups-tagging-api-getresources-never-answers-for.md, 20261024T043741Z-apigateway-lambda-logs-resource-groups-t-apigateway-lambda-logs-resource-groups-tagging-api-sqs.md); neither covers glue:database.

Traces: glue__resource-groups-tagging-api.glue-database-tags-visible-via-resource-groups-tagging-api (mismatch, timed out at 60 s while polling GetResources).

## Evidence
Expected: glue.create_database(DatabaseInput={Name:<db>}, Tags={"Environment":"dev","Project":"project1"}) followed by rgt.get_resources(ResourceARNList=["arn:aws:glue:<region>:<account>:database/<db>"]) returns exactly 1 ResourceTagMapping with that ResourceARN and Tags Environment=dev, Project=project1.
Actual: every page of GetResources comes back with an empty ResourceTagMappingList; the bounded poll never satisfies len(mappings)==1 and the check ends with "Check timed out after 60.0s".

## Steps to Reproduce
1. glue CreateDatabase with DatabaseInput={Name: <db>} and Tags={Environment: dev, Project: project1}.
2. Call resourcegroupstaggingapi GetResources with ResourceARNList=["arn:aws:glue:<region>:<account>:database/<db>"], following PaginationToken.
3. Observe no mapping is ever returned for the database ARN.
