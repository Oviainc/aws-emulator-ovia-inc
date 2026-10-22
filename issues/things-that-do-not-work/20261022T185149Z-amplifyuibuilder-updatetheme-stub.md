Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplifyuibuilder.update-theme-persists-value-changes
Issue-Shape-Id: com.amazonaws.amplifyuibuilder#AmplifyUIBuilder
Issue-Key: amplifyuibuilder|UpdateTheme|NotImplemented
Issue-Commit: 6136e2766a973f12a1067e5538f7a2dced441244

# Implement amplifyuibuilder UpdateTheme: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6136e2766a973f12a1067e5538f7a2dced441244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateTheme

## Description
`UpdateTheme` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_theme.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateTheme` call, so each one unblocks only when `UpdateTheme` works: `amplifyuibuilder.update-theme-persists-value-changes`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_theme.rs`. Replaying the corpus, the emulator refused the `UpdateTheme` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateTheme operation: operation UpdateTheme is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the amplifyuibuilder emulator
Call UpdateTheme and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

## Status note (added by the DeleteTheme fix)

`UpdateTheme` was IMPLEMENTED as part of the fix for
`20261022T185149Z-amplifyuibuilder-deletetheme-stub.md`: that issue's recorded unit
(`amplifyuibuilder.delete-theme-state-transition-then-not-found`) also requires the deleted
theme to be absent for `UpdateTheme`, which it could not be while the operation answered
501. `src/ops/update_theme.rs` now merges the `updatedTheme` patch into the stored
[`StoredTheme`] record, refuses an absent id with the untyped 404 and a payload naming
another theme with the untyped 400, and its `//! SCAFFOLDED, NOT IMPLEMENTED:` marker is
deleted; `tests/all/theme_update.rs` covers it end to end. This report is left here only
because its own recorded unit
(`amplifyuibuilder.update-theme-persists-value-changes`) has not been replayed from this
session — whoever picks it up should replay that unit and, if it passes, close this report
rather than re-implementing the handler.
