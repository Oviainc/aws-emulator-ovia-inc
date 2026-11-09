Issue-Source: trace-sweep-divergence
Issue-Trace: ebs-get-snapshot-block-token-rejections
Issue-Step: teardown#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ebs-get-snapshot-block-token-rejections ebs-list-changed-blocks-rejections ebs-put-snapshot-block-rejections ebs-snapshot-lifecycle-and-block-listing ebs-start-snapshot-idempotency-and-validation ebs-start-snapshot-invalid-inputs
Issue-Shape-Id: com.amazonaws.ebs#Ebs
Issue-Key: ec2|stub:DeleteSnapshot
Issue-Commit: c0919b0ae1fd753648687abf4b08014f282092d3

# Implement ec2 DeleteSnapshot: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c0919b0ae1fd753648687abf4b08014f282092d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSnapshot

## Description
`DeleteSnapshot` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_snapshot.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `DeleteSnapshot` call, so each one unblocks only when `DeleteSnapshot` works: `ebs-get-snapshot-block-token-rejections#teardown#6`, `ebs-list-changed-blocks-rejections#teardown#6`, `ebs-put-snapshot-block-rejections#teardown#6`, `ebs-snapshot-lifecycle-and-block-listing#teardown#6`, `ebs-start-snapshot-idempotency-and-validation#teardown#6`, `ebs-start-snapshot-idempotency-and-validation#teardown#7`, `ebs-start-snapshot-invalid-inputs#teardown#5`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_snapshot.rs`. Replaying the corpus, the emulator refused the `DeleteSnapshot` call: status: expected 200, got 501
$.DeleteSnapshotResponse: missing (expected {'requestId': '<shaped-not-asserted>', 'return': 'true'}, got None)
$headers.content-type: value_mismatch (expected 'text/xml;charset=UTF-8', got 'text/xml')

## Steps to Reproduce
Boot the ec2 emulator
Call DeleteSnapshot and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
