Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-capacity-reservation-usage-and-groups
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-capacity-reservation-usage-and-groups
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:GetCapacityReservationUsage
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# Implement ec2 GetCapacityReservationUsage: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCapacityReservationUsage

## Description
`GetCapacityReservationUsage` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/get_capacity_reservation_usage.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetCapacityReservationUsage` call, so each one unblocks only when `GetCapacityReservationUsage` works: `ec2-capacity-reservation-usage-and-groups#task#5`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/get_capacity_reservation_usage.rs`. Replaying the corpus, the emulator refused the `GetCapacityReservationUsage` call: status: expected 400, got 501
$.Response.Errors.Error.Code: value_mismatch (expected 'InvalidCapacityReservationId.Malformed', got 'NotImplemented')
$.Response.Errors.Error.Message: value_mismatch (expected "Capacity Reservation ID 'cr-00000000000000000' is malformed", got 'operation GetCapacityReservationUsage is scaffolded but not implemented in this emulator')
$.Response.RequestID: missing (expected 'a3178dae-f60b-4640-86c5-c48f6365c3fa', got None)
$headers.content-type: value_mismatch (expected 'text/xml;charset=UTF-8', got 'text/xml')

## Steps to Reproduce
Boot the ec2 emulator
Call GetCapacityReservationUsage and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

## Follow-up after the implementation (what this report asked for is done; one recorded
## difference remains)
The operation is implemented and the scaffold census line is gone: it reports the stored
Capacity Reservation and the instances using it, pages with the service's own `NextToken`,
and refuses an id the account holds no reservation for with HTTP 400
`InvalidCapacityReservationId.NotFound` — the code every other reader of a Capacity
Reservation id in this crate answers, and the code the recording of
`CreateInterruptibleCapacityReservationAllocation` shows AWS using for the very same id
(`The capacity-reservation ID 'cr-00000000000000000' does not exist`).

The recorded unit of THIS report expects `InvalidCapacityReservationId.Malformed` for that
id instead, so it still diverges on the error code. `cr-` plus 17 hexadecimal digits is the
well-formed id this service mints and the form the model describes, so nothing observable
says which ids AWS rejects as malformed rather than as not found — and the two recordings
of the same id disagree with each other. Refusing that one literal id as malformed would be
fitting the recorded instance rather than a rule, so it is deliberately not done here. A
`Malformed` code belongs to whichever form check AWS documents; if that rule is ever
established, it belongs in `require_known_capacity_reservation` (the one lookup every
Capacity Reservation reader of this crate goes through) rather than in this operation.
