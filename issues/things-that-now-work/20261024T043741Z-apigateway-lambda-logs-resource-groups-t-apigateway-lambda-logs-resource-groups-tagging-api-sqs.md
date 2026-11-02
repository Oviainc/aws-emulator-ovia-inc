Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda__logs__resourcegroupstaggingapi__sqs.api-key-discovered-tagged-and-removed-from-index-on-delete
Issue-Key: apigateway__lambda__logs__resourcegroupstaggingapi__sqs.api-key-discovered-tagged-and-removed-from-index-on-delete|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Commit: 3ecafc116f2b44ac9fb7ddf17b83f092c778a529

# apigateway__lambda__logs__resource-groups-tagging-api__sqs: a tagged API Gateway API key never appears in ResourceGroupsTaggingAPI GetResources, so the discovery check polls to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `3ecafc116f2b44ac9fb7ddf17b83f092c778a529`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResources

## Description
Cause 1 of 1: an API Gateway API key created WITH tags is not indexed in the resource-groups tagging index.
AWS: CreateApiKey(tags={...}) registers the key in the tagging index, so GetResources(ResourceTypeFilters=['apigateway:apikeys'], TagFilters=[{Key,Values}]) returns exactly one ResourceTagMappingList entry whose ResourceARN has API Gateway's path-style, account-less shape 'arn:aws:apigateway:<region>::/apikeys/<id>'; TagResources/UntagResources against that discovered ARN land in the key's own GetApiKey tags, and deleting the key retracts the index entry with no explicit untag. Recorded behavior for the ARN shape; the structural claim is that the tagged resource becomes discoverable.
Emulator: the first leg never becomes true — the check's bounded poll of GetResources (filtered to the run tag) never sees the one expected mapping and the check exhausts the harness's 60 s budget ('Check timed out after 60.0s'), so the ARN shape, the tag round-trip and the delete-retraction legs are never reached.
Why: tag-based discovery is how clients enumerate and clean up resources across services; an API key that is tagged at creation but invisible to GetResources makes the tagging index silently incomplete, and a caller that waits for it to appear waits forever rather than getting an empty-but-authoritative answer.
Fix: the tagging index write path — CreateApiKey (and the other apigateway resources' tag writes) must publish the resource and its tags to the resource-groups tagging store under the apigateway:apikeys resource type with the path-style ARN, and delete must retract it. Open issue 20261022T081518Z-resource-groups-tagging-api-tagresources-is-over-permissive.md and 20261023T151100Z-apigateway-lambda-logs-resource-groups-t-resource-groups-tagging-api-tagresources-accepts-a-well.md cover TagResources' over-permissiveness, not this indexing gap, and neither carries this key.
Traces: apigateway__lambda__logs__resource-groups-tagging-api__sqs.api-key-discovered-tagged-and-removed-from-index-on-delete.

## Evidence
Error: Check timed out after 60.0s while the check polled rgt.get_resources(ResourceTypeFilters=['apigateway:apikeys'], TagFilters=[{'Key':'Run','Values':[<run tag>]}]) for len(mappings) == 1 (the check's own budget for that poll is 120 s, so it never saw the key). create_api_key(name=..., enabled=True, tags={'Run': <run tag>, 'Stage': 'initial'}) itself returned an id.

## Steps to Reproduce
1. apigateway.create_api_key(name=<unique>, enabled=True, tags={'Run': <unique>, 'Stage': 'initial'}).
2. resourcegroupstaggingapi.get_resources(ResourceTypeFilters=['apigateway:apikeys'], TagFilters=[{'Key':'Run','Values':[<unique>]}]).
3. Observe the ResourceTagMappingList never contains the key (expected exactly one entry with ResourceARN 'arn:aws:apigateway:<region>::/apikeys/<id>').
4. Or replay apigateway__lambda__logs__resource-groups-tagging-api__sqs.api-key-discovered-tagged-and-removed-from-index-on-delete.
