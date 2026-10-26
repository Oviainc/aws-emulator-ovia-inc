Issue-Severity: HIGH
Issue-Gap-Class: stub
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# Implement medialive DescribeReservation: unimplemented scaffold stub

_Filed: 2026-10-24T01:07:38Z_
_Operation: DescribeReservation | Class: stub_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DescribeReservation` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-medialive-medialive/src/ops/describe_reservation.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

## Resolution note

`DescribeReservation` was implemented alongside `PurchaseOffering` (see
`issues/things-that-now-work/20261024T010738Z-medialive-purchaseoffering-stub.md`): a create
must be provable by reading the resource back, and this was the reader. The handler no longer
answers 501 and its `//! SCAFFOLDED, NOT IMPLEMENTED:` census line is gone, so nothing here is
left to implement; `tests/all/reservation_purchase.rs` exercises it end to end.

This record is retired here, by the owner of this issue: the handler was re-read against the
current code base (no census line, every member answered from the store), and
`tests/all/describe_reservation_reporting.rs` was added to pin the read as a KEYED REPORT — each
of several held reservations read back out of purchase order reports its own stored members, a
repeated read reports the same thing, and an id the account holds no reservation for is the
modelled 404 — so a regression to a scaffold is caught by a test this operation owns rather than
by the purchase suite's round trip.

## Evidence

The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-medialive-medialive/src/ops/describe_reservation.rs`.

## Steps to Reproduce

1. Boot the medialive emulator
2. Call DescribeReservation and observe HTTP 501 with no state persisted
