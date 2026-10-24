Issue-Severity: HIGH
Issue-Gap-Class: stub
Issue-Commit: b4572519ed5f295b6edc2ad3f0f0ced732a31b98

# Implement codeguruprofiler GetFindingsReportAccountSummary: unimplemented scaffold stub

_Filed: 2026-10-24T06:11:47Z_
_Operation: GetFindingsReportAccountSummary | Class: stub_

> _Note: this issue was identified against the code base at commit `b4572519ed5f295b6edc2ad3f0f0ced732a31b98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetFindingsReportAccountSummary` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/get_findings_report_account_summary.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

## Evidence

The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/get_findings_report_account_summary.rs`.

## Steps to Reproduce

1. Boot the codeguruprofiler emulator
2. Call GetFindingsReportAccountSummary and observe HTTP 501 with no state persisted
