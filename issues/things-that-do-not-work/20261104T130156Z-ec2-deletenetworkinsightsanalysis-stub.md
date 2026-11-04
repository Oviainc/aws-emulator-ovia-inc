Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-network-insights-path-analysis-lifecycle
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-network-insights-path-analysis-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:DeleteNetworkInsightsAnalysis
Issue-Commit: 30059ba7efbea1c9406793a7f2ccf8806d86986b

# Implement ec2 DeleteNetworkInsightsAnalysis: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `30059ba7efbea1c9406793a7f2ccf8806d86986b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteNetworkInsightsAnalysis

## Description
`DeleteNetworkInsightsAnalysis` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_network_insights_analysis.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteNetworkInsightsAnalysis` call, so each one unblocks only when `DeleteNetworkInsightsAnalysis` works: `ec2-network-insights-path-analysis-lifecycle#task#8`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_network_insights_analysis.rs`. Replaying the corpus, the emulator refused the `DeleteNetworkInsightsAnalysis` call: status: expected 200, got 501
$.DeleteNetworkInsightsAnalysisResponse: missing (expected {'requestId': '<shaped-not-asserted>', 'networkInsightsAnalysisId': 'nia-00000000000000034'}, got None)

## Steps to Reproduce
Boot the ec2 emulator
Call DeleteNetworkInsightsAnalysis and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
