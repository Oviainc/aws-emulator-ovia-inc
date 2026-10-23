Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__elbv2.asghc-target-group-health-check-port-out-of-range-rejected elbv2.modify-target-group-attributes-out-of-range-threshold-rejected elbv2.target-group-health-check-numeric-precision-round-trip
Issue-Key: ec2__elbv2.asghc-target-group-health-check-port-out-of-range-rejected|run#6|AssertionError|assert code == "ValidationError", f"health-check port 70000 was answered {code!r}: {msg!r}"
Issue-Key: elastic-load-balancing-v2|ModifyTargetGroup|NotImplemented
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# Implement elastic-load-balancing-v2 ModifyTargetGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyTargetGroup

## Description
`ModifyTargetGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/modify_target_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `ModifyTargetGroup` call, so each one unblocks only when `ModifyTargetGroup` works: `ec2__elbv2.asghc-target-group-health-check-port-out-of-range-rejected`, `elbv2.modify-target-group-attributes-out-of-range-threshold-rejected`, `elbv2.target-group-health-check-numeric-precision-round-trip`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/modify_target_group.rs`. Replaying the corpus, the emulator refused the `ModifyTargetGroup` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_ec2__elbv2_asghc_target_group_health_check_port_out_of_range_rejected.py", line 52, in run
    assert code == "ValidationError", f"health-check port 70000 was answered {code!r}: {msg!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: health-check port 70000 was answered 'NotImplemented': 'operation ModifyTargetGroup is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the elastic-load-balancing-v2 emulator
Call ModifyTargetGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
