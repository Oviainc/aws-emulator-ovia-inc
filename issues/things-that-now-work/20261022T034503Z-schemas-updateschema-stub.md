Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: schemas.delete-last-schema-version-removes-schema schemas.delete-schema-version-keeps-remaining-versions schemas.describe-schema-specific-version-returns-original-content schemas.list-schema-versions-reflects-all-versions schemas.update-schema-idempotency-token-reuses-version schemas.update-schema-invalid-content-rejected-state-unchanged schemas.update-schema-new-content-increments-version
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|UpdateSchema|NotImplemented
Issue-Key: schemas.update-schema-invalid-content-rejected-state-unchanged|run#10|AssertionError|assert err["Code"] == "BadRequestException", f"unexpected error: {err!r}"
Issue-Commit: 37e4b54b7f2f231778aa0090f22742466dbbad1b

# Implement schemas UpdateSchema: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `37e4b54b7f2f231778aa0090f22742466dbbad1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateSchema

## Description
`UpdateSchema` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-schemas-schemas/src/ops/update_schema.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `UpdateSchema` call, so each one unblocks only when `UpdateSchema` works: `schemas.delete-last-schema-version-removes-schema`, `schemas.delete-schema-version-keeps-remaining-versions`, `schemas.describe-schema-specific-version-returns-original-content`, `schemas.list-schema-versions-reflects-all-versions`, `schemas.update-schema-idempotency-token-reuses-version`, `schemas.update-schema-invalid-content-rejected-state-unchanged`, `schemas.update-schema-new-content-increments-version`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-schemas-schemas/src/ops/update_schema.rs`. Replaying the corpus, the emulator refused the `UpdateSchema` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateSchema operation: operation UpdateSchema is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the schemas emulator
Call UpdateSchema and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
