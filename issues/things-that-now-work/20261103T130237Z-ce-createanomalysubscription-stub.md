Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ce.anomaly-monitor-delete-orphans-subscription ce.anomaly-subscription-accepts-unreachable-sns ce.anomaly-subscription-threshold-shorthand-expands ce.tag-resource-scopes-tags-per-resource ce.update-anomaly-subscription-rejects-conflicting-threshold
Issue-Shape-Id: com.amazonaws.costexplorer#AWSInsightsIndexService
Issue-Key: cost-explorer|CreateAnomalySubscription|NotImplemented
Issue-Commit: ed09b88e064ad522a438f1cda8cc30ae911567ef

# Implement ce CreateAnomalySubscription: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ed09b88e064ad522a438f1cda8cc30ae911567ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAnomalySubscription

## Description
`CreateAnomalySubscription` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-costexplorer-awsinsightsindexservice/src/ops/create_anomaly_subscription.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `CreateAnomalySubscription` call, so each one unblocks only when `CreateAnomalySubscription` works: `ce.anomaly-monitor-delete-orphans-subscription`, `ce.anomaly-subscription-accepts-unreachable-sns`, `ce.anomaly-subscription-threshold-shorthand-expands`, `ce.tag-resource-scopes-tags-per-resource`, `ce.update-anomaly-subscription-rejects-conflicting-threshold`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-costexplorer-awsinsightsindexservice/src/ops/create_anomaly_subscription.rs`. Replaying the corpus, the emulator refused the `CreateAnomalySubscription` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateAnomalySubscription operation: operation CreateAnomalySubscription is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ce emulator
Call CreateAnomalySubscription and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
