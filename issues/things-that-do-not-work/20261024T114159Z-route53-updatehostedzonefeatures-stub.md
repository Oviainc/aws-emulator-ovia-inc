Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.update-hosted-zone-features-limits-exceeded-rejected route53.update-hosted-zone-features-persists-and-enforced
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.update-hosted-zone-features-limits-exceeded-rejected|run#7|AssertionError|assert real_zone_error.get("Code") == "InvalidInput", (
Issue-Key: route-53|UpdateHostedZoneFeatures|NotImplemented
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Implement route53 UpdateHostedZoneFeatures: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateHostedZoneFeatures

## Description
`UpdateHostedZoneFeatures` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/update_hosted_zone_features.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `UpdateHostedZoneFeatures` call, so each one unblocks only when `UpdateHostedZoneFeatures` works: `route53.update-hosted-zone-features-limits-exceeded-rejected`, `route53.update-hosted-zone-features-persists-and-enforced`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/update_hosted_zone_features.rs`. Replaying the corpus, the emulator refused the `UpdateHostedZoneFeatures` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_route53_update_hosted_zone_features_limits_exceeded_rejected.py", line 67, in run
    assert real_zone_error.get("Code") == "InvalidInput", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected InvalidInput for a redundant disable on a real zone, got {'Message': 'operation UpdateHostedZoneFeatures is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented', 'Type': 'Receiver'}

## Steps to Reproduce
Boot the route53 emulator
Call UpdateHostedZoneFeatures and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
