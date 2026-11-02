Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__resourcegroupstaggingapi.key-tags-visible-through-get-resources-both-directions
Issue-Key: kms__resourcegroupstaggingapi|TagResource|NotImplemented
Issue-Commit: 5ef273b5ec187d9b55db2ccfdad00816d8c5c39a

# Implement kms TagResource: the operation answers NotImplemented, so no KMS key can be tagged natively

> _Note: this issue was identified against the code base at commit `5ef273b5ec187d9b55db2ccfdad00816d8c5c39a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Cause 1 of 1: KMS's own TagResource is a scaffold stub.

AWS: TagResource is a bound KMS operation (awsJson1.1, X-Amz-Target TrentService.TagResource) that adds or edits tags on a customer managed key and returns an empty 200 body; its modeled failures are KMS's own client errors (NotFoundException, InvalidArnException, TagException, KMSInvalidStateException, LimitExceededException, KMSInternalException). NotImplemented is not a KMS error code and no KMS operation can answer it. Tags written here are also what KMS ListResourceTags and the cross-service tagging index must report.
Emulator: "An error occurred (NotImplemented) when calling the TagResource operation: operation TagResource is scaffolded but not implemented in this emulator" — a 501-class refusal from the generated scaffold rather than a tag write.
Why: a caller cannot tag a KMS key at all; key tags are how keys are attributed to owners, environments and cost centres, and neither ListResourceTags nor tagging-API discovery can ever return one. The refusal is a server-class error, so no client retry or fallback recovers it.
Fix: the TagResource handler in the kms service crate (the scaffolded ops/tag_resource stub) plus the key tag state it must write, so ListResourceTags and the tagging index both observe it. The report did not mark this failure as already filed, and no open issue names kms TagResource.
Traces: kms__resource-groups-tagging-api.key-tags-visible-through-get-resources-both-directions (dies on the native TagResource call before any tagging-API assertion runs).

## Evidence
Expected: TagResource(KeyId=<key id>, Tags=[{TagKey,TagValue}]) returns an empty 200 and the tag becomes visible through KMS ListResourceTags and through ResourceGroupsTaggingAPI GetResources.
Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TagResource operation: operation TagResource is scaffolded but not implemented in this emulator.

## Steps to Reproduce
1. Boot the emulator and point a botocore 'kms' client at it.
2. Call create_key(), then tag_resource(KeyId=<key id>, Tags=[{'TagKey':'k','TagValue':'v'}]).
3. Observe the NotImplemented refusal instead of an empty 200.
