Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.capacity-reservation-cancellation-quote-then-cancel-consistency ec2.capacity-reservation-split-produces-child-reservations ec2.capacity-reservation-usage-reflects-active-reservations ec2.concurrent-capacity-reservation-split-oversubscription-rejected ec2.create-capacity-reservation-by-splitting-insufficient-capacity-rejected ec2.create-capacity-reservation-cancellation-quote-unknown-reservation-rejected ec2.create-interruptible-capacity-reservation-allocation-invalid-target-rejected ec2.groups-for-capacity-reservation-reflects-fleet-association ec2.interruptible-capacity-reservation-allocation-lifecycle ec2.op-contract-acceptCapacityReservationBillingOwnership ec2.op-contract-acceptCapacityReservationBillingOwnership-b ec2.op-contract-associate-capacity-reservation-billing-owner ec2.op-contract-cancel-capacity-reservation ec2.op-contract-disassociate-capacity-reservation-billing-owner ec2.op-contract-get-capacity-reservation-usage ec2.op-contract-get-groups-for-capacity-reservation ec2.op-contract-modify-capacity-reservation-state-conflict ec2.op-contract-reject-capacity-reservation-billing-ownership-rejected ec2.op-contract-update-interruptible-capacity-reservation-allocation
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateCapacityReservation|NotImplemented
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Implement ec2 CreateCapacityReservation: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCapacityReservation

## Description
`CreateCapacityReservation` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_capacity_reservation.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

19 recorded unit(s) fail on their `CreateCapacityReservation` call, so each one unblocks only when `CreateCapacityReservation` works: `ec2.capacity-reservation-cancellation-quote-then-cancel-consistency`, `ec2.capacity-reservation-split-produces-child-reservations`, `ec2.capacity-reservation-usage-reflects-active-reservations`, `ec2.concurrent-capacity-reservation-split-oversubscription-rejected`, `ec2.create-capacity-reservation-by-splitting-insufficient-capacity-rejected`, `ec2.create-capacity-reservation-cancellation-quote-unknown-reservation-rejected`, `ec2.create-interruptible-capacity-reservation-allocation-invalid-target-rejected`, `ec2.groups-for-capacity-reservation-reflects-fleet-association`, `ec2.interruptible-capacity-reservation-allocation-lifecycle`, `ec2.op-contract-acceptCapacityReservationBillingOwnership`, `ec2.op-contract-acceptCapacityReservationBillingOwnership-b`, `ec2.op-contract-associate-capacity-reservation-billing-owner`, `ec2.op-contract-cancel-capacity-reservation`, `ec2.op-contract-disassociate-capacity-reservation-billing-owner`, `ec2.op-contract-get-capacity-reservation-usage`, `ec2.op-contract-get-groups-for-capacity-reservation`, `ec2.op-contract-modify-capacity-reservation-state-conflict`, `ec2.op-contract-reject-capacity-reservation-billing-ownership-rejected`, `ec2.op-contract-update-interruptible-capacity-reservation-allocation`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_capacity_reservation.rs`. Replaying the corpus, the emulator refused the `CreateCapacityReservation` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_ec2_capacity_reservation_cancellation_quote_then_cancel_consistency.py", line 58, in run
    raise AssertionError(
    ...<2 lines>...
    )
AssertionError: future-dated Capacity Reservations are not obtainable in this account/region/instance-type combination: An error occurred (NotImplemented) when calling the CreateCapacityReservation operation: operation CreateCapacityReservation is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ec2 emulator
Call CreateCapacityReservation and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
