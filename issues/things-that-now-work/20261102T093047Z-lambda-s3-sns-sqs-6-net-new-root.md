Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__sns__sqs.refarch-s3ev-queue-configuration-round-trips-capitalised-filter-rules lambda__s3__sns__sqs.refarch-s3ev-cli-installed-notification-matches-sdk-round-trip lambda__s3__sns__sqs.refarch-s3ev-put-notification-rejected-when-queue-policy-omits-s3 lambda__s3__sns__sqs.refarch-s3ev-put-notification-rejects-ambiguous-overlapping-rules lambda__s3__sns__sqs.refarch-s3ev-put-notification-rejects-second-suffix-rule lambda__s3__sns__sqs.refarch-s3ev-put-notification-rejects-unsupported-event-type lambda__s3__sns__sqs.refarch-s3ev-e2e-diagnose-s3-event-notification-break-lifecycle
Issue-Key: lambda__s3__sns__sqs.refarch-s3ev-queue-configuration-round-trips-capitalised-filter-rules|run#13|AssertionError|assert [r["Name"] for r in rules] == ["Prefix", "Suffix"], (
Issue-Key: lambda__s3__sns__sqs.refarch-s3ev-cli-installed-notification-matches-sdk-round-trip|run#19|AssertionError|assert entry["Filter"]["Key"]["FilterRules"] == [{"Name": "Suffix", "Value": ".csv"}], (
Issue-Key: lambda__s3__sns__sqs.refarch-s3ev-put-notification-rejected-when-queue-policy-omits-s3|_error#4|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, which is the whole claim")
Issue-Key: lambda__s3__sns__sqs.refarch-s3ev-put-notification-rejects-ambiguous-overlapping-rules|_client_error#5|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, which is the whole claim")
Issue-Key: lambda__s3__sns__sqs.refarch-s3ev-put-notification-rejects-second-suffix-rule|_error#4|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, which is the whole claim")
Issue-Key: lambda__s3__sns__sqs.refarch-s3ev-put-notification-rejects-unsupported-event-type|run#4|AssertionError|assert (code, message) == ("InvalidArgument", "The event is not supported for notifications"), (
Issue-Key: lambda__s3__sns__sqs.refarch-s3ev-e2e-diagnose-s3-event-notification-break-lifecycle|phase#8|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 977af42ea79b884a07e35b887b5e587a14d941a9

# lambda__s3__sns__sqs: 6 net-new root causes in S3 bucket-notification configuration — filter-rule names are not re-cased, destination-permission/overlap/duplicate-suffix validation is absent, an unsupported event type answers MalformedXML, and a CFN AWS::SNS::TopicPolicy is never applied

> _Note: this issue was identified against the code base at commit `977af42ea79b884a07e35b887b5e587a14d941a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
This issue bundles SIX net-new root causes of service `lambda__s3__sns__sqs`. All 6 must be closed before submitting. None of the seven grounding checks was marked ALREADY FILED or OWNED by the replay report, and there is no open issue for this service token.

=== Cause 1 of 6: a stored FilterRule name is not re-cased, so the configuration does not round-trip ===
AWS: Smithy (com.amazonaws.s3#FilterRuleName) is an enum whose wire values are `prefix` and `suffix`, which is what a caller sends; S3 stores the rule with the CANONICAL capitalised name and GetBucketNotificationConfiguration reports `Prefix` / `Suffix`, preserving the order the caller supplied. Recorded behavior for the re-casing (the enum is the request vocabulary, not the stored projection); the Id, QueueArn and Events members survive untouched and FilterRules stays a list of maps. The same stored document must be reported identically to every client, so the `aws s3api get-bucket-notification-configuration` CLI read and an SDK read of the same bucket agree byte for byte.
Emulator: the names are echoed back exactly as sent — a read reports `[{'Name': 'prefix', ...}, {'Name': 'suffix', ...}]` where AWS reports `['Prefix', 'Suffix']`.
Why: any client that keys on the canonical names (the published documentation's shape, CloudFormation/CDK drift comparison, Terraform's state refresh, and the many tools that match `Name == "Prefix"`) sees a configuration that differs from the one it installed, so a no-op re-apply reads as a change and filters appear unset.
Fix: the PutBucketNotificationConfiguration handler's filter-rule normalisation (store the canonical capitalised name) or the GetBucketNotificationConfiguration serializer that projects it.
Traces: lambda__s3__sns__sqs.refarch-s3ev-queue-configuration-round-trips-capitalised-filter-rules, lambda__s3__sns__sqs.refarch-s3ev-cli-installed-notification-matches-sdk-round-trip.

=== Cause 2 of 6: a destination whose resource policy does not permit S3 to publish is accepted ===
AWS: PutBucketNotificationConfiguration validates every destination before storing (Smithy declares no operation errors for it and models the opt-out header `x-amz-skip-destination-validation`, so validation is the default path; the refusal itself is recorded behavior). A QueueConfiguration naming an SQS queue whose policy does not allow `s3.amazonaws.com` `sqs:SendMessage` from that bucket is refused with error code `InvalidArgument` and the message `Unable to validate the following destination configurations` — fixed text — plus the per-destination detail members `ArgumentName1` = the destination ARN from the request and `ArgumentValue1` = `Permissions on the destination queue do not allow S3 to publish notifications from this bucket` (fixed text; a nonexistent destination reads differently). Nothing is stored: GetBucketNotificationConfiguration still answers an empty document. Adding that one statement makes the identical request succeed.
Emulator: the put returns success and installs the configuration.
Why: the destination permission is the whole wiring of S3 event notifications. A client that forgets the queue policy is told it succeeded and then sees no events, with nothing in any API answer pointing at the missing grant — exactly the misconfiguration this pre-flight exists to surface.
Fix: destination validation in the PutBucketNotificationConfiguration handler — resolve each Queue/Topic/LambdaFunction destination, evaluate its resource policy for the S3 service principal and this bucket, and on failure refuse with the code, message and ArgumentName1/ArgumentValue1 members above, storing nothing (honouring x-amz-skip-destination-validation when set).
Traces: lambda__s3__sns__sqs.refarch-s3ev-put-notification-rejected-when-queue-policy-omits-s3.

=== Cause 3 of 6: two rules whose prefixes overlap for the same event type are accepted ===
AWS: recorded behavior — two QueueConfigurations for the same event type whose prefixes overlap (e.g. `CategoryA/` and `CategoryA/sub/`) make the document ambiguous and the whole put is refused with error code `InvalidArgument` and the message `Configuration is ambiguously defined. Cannot have overlapping suffixes in two rules if the prefixes are overlapping for the same event type.` (entirely fixed text, no values interpolated). The refusal is whole-document: the previously stored configuration survives unchanged. A pair of the same shape with DISJOINT prefixes is accepted as two entries, so the rule is the overlap and not the rule count.
Emulator: the overlapping pair is accepted and stored.
Why: S3 refuses an ambiguous notification set because it cannot decide which rule an object matches; accepting it means a client's events are delivered by whichever rule the emulator happens to pick, and the configuration it validated locally is rejected the first time it reaches a real bucket.
Fix: the PutBucketNotificationConfiguration handler's cross-rule validation — for each event type, reject a document whose prefix/suffix ranges overlap between two configurations, with the message above, before anything is stored.
Traces: lambda__s3__sns__sqs.refarch-s3ev-put-notification-rejects-ambiguous-overlapping-rules.

=== Cause 4 of 6: a second rule of the same kind inside one filter is accepted ===
AWS: recorded behavior — a single `Filter.Key.FilterRules` may carry at most ONE rule of each kind. Two suffix rules are refused with error code `InvalidArgument` and the message `Cannot specify more than one suffix rule in a filter.` (fixed text; the prefix form is the matching sentence for two prefix rules), and nothing is stored — the bucket's notification document stays empty. One prefix plus one suffix in the same filter is legal and is accepted, reading back as `['Prefix', 'Suffix']`.
Emulator: the duplicate-kind filter is accepted and stored.
Why: a template or script that accidentally emits two suffix rules is accepted here and refused in the account, and in the meantime only one of the two rules can possibly be in force — so the filter the client believes it installed is not the one that filters.
Fix: the PutBucketNotificationConfiguration handler's per-filter validation — count rules by kind inside each Filter.Key.FilterRules and refuse a duplicate kind with the message above, storing nothing.
Traces: lambda__s3__sns__sqs.refarch-s3ev-put-notification-rejects-second-suffix-rule.

=== Cause 5 of 6: an unsupported event type is refused with MalformedXML instead of InvalidArgument and the event-specific message ===
AWS: Smithy (com.amazonaws.s3#Event) is a closed enum (s3:ObjectCreated:*, s3:ObjectRemoved:Delete, s3:LifecycleExpiration:*, … ). An Events entry outside that vocabulary is refused with error code `InvalidArgument` and the message `The event is not supported for notifications` — fixed text. S3 uses one generic code for every notification-validation failure, so the MESSAGE is what identifies which rule was broken.
Emulator: the refusal is error code `MalformedXML` with the message `The XML you provided was not well-formed or did not validate against our published schema` — it refuses, but with the wrong code and a message that blames the document's shape rather than the event name.
Why: a client branching on the refusal cannot tell a bad event name from a genuinely malformed body, and a tool that retries or reports "malformed XML" sends the operator looking at serialization instead of at the one unsupported event in its list.
Fix: the PutBucketNotificationConfiguration handler's Events validation — check each entry against the Event enum and refuse with InvalidArgument and the sentence above, instead of letting the unknown value fall through to the generic schema-validation refusal.
Traces: lambda__s3__sns__sqs.refarch-s3ev-put-notification-rejects-unsupported-event-type.

=== Cause 6 of 6: a CloudFormation-deployed SNS topic policy is never applied to the topic ===
AWS: recorded behavior (AWS::SNS::TopicPolicy has no Smithy operation of its own). A stack declaring a topic-policy resource whose document allows `s3.amazonaws.com` `sns:Publish` on the topic, conditioned on the source bucket ARN, makes GetTopicAttributes report THAT document as the topic's Policy once the stack is CREATE_COMPLETE — which is what lets S3 install a TopicConfiguration against the topic at all.
Emulator: after the stack reaches CREATE_COMPLETE the topic's Policy is still the service default — `{"Version":"2008-10-17","Id":"__default_policy_ID","Statement":[{"Sid":"__default_statement_ID","Effect":"Allow","Principal":{"AWS":"*"},"Action":[..."SNS:Publish"],"Resource":"<topic arn>","Condition":{"StringEquals":{"AWS:SourceOwner":"<account>"}}}]}` — with no statement naming s3.amazonaws.com or the bucket, so the deploy phase of the lifecycle check fails immediately.
Why: the deployed least-privilege policy is silently replaced by an allow-any-principal-in-this-account default: the grant the template declared is absent (so any emulator-side S3-to-SNS authorization cannot be exercised) while the effective policy is far broader than the one the client wrote. An IaC user cannot deploy or test an S3→SNS notification pipeline. Related open issue 20261025T025317Z-sns-cloudformation-provisioned-sns-resources-topicinlinepolicy-unknown-a-subscription.md records neighbouring CFN-provisioned SNS gaps but does not carry this check's id.
Fix: the CloudFormation provisioner for AWS::SNS::TopicPolicy — write the rendered policy document through to the topic's Policy attribute (the same path SetTopicAttributes(AttributeName="Policy") takes) on create and on update.
Traces: lambda__s3__sns__sqs.refarch-s3ev-e2e-diagnose-s3-event-notification-break-lifecycle.

## Evidence
Cause 1 — refarch-s3ev-queue-configuration-round-trips-capitalised-filter-rules [mismatch]:
  AssertionError: the stored rule names are ['prefix', 'suffix']; real AWS re-cases the lower-case names the upstream stack sends to ['Prefix', 'Suffix'] and preserves their order
  refarch-s3ev-cli-installed-notification-matches-sdk-round-trip [mismatch]:
  AssertionError: the CLI-installed filter reads back as [{'Name': 'suffix', 'Value': '.csv'}], expected the same re-cased single suffix rule the SDK path produces

Cause 2 — refarch-s3ev-put-notification-rejected-when-queue-policy-omits-s3 [mismatch]:
  AssertionError: the call SUCCEEDED; real AWS rejects it, which is the whole claim
  expected: ('InvalidArgument', 'Unable to validate the following destination configurations') with ArgumentName1 == <queue arn> and ArgumentValue1 == 'Permissions on the destination queue do not allow S3 to publish notifications from this bucket', and GetBucketNotificationConfiguration still empty.
  actual: PutBucketNotificationConfiguration succeeded and stored the configuration.

Cause 3 — refarch-s3ev-put-notification-rejects-ambiguous-overlapping-rules [mismatch]:
  AssertionError: the call SUCCEEDED; real AWS rejects it, which is the whole claim
  expected: ('InvalidArgument', 'Configuration is ambiguously defined. Cannot have overlapping suffixes in two rules if the prefixes are overlapping for the same event type.') for two s3:ObjectCreated:* QueueConfigurations with prefixes 'CategoryA/' and 'CategoryA/sub/', with the previously stored solo entry intact.

Cause 4 — refarch-s3ev-put-notification-rejects-second-suffix-rule [mismatch]:
  AssertionError: the call SUCCEEDED; real AWS rejects it, which is the whole claim
  expected: ('InvalidArgument', 'Cannot specify more than one suffix rule in a filter.') for FilterRules [{suffix: '.csv'}, {suffix: '.tsv'}], nothing stored.

Cause 5 — refarch-s3ev-put-notification-rejects-unsupported-event-type [mismatch]:
  AssertionError: an unsupported event type must be refused InvalidArgument 'The event is not supported for notifications'; got ('MalformedXML', 'The XML you provided was not well-formed or did not validate against our published schema')

Cause 6 — refarch-s3ev-e2e-diagnose-s3-event-notification-break-lifecycle [mismatch]:
  AssertionError: [phase 1/4 deploy] the deployed topic policy does not allow s3.amazonaws.com sns:Publish scoped to 'arn:aws:s3:::<bucket>': {'Version': '2008-10-17', 'Id': '__default_policy_ID', 'Statement': [{'Sid': '__default_statement_ID', 'Effect': 'Allow', 'Principal': {'AWS': '*'}, 'Action': ['SNS:GetTopicAttributes', …, 'SNS:Publish'], 'Resource': '<topic arn>', 'Condition': {'StringEquals': {'AWS:SourceOwner': '<account>'}}}]}

## Steps to Reproduce
Boot the emulator and replay the seven check ids in trace_ids from suite simparity-behavioral-aws-v1.

By hand (one bucket plus one queue whose policy allows s3.amazonaws.com sqs:SendMessage for that bucket, except where noted):
1. Cause 1: PutBucketNotificationConfiguration with QueueConfigurations[0].Filter.Key.FilterRules = [{Name:'prefix',Value:'CategoryA/'},{Name:'suffix',Value:'.csv'}], then GetBucketNotificationConfiguration — the names read back 'prefix'/'suffix' instead of 'Prefix'/'Suffix'. Read the same bucket with `aws s3api get-bucket-notification-configuration` and compare: both clients must report the identical stored document.
2. Cause 2: use a queue with NO policy statement for s3.amazonaws.com and put the same configuration — it succeeds instead of being refused InvalidArgument 'Unable to validate the following destination configurations'; then add the statement and observe the identical put succeed (the positive control).
3. Cause 3: install one entry with prefix 'CategoryA/', then put two entries for s3:ObjectCreated:* with prefixes 'CategoryA/' and 'CategoryA/sub/' — accepted instead of refused; a disjoint pair ('CategoryA/', 'CategoryC/') must stay accepted.
4. Cause 4: put one QueueConfiguration whose FilterRules are two suffix rules — accepted instead of refused 'Cannot specify more than one suffix rule in a filter.'; one prefix plus one suffix must stay accepted.
5. Cause 5: put a QueueConfiguration whose Events contains a name outside the Event enum — observe MalformedXML instead of InvalidArgument 'The event is not supported for notifications'.
6. Cause 6: CreateStack with an AWS::SNS::Topic plus an AWS::SNS::TopicPolicy allowing Principal {"Service":"s3.amazonaws.com"} sns:Publish on that topic conditioned on the source bucket ARN; after CREATE_COMPLETE call sns.GetTopicAttributes and inspect Policy — it is the __default_policy_ID document, not the deployed one.
