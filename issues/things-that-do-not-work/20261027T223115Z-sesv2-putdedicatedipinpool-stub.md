Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.put-dedicated-ip-in-pool-unknown-ip-rejected
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2.put-dedicated-ip-in-pool-unknown-ip-rejected|run#8|AssertionError|assert err == expected, f"expected {expected!r}, got {err!r}"
Issue-Commit: 65fd8797851bb5c5937d1ea0792f5c718c73df47

# Implement sesv2 PutDedicatedIpInPool: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `65fd8797851bb5c5937d1ea0792f5c718c73df47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDedicatedIpInPool

## Description
`PutDedicatedIpInPool` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/put_dedicated_ip_in_pool.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `PutDedicatedIpInPool` call, so each one unblocks only when `PutDedicatedIpInPool` works: `sesv2.put-dedicated-ip-in-pool-unknown-ip-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/put_dedicated_ip_in_pool.rs`. Replaying the corpus, the emulator refused the `PutDedicatedIpInPool` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_sesv2_put_dedicated_ip_in_pool_unknown_ip_rejected.py", line 28, in run
    assert err == expected, f"expected {expected!r}, got {err!r}"
           ^^^^^^^^^^^^^^^
AssertionError: expected ('NotFoundException', 'Could not find dedicated IP <198.51.100.77> under this account.'), got ('NotImplemented', 'operation PutDedicatedIpInPool is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the sesv2 emulator
Call PutDedicatedIpInPool and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
