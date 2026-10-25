Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.list-resource-tags-empty
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms|ListResourceTags|NotImplemented
Issue-Commit: aa31aa657622ba455ff8faedd6354f5956e3380b

# Implement kms ListResourceTags: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `aa31aa657622ba455ff8faedd6354f5956e3380b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListResourceTags

## Description
`ListResourceTags` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kms-trentservice/src/ops/list_resource_tags.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListResourceTags` call, so each one unblocks only when `ListResourceTags` works: `kms.list-resource-tags-empty`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kms-trentservice/src/ops/list_resource_tags.rs`. Replaying the corpus, the emulator refused the `ListResourceTags` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListResourceTags operation: operation ListResourceTags is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the kms emulator
Call ListResourceTags and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

## Status note (added by the TagResource fix)
The Evidence and Steps above describe the code BEFORE the fix for
`20261024T102105Z-kms-tagresource-stub.md`. That fix implemented
`ListResourceTags` and deleted its `//! SCAFFOLDED, NOT IMPLEMENTED:` marker,
because it is this service's only reader of a key's tags and so the only way to
prove the `TagResource` write it was fixing: the operation now reports the tag
map the key holds, paged by `Limit`/`Marker`, covered by
`services/service-com-amazonaws-kms-trentservice/tests/all/resource_tags.rs`.
This report stays filed here for its own work item to close, which is the only
place its recorded unit (`kms.list-resource-tags-empty`) can be replayed and
the report moved.
