Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config__sns__sqs.config-item-change-notification-diff-names-the-changed-field
Issue-Key: config__sns__sqs.config-item-change-notification-diff-names-the-changed-field|_drain_notifications#9|Timeout|resp = sqs.receive_message(
Issue-Commit: 699f6ed7a7b73d87a9db3c950b6f69a120aa392c

# Config publishes no ConfigurationItemChangeNotification to the delivery channel's SNS topic, so no configurationItemDiff ever names the changed field

> _Note: this issue was identified against the code base at commit `699f6ed7a7b73d87a9db3c950b6f69a120aa392c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDeliveryChannel

## Description
ONE CAUSE.

Cause 1 of 1: a delivery channel carrying an snsTopicARN never publishes configuration-item change notifications.
AWS: confirmed against real AWS by the check (us-west-2, 2026-09-17) — a delivery channel whose snsTopicARN names a real topic publishes ConfigurationItemChangeNotification messages to that topic, which reach an SQS subscriber. The body is an SNS envelope (Type=Notification, TopicArn, Message as a JSON string) whose Message carries messageType, configurationItem and configurationItemDiff; configurationItem.configuration arrives as a native JSON object (not a JSON-encoded string, unlike GetResourceConfigHistory). The discovery notification's configurationItemDiff.changeType is "CREATE"; changing one attribute of the recorded resource produces a second notification with top-level changeType "UPDATE" and a changedProperties entry NAMING the changed field (e.g. a key ending in "VisibilityTimeout"), with previousValue/updatedValue equal to the old/new attribute values.
Emulator: the check sets up the recorder, the delivery channel with the SNS topic and the SQS subscription, mutates the resource, and polls the queue until its 60s deadline ("Check timed out after 60.0s"). No notification of any shape arrives.
Why: the notification leg of Config does not exist, so change-driven consumers (pipelines subscribed to Config's topic) never run, and the configurationItemDiff — the only place AWS names WHICH property changed and its previous/updated values — is unavailable. A client cannot distinguish "resource unchanged" from "notifications are not implemented".
Fix: the delivery-channel notification path — when a configuration item is recorded or changes, publish a ConfigurationItemChangeNotification (with configurationItemDiff.changeType and changedProperties computed from the previous item) to the channel's snsTopicARN via the SNS service (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_delivery_channel.rs's channel state plus the recorder's change-notification emission). Depends on the recorder actually producing configuration items.
Traces: config__sns__sqs.config-item-change-notification-diff-names-the-changed-field.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from mainline (emulator serving, bootstrap_signal false).

config__sns__sqs.config-item-change-notification-diff-names-the-changed-field:
  Error: Check timed out after 60.0s

The check carries its AWS measurements inline (marked CONFIRMED against real AWS, us-west-2, 2026-09-17): SNS envelope with Message carrying messageType/configurationItem/configurationItemDiff; configurationItem.configuration as a native JSON object; discovery diff changeType "CREATE"; post-mutation diff changeType "UPDATE" with a changedProperties entry keyed on the changed field and previousValue/updatedValue equal to the old/new attribute strings. Nothing was delivered to the subscribed SQS queue before the deadline, so none of those assertions was even reached.

Note: this check's service token is the composite suite token `config__sns__sqs`; the services involved (configservice, sns, sqs) are all bootstrapped.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay the behavioral suite simparity-behavioral-aws-v1 and re-replay config__sns__sqs.config-item-change-notification-diff-names-the-changed-field.
3. Or directly: create an SNS topic with an owner access policy enumerating the SNS actions, subscribe an SQS queue to it, PutDeliveryChannel with that snsTopicARN, StartConfigurationRecorder, create an SQS queue as the recorded resource and then change one of its attributes (e.g. VisibilityTimeout 31 -> 41), and ReceiveMessage on the subscriber queue — no ConfigurationItemChangeNotification arrives.
