Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy__sns__sqs.deployment-trigger-delivers-to-the-subscribed-event-topic-only
Issue-Key: codedeploy__sns__sqs.deployment-trigger-delivers-to-the-subscribed-event-topic-only|_drain_queue#17|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=10)
Issue-Commit: d6e9bd85505b2fcf81ff15dd47c3206271b87270

# Deployment-group triggerConfigurations are stored but never publish an SNS notification on DeploymentSuccess/DeploymentFailure

> _Note: this issue was identified against the code base at commit `d6e9bd85505b2fcf81ff15dd47c3206271b87270`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeploymentGroup

## Description
Cause 1 of 1: a deployment group's triggerConfigurations round-trip through GetDeploymentGroup but no notification is ever published to the configured SNS topic, for any deployment outcome.
AWS: Smithy: CreateDeploymentGroup/UpdateDeploymentGroup accept triggerConfigurations (TriggerConfig), documented as "Information about notification triggers for the deployment group"; TriggerConfig.triggerTargetArn is "The Amazon Resource Name (ARN) of the Amazon Simple Notification Service topic through which notifications about deployment or instance events are sent" and TriggerConfig.triggerEvents is "The event type or types for which notifications are triggered" (TriggerEventType = DeploymentStart | DeploymentSuccess | DeploymentFailure | DeploymentStop | DeploymentRollback | DeploymentReady | InstanceStart | InstanceSuccess | InstanceFailure | InstanceReady). Recorded behavior (us-west-2, per the check's measurement header): the trigger really publishes to its topic on the matching outcome and only on the matching outcome; the body is JSON delivered through the standard SNS envelope carrying deploymentId, applicationName, deploymentGroupName and status, with eventTriggerName echoing the trigger CONFIGURATION's own triggerName. Setting triggerConfigurations also publishes a one-time non-JSON verification notification as a side effect of CreateDeploymentGroup itself. Field values, latency and the exact body text are examples; the requirement is that a matching outcome publishes to the subscribed topic and a non-matching outcome does not.
Emulator: the trigger list is accepted, validated and echoed only. services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/state.rs holds DeploymentGroup.trigger_configurations: Vec<TriggerConfig> and the only other reference to it is deployment_group_info's `trigger_configurations: Some(group.trigger_configurations.clone())` echo; there is no publish path, no SNS client use and no reference to triggerEvents in any deployment transition. Consequently a deployment that reaches a terminal status delivers nothing to the subscribed topic, and the consumer-side observation the check makes never completes.
Why: a client that wires CodeDeploy notifications to SNS (and onward to SQS, Lambda, chat or a pipeline gate) sees deployments complete with no notification at all. Both halves of the contract are unusable against this emulator: the positive (the success topic receives a message naming the deployment) and the negative (the failure topic receives nothing) are indistinguishable, so event-filter logic and any downstream consumer cannot be tested, and a deployment-gating consumer silently never fires.
Fix: the deployment clock / state machine in the codedeploy service state — on each deployment lifecycle outcome it must match the deployment group's triggerConfigurations by triggerEvents and publish the notification to each matching triggerTargetArn (plus the one-time verification notification at CreateDeploymentGroup time), rather than storing the list only.
Traces: codedeploy__sns__sqs.deployment-trigger-delivers-to-the-subscribed-event-topic-only.

Note on the observed symptom: this check is reported as a 60 s timeout rather than an assertion diff, and the check's own fixed waits (a 20 s subscription-settle plus a >=45 s calibrated absence window) mean the timeout alone does not pin the failure point. The defect is grounded in the stored-but-never-read trigger configuration above, which is what makes the check's success-notification wait unsatisfiable. Confirm by re-replaying this check with a budget above its 205 s measured wall clock.

## Evidence
Suite: simparity-behavioral-ga-prod-v1, codedeploy__sns__sqs 0/1 (0%).

[mismatch] codedeploy__sns__sqs.deployment-trigger-delivers-to-the-subscribed-event-topic-only
  Error: Check timed out after 60.0s

The check creates two SNS topics each subscribed by its own SQS queue, creates a deployment group with triggerConfigurations = [{triggerName: 'sim2real-on-success', triggerTargetArn: <success topic>, triggerEvents: ['DeploymentSuccess']}, {triggerName: 'sim2real-on-failure', triggerTargetArn: <failure topic>, triggerEvents: ['DeploymentFailure']}], and then, after a succeeding deployment, requires the success queue to receive a notification naming that deploymentId while the failure queue stays empty (asserted both by an empty receive and by ApproximateNumberOfMessages == 0), followed by a failing deployment that must deliver on the failure topic only.

Expected: GetDeploymentGroup echoes both triggers (it does), AND the matching topic receives a message per outcome.
Observed: nothing is ever published. Source corroboration: in services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/state.rs, trigger_configurations is written at CreateDeploymentGroup and read back only in the GetDeploymentGroup projection (`trigger_configurations: Some(group.trigger_configurations.clone())`); no deployment transition reads triggerEvents and no SNS publish exists anywhere in the service, so neither the per-outcome notification nor the one-time trigger-verification notification can be produced.

## Steps to Reproduce
1. CreateTopic T_success and T_failure; CreateQueue Q_success and Q_failure; Subscribe each queue to its topic and set the queue policies.
2. CreateApplication(computePlatform=Lambda) and CreateDeploymentGroup with triggerConfigurations = [{triggerName: 'on-success', triggerTargetArn: T_success, triggerEvents: ['DeploymentSuccess']}, {triggerName: 'on-failure', triggerTargetArn: T_failure, triggerEvents: ['DeploymentFailure']}].
3. Observe Q_success/Q_failure: AWS delivers a one-time trigger-verification notification to each topic as a side effect of step 2; the emulator delivers none.
4. CreateDeployment with a valid AppSpecContent revision and wait for it to reach Succeeded.
5. ReceiveMessage on Q_success. AWS: a JSON notification carrying this deploymentId, applicationName, deploymentGroupName and status, with eventTriggerName == 'on-success'. Emulator: no message, and ApproximateNumberOfMessages stays 0.
6. CreateDeployment with a revision naming a non-existent Lambda alias and wait for Failed; ReceiveMessage on Q_failure. AWS: a notification for that deployment on the failure topic only. Emulator: no message on either topic.
