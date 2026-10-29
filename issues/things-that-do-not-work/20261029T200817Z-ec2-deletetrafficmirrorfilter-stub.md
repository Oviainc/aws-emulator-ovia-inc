Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-describe-traffic-mirror-filters-notfound-rejected
Issue-Step: teardown#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-describe-traffic-mirror-filters-notfound-rejected ec2-traffic-mirror-filter-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:DeleteTrafficMirrorFilter
Issue-Commit: c42b5bef1562db44010b2579a8522ff39a3564f6

# Implement ec2 DeleteTrafficMirrorFilter: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c42b5bef1562db44010b2579a8522ff39a3564f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteTrafficMirrorFilter

## Description
`DeleteTrafficMirrorFilter` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_traffic_mirror_filter.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `DeleteTrafficMirrorFilter` call, so each one unblocks only when `DeleteTrafficMirrorFilter` works: `ec2-describe-traffic-mirror-filters-notfound-rejected#teardown#4`, `ec2-traffic-mirror-filter-lifecycle#teardown#11`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_traffic_mirror_filter.rs`. Replaying the corpus, the emulator refused the `DeleteTrafficMirrorFilter` call: status: expected 200, got 501

## Steps to Reproduce
Boot the ec2 emulator
Call DeleteTrafficMirrorFilter and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
