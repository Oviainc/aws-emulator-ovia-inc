Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.create-delivery-lifecycle logs.delivery-full-chain-lifecycle logs.update-delivery-configuration-changes-take-effect
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: cloudwatch-logs|UpdateDeliveryConfiguration|NotImplemented
Issue-Commit: 8c118d1d43e9fad6226f410894186130d2b1ac53

# Implement logs UpdateDeliveryConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `8c118d1d43e9fad6226f410894186130d2b1ac53`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDeliveryConfiguration

## Description
`UpdateDeliveryConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/update_delivery_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `UpdateDeliveryConfiguration` call, so each one unblocks only when `UpdateDeliveryConfiguration` works: `logs.create-delivery-lifecycle`, `logs.delivery-full-chain-lifecycle`, `logs.update-delivery-configuration-changes-take-effect`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/update_delivery_configuration.rs`. Replaying the corpus, the emulator refused the `UpdateDeliveryConfiguration` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateDeliveryConfiguration operation: operation UpdateDeliveryConfiguration is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the logs emulator
Call UpdateDeliveryConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
