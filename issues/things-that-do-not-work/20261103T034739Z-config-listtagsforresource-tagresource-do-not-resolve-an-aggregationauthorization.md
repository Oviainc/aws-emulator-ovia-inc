Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.aggregation-authorization-tags-round-trip-and-deleted-not-found
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config-service|ListTagsForResource|ResourceNotFoundException
Issue-Commit: 70ed2d2e7473e9995477fa4e995826b18e438b3d

# config: ListTagsForResource/TagResource do not resolve an AggregationAuthorization ARN, so a live authorization's tags are unreachable (400 ResourceNotFoundException)

> _Note: this issue was identified against the code base at commit `70ed2d2e7473e9995477fa4e995826b18e438b3d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
REOPENS 20261030T003924Z-config-recorder-tagging-and-retention-refusals-answer-wrong.md: its fix left the call refused. That issue's cause 6/9 corrected the WORDING of the config tagging not-found error for ListTagsForResource/TagResource, and the emulator now emits exactly AWS's sentence — but it emits it for an aggregation authorization that EXISTS, so ListTagsForResource is still refused on a resource AWS answers.

ONE CAUSE.

AWS: ListTagsForResource explicitly supports an AggregationAuthorization ARN — the model documents the taggable resource list on ListTagsForResourceRequest.ResourceArn as ConfigurationRecorder, ConfigRule, OrganizationConfigRule, ConformancePack, OrganizationConformancePack, ConfigurationAggregator, AggregationAuthorization, StoredQuery, Connector (Smithy: ListTagsForResourceRequest$ResourceArn documentation). PutAggregationAuthorization accepts Tags (TagsList, @length 0..50) and returns AggregationAuthorization.AggregationAuthorizationArn of the form arn:<partition>:config:<region>:<accountId>:aggregation-authorization/<authorizedAccountId>/<authorizedAwsRegion> (Smithy: PutAggregationAuthorization input/output). The recording shows that the create-time Tags read back verbatim through ListTagsForResource on that ARN, that TagResource/UntagResource on the same ARN add/overwrite/remove keys, and that only AFTER DeleteAggregationAuthorization does the ARN answer 400 ResourceNotFoundException with the fixed text "Resource " + the ARN from the request + " does not exist for account " + the caller's account id.
Emulator: ListTagsForResource on the ARN that PutAggregationAuthorization has just returned (the authorization is still listed by DescribeAggregationAuthorizations) is refused with 400 ResourceNotFoundException and that same post-delete sentence — i.e. the tagging ARN resolver does not recognise the aggregation-authorization resource type at all, and the Tags supplied to PutAggregationAuthorization are not reachable through any read.
Why: tags on an aggregation authorization are unusable end to end — a client cannot read the tags it just created, cannot add or remove tags, and the "does not exist" refusal actively misreports a resource that does exist, so tag-driven inventory/cost-allocation and any create-then-verify flow break. The genuine post-delete not-found case is also indistinguishable from the live case.
Fix: the shared config tagging ARN resolution/state used by ListTagsForResource, TagResource and UntagResource — register the aggregation-authorization resource type and have PutAggregationAuthorization persist its Tags into that store (and keep the existing not-found sentence for the ARN only once the authorization is deleted).
Traces: config.aggregation-authorization-tags-round-trip-and-deleted-not-found.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout.

config.aggregation-authorization-tags-round-trip-and-deleted-not-found [error]:
  PutAggregationAuthorization(AuthorizedAccountId=<caller account>, AuthorizedAwsRegion=<caller region>, Tags=[{team,audit},{env,dev}]) succeeds and returns an AggregationAuthorizationArn matching the documented form (the check's regex assertion on the ARN passes).
  ListTagsForResource(ResourceArn=<that ARN>) then raises, uncaught:
    botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the ListTagsForResource operation: Resource arn:aws:config:us-west-2:000000000000:aggregation-authorization/000000000000/us-west-2 does not exist for account 000000000000
  expected: 200 with Tags == {team: audit, env: dev}, then the same round trip after TagResource ({owner: platform}, env overwritten to prod) and UntagResource (team removed); ResourceNotFoundException only AFTER DeleteAggregationAuthorization.
  (the ARN, account id 000000000000 and region in the message are per-environment values; the requirement is that a LIVE authorization ARN resolves and carries its tags, and that the not-found sentence appears only post-delete)

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): ListTagsForResourceRequest$ResourceArn's documentation lists AggregationAuthorization among the supported resources; ListTagsForResource models ResourceNotFoundException (client, 400); PutAggregationAuthorization accepts Tags (TagsList 0..50); TagResource/UntagResource take the same AmazonResourceName.

## Steps to Reproduce
1. git clone the repo; build and boot the emulator; run the behavioral suite (simparity-behavioral-aws-v1).
2. Replay config.aggregation-authorization-tags-round-trip-and-deleted-not-found.
3. Directly: PutAggregationAuthorization(AuthorizedAccountId=<caller account>, AuthorizedAwsRegion=<caller region>, Tags=[{Key:team,Value:audit},{Key:env,Value:dev}]) and take AggregationAuthorization.AggregationAuthorizationArn from the response.
4. ListTagsForResource(ResourceArn=<that ARN>) — it must answer 200 with both tags; the emulator answers 400 ResourceNotFoundException.
5. TagResource/UntagResource on the same ARN must also apply, and ListTagsForResource must only answer ResourceNotFoundException ('Resource <arn> does not exist for account <accountId>') after DeleteAggregationAuthorization.
6. Re-replay the check id in trace_ids to confirm the fix.
