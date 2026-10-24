Issue-Severity: HIGH
Issue-Gap-Class: stub
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# Implement sso-admin CreateApplicationAssignment: unimplemented scaffold stub

_Filed: 2026-10-24T06:32:48Z_
_Operation: CreateApplicationAssignment | Class: stub_

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateApplicationAssignment` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/create_application_assignment.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

## Evidence

The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/create_application_assignment.rs`.

## Steps to Reproduce

1. Boot the sso-admin emulator
2. Call CreateApplicationAssignment and observe HTTP 501 with no state persisted
