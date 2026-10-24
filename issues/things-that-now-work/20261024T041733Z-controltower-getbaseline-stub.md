Issue-Severity: HIGH
Issue-Gap-Class: stub
Issue-Commit: 6395e76ed9f748dc97d54e4052a4e63524456c87

# Implement controltower GetBaseline: unimplemented scaffold stub

_Filed: 2026-10-24T04:17:33Z_
_Operation: GetBaseline | Class: stub_

> _Note: this issue was identified against the code base at commit `6395e76ed9f748dc97d54e4052a4e63524456c87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetBaseline` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/get_baseline.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

## Evidence

The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/get_baseline.rs`.

## Steps to Reproduce

1. Boot the controltower emulator
2. Call GetBaseline and observe HTTP 501 with no state persisted
