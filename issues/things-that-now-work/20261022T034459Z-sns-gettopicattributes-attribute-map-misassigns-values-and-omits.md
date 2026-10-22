Issue-Source: trace-sweep-divergence
Issue-Trace: sns-fifo-topic-ordered-publish
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sns-fifo-topic-ordered-publish sns-kms-topic-attribute-roundtrip sns-publish-single-and-batch sns-topic-access-policy-permission-management sns-topic-lifecycle-crud
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns|GetTopicAttributes|2xx->2xx|$.GetTopicAttributesResponse.GetTopicAttributesResult.Attributes.entry[*].value
Issue-Key: sns|GetTopicAttributes|2xx->2xx|$.GetTopicAttributesResponse.GetTopicAttributesResult.Attributes.entry[*]
Issue-Commit: 37e4b54b7f2f231778aa0090f22742466dbbad1b

# SNS GetTopicAttributes attribute map misassigns values and omits documented topic attributes

> _Note: this issue was identified against the code base at commit `37e4b54b7f2f231778aa0090f22742466dbbad1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTopicAttributes

## Description
GetTopicAttributes answers 200 in every case below, but the attribute map it serializes does not carry the attribute values AWS reports: several entries carry another attribute's value (the account id, a subscription counter, an empty string) and attributes that were explicitly set earlier in the fixture are absent. This issue bundles 4 causes, all in the same topic-attribute map projection.

Cause 1 of 4 — EffectiveDeliveryPolicy is not populated.
AWS: Smithy GetTopicAttributes -> GetTopicAttributesResponse.Attributes (TopicAttributesMap); the member documentation lists EffectiveDeliveryPolicy as "the JSON serialization of the effective delivery policy, taking system defaults into account", so a topic with no explicit DeliveryPolicy still reports the system-default JSON document (recorded responses show the http/defaultHealthyRetryPolicy document). Response status 200.
Emulator: the entry at the EffectiveDeliveryPolicy position carries an unrelated scalar instead of a JSON policy document — the account id ('000000000000'), a counter ('0'), or an empty string.
Why: a client that parses EffectiveDeliveryPolicy as JSON to read retry/backoff settings fails to parse, or silently reads the owner account id as a delivery policy. Because the mispairing shifts values across the map, neighbouring attributes are wrong too (see cause 4).
Fix: the GetTopicAttributes handler/serializer that builds the topic attribute map — the system-default effective delivery policy must be synthesized and each value bound to its own key.
Traces: sns-fifo-topic-ordered-publish#task#2 (cluster of 6: also sns-kms-topic-attribute-roundtrip#task#2, sns-publish-single-and-batch#task#5, sns-topic-access-policy-permission-management#task#2 and #task#7, sns-topic-lifecycle-crud#task#2).

Cause 2 of 4 — KmsMasterKeyId is missing after it has been set.
AWS: the Attributes member documentation lists KmsMasterKeyId ("applies only to server-side-encryption") as an attribute of the map; after a topic is given a KMS key, GetTopicAttributes reports KmsMasterKeyId with that key id (recording: value 'alias/aws/sns'). Status 200.
Emulator: no KmsMasterKeyId entry is present at all — the position where it is expected holds the TopicArn entry instead — and EffectiveDeliveryPolicy is the empty string.
Why: a client that reads back KmsMasterKeyId to confirm encryption-at-rest configuration concludes the topic is unencrypted, and configuration round-trips (set, then get) lose the setting.
Fix: topic state for the SSE attribute plus the GetTopicAttributes projection that must emit it when set.
Traces: sns-kms-topic-attribute-roundtrip#task#4 (cluster also covers #task#6).

Cause 3 of 4 — the Policy attribute does not reflect the topic access policy.
AWS: the Attributes documentation lists Policy as "the JSON serialization of the topic's access control policy"; a freshly created topic reports the default policy document (__default_policy_ID/__default_statement_ID) and, after AddPermission, that document with the added statement. Status 200.
Emulator: the Policy entry's value is '0' — not a JSON document at all — and the statement added by AddPermission is nowhere in the response; the EffectiveDeliveryPolicy entry in the same response carries the topic ARN.
Why: a client that reads Policy to audit or to edit-and-resubmit the topic policy gets a non-JSON scalar, and a permission grant it just made appears not to have taken effect.
Fix: topic state written by AddPermission/RemovePermission/SetTopicAttributes plus the Policy serialization in the GetTopicAttributes projection.
Traces: sns-topic-access-policy-permission-management#task#4.

Cause 4 of 4 — DisplayName does not reflect the value set on the topic.
AWS: the Attributes documentation lists DisplayName ("the human-readable name used in the From field"); after it is set on the topic, GetTopicAttributes reports it (recording: 'probe-display-name'). Status 200.
Emulator: the DisplayName entry carries the owner account id ('000000000000') instead of the configured display name, and EffectiveDeliveryPolicy in the same response is '0'.
Why: a set-then-get round trip on DisplayName silently loses the value, and any client reading DisplayName for presentation shows an account id.
Fix: the SetTopicAttributes write path into topic state plus the key/value binding in the GetTopicAttributes projection.
Traces: sns-topic-lifecycle-crud#task#4.

## Evidence
Cause 1 (sns-fifo-topic-ordered-publish#task#2, expected 200 / actual 200): $.GetTopicAttributesResponse.GetTopicAttributesResult.Attributes.entry[4].value expected '{"http":{"defaultHealthyRetryPolicy":{"minDelayTarget":20,"maxDelayTarget":20,"numRetries":3,"numMaxDelayRetries":0,"numNoDelayRetries":0,"numMinDelayRetries":0,"backoffFunction":"linear"},"disableSubscriptionOverrides":false,"defaultRequestPolicy":{"headerContentType":"text/plain; charset=UTF-8"}}}', got '000000000000'. Same cluster reproduced in 6 fixture steps.
Cause 2 (sns-kms-topic-attribute-roundtrip#task#4, 200/200): entry[3] expected {'key': 'KmsMasterKeyId', 'value': 'alias/aws/sns'}, got {'key': 'TopicArn', 'value': 'arn:aws:sns:...:sim2real-probe-sns-kmstopic-...'}; entry[5].value expected the EffectiveDeliveryPolicy JSON above, got ''.
Cause 3 (sns-topic-access-policy-permission-management#task#4, 200/200): entry[0].value expected the default topic policy JSON including the added 'CrossAccountPublish' statement for SNS:Publish, got '0'; entry[4].value expected the EffectiveDeliveryPolicy JSON, got the topic ARN.
Cause 4 (sns-topic-lifecycle-crud#task#4, 200/200): entry[4].value expected the EffectiveDeliveryPolicy JSON, got '0'; entry[6].value expected 'probe-display-name', got '000000000000'.

## Steps to Reproduce
1. Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator.
2. Cause 1: fixture sns-fifo-topic-ordered-publish, step task#2 — CreateTopic then GetTopicAttributes; read EffectiveDeliveryPolicy from the returned map.
3. Cause 2: fixture sns-kms-topic-attribute-roundtrip, steps task#4 and task#6 — set a KMS master key on the topic, then GetTopicAttributes and look for a KmsMasterKeyId entry.
4. Cause 3: fixture sns-topic-access-policy-permission-management, step task#4 — AddPermission on the topic, then GetTopicAttributes and parse the Policy attribute as JSON.
5. Cause 4: fixture sns-topic-lifecycle-crud, step task#4 — set DisplayName on the topic, then GetTopicAttributes and read DisplayName.
