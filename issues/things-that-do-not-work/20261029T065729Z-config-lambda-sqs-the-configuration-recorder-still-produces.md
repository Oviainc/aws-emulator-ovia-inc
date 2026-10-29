Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config__lambda__sqs.change-triggered-custom-rule-is-push-invoked-with-a-real-configuration-item
Issue-Key: config__lambda__sqs.change-triggered-custom-rule-is-push-invoked-with-a-real-configuration-item|run#33|AssertionError|assert items, (
Issue-Commit: c17c6e54c038586d9d6328bb68b9d1b3d6bcf360

# config__lambda__sqs: the configuration recorder still produces no configuration item for an SQS queue, so a change-triggered CUSTOM_LAMBDA rule is never push-invoked (1 cause)

> _Note: this issue was identified against the code base at commit `c17c6e54c038586d9d6328bb68b9d1b3d6bcf360`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourceConfigHistory

## Description
ONE root cause. ONE CAUSE must be closed before submitting.

The check's service token is the composite suite token `config__lambda__sqs`; all three underlying services (configservice, lambda, sqs) are bootstrapped and the defect belongs to configservice's recording pipeline.

Cause 1 of 1: a RUNNING customer-managed configuration recorder whose recordingGroup has allSupported=true never discovers or records a resource that lives in another emulated service, so no configuration item is ever produced for an AWS::SQS::Queue and the change-triggered (ConfigurationItemChangeNotification) CUSTOM_LAMBDA rule that would be pushed that item is never invoked.

AWS: recorded behavior only, with model grounding. Smithy (configservice): RecordingGroup.allSupported — Config records configuration changes for all supported resource types; GetResourceConfigHistory{resourceType, resourceId required} -> {configurationItems, nextToken}, paginated, with ResourceNotDiscoveredException (errorKind client, httpError 400) for a key Config has recorded nothing for; ConfigurationItem carries configurationItemCaptureTime, configurationItemStatus (ConfigurationItemStatus = OK | ResourceDiscovered | ResourceNotRecorded | ResourceDeleted | ResourceDeletedNotRecorded), arn, resourceType, resourceId, resourceName, awsRegion and the JSON-encoded `configuration` string. Recorded behavior measured inline by this check: with a fresh allSupported recorder and a delivery channel, a queue created afterwards gets its FIRST configuration item (the discovery item) in ~60 s; GetResourceConfigHistory then returns it with arn equal to the queue's QueueArn, resourceName equal to the queue name and a configuration JSON whose VisibilityTimeout matches the live attribute. Config then PUSH-invokes a CUSTOM_LAMBDA rule whose SourceDetails is [{EventSource: aws.config, MessageType: ConfigurationItemChangeNotification}] roughly 20 s after PutConfigRule, handing the function that item in invokingEvent.configurationItem — with no StartConfigRulesEvaluation involved. The queue url/name, arn and the VisibilityTimeout value are request/state values; the fixed requirement is that the item exists and that it is what Config pushes to the evaluator.

Emulator: nothing is recorded. The check now fails with an explicit assertion rather than a bare harness timeout: AssertionError "DATAPLANE_UNIMPLEMENTED: the configuration recorder produced no configuration item for queue <queue url> in 50s. Recorder status: [{'name': <recorder>, 'recording': True, 'lastStatus': 'Success', 'lastErrorCode': None, 'lastErrorMessage': None}]. Real AWS produced the first item in 60 s. Without a recorded item there is nothing for Config to push, so every assertion below is unreachable" (check line 432). PutConfigurationRecorder / PutDeliveryChannel / StartConfigurationRecorder all succeed and DescribeConfigurationRecorderStatus reports recording=true with lastStatus Success and no error, yet Config's inventory only ever grows from an explicit client write through PutResourceConfig — there is no path by which a queue created through the SQS service enters it. Nothing is refused.

Related open issue, named for overlap but NOT relied on as cover (the suite did not mark this check OWNED by it — its replay key has changed from a 60 s harness timeout to this assertion at check line 432, so the key it carries no longer matches): 20261025T144945Z-config-the-configuration-recorder-never-records-resources-from.md. Also named, not relied on: 20261022T190340Z-config-sns-sqs-config-publishes-no-configurationitemchangenotification-to-the-delivery-channel.md, which covers publishing the notification to the channel's SNS topic and depends on this same missing recording.

Why: a client cannot use Config for what Config exists for. Resource-inventory reads are indistinguishable from "the account has nothing" while the recorder reports recording=true and lastStatus Success, so an inventory or compliance consumer reads empty forever with no error to tell a stub from a working recorder. Because no configuration item exists, no change-triggered Config rule can ever be evaluated: a CUSTOM_LAMBDA guardrail that would mark a newly created or modified queue NON_COMPLIANT silently never runs and the account looks green.

Fix: the configuration-recorder recording loop of the Config service — while a customer-managed recorder is recording and its recordingGroup covers the type, resources created/changed/deleted in the other emulated services must be captured into the recorder's inventory (the `discovered_resources` state behind services/service-com-amazonaws-configservice-starlingdoveservice/src/state.rs, today written only by the PutResourceConfig path) as configuration items with configurationItemStatus ResourceDiscovered on first capture and OK on subsequent states; the change-triggered rule-evaluation path must then push that item to the rule's CUSTOM_LAMBDA function as invokingEvent.configurationItem without any StartConfigRulesEvaluation call.

Traces: config__lambda__sqs.change-triggered-custom-rule-is-push-invoked-with-a-real-configuration-item (check line 432).

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline (emulator serving, bootstrap_signal false; 70/113 checks passed, 29 mismatched / 14 errored). config__lambda__sqs is 0/2 (0%); the sibling check is OWNED and skipped, this one is marked neither ALREADY FILED nor OWNED.

[mismatch] config__lambda__sqs.change-triggered-custom-rule-is-push-invoked-with-a-real-configuration-item
  File "check_config__lambda__sqs_change_triggered_custom_rule_is_push_invoked_with_a_real_configuration_item.py", line 432, in run
    assert items, (
  AssertionError: DATAPLANE_UNIMPLEMENTED: the configuration recorder produced no configuration item for queue 'https://<host>/000000000000/sim2real-behav-f4cfgq-fd1403992a90' in 50s. Recorder status: [{'name': 'sim2real-behav-cfg-recorder-00aa84325c70', 'recording': True, 'lastStatus': 'Success', 'lastErrorCode': None, 'lastErrorMessage': None}]. Real AWS produced the first item in 60 s. Without a recorded item there is nothing for Config to push, so every assertion below is unreachable

(The queue URL host/port, the account id and the generated queue/recorder names are per-run values and are not part of the requirement; the emulator's own recorder status is quoted because it shows the recorder reporting healthy while recording nothing.)

Check shape (from the replayed source): SERVICES = ["config", "lambda", "sqs"]; an IAM role trusting config.amazonaws.com with AWS_ConfigRole; a Lambda evaluator that reads the configurationItem Config HANDED it and echoes its fields back through Annotation (the check never passes the queue's state to the function); lambda:AddPermission for config.amazonaws.com; a recorder with allSupported=true plus a delivery channel; sqs:CreateQueue; _CI_BUDGET_S = 1400 s for the first configuration item, _FIRST_EVAL_BUDGET_S = 300 s for the first push evaluation, _FLIP_BUDGET_S = 600 s for the second. Expected (AWS, measured inline): the first item in ~60 s and the first push evaluation ~20 s after PutConfigRule. Actual (emulator): no item at all, so the push-invocation and annotation-echo assertions are unreachable.

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): GetResourceConfigHistory{resourceType, resourceId required} -> {configurationItems, nextToken}, paginated, errors include ResourceNotDiscoveredException (client, 400); RecordingGroup{allSupported, includeGlobalResourceTypes, resourceTypes, ...}; ConfigurationItem{configurationItemStatus, arn, resourceType, resourceId, resourceName, awsRegion, configuration, relationships}.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline.
2. Replay simparity-behavioral-ga-prod-v1 and re-replay config__lambda__sqs.change-triggered-custom-rule-is-push-invoked-with-a-real-configuration-item.
3. Manually: iam create-role trusting config.amazonaws.com with AWS_ConfigRole attached; s3 create-bucket + put-bucket-policy with the three AWSConfigBucket* statements; config put-configuration-recorder --configuration-recorder name=<rec>,roleARN=<role>,recordingGroup={allSupported=true}; config put-delivery-channel --delivery-channel name=<chan>,s3BucketName=<bucket>; config start-configuration-recorder --configuration-recorder-name <rec> (recording=true, lastStatus Success).
4. sqs create-queue --queue-name <q> --attributes VisibilityTimeout=137, then poll config get-resource-config-history --resource-type AWS::SQS::Queue --resource-id <queue url>: on AWS a configuration item appears within ~1 minute; the emulator answers ResourceNotDiscoveredException / an empty configurationItems list indefinitely.
5. Create a Lambda evaluator that echoes event['invokingEvent']['configurationItem'] fields into its PutEvaluations Annotation; lambda add-permission --principal config.amazonaws.com; config put-config-rule with Source={Owner: CUSTOM_LAMBDA, SourceIdentifier: <fn arn>, SourceDetails: [{EventSource: aws.config, MessageType: ConfigurationItemChangeNotification}]} and WITHOUT calling config start-config-rules-evaluation, poll config get-compliance-details-by-config-rule: on AWS a verdict carrying the pushed item's fields appears in ~20 s; the emulator never produces one.
6. Change the queue (sqs set-queue-attributes --attributes VisibilityTimeout=60) and observe that no second configuration item and no second push evaluation ever occur.
