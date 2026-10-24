Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wafv2.get-top-path-statistics-pricing-plan-rejected wafv2.top-path-statistics-reflects-traffic-volume-ordering
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2|GetTopPathStatisticsByTraffic|NotImplemented
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# Implement wafv2 GetTopPathStatisticsByTraffic: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTopPathStatisticsByTraffic

## Description
`GetTopPathStatisticsByTraffic` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-wafv2-awswaf-20190729/src/ops/get_top_path_statistics_by_traffic.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `GetTopPathStatisticsByTraffic` call, so each one unblocks only when `GetTopPathStatisticsByTraffic` works: `wafv2.get-top-path-statistics-pricing-plan-rejected`, `wafv2.top-path-statistics-reflects-traffic-volume-ordering`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-wafv2-awswaf-20190729/src/ops/get_top_path_statistics_by_traffic.rs`. Replaying the corpus, the emulator refused the `GetTopPathStatisticsByTraffic` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_wafv2_get_top_path_statistics_pricing_plan_rejected.py", line 83, in run
    assert code == "WAFFeatureNotIncludedInPricingPlanException", f"got {code!r} instead"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: got 'NotImplemented' instead

## Steps to Reproduce
Boot the wafv2 emulator
Call GetTopPathStatisticsByTraffic and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
