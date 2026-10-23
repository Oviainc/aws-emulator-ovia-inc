Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.deploy-truststore-cfn-bundle-integrity-behaves
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elastic-load-balancing-v2|DescribeTrustStores|InvalidAction
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# Implement elastic-load-balancing-v2 DescribeTrustStores: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTrustStores

## Description
`DescribeTrustStores` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/describe_trust_stores.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DescribeTrustStores` call, so each one unblocks only when `DescribeTrustStores` works: `elbv2.deploy-truststore-cfn-bundle-integrity-behaves`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/describe_trust_stores.rs`. Replaying the corpus, the emulator refused the `DescribeTrustStores` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (InvalidAction) when calling the DescribeTrustStores operation: Could not find operation DescribeTrustStores for version 2015-12-01

## Steps to Reproduce
Boot the elastic-load-balancing-v2 emulator
Call DescribeTrustStores and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
