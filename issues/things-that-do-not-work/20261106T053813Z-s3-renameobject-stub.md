Issue-Source: trace-sweep-divergence
Issue-Trace: s3-mvb-rename-object-general-purpose
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-mvb-rename-object-general-purpose
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|stub:RenameObject
Issue-Commit: 4e248580a2e5ff0df0d7326e549cf11eb17f0a22

# Implement s3 RenameObject: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `4e248580a2e5ff0df0d7326e549cf11eb17f0a22`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RenameObject

## Description
`RenameObject` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3-amazons3/src/ops/rename_object.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `RenameObject` call, so each one unblocks only when `RenameObject` works: `s3-mvb-rename-object-general-purpose#task#3`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3-amazons3/src/ops/rename_object.rs`. Replaying the corpus, the emulator refused the `RenameObject` call: $.Error.Message: value_mismatch (expected 'A header you provided implies functionality that is not implemented', got 'A query parameter you provided implies functionality that is not implemented')
$.Error.Header: missing (expected 'x-amz-rename-source', got None)

## Steps to Reproduce
Boot the s3 emulator
Call RenameObject and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
