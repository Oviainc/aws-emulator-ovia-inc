Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: geo-routes.calculate-isolines-basic-success geo-routes.calculate-isolines-car-reaches-farther-than-pedestrian geo-routes.calculate-isolines-destination-based-snaps-destination-not-origin geo-routes.calculate-isolines-distance-threshold geo-routes.calculate-isolines-distance-thresholds-success geo-routes.calculate-isolines-finer-granularity-yields-more-points geo-routes.calculate-isolines-missing-origin-and-destination-rejected geo-routes.calculate-isolines-mixed-threshold-types-rejected geo-routes.calculate-isolines-multiple-thresholds-ordering geo-routes.calculate-isolines-time-thresholds-success
Issue-Shape-Id: com.amazonaws.georoutes#RoutesService
Issue-Key: geo-routes|CalculateIsolines|NotImplemented
Issue-Key: geo-routes.calculate-isolines-missing-origin-and-destination-rejected|run#3|AssertionError|assert error["Code"] == "ValidationException", f"expected ValidationException, got {error!r}"
Issue-Key: geo-routes.calculate-isolines-mixed-threshold-types-rejected|run#3|AssertionError|assert error.get("Code") == "ValidationException", (
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# Implement geo-routes CalculateIsolines: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CalculateIsolines

## Description
`CalculateIsolines` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-georoutes-routesservice/src/ops/calculate_isolines.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

10 recorded unit(s) fail on their `CalculateIsolines` call, so each one unblocks only when `CalculateIsolines` works: `geo-routes.calculate-isolines-basic-success`, `geo-routes.calculate-isolines-car-reaches-farther-than-pedestrian`, `geo-routes.calculate-isolines-destination-based-snaps-destination-not-origin`, `geo-routes.calculate-isolines-distance-threshold`, `geo-routes.calculate-isolines-distance-thresholds-success`, `geo-routes.calculate-isolines-finer-granularity-yields-more-points`, `geo-routes.calculate-isolines-missing-origin-and-destination-rejected`, `geo-routes.calculate-isolines-mixed-threshold-types-rejected`, `geo-routes.calculate-isolines-multiple-thresholds-ordering`, `geo-routes.calculate-isolines-time-thresholds-success`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-georoutes-routesservice/src/ops/calculate_isolines.rs`. Replaying the corpus, the emulator refused the `CalculateIsolines` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CalculateIsolines operation: operation CalculateIsolines is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the geo-routes emulator
Call CalculateIsolines and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
