Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: databrew.delete-dataset-referenced-by-project-rejected databrew.recipe-job-creation-rejects-untrusted-role-and-missing-outputs databrew.recipe-job-filter-drops-rows-from-s3-output databrew.recipe-job-on-mismatched-dataset-schema-fails-and-writes-nothing databrew.recipe-job-rename-and-type-coercion-land-in-output databrew.recipe-version-publish-and-latest-working-split
Issue-Shape-Id: com.amazonaws.databrew#AWSGlueDataBrew
Issue-Key: databrew|CreateRecipe|NotImplemented
Issue-Commit: 982a07ada62dfbeb289a85399a86f8ade68b0ada

# Implement databrew CreateRecipe: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `982a07ada62dfbeb289a85399a86f8ade68b0ada`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRecipe

## Description
`CreateRecipe` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/create_recipe.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `CreateRecipe` call, so each one unblocks only when `CreateRecipe` works: `databrew.delete-dataset-referenced-by-project-rejected`, `databrew.recipe-job-creation-rejects-untrusted-role-and-missing-outputs`, `databrew.recipe-job-filter-drops-rows-from-s3-output`, `databrew.recipe-job-on-mismatched-dataset-schema-fails-and-writes-nothing`, `databrew.recipe-job-rename-and-type-coercion-land-in-output`, `databrew.recipe-version-publish-and-latest-working-split`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/create_recipe.rs`. Replaying the corpus, the emulator refused the `CreateRecipe` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateRecipe operation: operation CreateRecipe is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the databrew emulator
Call CreateRecipe and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
