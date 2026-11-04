Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: timestream-influxdb.create-db-cluster-reaches-available
Issue-Shape-Id: com.amazonaws.timestreaminfluxdb#AmazonTimestreamInfluxDB
Issue-Key: timestream-influxdb|CreateDbCluster|NotImplemented
Issue-Commit: 9b14a11a85da2dc3914d7c191991ddabd7c195c3

# Implement timestream-influxdb CreateDbCluster: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `9b14a11a85da2dc3914d7c191991ddabd7c195c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDbCluster

## Description
`CreateDbCluster` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb/src/ops/create_db_cluster.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateDbCluster` call, so each one unblocks only when `CreateDbCluster` works: `timestream-influxdb.create-db-cluster-reaches-available`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb/src/ops/create_db_cluster.rs`. Replaying the corpus, the emulator refused the `CreateDbCluster` call: File "run.py", line 342, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_timestream_influxdb_create_db_cluster_reaches_available.py", line 50, in run
    assert code == "ValidationException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationException for the missing Read Replicas Marketplace entitlement, got 'NotImplemented': 'operation CreateDbCluster is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the timestream-influxdb emulator
Call CreateDbCluster and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
