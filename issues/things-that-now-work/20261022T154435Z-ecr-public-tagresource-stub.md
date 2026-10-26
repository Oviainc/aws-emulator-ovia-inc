Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr-public.concurrent-tag-resource-additions-merge ecr-public.tag-resource-invalid-key-characters-rejected ecr-public.tag-resource-lifecycle ecr-public.tag-resource-malformed-arn-rejected ecr-public.tag-resource-reserved-prefix ecr-public.tag-resource-too-many-tags
Issue-Shape-Id: com.amazonaws.ecrpublic#SpencerFrontendService
Issue-Key: ecr-public|TagResource|NotImplemented
Issue-Key: ecr-public.tag-resource-reserved-prefix|run#6|AssertionError|assert (err.get("Code"), err.get("Message")) == expected, f"expected {expected}, got {err}"
Issue-Commit: db1fbaa8559703996072fdf323a0d84f8f863f33

# Implement ecr-public TagResource: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `db1fbaa8559703996072fdf323a0d84f8f863f33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
`TagResource` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ecrpublic-spencerfrontendservice/src/ops/tag_resource.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `TagResource` call, so each one unblocks only when `TagResource` works: `ecr-public.concurrent-tag-resource-additions-merge`, `ecr-public.tag-resource-invalid-key-characters-rejected`, `ecr-public.tag-resource-lifecycle`, `ecr-public.tag-resource-malformed-arn-rejected`, `ecr-public.tag-resource-reserved-prefix`, `ecr-public.tag-resource-too-many-tags`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ecrpublic-spencerfrontendservice/src/ops/tag_resource.rs`. Replaying the corpus, the emulator refused the `TagResource` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TagResource operation: operation TagResource is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ecr-public emulator
Call TagResource and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
