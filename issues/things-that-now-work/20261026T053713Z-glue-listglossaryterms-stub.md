Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.delete-glossary-with-terms-conflict-allowed glue.glossary-deletion-with-existing-term-succeeds-and-orphans-term
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|ListGlossaryTerms|NotImplemented
Issue-Commit: ef9b9c4f59cabcf171f74b1ebb476d6a7b829294

# Implement glue ListGlossaryTerms: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ef9b9c4f59cabcf171f74b1ebb476d6a7b829294`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListGlossaryTerms

## Description
`ListGlossaryTerms` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-glue-awsglue/src/ops/list_glossary_terms.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `ListGlossaryTerms` call, so each one unblocks only when `ListGlossaryTerms` works: `glue.delete-glossary-with-terms-conflict-allowed`, `glue.glossary-deletion-with-existing-term-succeeds-and-orphans-term`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-glue-awsglue/src/ops/list_glossary_terms.rs`. Replaying the corpus, the emulator refused the `ListGlossaryTerms` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListGlossaryTerms operation: operation ListGlossaryTerms is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the glue emulator
Call ListGlossaryTerms and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
