Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__config__s3.config-managed-rule-verdict-flips-to-compliant-after-resource-is-fixed cloudformation__config__s3.config-rule-scoped-to-one-resource-id-evaluates-only-that-resource cloudformation__config__s3.deploy-config-delivery-channel-by-cloudformation-delivers-a-snapshot-containing-the-recorded-item config__lambda__sqs.change-triggered-custom-rule-is-push-invoked-with-a-real-configuration-item config__lambda__sqs.change-triggered-rule-creation-is-denied-when-config-cannot-invoke-the-evaluator config__sqs.config-batch-get-resource-config-returns-only-the-recorded-key-of-a-mixed-batch config__sqs.config-history-rejects-unrecorded-queue-while-batch-get-omits-it config__sqs.config-queue-deletion-recorded-as-resource-deleted-configuration-item config__sqs.config-recorded-queue-configuration-item-echoes-live-attributes config__sqs.config-select-resource-config-returns-the-recorded-queue-row config__sqs.config-stopped-recorder-records-no-configuration-item-until-restarted
Issue-Key: config-service|GetResourceConfigHistory|NotImplemented
Issue-Commit: 699f6ed7a7b73d87a9db3c950b6f69a120aa392c

# Implement config-service GetResourceConfigHistory: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `699f6ed7a7b73d87a9db3c950b6f69a120aa392c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourceConfigHistory

## Description
`GetResourceConfigHistory` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/get_resource_config_history.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

11 recorded unit(s) fail on their `GetResourceConfigHistory` call, so each one unblocks only when `GetResourceConfigHistory` works: `cloudformation__config__s3.config-managed-rule-verdict-flips-to-compliant-after-resource-is-fixed`, `cloudformation__config__s3.config-rule-scoped-to-one-resource-id-evaluates-only-that-resource`, `cloudformation__config__s3.deploy-config-delivery-channel-by-cloudformation-delivers-a-snapshot-containing-the-recorded-item`, `config__lambda__sqs.change-triggered-custom-rule-is-push-invoked-with-a-real-configuration-item`, `config__lambda__sqs.change-triggered-rule-creation-is-denied-when-config-cannot-invoke-the-evaluator`, `config__sqs.config-batch-get-resource-config-returns-only-the-recorded-key-of-a-mixed-batch`, `config__sqs.config-history-rejects-unrecorded-queue-while-batch-get-omits-it`, `config__sqs.config-queue-deletion-recorded-as-resource-deleted-configuration-item`, `config__sqs.config-recorded-queue-configuration-item-echoes-live-attributes`, `config__sqs.config-select-resource-config-returns-the-recorded-queue-row`, `config__sqs.config-stopped-recorder-records-no-configuration-item-until-restarted`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/get_resource_config_history.rs`. Replaying the corpus, the emulator refused the `GetResourceConfigHistory` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetResourceConfigHistory operation: operation GetResourceConfigHistory is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the config-service emulator
Call GetResourceConfigHistory and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
