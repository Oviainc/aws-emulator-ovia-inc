Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__resourcegroupstaggingapi.tag-resources-nonexistent-cluster-rejected-per-resource
Issue-Key: ecs__resourcegroupstaggingapi|ListTagsForResource|NotImplemented
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# ECS ListTagsForResource answers 501 NotImplemented, so tags applied to a live cluster through the Tagging API can never be read back

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
One cause.

Cause 1 of 1: ecs ListTagsForResource is an unimplemented scaffold stub, which breaks the cross-service tag-propagation half of a TagResources partial-failure check.

AWS: ECS ListTagsForResource is a normal read operation: given resourceArn of an existing cluster it answers 200 with the cluster's tags list ({key,value} pairs), including tags written through ResourceGroupsTaggingAPI TagResources (Smithy: resourcegroupstaggingapi#TagResources, output FailedResourcesMap of FailureInfo{StatusCode,ErrorCode,ErrorMessage}, whose ErrorCode may be "any valid error code returned by the Amazon Web Services service that hosts the resource"). In the recording, the TagResources call returns 200 with only the absent cluster in FailedResourcesMap (ClusterNotFoundException / 400) and the live cluster's own ListTagsForResource then shows the tag that succeeded.

Emulator: the per-resource failure half of the check passes, but the subsequent ecs ListTagsForResource call fails with HTTP 501 and error code NotImplemented — "operation ListTagsForResource is scaffolded but not implemented in this emulator" — so whether the live cluster actually received the tag cannot be observed at all.

Why: a client cannot read an ECS resource's tags, and the partial-success guarantee of a cross-service TagResources batch (the tags that succeeded really were applied) is unverifiable and probably unimplemented; every tag-driven ECS workflow fails with a 501 that is not a modeled ECS error.

Fix: implement the ECS ListTagsForResource handler against the ECS tag state (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/list_tags_for_resource.rs) and make it reflect tags written through the tagging API. Note the existing open stub issue 20261024T081305Z-ecs-listtagsforresource-stub.md, which names the same operation from the static scaffold sweep — this entry records the behavioral consequence and the two can be closed together.

Traces: ecs__resource-groups-tagging-api.tag-resources-nonexistent-cluster-rejected-per-resource (error, NotImplemented on ListTagsForResource).

## Evidence
Expected: ecs.list_tags_for_resource(resourceArn=<live cluster ARN>) returns 200 with tags including the key written by rgt.tag_resources (value "partial").
Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListTagsForResource operation: operation ListTagsForResource is scaffolded but not implemented in this emulator (HTTP 501).

## Steps to Reproduce
1. ecs CreateCluster with one tag; keep its clusterArn.
2. resourcegroupstaggingapi TagResources with ResourceARNList=[live cluster ARN, a well-formed but absent cluster ARN] and one tag (this part answers 200 with the absent ARN in FailedResourcesMap).
3. Call ecs ListTagsForResource with resourceArn=<live cluster ARN>.
4. Observe 501 NotImplemented instead of the cluster's tags.
