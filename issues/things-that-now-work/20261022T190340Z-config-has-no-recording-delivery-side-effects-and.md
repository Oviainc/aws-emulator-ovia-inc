Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.deploy-config-rule-cloudformation-evaluates-resource config.deploy-configuration-recorder-cloudformation-actually-records config.delivery-channel-bucket-policy-statement-required config.e2e-remediation-auto-execution-consequence config.remediation-exceptions-lifecycle
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config-service|DescribeConfigRules|NoSuchConfigRuleException
Issue-Key: config.deploy-configuration-recorder-cloudformation-actually-records|_poll_recorder_exists#10|Timeout|time.sleep(5)
Issue-Key: config.delivery-channel-bucket-policy-statement-required|_poll_until#25|Timeout|time.sleep(interval_s)
Issue-Key: config|DescribeImages|NotImplemented
Issue-Commit: 699f6ed7a7b73d87a9db3c950b6f69a120aa392c

# Config has no recording/delivery side effects and CloudFormation does not provision Config resources: CFN-declared ConfigRule/ConfigurationRecorder never materialise, no snapshot is ever delivered to S3, and ec2 DescribeImages blocks the remediation flows

> _Note: this issue was identified against the code base at commit `699f6ed7a7b73d87a9db3c950b6f69a120aa392c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartConfigurationRecorder

## Description
Four causes where the emulator accepts the control-plane calls but the consequence AWS produces never happens, so checks that poll for the effect run to their deadline. FOUR CAUSES must be closed.

Cause 1 of 4: a CloudFormation stack declaring AWS::Config::ConfigRule does not create the rule.
AWS: recorded behavior only — a CloudFormation stack whose template declares an AWS::Config::ConfigRule reaches CREATE_COMPLETE and the rule is then visible to Config: DescribeConfigRules(ConfigRuleNames=[<rule>]) returns it and it evaluates the in-scope resource. Config's own model grounds the observation: DescribeConfigRules raises NoSuchConfigRuleException (client, httpError 400) only for a rule that does not exist (Smithy: NoSuchConfigRuleException — "The Config rule in the request is not valid. Verify that the rule ... name is correct").
Emulator: after the stack operation, DescribeConfigRules for the stack's rule name is rejected with NoSuchConfigRuleException — the resource was never provisioned.
Why: infrastructure-as-code flows are silently no-ops for Config resources: a client deploys a compliance control via CloudFormation, the stack reports success, and nothing exists to evaluate anything.
Fix: CloudFormation's resource provisioning for the AWS::Config::* resource types — the stack-execution path must create the Config rule in Config's state (services/service-com-amazonaws-cloudformation-cloudformation/src/, resource-type dispatch) rather than completing the stack without it.
Traces: config.deploy-config-rule-cloudformation-evaluates-resource.

Cause 2 of 4: a CloudFormation-deployed AWS::Config::ConfigurationRecorder never records.
AWS: recorded behavior only — a stack declaring a configuration recorder (plus delivery channel) leaves a recorder that actually records: configuration items for the in-scope resources become observable through Config's read surface within the check's budget.
Emulator: the check polls for the recorded configuration item until it hits the 60s deadline ("Check timed out after 60.0s") — the recorder exists but never produces a configuration item.
Why: every consumer of Config's recorded inventory (history, snapshots, Select queries, notifications) is permanently empty, so a client cannot tell "not yet recorded" from "recording is not implemented".
Fix: the configuration-recorder state machine — StartConfigurationRecorder must begin producing configuration items for in-scope resource types into Config's recorded state (.../service-com-amazonaws-configservice-starlingdoveservice/src/ops/start_configuration_recorder.rs and the recorder's state module), together with Cause 1's CloudFormation provisioning.
Traces: config.deploy-configuration-recorder-cloudformation-actually-records.

Cause 3 of 4: a delivery channel whose bucket policy permits Config never receives a delivery.
AWS: recorded behavior only — with a running recorder and a delivery channel pointing at a bucket whose policy grants config.amazonaws.com s3:GetBucketAcl/s3:ListBucket/s3:PutObject, Config delivers configuration snapshots/history objects into that bucket, and the delivery is observable (the object appears / DescribeDeliveryChannelStatus reports a successful delivery).
Emulator: the check polls for the delivered object until the 60s deadline ("Check timed out after 60.0s") — nothing is ever written to the bucket.
Why: the delivery leg of Config does not exist, so the bucket-policy requirement cannot be observed either way: a client that validates its Config delivery wiring end to end can never succeed, and the negative case (policy removed → delivery denied) is indistinguishable from the positive one.
Fix: the delivery pipeline behind the delivery channel — periodic/triggered snapshot delivery that writes to the channel's S3 bucket and records delivery status (.../src/ops/put_delivery_channel.rs's channel state plus the delivery task, and DescribeDeliveryChannelStatus).
Traces: config.delivery-channel-bucket-policy-statement-required.

Cause 4 of 4: ec2 DescribeImages is an unimplemented scaffold stub, which blocks Config's remediation flows.
AWS: DescribeImages is a normal EC2 read that returns the Images list; the two checks use it only to resolve an AMI for the resource Config will then remediate.
Emulator: the call fails with Error.Code "NotImplemented" ("operation DescribeImages is scaffolded but not implemented in this emulator"), so both remediation checks abort before reaching Config.
Why: any flow that has to resolve an AMI before exercising remediation is unreachable, and the emulator answers an error code that does not exist in the EC2 API.
Fix: the EC2 DescribeImages handler (services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_images.rs). Filed here because both failing checks are Config checks (check ids config.*); the handler to implement is EC2's.
Traces: config.e2e-remediation-auto-execution-consequence, config.remediation-exceptions-lifecycle.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from mainline (emulator serving, bootstrap_signal false).

Cause 1 — config.deploy-config-rule-cloudformation-evaluates-resource:
  botocore.errorfactory.NoSuchConfigRuleException: An error occurred (NoSuchConfigRuleException) when calling the DescribeConfigRules operation: The ConfigRule 'sim2real-cfn-rule-<id>' provided in the request is invalid. Please check the configRule name.
  i.e. the rule the CloudFormation stack declared does not exist in Config after the stack operation.

Cause 2 — config.deploy-configuration-recorder-cloudformation-actually-records:
  Error: Check timed out after 60.0s — the check polls for a configuration item produced by the CFN-deployed recorder and never sees one.

Cause 3 — config.delivery-channel-bucket-policy-statement-required:
  Error: Check timed out after 60.0s — the check polls for Config's delivery into the policied S3 bucket and never sees one.

Cause 4 — config.e2e-remediation-auto-execution-consequence and config.remediation-exceptions-lifecycle:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeImages operation: operation DescribeImages is scaffolded but not implemented in this emulator

Model grounding: NoSuchConfigRuleException is errorKind client, httpError 400 (defaulted) in services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json; its presence on the DescribeConfigRules lookup is the proof the CFN-declared rule was never created.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay the behavioral suite simparity-behavioral-aws-v1 and re-replay the check ids in trace_ids.
3. Cause 1: CreateStack with a template declaring an AWS::Config::ConfigRule, wait for the stack to finish, then call DescribeConfigRules with that rule name — it raises NoSuchConfigRuleException instead of returning the rule.
4. Cause 2: CreateStack with a template declaring an AWS::Config::ConfigurationRecorder (+ delivery channel), start recording, then poll Config's read surface for a configuration item for an in-scope resource — none ever appears.
5. Cause 3: create a bucket with the AWSConfigBucketPermissionsCheck / AWSConfigBucketExistenceCheck / AWSConfigBucketDelivery statements, PutDeliveryChannel at it, StartConfigurationRecorder, then poll the bucket for a delivered Config object — none ever appears.
6. Cause 4: call ec2 DescribeImages — it returns Error.Code NotImplemented.
