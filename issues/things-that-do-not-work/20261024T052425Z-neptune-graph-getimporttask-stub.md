Issue-Severity: HIGH
Issue-Gap-Class: stub
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# Implement neptune-graph GetImportTask: unimplemented scaffold stub

_Filed: 2026-10-24T05:24:25Z_
_Operation: GetImportTask | Class: stub_

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetImportTask` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/get_import_task.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

## Evidence

The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/get_import_task.rs`.

## Steps to Reproduce

1. Boot the neptune-graph emulator
2. Call GetImportTask and observe HTTP 501 with no state persisted
