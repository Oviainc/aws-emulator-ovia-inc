Issue-Source: trace-sweep-divergence
Issue-Trace: email-configuration-set-full-lifecycle-with-event-destinations
Issue-Step: task#18
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: email-configuration-set-full-lifecycle-with-event-destinations
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2|stub:ListConfigurationSets
Issue-Commit: a9006feb831fdd789c02a15722f93d2f2e52852a

# Implement sesv2 ListConfigurationSets: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `a9006feb831fdd789c02a15722f93d2f2e52852a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListConfigurationSets

## Description
`ListConfigurationSets` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/list_configuration_sets.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListConfigurationSets` call, so each one unblocks only when `ListConfigurationSets` works: `email-configuration-set-full-lifecycle-with-event-destinations#task#18`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/list_configuration_sets.rs`. Replaying the corpus, the emulator refused the `ListConfigurationSets` call: status: expected 200, got 501
$.ConfigurationSets: missing (expected ['sim2real-probe-sesv2-cfgset-44ed6c39'], got None)

## Steps to Reproduce
Boot the sesv2 emulator
Call ListConfigurationSets and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
