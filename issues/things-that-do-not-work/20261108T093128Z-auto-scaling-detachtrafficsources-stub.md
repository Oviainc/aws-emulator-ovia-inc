Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.asghc-detach-mixing-an-attached-and-a-never-attached-target-group-is-refused-whole
Issue-Key: autoscaling__ec2__elbv2.asghc-detach-mixing-an-attached-and-a-never-attached-target-group-is-refused-whole|run#14|AssertionError|assert err == expected, f"the mixed traffic-source detach was answered {err!r}"
Issue-Commit: d8469fdbee4aefb676d6a1b6581464fb2b4efb05

# Implement auto-scaling DetachTrafficSources: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d8469fdbee4aefb676d6a1b6581464fb2b4efb05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetachTrafficSources

## Description
`DetachTrafficSources` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/detach_traffic_sources.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DetachTrafficSources` call, so each one unblocks only when `DetachTrafficSources` works: `autoscaling__ec2__elbv2.asghc-detach-mixing-an-attached-and-a-never-attached-target-group-is-refused-whole`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/detach_traffic_sources.rs`. Replaying the corpus, the emulator refused the `DetachTrafficSources` call: File "run.py", line 353, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_autoscaling__ec2__elbv2_asghc_detach_mixing_an_attached_and_a_never_attached_target_group_is_refused_whole.py", line 104, in run
    assert err == expected, f"the mixed traffic-source detach was answered {err!r}"
           ^^^^^^^^^^^^^^^
AssertionError: the mixed traffic-source detach was answered ('NotImplemented', 'operation DetachTrafficSources is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the auto-scaling emulator
Call DetachTrafficSources and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
