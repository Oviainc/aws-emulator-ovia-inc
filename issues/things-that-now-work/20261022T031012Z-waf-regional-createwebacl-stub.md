Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: waf-regional.webacl-create-blocked-by-classic-sunset
Issue-Shape-Id: com.amazonaws.wafregional#AWSWAF_Regional_20161128
Issue-Key: waf-regional.webacl-create-blocked-by-classic-sunset|run#3|AssertionError|assert result == expected, (default_action, result)
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# Implement waf-regional CreateWebACL: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWebACL

## Description
`CreateWebACL` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-wafregional-awswaf-regional-20161128/src/ops/create_web_acl.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateWebACL` call, so each one unblocks only when `CreateWebACL` works: `waf-regional.webacl-create-blocked-by-classic-sunset`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-wafregional-awswaf-regional-20161128/src/ops/create_web_acl.rs`. Replaying the corpus, the emulator refused the `CreateWebACL` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_waf_regional_webacl_create_blocked_by_classic_sunset.py", line 51, in run
    assert result == expected, (default_action, result)
           ^^^^^^^^^^^^^^^^^^
AssertionError: ('ALLOW', ('NotImplemented', 'operation CreateWebACL is scaffolded but not implemented in this emulator'))

## Steps to Reproduce
Boot the waf-regional emulator
Call CreateWebACL and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
