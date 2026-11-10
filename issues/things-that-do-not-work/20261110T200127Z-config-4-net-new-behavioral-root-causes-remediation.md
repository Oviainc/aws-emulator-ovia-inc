Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.remediation-configuration-and-exception-lifecycle config.remediation-exceptions-empty-vs-absent-message-preserved config.deploy-configuration-recorder-cloudformation-actually-records config.resource-config-unicode-and-binary-blob-round-trip
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.remediation-configuration-and-exception-lifecycle|_error#35|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: config.remediation-exceptions-empty-vs-absent-message-preserved|run#29|AssertionError|raise AssertionError(m)
Issue-Key: config|PutBucketTagging|InvalidTag
Issue-Key: config.resource-config-unicode-and-binary-blob-round-trip|_poll_until#3|Timeout|time.sleep(interval_s)
Issue-Commit: 7ddd296fa3724341be1a6933a8dc37e7146d5c05

# config: 4 net-new behavioral root causes — remediation-exception reads never refuse, PutRemediationConfigurations rejects AWS-owned SSM documents, a CFN-created bucket cannot be re-tagged, and the resource-config check never converges

> _Note: this issue was identified against the code base at commit `7ddd296fa3724341be1a6933a8dc37e7146d5c05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRemediationExceptions

## Description
Four distinct, net-new root causes on the AWS Config surface, all reached by checks the suite lists under `### config`. None carries the replay key of an open issue (the report marked none of these four checks OWNED or ALREADY FILED). A fixer must close ALL FOUR causes before submitting.

=== Cause 1 of 4: DescribeRemediationExceptions answers 200 with an empty list for a rule that has NO remediation configuration, where AWS refuses ===
Failing check: config.remediation-configuration-and-exception-lifecycle
AWS: remediation exceptions hang off a rule's remediation CONFIGURATION (Smithy doc, DescribeRemediationExceptions: "Config generates a remediation exception when a problem occurs executing a remediation action to a specific resource"), so a rule with no remediation configuration has no exception list to read at all. Smithy: DescribeRemediationExceptions models InvalidNextTokenException and InvalidParameterValueException, both client/HTTP 400. Recorded behavior: AWS refuses the read with HTTP 400, Error.Code `InvalidParameterValueException` and the message `No Remediation Configuration found for exception` — that text is entirely FIXED (no variable parts; the rule name does not appear in it). The same emulator already refuses the WRITE path correctly for the identical precondition (PutRemediationExceptions answers InvalidParameterValueException "No RemediationConfiguration found for <ConfigRuleName from the request>" and that assertion passed), so only the read path diverges. The refusal is also independent of a preceding DeleteRemediationExceptions: a fresh describe refuses exactly as before.
Emulator: DescribeRemediationExceptions(ConfigRuleName=<existing rule with no remediation configuration>) answers HTTP 200 with body `{"RemediationExceptions": []}`.
Why: a client cannot distinguish "this rule has a remediation configuration and currently has zero exceptions" from "this rule has no remediation configuration at all" — the two require completely different follow-up (put an exception vs. put a remediation configuration first). Code that treats the empty list as proof the rule is remediation-enabled proceeds into calls AWS will refuse, and any caller asserting the modeled refusal diverges.
Fix: the DescribeRemediationExceptions handler's precondition — before listing, it must resolve the rule's remediation configuration and, when there is none, raise the modeled InvalidParameterValueException with the fixed message above, rather than returning an empty list. The existing PutRemediationExceptions precondition is the model to follow; ordering relative to the rule-existence check must stay (NoSuchConfigRuleException for an unknown rule name).

=== Cause 2 of 4: PutRemediationConfigurations refuses an AWS-owned SSM Automation document as a missing remediation target ===
Failing check: config.remediation-exceptions-empty-vs-absent-message-preserved
AWS: Smithy doc, PutRemediationConfigurations: "The Config rule must already exist for you to add a remediation configuration. The target (SSM document) must exist and have permissions to use the target." AWS-owned public Automation documents (here TargetType `SSM_DOCUMENT`, TargetId `AWS-StartEC2Instance`, TargetVersion "1") exist in every account and Region, so the target-existence gate passes and AWS answers HTTP 200 with `FailedBatches: []`. AWS's refusal for a target that genuinely does not exist is HTTP 400 `InvalidParameterValueException` with the two-sentence template whose only variable part is the rule name from the request: fixed text `The remediation target for ` + <ConfigRuleName> + ` could not be found. Specify the appropriate remediation target and try again.`
Emulator: that same refusal is returned for `AWS-StartEC2Instance` — i.e. for every SSM_DOCUMENT target, including AWS-managed ones. The check retries the Put for 30 s and the refusal never clears, so it never reaches its representation assertions on remediation-exception Message handling. Note the emulator's handling of a genuinely absent document is correct: config.remediation-configuration-and-exception-lifecycle asserts this exact code and message for TargetId "sim2real-behav-NoSuchAutomationDocument" and passes. Hypothesis: the target-existence check consults only customer-created SSM documents and the emulator's SSM document catalog contains no AWS-owned (`AWS-*`) Automation documents.
Why: the documented remediation path for Config is an AWS-managed Automation document — AWS-StartEC2Instance, AWS-EnableS3BucketVersioning, AWS-ConfigureS3BucketLogging and friends. With this gate, no remediation configuration can be created at all against the emulator, so auto-remediation, remediation exceptions and every workflow built on them are unreachable, and the client sees a "target not found" error naming a document that does exist.
Fix: the PutRemediationConfigurations handler's remediation-target resolution (and the shared SSM-document lookup it calls) must treat AWS-owned Automation documents as existing — either by seeding the SSM document catalog with the AWS-* Automation documents Config remediation uses, or by accepting an `AWS-` prefixed SSM_DOCUMENT TargetId whose version resolves. The refusal must remain for a TargetId that resolves to nothing, with the message template unchanged.

=== Cause 3 of 4: an S3 PutBucketTagging that merely re-sends CloudFormation's unchanged aws: system tags is refused InvalidTag, blocking the CFN-deployed recorder check ===
Failing check: config.deploy-configuration-recorder-cloudformation-actually-records
AWS: S3 PutBucketTagging replaces the whole tag set, and S3 refuses only the ADD or UPDATE of a key in the reserved `aws:` namespace — HTTP 400, error code `InvalidTag`, fixed message `System tags cannot be added/updated by requester` (no variable parts). Smithy: PutBucketTagging (PUT /{Bucket}?tagging, Tagging @required/@httpPayload) models no errors, so this rule is documented rather than modeled. Consequently the correct, recorded way to add one user tag to a bucket CloudFormation created is GetBucketTagging, append the user tag, and PUT the whole set back INCLUDING the unchanged aws:cloudformation:stack-name / stack-id / logical-id entries; that request adds and updates nothing in the reserved namespace, so AWS answers success and the merged set reads back.
Emulator: that call is refused HTTP 400 `InvalidTag` "System tags cannot be added/updated by requester" — the validator rejects the mere PRESENCE of an aws:-prefixed key in the submitted TagSet instead of diffing it against the stored tag set. The check therefore never gets to make its recordable change, and the CloudFormation-deployed recorder's GetResourceConfigHistory assertions are never exercised.
Why: no user tag can be changed on any bucket CloudFormation created, because every correct read-modify-write carries CloudFormation's own system tags back. This blocks tagging-governance, drift and — as here — any Config recording flow whose recordable change is a bucket tag edit.
Related but NOT covering: open issue 20261110T153718Z-cloudformation-s3-putbuckettagging-refuses-a-tag-set-that.md describes the same emulator predicate from the cloudformation__s3 suite; it carries only the replay key of cloudformation__s3.cfn-s3-bucket-tags-drift-reports-expected-and-actual, not this config check, so this check is not covered by it and is recorded here.
Fix: the PutBucketTagging handler's tag validation — compare submitted `aws:`-prefixed entries against the bucket's currently stored tag set and refuse only entries that are new or whose value differs; an identical re-send must pass through. Any sibling validator sharing that system-tag check needs the same rule.

=== Cause 4 of 4: config.resource-config-unicode-and-binary-blob-round-trip never converges and times out ===
Failing check: config.resource-config-unicode-and-binary-blob-round-trip
AWS: every call this check makes answers promptly: a customer-managed recorder can be put, given a delivery channel and started (Config validates role/delivery synchronously, so each retry loop clears on the first or second attempt); PutResourceConfig of an unregistered type is refused immediately with HTTP 400 `ValidationException` and message `Cannot find registered Resource Type with name ` + <ResourceType from the request> + `. Please register the Resource Type with Cloudformation and try again.` (Smithy: PutResourceConfig models ValidationException, InsufficientPermissionsException, MaxActiveResourcesExceededException, NoRunningConfigurationRecorderException — all client/400); GetResourceConfigHistory of a never-ingested resource is refused `ResourceNotDiscoveredException` with `Resource <resourceId> of resourceType:AWS::S3::Bucket is unknown or has not been discovered`; BatchGetResourceConfig returns `baseConfigurationItems: []`.
Emulator: the check does not complete — replay reports `Check timed out after 60.0s`, so no assertion in it is ever reached. The only unbounded work in the check is its recorder-provisioning retry loops, each of which re-polls a refusal it expects to clear: PutConfigurationRecorder past `InvalidRoleException`, PutDeliveryChannel past `InsufficientDeliveryPolicyException`, StartConfigurationRecorder past `NoAvailableDeliveryChannelException`. Hypothesis: one of those three refusals is returned permanently by the emulator (so the poll can never converge) or one of the calls on this path does not answer at all; the suite's prior recording of this check failed on its first ASSERTION instead, so the hang is a change in behavior on this path and not the known PutResourceConfig classification bug.
Why: a client that provisions a recorder the way AWS documents (put recorder, put delivery channel, start) either hangs or retries forever against the emulator, and no caller of the resource-config plane behind that setup can run at all. A timeout is also indistinguishable to a client from a hung service.
Related but NOT covering: open issue 20261030T013728Z-config-2-net-new-behavioral-root-causes-putresourceconfig.md lists this check among its trace ids, but its replay key for it is the `assert code == "ValidationException"` assertion — an assertion this run never reaches, which is why the report did not mark this failure as owned. The non-convergence/timeout is recorded here; fixing the PutResourceConfig error class alone will not make this check pass.
Fix: the configuration-recorder/delivery-channel provisioning path — the PutConfigurationRecorder role gate, the PutDeliveryChannel delivery-policy gate and the StartConfigurationRecorder delivery-channel gate in the Config handlers and their shared state: each must accept a correctly provisioned role/bucket/channel rather than refusing permanently, and each operation must answer within a normal request budget.

Traces: config.remediation-configuration-and-exception-lifecycle (cause 1); config.remediation-exceptions-empty-vs-absent-message-preserved (cause 2); config.deploy-configuration-recorder-cloudformation-actually-records (cause 3); config.resource-config-unicode-and-binary-blob-round-trip (cause 4).

## Evidence
Cause 1 — config.remediation-configuration-and-exception-lifecycle, verdict [mismatch].
The check creates a rule with no remediation configuration and calls cfg.describe_remediation_exceptions(ConfigRuleName=<rule>).
Expected (AWS): HTTP 400, Error.Code="InvalidParameterValueException", Error.Message="No Remediation Configuration found for exception" (fixed text, no variable parts).
Actual (emulator): HTTP 200, body {"RemediationExceptions": []}.
Replay output:
  File "check_config_remediation_configuration_and_exception_lifecycle.py", line 303, in run
    ec, em = _error(lambda: cfg.describe_remediation_exceptions(ConfigRuleName=rule_name))
  AssertionError: expected the call to be rejected, but it succeeded and returned {'RemediationExceptions': [], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
Everything before this point in the check matched, including the PutRemediationExceptions refusal "No RemediationConfiguration found for <rule>" and the DeleteRemediationConfiguration refusal "No RemediationConfiguration for rule <rule> exists" — so the stored precondition exists and only the read path ignores it. (The content-length/date headers above are per-run values, not requirements.)

Cause 2 — config.remediation-exceptions-empty-vs-absent-message-preserved, verdict [mismatch].
The check puts RemediationConfigurations=[{ConfigRuleName:<rule>, TargetType:"SSM_DOCUMENT", TargetId:"AWS-StartEC2Instance", TargetVersion:"1", Parameters:{AutomationAssumeRole:{StaticValue:{Values:[<role arn>]}}, InstanceId:{StaticValue:{Values:["i-00000000000000000"]}}}}], retrying for 30 s.
Expected (AWS): HTTP 200, {"FailedBatches": []}.
Actual (emulator): HTTP 400, Error.Code="InvalidParameterValueException", Error.Message="The remediation target for <ConfigRuleName> could not be found. Specify the appropriate remediation target and try again." — raised on every attempt until the retry budget expires.
Replay output:
  File "check_config_remediation_exceptions_empty_vs_absent_message_preserved.py", line 130, in run
    raise AssertionError(m)
  AssertionError: The remediation target for sim2real-remexc-rule-9f21598fe271 could not be found. Specify the appropriate remediation target and try again.
Message anatomy: "The remediation target for " / " could not be found. Specify the appropriate remediation target and try again." is AWS's fixed text for the missing-target class; the rule name is the one variable part (the id in the line above is a per-run value). The wording is correct text raised on the wrong request — the defect is the target-existence predicate.

Cause 3 — config.deploy-configuration-recorder-cloudformation-actually-records, verdict [error].
The check deploys a stack (bucket + bucket policy + role + recorder + delivery channel), starts the recorder, then makes one recordable change:
  existing_tags = s3.get_bucket_tagging(Bucket=bucket)["TagSet"]
  s3.put_bucket_tagging(Bucket=bucket, Tagging={"TagSet": existing_tags + [{"Key": "sim2real-marker", "Value": <marker>}]})
Expected (AWS): success; GetBucketTagging then reports sim2real-marker plus the unchanged aws:cloudformation:* tags, and the tag edit shows up in GetResourceConfigHistory for AWS::S3::Bucket.
Actual (emulator):
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.exceptions.ClientError: An error occurred (InvalidTag) when calling the PutBucketTagging operation: System tags cannot be added/updated by requester
Message anatomy: "System tags cannot be added/updated by requester" is AWS's fixed InvalidTag text for the reserved-namespace class — correct text on a request AWS accepts. No variable parts; the defect is the predicate.

Cause 4 — config.resource-config-unicode-and-binary-blob-round-trip, verdict [mismatch].
Replay output: `Error: Check timed out after 60.0s` — no assertion in the check body was reached.
Expected (AWS): the setup sequence (CreateRole/AttachRolePolicy/PutRolePolicy, CreateBucket, PutConfigurationRecorder, PutDeliveryChannel, StartConfigurationRecorder) converges within seconds, then PutResourceConfig(ResourceType="Sim2RealTest::Widget::Resource", SchemaVersionId="1-0-0", ResourceId=<id>, Configuration='{"greeting": "héllo 日本語"}') is refused 400 ValidationException "Cannot find registered Resource Type with name Sim2RealTest::Widget::Resource. Please register the Resource Type with Cloudformation and try again.", GetResourceConfigHistory of the never-ingested id is refused ResourceNotDiscoveredException "Resource <id> of resourceType:AWS::S3::Bucket is unknown or has not been discovered", and BatchGetResourceConfig returns baseConfigurationItems: [].
Actual (emulator): the check never finishes inside its 60 s budget. Its only unbounded loops are the three provisioning polls that retry past InvalidRoleException (PutConfigurationRecorder, 60 s), InsufficientDeliveryPolicyException (PutDeliveryChannel, 90 s) and NoAvailableDeliveryChannelException (StartConfigurationRecorder, 60 s), so one of those refusals is being returned permanently or one call on that path does not answer.

## Steps to Reproduce
Cause 1 (DescribeRemediationExceptions never refuses):
1. PutConfigRule a rule (e.g. Source AWS / S3_BUCKET_VERSIONING_ENABLED) and wait for ConfigRuleState ACTIVE. Do NOT put a remediation configuration.
2. Call DescribeRemediationExceptions(ConfigRuleName=<rule>).
3. AWS answers 400 InvalidParameterValueException "No Remediation Configuration found for exception"; the emulator answers 200 {"RemediationExceptions": []}.
4. Re-replay config.remediation-configuration-and-exception-lifecycle.

Cause 2 (AWS-owned SSM document rejected as a missing target):
1. PutConfigRule a rule as above and create an IAM role assumable by ssm.amazonaws.com for AutomationAssumeRole.
2. Call PutRemediationConfigurations with one entry: ConfigRuleName=<rule>, TargetType="SSM_DOCUMENT", TargetId="AWS-StartEC2Instance", TargetVersion="1", Parameters={"AutomationAssumeRole":{"StaticValue":{"Values":[<role arn>]}},"InstanceId":{"StaticValue":{"Values":["i-00000000000000000"]}}}.
3. AWS answers 200 {"FailedBatches": []}; the emulator answers 400 InvalidParameterValueException "The remediation target for <rule> could not be found. Specify the appropriate remediation target and try again."
4. Re-replay config.remediation-exceptions-empty-vs-absent-message-preserved.

Cause 3 (re-sending unchanged aws: system tags):
1. CreateStack with an AWS::S3::Bucket; wait CREATE_COMPLETE and read the bucket's PhysicalResourceId.
2. GetBucketTagging on that bucket (observe the aws:cloudformation:* tags).
3. PutBucketTagging with that exact TagSet plus one new user tag {"Key":"sim2real-marker","Value":<any>}.
4. AWS succeeds; the emulator answers 400 InvalidTag "System tags cannot be added/updated by requester".
5. Re-replay config.deploy-configuration-recorder-cloudformation-actually-records.

Cause 4 (provisioning path never converges):
1. Create an IAM role trusted by config.amazonaws.com with AWS_ConfigRole plus an inline s3:PutObject/PutObjectAcl/GetBucketAcl grant, and an S3 delivery bucket.
2. Call PutConfigurationRecorder (role above, recordingGroup allSupported=true), then PutDeliveryChannel (that bucket), then StartConfigurationRecorder, retrying each past InvalidRoleException / InsufficientDeliveryPolicyException / NoAvailableDeliveryChannelException respectively.
3. Observe which of the three refusals never clears (or which call never answers) — against AWS all three converge within seconds.
4. Re-replay config.resource-config-unicode-and-binary-blob-round-trip and confirm it finishes inside its 60 s budget and reaches its PutResourceConfig / GetResourceConfigHistory / BatchGetResourceConfig assertions.
