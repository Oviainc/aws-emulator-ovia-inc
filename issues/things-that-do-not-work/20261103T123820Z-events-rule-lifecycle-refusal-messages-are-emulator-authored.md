Issue-Source: trace-sweep-divergence
Issue-Trace: events-ebecs-launch-type-and-network-mistakes
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: events-ebecs-launch-type-and-network-mistakes events-ebecs-rule-state-toggle events-health-export-cleanup-delete-rule-with-targets events-poller-schedule-rule-target-introspection
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|PutTargets|4xx->4xx|$.message
Issue-Key: events|DeleteRule|4xx->4xx|$.message
Issue-Related: 20261103T123820Z-events-putrule-and-puttargets-accept-requests-aws-refuses.md
Issue-Commit: e0cc4f83c3608c610536f43d6e35ff223081a6f4

# events: rule-lifecycle refusal messages are emulator-authored instead of AWS's fixed text (3 causes)

> _Note: this issue was identified against the code base at commit `e0cc4f83c3608c610536f43d6e35ff223081a6f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Three net-new wire root causes in the events service. Each returns the right HTTP status and the right error code, but the $.message body differs from the text AWS produces for that error class. Counts: 3 causes.

Cause 1 of 3: the constraint-violation message for EcsParameters.LaunchType lists the enum value set in the wrong order.
AWS: a PutTargets entry whose ecsParameters.launchType is not one of the modeled values is refused with HTTP 400 and $.message = "1 validation error detected: Value '<offending value>' at '<member path>' failed to satisfy constraint: Member must satisfy enum value set: [FARGATE, EXTERNAL, EC2]". The fixed text is the '1 validation error detected: Value ... failed to satisfy constraint: Member must satisfy enum value set: ' frame AND the bracketed set itself, including its order; the variable parts are only the offending value and the member path, both taken from the request (the recording shows value 'fargate' at 'targets.1.member.ecsParameters.launchType'). Smithy (com.amazonaws.eventbridge): enum LaunchType declares its values as [EC2, FARGATE, EXTERNAL] — AWS's message order is NOT the model's declaration order, so the rendered order is recorded behavior only.
Emulator: same status and frame but the set is rendered in Smithy declaration order, '[EC2, FARGATE, EXTERNAL]'.
Why: a client or recorded-response comparison that matches the refusal text (CLI wrappers, contract tests, log scrapers that distinguish an enum violation from other validation failures) sees a message the service never emits, so the assertion fails even though the request was correctly refused.
Fix: the events crate's constraint-violation message rendering — carry AWS's fixed value-set ordering per enum rather than iterating the generated enum's declaration order.
Traces: events-ebecs-launch-type-and-network-mistakes#task#5.

Cause 2 of 3: the same defect on PutRule's State enum, with a different order.
AWS: PutRule with an unmodeled State is refused with HTTP 400 and $.message = "1 validation error detected: Value '<offending value>' at 'state' failed to satisfy constraint: Member must satisfy enum value set: [ENABLED_WITH_ALL_CLOUDTRAIL_MANAGEMENT_EVENTS, ENABLED, DISABLED]" — again the bracketed set and its order are fixed text, and only the offending value (recording: 'enabled') and the member path come from the request. Smithy: enum RuleState declares [ENABLED, DISABLED, ENABLED_WITH_ALL_CLOUDTRAIL_MANAGEMENT_EVENTS], so AWS's order is recorded behavior only and differs per enum — it cannot be derived from the model, which is why this is a second cause and not the same rendering bug's second instance by accident.
Emulator: '[ENABLED, DISABLED, ENABLED_WITH_ALL_CLOUDTRAIL_MANAGEMENT_EVENTS]'.
Why: as cause 1 — the refusal is correct but unmatchable against AWS's wording.
Fix: same component; the per-enum fixed ordering must be supplied for RuleState as well as LaunchType.
Traces: events-ebecs-rule-state-toggle#task#9.

Cause 3 of 3: DeleteRule's has-targets refusal is rewritten and interpolates values AWS never includes.
AWS: DeleteRule without Force on a rule that still has targets is refused with HTTP 400 and $.message = "Rule can't be deleted since it has targets." The entire message is fixed text — no rule name, no target count, no remediation clause — and the targets survive. Smithy: DeleteRule models Name, EventBusName and Force (default false) and its modeled errors are ConcurrentModificationException / InternalException / ManagedRuleException / ResourceNotFoundException, so the message for this refusal is recorded behavior only.
Emulator: 'Rule <rule name> cannot be deleted since it still has <n> target(s). Remove them with RemoveTargets first.' — wholly different fixed text, plus a rule name taken from the request and a target count taken from emulator state that AWS never emits.
Why: clients that branch on the refusal text to distinguish 'remove targets first' from other validation failures take the wrong branch, and any recorded-response comparison fails. Open issue 20261102T091831Z-events-api-destinations-never-deliver-or-dead-letter.md names this same message from the behavioral suite (its cause 3) but carries no replay key for these wire steps.
Fix: the DeleteRule handler's refusal construction in the events crate — emit the fixed AWS message for this error class instead of formatting request and state values into it.
Traces: events-health-export-cleanup-delete-rule-with-targets#task#4 (cluster also covers events-poller-schedule-rule-target-introspection#task#11).

## Evidence
Cause 1 — PutTargets, HTTP 400 both sides. Expected $.message "1 validation error detected: Value 'fargate' at 'targets.1.member.ecsParameters.launchType' failed to satisfy constraint: Member must satisfy enum value set: [FARGATE, EXTERNAL, EC2]"; actual the identical string ending '... enum value set: [EC2, FARGATE, EXTERNAL]'.
Cause 2 — PutRule, HTTP 400 both sides. Expected $.message "1 validation error detected: Value 'enabled' at 'state' failed to satisfy constraint: Member must satisfy enum value set: [ENABLED_WITH_ALL_CLOUDTRAIL_MANAGEMENT_EVENTS, ENABLED, DISABLED]"; actual '... enum value set: [ENABLED, DISABLED, ENABLED_WITH_ALL_CLOUDTRAIL_MANAGEMENT_EVENTS]'.
Cause 3 — DeleteRule, HTTP 400 both sides. Expected $.message "Rule can't be deleted since it has targets."; actual 'Rule <name> cannot be deleted since it still has 1 target(s). Remove them with RemoveTargets first.' (2 steps).

## Steps to Reproduce
Replay the corpus: replay_wire_corpus() against the emulator built from this checkout.
Cause 1: PutTargets an ECS target with EcsParameters.LaunchType set to a lowercase/unmodeled value and read $.message (events-ebecs-launch-type-and-network-mistakes#task#5).
Cause 2: PutRule with State set to 'enabled' and read $.message (events-ebecs-rule-state-toggle#task#9).
Cause 3: PutRule a rule, PutTargets one target, then DeleteRule without Force and read $.message (events-health-export-cleanup-delete-rule-with-targets#task#4).
