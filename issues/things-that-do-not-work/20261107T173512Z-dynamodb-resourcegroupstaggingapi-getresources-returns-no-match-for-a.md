Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__resourcegroupstaggingapi.table-native-tags-visible-through-get-resources
Issue-Key: dynamodb__resourcegroupstaggingapi.table-native-tags-visible-through-get-resources|run#12|AssertionError|assert len(found) == 1, f"GetResources matched {len(found)} resources, expected exactly 1: {found!r}"
Issue-Commit: edd8d27229a8bc99e308a4b32d393f13d6f32cb7

# dynamodb__resourcegroupstaggingapi: GetResources returns no match for a DynamoDB table tagged through DynamoDB's own tag plane

> _Note: this issue was identified against the code base at commit `edd8d27229a8bc99e308a4b32d393f13d6f32cb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResources

## Description
Cause 1 of 1: tags written by a resource's native service are not indexed for the Resource Groups Tagging API, so GetResources finds nothing.
AWS: Smithy: resourcegroupstaggingapi GetResources (POST /GetResources, awsJson; input TagFilters, ResourceTypeFilters, ResourceARNList; output ResourceTagMappingList, paginated on PaginationToken). Tags applied through a resource's own API — here dynamodb TagResource / CreateTable's Tags — are visible through the tagging index: GetResources with a TagFilter naming that key/value (optionally narrowed by ResourceTypeFilters ['dynamodb:table']) returns exactly one ResourceTagMappingList entry whose ResourceARN is the table ARN and whose Tags carry the key/value that was set.
Emulator: GetResources matches nothing for a table that was just tagged natively: `GetResources matched 0 resources, expected exactly 1: []`. The call succeeds with an empty ResourceTagMappingList, so the absence is indistinguishable from the tag never having been written.
Why: tag-driven discovery, cost allocation and tag-condition tooling all read through GetResources; a client that tags resources at the service level sees an empty inventory and must conclude its tags were not applied.
Fix: the resourcegroupstaggingapi resource index and the dynamodb tag-write path that should feed it — a native TagResource/CreateTable(Tags) must register the resource ARN and its tags with the tagging index that GetResources queries.
Traces: dynamodb__resourcegroupstaggingapi.table-native-tags-visible-through-get-resources. Related open issue that does not carry this check's replay key: 20261102T083644Z-resourcegroupstaggingapi-getresources-never-returns-resources-tagged-by-their-native.md.

## Evidence
Expected: GetResources with a TagFilter matching the tag set via dynamodb TagResource returns exactly one ResourceTagMappingList entry, ResourceARN = the table ARN. Actual: `AssertionError: GetResources matched 0 resources, expected exactly 1: []` — a 200 with an empty ResourceTagMappingList.

## Steps to Reproduce
CreateTable (or TagResource) with a distinctive tag key/value, then call resourcegroupstaggingapi GetResources with TagFilters=[{Key: <key>, Values: [<value>]}] (and optionally ResourceTypeFilters=['dynamodb:table']). Observe an empty ResourceTagMappingList. Re-replay dynamodb__resourcegroupstaggingapi.table-native-tags-visible-through-get-resources.
