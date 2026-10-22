Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.vpcconnection-read-paths-report-clean-miss
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.vpcconnection-read-paths-report-clean-miss|run#4|AssertionError|assert describe_code == _BAD_REQUEST, f"describe: expected {_BAD_REQUEST!r}, got {describe_code!r}"
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Implement kafka DescribeVpcConnection: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcConnection

## Description
`DescribeVpcConnection` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kafka-kafka/src/ops/describe_vpc_connection.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DescribeVpcConnection` call, so each one unblocks only when `DescribeVpcConnection` works: `kafka.vpcconnection-read-paths-report-clean-miss`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kafka-kafka/src/ops/describe_vpc_connection.rs`. Replaying the corpus, the emulator refused the `DescribeVpcConnection` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_kafka_vpcconnection_read_paths_report_clean_miss.py", line 49, in run
    assert describe_code == _BAD_REQUEST, f"describe: expected {_BAD_REQUEST!r}, got {describe_code!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: describe: expected ('BadRequestException', 'One or more of the parameters are not valid.'), got ('NotImplemented', 'operation DescribeVpcConnection is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the kafka emulator
Call DescribeVpcConnection and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
