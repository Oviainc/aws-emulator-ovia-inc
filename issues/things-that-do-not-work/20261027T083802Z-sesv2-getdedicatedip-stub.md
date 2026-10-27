Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.get-dedicated-ip-unknown-rejected
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2.get-dedicated-ip-unknown-rejected|run#4|AssertionError|assert err.get("Code") == "NotFoundException", (
Issue-Commit: 6e8bba18a5a906dda1323a55e22368d1363d6826

# Implement sesv2 GetDedicatedIp: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6e8bba18a5a906dda1323a55e22368d1363d6826`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDedicatedIp

## Description
`GetDedicatedIp` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/get_dedicated_ip.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetDedicatedIp` call, so each one unblocks only when `GetDedicatedIp` works: `sesv2.get-dedicated-ip-unknown-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/get_dedicated_ip.rs`. Replaying the corpus, the emulator refused the `GetDedicatedIp` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_sesv2_get_dedicated_ip_unknown_rejected.py", line 21, in run
    assert err.get("Code") == "NotFoundException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: GetDedicatedIp for '203.0.113.42', which is not a dedicated IP owned by this account, returned {'Message': 'operation GetDedicatedIp is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}, expected 'NotFoundException'

## Steps to Reproduce
Boot the sesv2 emulator
Call GetDedicatedIp and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
