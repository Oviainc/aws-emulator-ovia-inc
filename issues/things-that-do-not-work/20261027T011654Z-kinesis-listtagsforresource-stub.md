Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.deploy-cfn-stream-tags-applied kinesis.legacy-and-generic-tag-apis-share-the-same-tag-store kinesis.list-tags-for-resource-nonexistent-arn-rejected kinesis.tag-resource-concurrent-updates-consistent kinesis.tag-resource-concurrent-updates-last-writer-wins kinesis.tag-resource-lifecycle kinesis.tag-resource-unicode-value-round-trip kinesis.untag-resource-nonexistent-arn-fails
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis|ListTagsForResource|NotImplemented
Issue-Key: kinesis.list-tags-for-resource-nonexistent-arn-rejected|run#7|AssertionError|assert error.get("Code") == "ResourceNotFoundException", error
Issue-Commit: d5c75e29b1e90f1499e967e91f7af40c6cd43d58

# Implement kinesis ListTagsForResource: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d5c75e29b1e90f1499e967e91f7af40c6cd43d58`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
`ListTagsForResource` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/list_tags_for_resource.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

8 recorded unit(s) fail on their `ListTagsForResource` call, so each one unblocks only when `ListTagsForResource` works: `kinesis.deploy-cfn-stream-tags-applied`, `kinesis.legacy-and-generic-tag-apis-share-the-same-tag-store`, `kinesis.list-tags-for-resource-nonexistent-arn-rejected`, `kinesis.tag-resource-concurrent-updates-consistent`, `kinesis.tag-resource-concurrent-updates-last-writer-wins`, `kinesis.tag-resource-lifecycle`, `kinesis.tag-resource-unicode-value-round-trip`, `kinesis.untag-resource-nonexistent-arn-fails`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/list_tags_for_resource.rs`. Replaying the corpus, the emulator refused the `ListTagsForResource` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListTagsForResource operation: operation ListTagsForResource is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the kinesis emulator
Call ListTagsForResource and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
