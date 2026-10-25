Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.data-protection-policy-deidentify-masks-the-delivered-body sns.data-protection-policy-deny-blocks-a-publish-containing-a-managed-identifier sns.data-protection-policy-unicode-content-preserved sns.put-data-protection-policy-malformed-json-rejected
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.data-protection-policy-deidentify-masks-the-delivered-body|_error#9|AssertionError|raise AssertionError(
Issue-Key: sns.data-protection-policy-deny-blocks-a-publish-containing-a-managed-identifier|_error#9|AssertionError|raise AssertionError(
Issue-Key: sns.data-protection-policy-unicode-content-preserved|run#8|AssertionError|raise AssertionError(
Issue-Key: sns.put-data-protection-policy-malformed-json-rejected|_error#7|AssertionError|raise AssertionError(f"expected a ClientError, call succeeded with: {result!r}")
Issue-Commit: cc62856ae07f45464bafde33e00f4ba8357575c6

# SNS PutDataProtectionPolicy accepts every policy document with HTTP 200 instead of refusing it with InvalidAction (message data protection is withdrawn)

> _Note: this issue was identified against the code base at commit `cc62856ae07f45464bafde33e00f4ba8357575c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDataProtectionPolicy

## Description
This issue carries 1 net-new root cause in SNS's data-protection-policy write path. No open issue covers it: the nearest SNS bundles (20261025T025317Z-sns-is-over-permissive-ten-validation-rules-aws.md, 20261025T025317Z-sns-refuses-four-calls-aws-accepts-deliverypolicy-on.md, 20261025T064350Z-sns-9-behavioral-divergences-unsigned-notification-envelopes-unvalidated.md) name neither PutDataProtectionPolicy nor these replay keys, and a search of the filed set for 'data-protection' returns nothing.

Cause 1 of 1: PutDataProtectionPolicy answers 200 for every payload, where AWS refuses the operation outright.

AWS: Smithy models PutDataProtectionPolicy with input {ResourceArn (topicARN, required), DataProtectionPolicy (attributeValue, required)}, an empty output, and the client errors AuthorizationError (403), InvalidParameter (400), InvalidSecurity (403), NotFound (404), InternalError (500). The refusal under test is NOT one of those modelled errors, so: recorded behavior only. Amazon SNS message data protection was withdrawn for new customers (docs: sns-message-data-protection-availability-change.html), and the recordings for all four checks state the measured AWS response in a non-grandfathered account (us-west-2, 2026-09-17) as an awsQuery error envelope with Error.Code `InvalidAction` and Error.Message `Operation (PutDataProtectionPolicy) is not supported in this region` — the gate is applied before SNS looks at the document, so a well-formed Deidentify/MaskConfig policy, a well-formed Inbound Deny policy, a policy whose Description/Sid carry CJK, Cyrillic and emoji text, and a syntactically truncated JSON string ('{\"Name\": \"broken\", \"Description\": \"unterminated') all get the identical refusal, never a payload-specific InvalidParameter. In that message the entire text is fixed except the parenthesised operation name, which is the operation named by the request; nothing in it comes from the recording's account, topic or clock. The companion read path is NOT gated: GetDataProtectionPolicy succeeds and reports no policy content for a topic that never acquired one.

Emulator: PutDataProtectionPolicy succeeds for every one of those four payloads. The malformed-JSON check captures the raw success: HTTPStatusCode 200, content-type text/xml, no Error element at all. The other three checks fail on the same unexpected success — the harness reports 'expected this call to be refused, but it succeeded' and 'PutDataProtectionPolicy unexpectedly succeeded on an account this check expected to be gated'. So the emulator both (a) exposes a capability AWS no longer grants, and (b) performs no document validation whatsoever behind it, since even unparseable JSON is accepted.

Why: a client that probes for the withdrawal sees the feature as available and will build a masking/deny control it believes is enforced; because the emulator stores whatever it is handed without parsing, a broken or semantically meaningless policy also reads back as installed. Error-handling code written against AWS — which branches on Code='InvalidAction' to fall back to no data protection — is never exercised, and negative tests that assert the malformed-JSON boundary pass vacuously.

Fix: the PutDataProtectionPolicy handler for SNS (services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/put_data_protection_policy.rs) must refuse the call with the awsQuery error envelope Code `InvalidAction`, HTTP 400, and Message `Operation (PutDataProtectionPolicy) is not supported in this region`, storing nothing; GetDataProtectionPolicy must stay ungated and keep reporting empty policy content for such a topic.

Traces: sns.data-protection-policy-deidentify-masks-the-delivered-body, sns.data-protection-policy-deny-blocks-a-publish-containing-a-managed-identifier, sns.data-protection-policy-unicode-content-preserved, sns.put-data-protection-policy-malformed-json-rejected.

## Evidence
Replayed suite, service sns (4 mismatches, one cause).

Expected (recorded AWS, all four payload classes): HTTP 400, Error.Code='InvalidAction', Error.Message='Operation (PutDataProtectionPolicy) is not supported in this region'; topic acquires no policy; GetDataProtectionPolicy returns empty DataProtectionPolicy.

Actual (emulator):
- sns.put-data-protection-policy-malformed-json-rejected: `AssertionError: expected a ClientError, call succeeded with: {'ResponseMetadata': {'RequestId': ..., 'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'text/xml', 'content-length': '275', ...}}}` — raised on the WELL-FORMED positive control at line 67, i.e. before the malformed payload was even attempted; the malformed payload is on the same code path.
- sns.data-protection-policy-deny-blocks-a-publish-containing-a-managed-identifier: `AssertionError: expected this call to be refused, but it succeeded; an over-permissive target must not be able to report a missing error code instead of an unexpected success` at the PutDataProtectionPolicy with a well-formed Inbound Deny on arn:aws:dataprotection::aws:data-identifier/EmailAddress.
- sns.data-protection-policy-deidentify-masks-the-delivered-body: same AssertionError, payload = Inbound Deidentify/MaskConfig over the EmailAddress managed identifier.
- sns.data-protection-policy-unicode-content-preserved: `AssertionError: PutDataProtectionPolicy unexpectedly succeeded on an account this check expected to be gated` — payload Description '中文描述 Привет 😀 café', Sid 'denyEmailInbound-日本語'.

All four checks pass their own positive controls first (CreateTopic + SetTopicAttributes/GetTopicAttributes round-trip, or publish->SQS delivery), so the divergence is isolated to PutDataProtectionPolicy and is not a broken account or an unreachable SNS.

## Steps to Reproduce
1. Boot the emulator and replay the four check ids in trace_ids.
2. Direct repro: sns CreateTopic(Name=<any>); then PutDataProtectionPolicy(ResourceArn=<that topic arn>, DataProtectionPolicy=json.dumps({"Name":"p","Description":"d","Version":"2021-06-01","Statement":[{"Sid":"denyEmailInbound","DataDirection":"Inbound","Principal":["*"],"DataIdentifier":["arn:aws:dataprotection::aws:data-identifier/EmailAddress"],"Operation":{"Deny":{}}}]})).
   AWS: HTTP 400 InvalidAction 'Operation (PutDataProtectionPolicy) is not supported in this region'. Emulator: HTTP 200, empty PutDataProtectionPolicyResponse.
3. Repeat step 2 with DataProtectionPolicy set to the unparseable string '{"Name": "broken", "Description": "unterminated' — AWS returns the same InvalidAction refusal (the gate precedes JSON parsing); the emulator again answers 200, showing no document parsing at all.
4. Then GetDataProtectionPolicy(ResourceArn=<topic arn>) — AWS reports no policy content; confirm the emulator likewise stores nothing once the Put is refused.
