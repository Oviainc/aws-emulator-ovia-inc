Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: databrew.recipe-version-publish-and-latest-working-split
Issue-Shape-Id: com.amazonaws.databrew#AWSGlueDataBrew
Issue-Key: databrew.recipe-version-publish-and-latest-working-split|run#29|AssertionError|raise AssertionError(f"DeleteRecipeVersion on LATEST_WORKING raised {code}: {message!r}")
Issue-Commit: abded3a6ac12b18365060f4d039abb5421f29c54

# Implement databrew DeleteRecipeVersion: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `abded3a6ac12b18365060f4d039abb5421f29c54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRecipeVersion

## Description
`DeleteRecipeVersion` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/delete_recipe_version.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteRecipeVersion` call, so each one unblocks only when `DeleteRecipeVersion` works: `databrew.recipe-version-publish-and-latest-working-split`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/delete_recipe_version.rs`. Replaying the corpus, the emulator refused the `DeleteRecipeVersion` call: File "run.py", line 353, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_databrew_recipe_version_publish_and_latest_working_split.py", line 91, in run
    raise AssertionError(f"DeleteRecipeVersion on LATEST_WORKING raised {code}: {message!r}")
AssertionError: DeleteRecipeVersion on LATEST_WORKING raised NotImplemented: 'operation DeleteRecipeVersion is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the databrew emulator
Call DeleteRecipeVersion and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
