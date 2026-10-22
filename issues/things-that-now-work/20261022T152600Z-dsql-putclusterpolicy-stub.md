Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dsql.concurrent-put-cluster-policy-optimistic-concurrency-race dsql.delete-cluster-policy-removes-policy dsql.put-cluster-policy-concurrent-version-race dsql.put-cluster-policy-get-round-trip dsql.put-cluster-policy-rejects-stale-version-and-lockout-policy dsql.put-cluster-policy-version-conflict-rejected
Issue-Shape-Id: com.amazonaws.dsql#DSQL
Issue-Key: dsql|PutClusterPolicy|NotImplemented
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# Implement dsql PutClusterPolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutClusterPolicy

## Description
`PutClusterPolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-dsql-dsql/src/ops/put_cluster_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `PutClusterPolicy` call, so each one unblocks only when `PutClusterPolicy` works: `dsql.concurrent-put-cluster-policy-optimistic-concurrency-race`, `dsql.delete-cluster-policy-removes-policy`, `dsql.put-cluster-policy-concurrent-version-race`, `dsql.put-cluster-policy-get-round-trip`, `dsql.put-cluster-policy-rejects-stale-version-and-lockout-policy`, `dsql.put-cluster-policy-version-conflict-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-dsql-dsql/src/ops/put_cluster_policy.rs`. Replaying the corpus, the emulator refused the `PutClusterPolicy` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutClusterPolicy operation: operation PutClusterPolicy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the dsql emulator
Call PutClusterPolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
