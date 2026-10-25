Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config__events__sqs.config-compliance-change-event-reaches-eventbridge-with-old-and-new-verdicts
Issue-Key: config__events__sqs.config-compliance-change-event-reaches-eventbridge-with-old-and-new-verdicts|_drain_events#12|Timeout|resp = sqs.receive_message(
Issue-Commit: 699f6ed7a7b73d87a9db3c950b6f69a120aa392c

# Config emits no 'Config Rules Compliance Change' events to EventBridge: a compliance verdict flip produces no event on the default bus

## Resolution

Config now evaluates the rules in whose scope a recorded resource falls, and publishes the
`Config Rules Compliance Change` notification onto the account's default EventBridge bus
whenever a rule's verdict for a resource changes — which is the whole path the recorded check
polls for, and that check now passes.

Three legs, all in `services/service-com-amazonaws-configservice-starlingdoveservice/`:

1. RECORDING. `src/state/recording.rs` is what a running configuration recorder does between
   calls: a pass is owed `RECORDING_PASS_DUE_AFTER` after the call that makes one due (starting
   a recorder, writing a rule) and is run by the in-process pump `drive_recorder_clock` starts,
   so a client that consumes only the resulting events makes no further Config call. The pass
   OBSERVES the resources of the types the recording group names and this emulator can serve
   (`OBSERVABLE_RESOURCE_TYPES`, S3 buckets today) through the owning service's own API over the
   in-process callback, and writes a configuration item per resource into the inventory
   `ListDiscoveredResources` already reads — with each feature's configuration in the
   `supplementaryConfiguration` block Config records it in, and the states a resource has left
   kept as its history.
2. EVALUATION. `MANAGED_RULES` is the catalogue of AWS managed rules this emulator evaluates
   (`S3_BUCKET_VERSIONING_ENABLED`, `S3_BUCKET_SERVER_SIDE_ENCRYPTION_ENABLED`), each verdict
   computed from the recorded configuration item alone. A rule's `Scope` decides which recorded
   resources it places; a rule whose implementation is not in the catalogue records nothing and
   keeps reporting `INSUFFICIENT_DATA` rather than a synthesised verdict.
3. EMISSION. A verdict that CHANGED is published through
   `crate::state::compliance_change_detail` (with `oldEvaluationResult` present exactly when a
   verdict was REPLACED) and `crate::state::publish_service_event` — the same pair
   `PutExternalEvaluation`, the verdict writer a client drives, already publishes through.

`GetResourceConfigHistory` is implemented in the same change because the recorded check reads
the bucket's configuration history between the two verdicts; its own stub issue is
`20261023T140058Z-config-getresourceconfighistory-stub.md`, and the other configuration-item
operations are left to theirs. The delivery leg — EventBridge INVOKING the SQS queue targets of
the rules a published event matched — landed on mainline independently and is kept as mainline
has it.

Pinned by
`services/service-com-amazonaws-configservice-starlingdoveservice/tests/all/config_recorded_resource_compliance.rs`
(Config + S3 + EventBridge + SQS over one engine: the verdict table, the scope table, the
configuration history and a stopped recorder observing nothing),
`core/router/tests/all/config_compliance_change_event.rs` (Config -> EventBridge -> SQS),
`.../tests/all/config_external_evaluation_compliance.rs` (the verdict a client delivers, read
back) and the tables beside the catalogue in `src/state/recording.rs`.


> _Note: this issue was identified against the code base at commit `699f6ed7a7b73d87a9db3c950b6f69a120aa392c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutConfigRule

## Description
ONE CAUSE.

Cause 1 of 1: a Config rule's compliance verdict change produces no EventBridge event.
AWS: recorded behavior only (the EventBridge reference for Config, events-ref-config.html, is the check's stated basis) — when a Config rule's evaluation verdict changes, Config publishes an event on the DEFAULT event bus with source "aws.config" and detail-type "Config Rules Compliance Change"; detail.messageType is "ComplianceChangeNotification", the verdict is at detail.newEvaluationResult.complianceType and the rule name at detail.newEvaluationResult.evaluationResultIdentifier.evaluationResultQualifier.configRuleName, and a verdict FLIP carries oldEvaluationResult alongside newEvaluationResult. An EventBridge rule filtering only on source + detail-type, targeting an SQS queue, therefore receives one message per verdict change.
Emulator: the check creates the recorder, delivery channel, rule and the EventBridge rule → SQS target, drives a verdict flip, and polls the queue until its 60s deadline ("Check timed out after 60.0s"). No event is ever delivered.
Why: every event-driven Config consumer is dead: compliance-change automation (remediation triggers, alerting, ticketing) that subscribes on the default bus never fires, and a client cannot distinguish "no verdict change yet" from "Config does not emit events at all". Because the event is also the only carrier of oldEvaluationResult, a consumer cannot see what the verdict changed FROM.
Fix: Config's evaluation/compliance state transition must publish the Config Rules Compliance Change event onto the default EventBridge bus when a rule's verdict changes — the notification/eventing side of the rule-evaluation path (services/service-com-amazonaws-configservice-starlingdoveservice/src/ — the rule evaluation state module and its event emission), delivered through the eventbridge service crate's default bus. This is a missing emission, not a wrong event body: no event of any shape arrives.
Traces: config__events__sqs.config-compliance-change-event-reaches-eventbridge-with-old-and-new-verdicts.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from mainline (emulator serving, bootstrap_signal false).

config__events__sqs.config-compliance-change-event-reaches-eventbridge-with-old-and-new-verdicts:
  Error: Check timed out after 60.0s

The check's own notes record the AWS behaviour it polls for: events on the default bus with source "aws.config" and detail-type "Config Rules Compliance Change", detail.messageType "ComplianceChangeNotification", verdict at detail.newEvaluationResult.complianceType, rule name at detail.newEvaluationResult.evaluationResultIdentifier.evaluationResultQualifier.configRuleName, and oldEvaluationResult present on a flip. Its EventBridge rule pattern filters on source + detail-type only, so any event of that type would have been captured; the SQS target received nothing before the deadline.

Note: this check's service token is the composite suite token `config__events__sqs`; the services involved (configservice, eventbridge, sqs) are all bootstrapped.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay the behavioral suite simparity-behavioral-aws-v1 and re-replay config__events__sqs.config-compliance-change-event-reaches-eventbridge-with-old-and-new-verdicts.
3. Or directly: create a configuration recorder + delivery channel, create an EventBridge rule on the default bus with pattern {"source":["aws.config"],"detail-type":["Config Rules Compliance Change"]} targeting an SQS queue, create a Config rule and drive its verdict from one compliance state to another, then ReceiveMessage on the queue — no message ever arrives.
