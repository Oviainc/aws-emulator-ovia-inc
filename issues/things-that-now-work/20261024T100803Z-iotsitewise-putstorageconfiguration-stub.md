Issue-Severity: HIGH
Issue-Gap-Class: stub
Issue-Commit: 7ebfb9678d6df6b0182d42dae67a3a93fc35190c

# Implement iotsitewise PutStorageConfiguration: unimplemented scaffold stub

_Filed: 2026-10-24T10:08:02Z_
_Operation: PutStorageConfiguration | Class: stub_

> _Note: this issue was identified against the code base at commit `7ebfb9678d6df6b0182d42dae67a3a93fc35190c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutStorageConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/put_storage_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

## Evidence

The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/put_storage_configuration.rs`.

## Steps to Reproduce

1. Boot the iotsitewise emulator
2. Call PutStorageConfiguration and observe HTTP 501 with no state persisted

## Resolution

Implemented, together with the `DescribeStorageConfiguration` reader that proves
the round trip (see
`issues/things-that-now-work/20261024T100803Z-iotsitewise-describestorageconfiguration-stub.md`
if that report has since been closed too). The setting is one account-wide
record in `src/state.rs`; both operations read and write it there, and
`tests/all/storage_configuration.rs` covers them end to end.

One deliberate divergence is tracked here rather than left implicit: a
`multiLayerStorage` given beside `SITEWISE_DEFAULT_STORAGE` is refused with a
400 `InvalidRequestException`, although no documented rule states that
combination is rejected (the documentation only requires the member FOR
`MULTI_LAYER_STORAGE`). The alternative would be reporting a cold-tier
destination for a configuration that keeps no cold copy, or silently dropping a
member the caller set. Revisit the refusal if a recording shows the real
service accepting it.

Second, a `warmTierRetentionPeriod` is accepted only where the cold tier keeps a
copy (`MULTI_LAYER_STORAGE`), which is the condition the member's own
documentation states ("You can set this only if cold tier is enabled"). It does
not additionally require `warmTier` `ENABLED`, so the documented condition is the
whole of the rule; a test row pins the cold-tier case with no warm-tier state
named, and another pins the refusal on the hot tier.
