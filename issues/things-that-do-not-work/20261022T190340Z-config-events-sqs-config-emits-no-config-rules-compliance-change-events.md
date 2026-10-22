Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config__events__sqs.config-compliance-change-event-reaches-eventbridge-with-old-and-new-verdicts
Issue-Key: config__events__sqs.config-compliance-change-event-reaches-eventbridge-with-old-and-new-verdicts|_drain_events#12|Timeout|resp = sqs.receive_message(
Issue-Commit: 699f6ed7a7b73d87a9db3c950b6f69a120aa392c

# Config emits no 'Config Rules Compliance Change' events to EventBridge: a compliance verdict flip produces no event on the default bus

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
