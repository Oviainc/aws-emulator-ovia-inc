Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.config-rule-scope-and-frequency-rejected config.op-put-config-rule-unknown-managed-source config.delivery-channel-invalid-arn-fields-rejected config.op-delivery-channel-unvalidated-bucket
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.config-rule-scope-and-frequency-rejected|_error#12|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: config.op-put-config-rule-unknown-managed-source|_error#12|AssertionError|raise AssertionError(
Issue-Key: config.delivery-channel-invalid-arn-fields-rejected|_error#16|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: config.op-delivery-channel-unvalidated-bucket|_error#21|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Commit: 699f6ed7a7b73d87a9db3c950b6f69a120aa392c

# Config PutConfigRule and PutDeliveryChannel are overly permissive: inconsistent rule bodies, unknown managed SourceIdentifier, malformed SNS/KMS ARNs and channel names, and a nonexistent S3 bucket are all accepted with HTTP 200

> _Note: this issue was identified against the code base at commit `699f6ed7a7b73d87a9db3c950b6f69a120aa392c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutConfigRule

## Description
Four request shapes that real AWS Config refuses with a modeled 400 are accepted by the emulator with HTTP 200 and an empty body, and the bad resource is persisted. FOUR CAUSES must be closed. Related open issue: "Config error paths unenforced: delivery-channel validation and absence answer 200 instead of 400" (20261022T170950Z-config-error-paths-unenforced-delivery-channel-validation-and) names PutDeliveryChannel's unvalidated delivery target as its Cause 1 from the wire corpus; Cause 4 below is the behavioral grounding of that same gap (and pins the error class the model reserves for it), while Causes 1-3 are not covered by any open issue.

Cause 1 of 4: PutConfigRule accepts internally inconsistent ConfigRule bodies.
AWS: PutConfigRule rejects each of three individually well-typed but inconsistent bodies with InvalidParameterValueException (client error, httpError 400 — Smithy: PutConfigRule.errors), and the message names the offending field. Measured on real AWS by the check: (a) a Scope naming ComplianceResourceId without exactly one ComplianceResourceType — "A single resourceType should be provided when resourceId is provided in scope"; (b) a MaximumExecutionFrequency on a change-triggered managed rule — "A maximum execution frequency for this rule is not allowed because only a change in resources triggers this managed rule."; (c) SourceDetails supplied for an Owner=AWS managed rule. In all three cases no rule is created.
Emulator: answers HTTP 200 with an empty body (ResponseMetadata HTTPStatusCode 200, content-length 2) and stores the rule.
Why: a client gets a rule that real AWS would never have created — e.g. a change-triggered managed rule carrying a periodic frequency — so the rule's trigger semantics silently differ between environments, and misconfiguration is never surfaced at the API.
Fix: PutConfigRule's handler validation — cross-field checks on Scope/MaximumExecutionFrequency/Source before persisting, raising 400 InvalidParameterValueException (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_config_rule.rs).
Traces: config.config-rule-scope-and-frequency-rejected.

Cause 2 of 4: PutConfigRule accepts an Owner=AWS SourceIdentifier that is not a real AWS managed rule.
AWS: rejects with 400 InvalidParameterValueException (Smithy: PutConfigRule.errors); message built from the request names the offending identifier — "The sourceIdentifier THIS_MANAGED_RULE_DOES_NOT_EXIST is invalid. Please refer to the documentation for a list of valid sourceIdentifiers that can be used when AWS is the Owner." A real managed identifier in the same call shape is accepted (positive control), so the refusal is attributable to the unknown identifier.
Emulator: answers HTTP 200 and stores the rule with the arbitrary identifier.
Why: typos in managed-rule identifiers are never caught; the rule exists but can never evaluate, so a client believes a control is in force when nothing will ever assess it.
Fix: PutConfigRule's handler validation — validate Source.SourceIdentifier against the known AWS managed-rule set when Source.Owner is AWS (.../src/ops/put_config_rule.rs).
Traces: config.op-put-config-rule-unknown-managed-source.

Cause 3 of 4: PutDeliveryChannel accepts malformed snsTopicARN, s3KmsKeyArn and channel name.
AWS: PutDeliveryChannel models a distinct 400 client error per field (Smithy: PutDeliveryChannel.errors): InvalidSNSTopicARNException, InvalidS3KmsKeyArnException and InvalidDeliveryChannelNameException. With exactly one field broken at a time and everything else proven acceptable by a control leg, AWS refuses each (recorded messages, examples: "The sns topic arn 'not-an-arn-at-all' is not valid."; "The arn 'arn:aws:kms:::key/' is not a valid kms key or alias arn."; "The delivery channel name 'sim2real behav cfg bad/name' is not valid, only  lower case and upper case alphanumerics, underscores, and hyphens are allowed."). The requirement is the 400 and the per-field error code.
Emulator: answers HTTP 200 with an empty body and persists the channel with the malformed field.
Why: a client that relies on AWS validating the notification target and the channel name gets a channel that cannot notify anyone (or whose name AWS would reject), and the three distinct per-field error classes are unreachable, so error handling that branches on them is dead.
Fix: PutDeliveryChannel's handler validation — validate snsTopicARN, s3KmsKeyArn and name before persisting and raise the matching modeled 400 (.../src/ops/put_delivery_channel.rs).
Traces: config.delivery-channel-invalid-arn-fields-rejected.

Cause 4 of 4: PutDeliveryChannel accepts an s3BucketName that does not exist.
AWS: refuses with the error the model reserves for that case, NoSuchBucketException (client, httpError 400 — Smithy: PutDeliveryChannel.errors); message built from the request: "No such s3 bucket with name '<bucket>'." A refused UPDATE must also leave the previously valid target in place (the check first proves PutDeliveryChannel succeeds against a real, correctly-policied bucket, so the refusal is attributable to the bogus bucket alone).
Emulator: answers HTTP 200 with an empty body and persists the channel pointing at the nonexistent bucket.
Why: Config delivery is reported as configured when it cannot deliver; the misconfiguration is discovered late or never, and every downstream assertion about delivery/snapshots runs against a channel AWS would have refused.
Fix: PutDeliveryChannel's handler validation — resolve the S3 bucket before persisting and raise 400 NoSuchBucketException when it is absent (.../src/ops/put_delivery_channel.rs). See also Cause 1 of issue 20261022T170950Z-config-error-paths-unenforced-delivery-channel-validation-and.
Traces: config.op-delivery-channel-unvalidated-bucket.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from mainline (emulator serving, bootstrap_signal false).

Cause 1 — config.config-rule-scope-and-frequency-rejected:
  AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', ...}, 'RetryAttempts': 0}}
  expected: 400 InvalidParameterValueException naming the offending field, and no rule created.

Cause 2 — config.op-put-config-rule-unknown-managed-source:
  AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ... 'content-length': '2' ...}}
  request: PutConfigRule ConfigRule={'ConfigRuleName': ..., 'Source': {'Owner': 'AWS', 'SourceIdentifier': 'THIS_MANAGED_RULE_DOES_NOT_EXIST'}}
  expected: 400 InvalidParameterValueException, message "The sourceIdentifier THIS_MANAGED_RULE_DOES_NOT_EXIST is invalid. ...". The positive control with a real managed SourceIdentifier succeeded, so the emulator is not refusing every managed source.

Cause 3 — config.delivery-channel-invalid-arn-fields-rejected:
  AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ... 'content-length': '2' ...}}
  legs, one broken field each against an otherwise-valid request: snsTopicARN='not-an-arn-at-all' (expected InvalidSNSTopicARNException), s3KmsKeyArn='arn:aws:kms:::key/' (expected InvalidS3KmsKeyArnException), name='sim2real behav cfg bad/name' (expected InvalidDeliveryChannelNameException).

Cause 4 — config.op-delivery-channel-unvalidated-bucket:
  AssertionError: expected the call to be rejected, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ... 'content-length': '2' ...}}
  expected: 400 NoSuchBucketException, message "No such s3 bucket with name '<bogus bucket>'.", and the existing valid target left untouched.

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): PutConfigRule models InvalidParameterValueException (client, 400); PutDeliveryChannel models NoSuchBucketException, InvalidSNSTopicARNException, InvalidS3KmsKeyArnException, InvalidDeliveryChannelNameException and InsufficientDeliveryPolicyException (all client, 400 defaulted).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay the behavioral suite simparity-behavioral-aws-v1 and re-replay the check ids in trace_ids.
3. Cause 1: create a configuration recorder, then PutConfigRule with (a) Scope={'ComplianceResourceId': ...} and no single ComplianceResourceType, (b) Source={'Owner':'AWS','SourceIdentifier':'S3_BUCKET_VERSIONING_ENABLED'} plus MaximumExecutionFrequency='TwentyFour_Hours', (c) Owner=AWS plus SourceDetails — each returns 200 instead of 400 InvalidParameterValueException.
4. Cause 2: PutConfigRule with Source={'Owner':'AWS','SourceIdentifier':'THIS_MANAGED_RULE_DOES_NOT_EXIST'} — returns 200; DescribeConfigRules then lists the bogus rule.
5. Cause 3: create a bucket with a valid Config delivery policy, then PutDeliveryChannel three times breaking exactly one of snsTopicARN / s3KmsKeyArn / name — each returns 200.
6. Cause 4: PutDeliveryChannel with s3BucketName set to a bucket that has never existed — returns 200 and the channel is persisted pointing at it.
