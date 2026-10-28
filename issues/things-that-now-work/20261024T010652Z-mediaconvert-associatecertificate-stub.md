Issue-Severity: HIGH
Issue-Gap-Class: stub
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# Implement mediaconvert AssociateCertificate: unimplemented scaffold stub

_Filed: 2026-10-24T01:06:52Z_
_Operation: AssociateCertificate | Class: stub_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AssociateCertificate` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/associate_certificate.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

## Evidence

The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/associate_certificate.rs`.

## Steps to Reproduce

1. Boot the mediaconvert emulator
2. Call AssociateCertificate and observe HTTP 501 with no state persisted

## Update

`AssociateCertificate` was implemented, and its `//! SCAFFOLDED, NOT IMPLEMENTED:` header
line deleted, alongside the fix for
`20261024T010652Z-mediaconvert-disassociatecertificate-stub.md`: the disassociation has
nothing to remove, and nothing a client can observe, unless the door that adds an
association exists, so both handlers and the account's certificate set landed in that one
change (`services/service-com-amazonaws-mediaconvert-mediaconvert/tests/all/certificate_association.rs`
covers them end to end). This report is left here for its own session to close; the
operation it describes is no longer a scaffold.
