Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53-recovery-control-config.control-panel-routing-control-count-tracks-creation route53-recovery-control-config.create-safety-rule-both-rule-types-rejected route53-recovery-control-config.describe-cluster-nonexistent-not-found route53-recovery-control-config.gating-rule-controls-and-rule-config-persist route53-recovery-control-config.get-resource-policy-without-attached-policy-not-found route53-recovery-control-config.list-associated-health-checks-empty-for-new-control route53-recovery-control-config.list-routing-controls-pagination-covers-all-items route53-recovery-control-config.list-safety-rules-returns-mixed-assertion-and-gating route53-recovery-control-config.safety-rule-threshold-exceeds-controls-rejected route53-recovery-control-config.tag-resource-round-trip route53-recovery-control-config.update-control-panel-name-persists route53-recovery-control-config.update-routing-control-name-persists route53-recovery-control-config.update-safety-rule-wait-period-persists
Issue-Shape-Id: com.amazonaws.route53recoverycontrolconfig#Route53RecoveryControlConfig
Issue-Key: route53-recovery-control-config|ListClusters|NotImplemented
Issue-Commit: 1e44d2b2032d56c599b46458071bcd07c3617563

# Implement route53-recovery-control-config ListClusters: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `1e44d2b2032d56c599b46458071bcd07c3617563`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListClusters

## Description
`ListClusters` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig/src/ops/list_clusters.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

13 recorded unit(s) fail on their `ListClusters` call, so each one unblocks only when `ListClusters` works: `route53-recovery-control-config.control-panel-routing-control-count-tracks-creation`, `route53-recovery-control-config.create-safety-rule-both-rule-types-rejected`, `route53-recovery-control-config.describe-cluster-nonexistent-not-found`, `route53-recovery-control-config.gating-rule-controls-and-rule-config-persist`, `route53-recovery-control-config.get-resource-policy-without-attached-policy-not-found`, `route53-recovery-control-config.list-associated-health-checks-empty-for-new-control`, `route53-recovery-control-config.list-routing-controls-pagination-covers-all-items`, `route53-recovery-control-config.list-safety-rules-returns-mixed-assertion-and-gating`, `route53-recovery-control-config.safety-rule-threshold-exceeds-controls-rejected`, `route53-recovery-control-config.tag-resource-round-trip`, `route53-recovery-control-config.update-control-panel-name-persists`, `route53-recovery-control-config.update-routing-control-name-persists`, `route53-recovery-control-config.update-safety-rule-wait-period-persists`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig/src/ops/list_clusters.rs`. Replaying the corpus, the emulator refused the `ListClusters` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListClusters operation: operation ListClusters is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the route53-recovery-control-config emulator
Call ListClusters and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
