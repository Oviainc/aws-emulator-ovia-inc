Issue-Severity: HIGH
Issue-Gap-Class: stub
Issue-Commit: 7ebfb9678d6df6b0182d42dae67a3a93fc35190c

# Implement iotsitewise DescribeApplication: unimplemented scaffold stub

_Filed: 2026-10-24T10:08:02Z_
_Operation: DescribeApplication | Class: stub_

> _Note: this issue was identified against the code base at commit `7ebfb9678d6df6b0182d42dae67a3a93fc35190c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DescribeApplication` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/describe_application.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

## Evidence

The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/describe_application.rs`.

## Steps to Reproduce

1. Boot the iotsitewise emulator
2. Call DescribeApplication and observe HTTP 501 with no state persisted

## Status note

`DescribeApplication` was implemented alongside `CreateApplication` (the
`20261024T100802Z-iotsitewise-createapplication-stub` fix), because a create has to be
proved through the reader that answers it: the handler now reports the stored
application record, the scaffold marker is gone from
`src/ops/describe_application.rs`, and `tests/all/application_lifecycle.rs` covers the
round trip, the `(workspaceName, id)` addressing and the not-found refusals. Nothing is
left to implement here; this report is kept only so the work item that owns it can close
it.
