Issue-Severity: HIGH
Issue-Gap-Class: stub
Issue-Commit: ab8204cef26f0e4f4f70841aaff540c660da88f2

# Implement wellarchitected CreateTemplateShare: unimplemented scaffold stub

_Filed: 2026-10-24T01:59:23Z_
_Operation: CreateTemplateShare | Class: stub_

> _Note: this issue was identified against the code base at commit `ab8204cef26f0e4f4f70841aaff540c660da88f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateTemplateShare` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/src/ops/create_template_share.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

## Evidence

The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/src/ops/create_template_share.rs`.

## Steps to Reproduce

1. Boot the wellarchitected emulator
2. Call CreateTemplateShare and observe HTTP 501 with no state persisted

## Resolution note

`CreateTemplateShare` is now IMPLEMENTED and its `//! SCAFFOLDED, NOT IMPLEMENTED:` header
line is gone: it was written alongside `ListTemplateShares`
(`issues/things-that-now-work/20261024T015924Z-wellarchitected-listtemplateshares-stub.md`),
because a list of review-template shares cannot be read back end-to-end until something
records one. The handler mints the share's `ShareId` and the `ShareInvitationId` of the
invitation it raises, records the share `PENDING` against the template's `TemplateArn` in
`src/state.rs` (`template_shares`), refuses a self-share (400), a principal that already
holds a share (409) and a token replayed with different parameters (409), replays the
original share for an unchanged token, and meters shares per template (402). It is covered
end-to-end by `tests/all/template_shares.rs`. Nothing remains to do for this report.

