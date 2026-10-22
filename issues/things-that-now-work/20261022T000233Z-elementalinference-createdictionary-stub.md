Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elementalinference.create-dictionary-status-available elementalinference.delete-dictionary-referenced-by-feed-rejected elementalinference.delete-dictionary-removes-it elementalinference.dictionary-reference-tracks-referencing-feed elementalinference.export-dictionary-entries-roundtrip elementalinference.get-dictionary-not-found elementalinference.list-dictionaries-pagination-maxresults elementalinference.list-tags-for-resource-not-found elementalinference.tag-resource-list-tags-roundtrip elementalinference.update-dictionary-entries-replaces-not-merges
Issue-Shape-Id: com.amazonaws.elementalinference#ElementalInference
Issue-Key: elementalinference|CreateDictionary|NotImplemented
Issue-Commit: 668d747e62a7a896ef2863c29b158e984b097a40

# Implement elementalinference CreateDictionary: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `668d747e62a7a896ef2863c29b158e984b097a40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDictionary

## Description
`CreateDictionary` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-elementalinference-elementalinference/src/ops/create_dictionary.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

10 recorded unit(s) fail on their `CreateDictionary` call, so each one unblocks only when `CreateDictionary` works: `elementalinference.create-dictionary-status-available`, `elementalinference.delete-dictionary-referenced-by-feed-rejected`, `elementalinference.delete-dictionary-removes-it`, `elementalinference.dictionary-reference-tracks-referencing-feed`, `elementalinference.export-dictionary-entries-roundtrip`, `elementalinference.get-dictionary-not-found`, `elementalinference.list-dictionaries-pagination-maxresults`, `elementalinference.list-tags-for-resource-not-found`, `elementalinference.tag-resource-list-tags-roundtrip`, `elementalinference.update-dictionary-entries-replaces-not-merges`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-elementalinference-elementalinference/src/ops/create_dictionary.rs`. Replaying the corpus, the emulator refused the `CreateDictionary` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDictionary operation: operation CreateDictionary is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the elementalinference emulator
Call CreateDictionary and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
