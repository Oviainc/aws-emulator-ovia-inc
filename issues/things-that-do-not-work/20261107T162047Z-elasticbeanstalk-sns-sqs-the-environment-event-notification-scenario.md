Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticbeanstalk__sns__sqs.eb-environment-event-notification-is-delivered-to-the-subscribed-queue
Issue-Key: elasticbeanstalk__sns__sqs.eb-environment-event-notification-is-delivered-to-the-subscribed-queue|_drain#39|Timeout|return bodies
Issue-Commit: 6bba0ea93be6d4db7a2caa629527597bd59ab221

# elasticbeanstalk__sns__sqs: the environment event-notification scenario never reaches an observable state — no PROCESSED version / Ready environment, so no event is ever published to the attached SNS topic

> _Note: this issue was identified against the code base at commit `6bba0ea93be6d4db7a2caa629527597bd59ab221`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateEnvironment

## Description
Cause 1 of 1: an Elastic Beanstalk environment wired to an SNS topic through the `aws:elasticbeanstalk:sns:topics` / `Notification Topic ARN` option setting never publishes its lifecycle events, and the scenario never even advances far enough to try: the check exhausts its whole 60 s budget and is cut off (`Check timed out after 60.0s`).

AWS: recorded behavior only for the end-to-end delivery (the Smithy model binds no notification semantics to an option setting). AWS documents the behavior at https://docs.aws.amazon.com/elasticbeanstalk/latest/dg/using-features.managing.sns.html — setting `Notification Topic ARN` in the `aws:elasticbeanstalk:sns:topics` namespace makes Elastic Beanstalk publish that environment's events to the named topic, and only to that topic. The pre-conditions the scenario walks are themselves modeled/recorded: `CreateApplicationVersion(Process=true)` must drive `DescribeApplicationVersions[].Status` from `PROCESSING` to `PROCESSED`; `CreateEnvironment` must drive `DescribeEnvironments[].Status` to `Ready`; `DescribeConfigurationSettings` must read back exactly one OptionSettings entry with Namespace `aws:elasticbeanstalk:sns:topics`, OptionName `Notification Topic ARN` and Value equal to the topic ARN supplied at create time; `UpdateEnvironment` with a changed option setting must answer `Status=Updating` and then settle back to `Ready`, publishing an event whose envelope carries `TopicArn` = the attached topic and whose `Subject`/`Message` names the environment.

Emulator: the check never returns a verdict. The SNS/SQS calibration leg of the scenario works (a direct `Publish` to the control topic is drained from the subscribed queue), so the hang is on the Elastic Beanstalk side: the application version does not leave `PROCESSING`/`Unprocessed` and/or the environment never settles `Ready`, so `UpdateEnvironment` is never reached and nothing is ever published to the attached topic. The runner terminates the check at its 60 s wall clock with `Check timed out after 60.0s` instead of any assertion message.

Why: a client that deploys an application version and then waits on `DescribeApplicationVersions`/`DescribeEnvironments`, or that relies on Beanstalk's SNS notifications to drive deployment automation (alarms, ChatOps, promotion gates), hangs forever and receives no event at all. Nothing distinguishes "the update is still running" from "the update will never happen", and the notification contract — events go to the attached topic and only there — is unverifiable.

Fix: the Elastic Beanstalk application-version processing state machine (CreateApplicationVersion/DescribeApplicationVersions handlers and their state), the environment lifecycle state machine (CreateEnvironment/UpdateEnvironment/DescribeEnvironments handlers and their state), and the event-publication path that fans environment events out to the `aws:elasticbeanstalk:sns:topics` / `Notification Topic ARN` topic via SNS Publish.

Related but NOT covering this check: open issues 20261029T180136Z-elasticbeanstalk-elastic-beanstalk-application-versions-stay-status-unprocessed-forever.md and 20261029T180136Z-elasticbeanstalk-elastic-beanstalk-lifecycle-and-log-retrieval-operations-are.md describe the same family of stalls for the `elasticbeanstalk` service alone; neither carries this cross-service check's replay key, and neither covers the SNS notification fan-out asserted here.

Traces: elasticbeanstalk__sns__sqs.eb-environment-event-notification-is-delivered-to-the-subscribed-queue.

## Evidence
Expected: `CreateApplicationVersion(Process=true)` -> `DescribeApplicationVersions[0].Status` becomes `PROCESSED`; `CreateEnvironment(... OptionSettings=[{Namespace: 'aws:elasticbeanstalk:sns:topics', OptionName: 'Notification Topic ARN', Value: <topic-arn>}])` -> `DescribeEnvironments[0].Status` becomes `Ready`; `DescribeConfigurationSettings` reads back exactly one entry for that namespace/option with Value = <topic-arn>; `UpdateEnvironment` answers `{EnvironmentName: <env>, Status: 'Updating'}`, settles back to `Ready`, and an SNS notification lands in the queue subscribed to <topic-arn> with `TopicArn` = <topic-arn> and the environment name appearing in `Subject` or `Message`; the unattached control topic's queue receives nothing but the later direct publish.

Actual: the check produces no verdict at all — the runner reports `Check timed out after 60.0s`. The SNS/SQS calibration publish to the control topic does arrive, so the wiring under test is live; the Beanstalk legs (version processing, environment Ready, UpdateEnvironment, event publication) never complete. No notification envelope is ever delivered to the attached queue.

(Values such as the generated application/environment/version names, topic and queue ARNs and nonce tokens are per-run and are not part of the requirement; the requirement is the status progression and that an event envelope carrying the attached TopicArn and naming the environment is delivered.)

## Steps to Reproduce
1. Create an SNS topic and an SQS queue; set the queue policy to allow sns:SendMessage from the topic and subscribe the queue to the topic (protocol `sqs`). Do the same for a second, unattached "control" topic/queue pair.
2. Publish directly to the control topic and drain the control queue — this calibration arrives, proving the SNS->SQS wiring is live.
3. elasticbeanstalk: CreateApplication; CreateStorageLocation; PutObject the source bundle; CreateApplicationVersion(ApplicationName, VersionLabel, SourceBundle, Process=true).
4. Poll DescribeApplicationVersions(ApplicationName, VersionLabels=[label]) until Status leaves `PROCESSING` — it never becomes `PROCESSED`.
5. CreateEnvironment with the base option settings plus {Namespace: 'aws:elasticbeanstalk:sns:topics', OptionName: 'Notification Topic ARN', Value: <attached topic ARN>} and poll DescribeEnvironments until Status == 'Ready'.
6. UpdateEnvironment with a changed option setting and poll back to Ready; then ReceiveMessage on the attached queue.
7. Observed: steps 4-6 never complete; the check is cut off at 60 s with `Check timed out after 60.0s` and no notification is ever delivered to the attached queue.

Replay: elasticbeanstalk__sns__sqs.eb-environment-event-notification-is-delivered-to-the-subscribed-queue
