Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__logs.xsvc-logs-2-subscription-filter-on-cmk-group-delivers-plaintext-to-kinesis
Issue-Key: kinesis|ListShards|NotImplemented
Issue-Commit: 564feb857476f3247a0693cb65aa2d44e3322d3a

# Implement kinesis ListShards: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `564feb857476f3247a0693cb65aa2d44e3322d3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListShards

## Description
`ListShards` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/list_shards.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListShards` call, so each one unblocks only when `ListShards` works: `kms__logs.xsvc-logs-2-subscription-filter-on-cmk-group-delivers-plaintext-to-kinesis`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/list_shards.rs`. Replaying the corpus, the emulator refused the `ListShards` call: File "check_kms__logs_xsvc_logs_2_subscription_filter_on_cmk_group_delivers_plaintext_to_kinesis.py", line 48, in run
    shards = _retry_code(lambda: kinesis.list_shards(StreamName=stream_name)["Shards"],
                         "ResourceInUseException", timeout_s=60.0,
                         description=f"ListShards on {stream_name!r} once it is ACTIVE")
  File "check_kms__logs_xsvc_logs_2_subscription_filter_on_cmk_group_delivers_plaintext_to_kinesis.py", line 131, in _retry_code
    raise AssertionError(f"{description} failed: {exc.response['Error']!r}") from exc
AssertionError: ListShards on 'sim2real-xsvc-kds-9aaf2ac06158' once it is ACTIVE failed: {'Message': 'operation ListShards is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the kinesis emulator
Call ListShards and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
