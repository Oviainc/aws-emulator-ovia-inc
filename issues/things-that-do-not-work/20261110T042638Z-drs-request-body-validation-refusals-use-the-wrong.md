Issue-Source: trace-sweep-divergence
Issue-Trace: drs-launch-configuration-template-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: drs-launch-configuration-template-lifecycle drs-launch-configuration-template-validation-errors drs-replication-configuration-template-lifecycle
Issue-Shape-Id: com.amazonaws.drs#ElasticDisasterRecoveryService
Issue-Key: drs|CreateLaunchConfigurationTemplate|4xx->4xx|$.message
Issue-Key: drs|CreateLaunchConfigurationTemplate|4xx->4xx|$headers.x-amzn-errortype
Issue-Related: 20261110T042638Z-drs-account-initialization-gate-refuses-every-call-masking.md
Issue-Commit: 563433aa8ea88fa1a8e8ad3e10fc27d869dea4a3

# drs: request-body validation refusals use the wrong error shape and message

> _Note: this issue was identified against the code base at commit `563433aa8ea88fa1a8e8ad3e10fc27d869dea4a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLaunchConfigurationTemplate

## Description
Two net-new wire divergences in the drs emulator: when a request body violates a modelled shape constraint, the emulator answers its own generic constraint-detail ValidationException instead of the refusal AWS's DRS frontend sends. Both causes must be closed before submitting. The open issue '20261109T163249Z-drs-4-net-new-behavioral-root-causes-the' reports the enum case from the BEHAVIORAL suite only; it carries no wire replay key, so neither fixture step below is covered by it and both must be re-replayed here.

Cause 1 of 2 — an out-of-enum member value is refused with a constraint-detail ValidationException instead of 'Invalid request body'.
AWS: Smithy: CreateLaunchConfigurationTemplate.recoveryMode targets RecoveryMode, whose value set is [FAST, OPTIMAL]; the operation declares ValidationException (400) but no BadRequestException, so this refusal comes from the service frontend's body deserialization rather than a modelled per-member validator. The recording shows HTTP 400 with message 'Invalid request body' — entirely fixed text: it names neither the offending member nor its value — and nothing created.
Emulator: HTTP 400 with x-amzn-errortype 'ValidationException' and the message "1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy enum value set: [FAST, OPTIMAL]", i.e. the generic AWS-framework constraint-report wording, not DRS's.
Why: a client branching on the error code and message for a malformed body never matches what DRS actually sends; SDK error-shape matching and any message assertion diverge from production, and the leaked constraint detail trains callers to parse a string AWS does not emit here.
Fix: the request-body deserialization/validation layer for this REST-JSON service (the pre-handler validate step that maps an unknown enum member onto an error), which must answer the recorded 'Invalid request body' refusal rather than synthesising a per-constraint ValidationException.
Traces: drs-launch-configuration-template-lifecycle#task#3 (cluster also covers drs-launch-configuration-template-validation-errors#task#1 and #2).

Cause 2 of 2 — a list-length violation is refused with the wrong error code and message.
AWS: Smithy: CreateReplicationConfigurationTemplate.pitPolicy is @required and targets PITPolicy with @length min 3, max 3, so a 2-element policy is invalid. The recording shows HTTP 400 with x-amzn-errortype 'BadRequestException' and message 'Invalid request body' — fixed text, carrying no member name, element count or bound — and nothing created.
Emulator: HTTP 400 with x-amzn-errortype 'ValidationException' and the message "1 validation error detected: Value '<n> items' at '<member>' failed to satisfy constraint: Member must have length greater than or equal to 3".
Why: the error code is one the DRS client model does not pair with this refusal, so error-type matching fails and callers mis-route a malformed-body error (which is never retryable and never an account/constraint-specific condition) into the wrong handling branch.
Fix: same request-body validation layer as Cause 1 — a shape-constraint violation on the body must surface as BadRequestException / 'Invalid request body' for this service rather than the framework's ValidationException constraint report.
Traces: drs-replication-configuration-template-lifecycle#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; 68 steps, 12 passed).

Cause 1 — drs-launch-configuration-template-lifecycle#task#3 (CreateLaunchConfigurationTemplate), status 400 vs 400:
  expected $.message 'Invalid request body' (x-amzn-errortype shaped, not asserted)
  actual   $.message "1 validation error detected: Value 'BOGUS' at 'recoveryMode' failed to satisfy constraint: Member must satisfy enum value set: [FAST, OPTIMAL]", header x-amzn-errortype 'ValidationException'
  ('BOGUS' and 'recoveryMode' are this recording's request values; the requirement is the fixed 'Invalid request body' text)
  cluster also: drs-launch-configuration-template-validation-errors#task#1, drs-launch-configuration-template-validation-errors#task#2

Cause 2 — drs-replication-configuration-template-lifecycle#task#6 (CreateReplicationConfigurationTemplate), status 400 vs 400:
  expected $.message 'Invalid request body', header x-amzn-errortype 'BadRequestException'
  actual   $.message "1 validation error detected: Value '2 items' at 'pitPolicy' failed to satisfy constraint: Member must have length greater than or equal to 3", header x-amzn-errortype 'ValidationException'

## Steps to Reproduce
Boot the emulator from this checkout and send REST-JSON requests to the drs endpoint.

Cause 1: POST /CreateLaunchConfigurationTemplate with recoveryMode set to any value outside [FAST, OPTIMAL]. Observe 400 ValidationException with the '1 validation error detected: ... enum value set' message instead of the recorded 'Invalid request body' refusal. Re-replay drs-launch-configuration-template-lifecycle#task#3.

Cause 2: POST /CreateReplicationConfigurationTemplate with an otherwise valid body whose pitPolicy has 2 rules (modelled @length is exactly 3). Observe 400 ValidationException with the '... length greater than or equal to 3' message instead of 400 BadRequestException / 'Invalid request body'. Re-replay drs-replication-configuration-template-lifecycle#task#6.
