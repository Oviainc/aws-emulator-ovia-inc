Issue-Source: trace-sweep-divergence
Issue-Trace: sns-error-unsubscribe-invalid-subscription-arn
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sns-error-unsubscribe-invalid-subscription-arn sns-create-topic-idempotent-and-delete sns-topic-lifecycle-crud sns-error-create-topic-fifo-name-mismatch sns-fifo-topic-ordered-publish sns-error-subscribe-invalid-protocol sns-subscription-pending-and-error-paths sns-publish-single-and-batch sns-sms-sandbox-number-verification-flow
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns|Unsubscribe|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: sns|Unsubscribe|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: sns|Unsubscribe|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: sns|Unsubscribe|4xx->4xx|status
Issue-Key: sns|GetTopicAttributes|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: sns|GetTopicAttributes|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: sns|CreateTopic|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: sns|CreateTopic|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: sns|Subscribe|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: sns|Subscribe|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: sns|Publish|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: sns|Publish|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: sns|CreateSMSSandboxPhoneNumber|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: sns|CreateSMSSandboxPhoneNumber|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: sns|ListSubscriptionsByTopic|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: sns|ListSubscriptionsByTopic|4xx->4xx|$.ErrorResponse.RequestId
Issue-Commit: 37e4b54b7f2f231778aa0090f22742466dbbad1b

# SNS query-protocol error responses omit RequestId and misclassify SubscriptionArn validation

> _Note: this issue was identified against the code base at commit `37e4b54b7f2f231778aa0090f22742466dbbad1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Unsubscribe

## Description
SNS speaks awsQuery (Smithy service_info: protocols ['awsQuery'], xmlNamespace http://sns.amazonaws.com/doc/2010-03-31/), whose error document is <ErrorResponse><Error>...</Error><RequestId>...</RequestId></ErrorResponse>. Every SNS error the corpus exercises comes back without the RequestId element, and one rejection is additionally classified as the wrong error with the wrong status. This issue bundles 8 causes, all in the SNS error path.

Cause 1 of 8 — Unsubscribe does not validate the SubscriptionArn's shape, so a malformed ARN is reported as a missing subscription.
AWS: Smithy Unsubscribe errors include InvalidParameterException (awsQueryError code 'InvalidParameter', httpResponseCode 400) and NotFoundException ('NotFound', 404). A SubscriptionArn that is not an ARN at all fails argument validation before lookup, so AWS answers 400 with Error.Code 'InvalidParameter' and Error.Type 'Sender' (recorded message, as an example of the rule "name the member and the constraint it failed": "Invalid parameter: SubscriptionArn Reason: An ARN must have at least 6 elements, not 1"). RequestId is present.
Emulator: 404 with Error.Code 'NotFound' (message example: "Subscription does not exist: not-a-valid-arn-..."), and no RequestId element.
Why: a client distinguishing "you sent me garbage" (4xx non-retryable input bug) from "it is gone" (idempotent delete already done) takes the wrong branch — an SDK error-shape switch on NotFoundException/InvalidParameterException dispatches to the wrong handler, and delete-idempotency logic swallows a real input defect.
Fix: Unsubscribe input validation, which must run an ARN shape check before the state lookup in its handler.
Traces: sns-error-unsubscribe-invalid-subscription-arn#task#1.

Cause 2 of 8 — CreateSMSSandboxPhoneNumber's validation message echoes the rejected @sensitive value.
AWS: Smithy CreateSMSSandboxPhoneNumber.PhoneNumber targets PhoneNumberString with pattern ^(\+[0-9]{8,}|[0-9]{0,9})$, length 0..20, and is marked @sensitive; errors include InvalidParameterException ('InvalidParameter', 400). The recorded rejection is 400 and its message states the constraint without reproducing the value ("1 validation error detected: Value at 'phoneNumber' failed to satisfy constraint: Member must satisfy regular expression pattern: ...") — the rule being that a sensitive member's value is not echoed. RequestId is present.
Emulator: same 400, but the message interpolates the rejected value ("Value 'not-a-phone-number' at 'phoneNumber' ...") and the response carries no RequestId.
Why: the emulator leaks a value the model marks sensitive into an error string that is typically logged, and clients/log scrubbers calibrated on AWS's redacted sentence do not expect it.
Fix: the validation-error message builder for @sensitive members, plus the awsQuery error serializer (RequestId).
Traces: sns-sms-sandbox-number-verification-flow#task#6.

Cause 3 of 8 — GetTopicAttributes NotFound error has no RequestId.
AWS: Smithy GetTopicAttributes errors include NotFoundException ('NotFound', 404). Recorded response: 404, Error.Code NotFound, with a RequestId element. The recorded message ("Topic does not exist") is an example; the requirement is the 404/NotFound pair plus the RequestId element.
Emulator: 404/NotFound, but $.ErrorResponse.RequestId is absent entirely.
Why: clients and proxies that log or correlate on RequestId have nothing to record, and strict awsQuery parsers that require the element fail to deserialize the error.
Fix: the awsQuery error serializer shared by all SNS operations.
Traces: sns-create-topic-idempotent-and-delete#task#4 (cluster also covers sns-topic-lifecycle-crud#task#7).

Cause 4 of 8 — CreateTopic FIFO-name rejection has no RequestId.
AWS: Smithy CreateTopic errors include InvalidParameterException ('InvalidParameter', 400); recorded 400 response carries Error.Code InvalidParameter and a RequestId. Recorded message "Invalid parameter: Topic Name" is an example.
Emulator: 400/InvalidParameter with no RequestId element.
Why: same as cause 3 — no correlation id on a rejection, and a missing required element of the error document.
Fix: the awsQuery error serializer.
Traces: sns-error-create-topic-fifo-name-mismatch#task#1 (cluster also covers sns-fifo-topic-ordered-publish#task#6).

Cause 5 of 8 — Subscribe invalid-protocol rejection has no RequestId.
AWS: Smithy Subscribe errors include InvalidParameterException ('InvalidParameter', 400); recorded 400 response carries a RequestId. Recorded message ("Invalid parameter: Amazon SNS does not support this protocol string: carrierpigeon") is an example.
Emulator: 400/InvalidParameter with no RequestId element.
Fix: the awsQuery error serializer.
Traces: sns-error-subscribe-invalid-protocol#task#2 (cluster also covers sns-subscription-pending-and-error-paths#task#11).

Cause 6 of 8 — Publish FIFO MessageGroupId rejection has no RequestId.
AWS: Smithy Publish errors include InvalidParameterException ('InvalidParameter', 400); publishing to a FIFO topic without MessageGroupId is rejected 400 and the response carries a RequestId. Recorded message "Invalid parameter: The MessageGroupId parameter is required for FIFO topics" is an example.
Emulator: 400/InvalidParameter with no RequestId element.
Fix: the awsQuery error serializer.
Traces: sns-fifo-topic-ordered-publish#task#5.

Cause 7 of 8 — Publish to a deleted topic has no RequestId.
AWS: Smithy Publish errors include NotFoundException ('NotFound', 404); recorded 404 response carries a RequestId. Recorded message "Topic does not exist" is an example.
Emulator: 404/NotFound with no RequestId element.
Fix: the awsQuery error serializer.
Traces: sns-publish-single-and-batch#task#10.

Cause 8 of 8 — ListSubscriptionsByTopic on an unknown topic has no RequestId.
AWS: Smithy ListSubscriptionsByTopic errors include NotFoundException ('NotFound', 404); recorded 404 response carries a RequestId. Recorded message "Topic does not exist" is an example.
Emulator: 404/NotFound with no RequestId element.
Fix: the awsQuery error serializer.
Traces: sns-subscription-pending-and-error-paths#task#12.

## Evidence
Cause 1 (sns-error-unsubscribe-invalid-subscription-arn#task#1): status expected 400, got 404; $.ErrorResponse.Error.Code expected 'InvalidParameter', got 'NotFound'; message expected 'Invalid parameter: SubscriptionArn Reason: An ARN must have at least 6 elements, not 1', got 'Subscription does not exist: not-a-valid-arn-...'; $.ErrorResponse.RequestId missing.
Cause 2 (sns-sms-sandbox-number-verification-flow#task#6, 400/400): message expected "1 validation error detected: Value at 'phoneNumber' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(\\+[0-9]{8,}|[0-9]{0,9})$", got "1 validation error detected: Value 'not-a-phone-number' at 'phoneNumber' failed to satisfy constraint: ..."; $.ErrorResponse.RequestId missing.
Cause 3 (sns-create-topic-idempotent-and-delete#task#4, 404/404): $.ErrorResponse.RequestId missing (expected present).
Cause 4 (sns-error-create-topic-fifo-name-mismatch#task#1, 400/400): $.ErrorResponse.RequestId missing (expected '31c634eb-6792-5314-9716-57f351826478', got None).
Cause 5 (sns-error-subscribe-invalid-protocol#task#2, 400/400): $.ErrorResponse.RequestId missing (expected 'ed76371a-7381-5857-a85f-540245903486', got None).
Cause 6 (sns-fifo-topic-ordered-publish#task#5, 400/400): $.ErrorResponse.RequestId missing.
Cause 7 (sns-publish-single-and-batch#task#10, 404/404): $.ErrorResponse.RequestId missing (expected '2cd572fa-f298-5588-ae81-1017418633fc', got None).
Cause 8 (sns-subscription-pending-and-error-paths#task#12, 404/404): $.ErrorResponse.RequestId missing (expected '6cfe9fd5-4e86-5e25-a566-fd38fb55ba0f', got None).
The recorded RequestId values themselves are environment-specific examples; the requirement is that the element is present (and, for cause 1, the 400/InvalidParameter classification).

## Steps to Reproduce
1. Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator.
2. Cause 1: fixture sns-error-unsubscribe-invalid-subscription-arn, step task#1 — call Unsubscribe with SubscriptionArn='not-a-valid-arn-...' and inspect status and ErrorResponse.Error.Code.
3. Cause 2: fixture sns-sms-sandbox-number-verification-flow, step task#6 — call CreateSMSSandboxPhoneNumber with a phone number violating the modeled pattern and inspect the message for the echoed value.
4. Causes 3-8: any SNS error path (GetTopicAttributes/Publish/ListSubscriptionsByTopic on a missing topic, CreateTopic with a non-.fifo FIFO name, Subscribe with an unsupported protocol, Publish to a FIFO topic without MessageGroupId) — the ErrorResponse body never contains a RequestId element.
