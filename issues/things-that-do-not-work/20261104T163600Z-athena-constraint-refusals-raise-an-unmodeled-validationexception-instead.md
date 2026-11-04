Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.athena-capacity-reservation-below-minimum-dpus-rejected athena.capacity-reservation-target-dpus-below-minimum-rejected athena.capacity-reservation-negative-dpu-target-rejected athena.update-capacity-reservation-malformed-name-rejected athena.batch-get-named-query-limit-exceeded athena.data-catalog-invalid-type-fails athena.list-query-executions-out-of-range-max-results-rejected
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena.athena-capacity-reservation-below-minimum-dpus-rejected|run#5|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code!r}"
Issue-Key: athena.capacity-reservation-target-dpus-below-minimum-rejected|run#7|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code!r}"
Issue-Key: athena|CreateCapacityReservation|ValidationException
Issue-Key: athena.update-capacity-reservation-malformed-name-rejected|run#4|AssertionError|assert code == "InvalidRequestException", (
Issue-Key: athena.batch-get-named-query-limit-exceeded|run#4|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException for 51 ids, got {code}"
Issue-Key: athena.data-catalog-invalid-type-fails|run#10|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException for invalid Type, got {code!r}"
Issue-Key: athena.list-query-executions-out-of-range-max-results-rejected|run#3|AssertionError|assert code == "InvalidRequestException", (
Issue-Commit: 1b83187822ca52f98e00143d7b0b9c4d0812f404

# Athena constraint refusals raise an unmodeled ValidationException instead of InvalidRequestException (6 causes)

> _Note: this issue was identified against the code base at commit `1b83187822ca52f98e00143d7b0b9c4d0812f404`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCapacityReservation

## Description
Every Athena operation whose input constraint is violated is refused with error code ValidationException. ValidationException is not bound to any Athena operation in the service's Smithy model, so no Athena SDK client or error handler can match it; AWS refuses all of these with InvalidRequestException. 6 causes, all one classification defect in the shared validation layer plus the per-operation message text.

The open issue 20261104T063259Z-athena-input-validation-returns-validationexception-without-athenaerrorcode-or.md describes the same classification from the wire corpus; it does not carry any of the behavioral check keys below.

Cause 1 of 6: CreateCapacityReservation with TargetDpus below the 4-DPU minimum.
AWS: Smithy: CreateCapacityReservation input TargetDpus (required, @range min 4); bound errors are only InternalServerException and InvalidRequestException. AWS refuses TargetDpus=1 with HTTP 400 InvalidRequestException and the wholly fixed Message 'Capacity must be requested in increments of 4.' (no request-derived part), and creates nothing under that Name.
Emulator: error code 'ValidationException'.
Why: a client catching Athena's InvalidRequestException sees an unknown error code and cannot classify a quota/increment violation.
Fix: the athena crate's CreateCapacityReservation input validation and the shared Athena error classifier that chooses the exception shape.
Traces: athena.athena-capacity-reservation-below-minimum-dpus-rejected, athena.capacity-reservation-target-dpus-below-minimum-rejected.

Cause 2 of 6: CreateCapacityReservation with a negative/unallocatable TargetDpus.
AWS: same modeled surface (@range min 4, errors InternalServerException/InvalidRequestException); an unallocatable TargetDpus is refused with HTTP 400 InvalidRequestException.
Emulator: 'ValidationException'.
Why: as cause 1 — the refusal is real but unclassifiable.
Fix: same handler and classifier.
Traces: athena.capacity-reservation-negative-dpu-target-rejected.

Cause 3 of 6: UpdateCapacityReservation with a malformed Name.
AWS: Smithy: UpdateCapacityReservation input Name (required, @length 1..128, @pattern ^[a-zA-Z0-9._-]+$); bound errors InternalServerException/InvalidRequestException only. A Name that violates the pattern is refused with HTTP 400 InvalidRequestException.
Emulator: 'ValidationException'.
Why: as cause 1.
Fix: the athena crate's UpdateCapacityReservation input validation and the shared classifier.
Traces: athena.update-capacity-reservation-malformed-name-rejected.

Cause 4 of 6: BatchGetNamedQuery with more than 50 NamedQueryIds.
AWS: Smithy: BatchGetNamedQuery input NamedQueryIds (required, @length 1..50); bound errors InternalServerException/InvalidRequestException only. 51 ids are refused outright — no partial processing — with HTTP 400 InvalidRequestException and the Message '1 validation error detected: Value at \'namedQueryIds\' failed to satisfy constraint: Member must have length less than or equal to 50'. Fixed text apart from the member name 'namedQueryIds' and the limit 50, both taken from the model; at exactly 50 ids the call succeeds and unresolvable ids come back in UnprocessedNamedQueryIds.
Emulator: 'ValidationException'.
Why: the client cannot tell a rejected batch from a transport-level validation failure, and the documented member/limit wording is lost.
Fix: the athena crate's BatchGetNamedQuery input validation and the shared classifier.
Traces: athena.batch-get-named-query-limit-exceeded.

Cause 5 of 6: CreateDataCatalog with a Type outside the DataCatalogType enum.
AWS: Smithy: CreateDataCatalog input Type (required, enum DataCatalogType); bound errors InternalServerException/InvalidRequestException only. A value outside the enum is refused with HTTP 400 InvalidRequestException.
Emulator: 'ValidationException'.
Why: as cause 1, on the catalog control plane.
Fix: the athena crate's CreateDataCatalog input validation and the shared classifier.
Traces: athena.data-catalog-invalid-type-fails.

Cause 6 of 6: ListQueryExecutions with MaxResults above the modeled maximum.
AWS: Smithy: ListQueryExecutions input MaxResults (MaxQueryExecutionsCount, @range 0..50); bound errors InternalServerException/InvalidRequestException only. MaxResults=51 is refused with HTTP 400 InvalidRequestException.
Emulator: 'ValidationException'.
Why: a paginating client cannot distinguish an out-of-range page size from any other failure.
Fix: the athena crate's ListQueryExecutions input validation and the shared classifier.
Traces: athena.list-query-executions-out-of-range-max-results-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout. In all six causes the HTTP status is 400 on both sides; the divergence is the error code (and, where asserted, the message).

Cause 1 — athena.athena-capacity-reservation-below-minimum-dpus-rejected, CreateCapacityReservation(Name=<name>, TargetDpus=1) with client-side parameter validation disabled: expected Code='InvalidRequestException' and Message='Capacity must be requested in increments of 4.'; actual Code='ValidationException'. athena.capacity-reservation-target-dpus-below-minimum-rejected: same, 'expected InvalidRequestException, got ValidationException'.
Cause 2 — athena.capacity-reservation-negative-dpu-target-rejected: 'expected InvalidRequestException for an unallocatable TargetDpus, got ValidationException'.
Cause 3 — athena.update-capacity-reservation-malformed-name-rejected: 'expected a malformed Name to be rejected with InvalidRequestException, got ValidationException'.
Cause 4 — athena.batch-get-named-query-limit-exceeded, BatchGetNamedQuery with 51 ids: 'expected InvalidRequestException for 51 ids, got ValidationException'.
Cause 5 — athena.data-catalog-invalid-type-fails: 'expected InvalidRequestException for invalid Type, got ValidationException'.
Cause 6 — athena.list-query-executions-out-of-range-max-results-rejected, ListQueryExecutions(MaxResults=51): 'expected InvalidRequestException for MaxResults=51 (above the documented max of 50), got ValidationException'.

Model check (services/service-com-amazonaws-athena-amazonathena/smithy-model): none of CreateCapacityReservation, UpdateCapacityReservation, BatchGetNamedQuery, CreateDataCatalog or ListQueryExecutions binds a ValidationException — each binds only InternalServerException and InvalidRequestException.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay suite simparity-behavioral-aws-v1 and inspect the six athena checks listed in trace_ids.
3. Minimal manual repro (use a botocore Config(parameter_validation=False) client so the SDK does not reject locally and the service's own refusal is observed):
   a. CreateCapacityReservation(Name='probe', TargetDpus=1) — expect 400 InvalidRequestException, Message 'Capacity must be requested in increments of 4.'
   b. CreateCapacityReservation(Name='probe2', TargetDpus=-4) — expect 400 InvalidRequestException.
   c. UpdateCapacityReservation(Name='bad name!', TargetDpus=4) — expect 400 InvalidRequestException.
   d. BatchGetNamedQuery with 51 ids — expect 400 InvalidRequestException with the '1 validation error detected: Value at \'namedQueryIds\' failed to satisfy constraint: Member must have length less than or equal to 50' Message; with 50 ids expect 200.
   e. CreateDataCatalog(Name='probe3', Type='NOT_A_TYPE') — expect 400 InvalidRequestException.
   f. ListQueryExecutions(MaxResults=51) — expect 400 InvalidRequestException.
Each currently answers Code='ValidationException'.
