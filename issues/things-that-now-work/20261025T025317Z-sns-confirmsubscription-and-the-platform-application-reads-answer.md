Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.op-confirm-subscription-invalid-token-rejected sns.list-endpoints-by-nonexistent-application-fails sns.platform-application-not-found
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.op-confirm-subscription-invalid-token-rejected|run#4|AssertionError|assert code == "InvalidParameter", f"expected InvalidParameter for a bogus token, got {code}"
Issue-Key: sns.list-endpoints-by-nonexistent-application-fails|run#4|AssertionError|assert code == "NotFound", f"ListEndpointsByPlatformApplication: expected NotFound, got {code}"
Issue-Key: sns.platform-application-not-found|run#4|AssertionError|assert code == "NotFound", (
Issue-Commit: 5f222c49021a8706c85b6c9c85441ae0b0065824

# SNS ConfirmSubscription and the platform-application reads answer NotImplemented instead of the modeled refusals (3 causes)

> _Note: this issue was identified against the code base at commit `5f222c49021a8706c85b6c9c85441ae0b0065824`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ConfirmSubscription

## Description
Three operations reached by these checks are still scaffold stubs that answer the non-AWS error code NotImplemented. Each has filed-issue coverage only for OTHER SNS stubs (CheckIfPhoneNumberIsOptedOut, CreatePlatformApplication, ListSubscriptions, Get/PutDataProtectionPolicy, DeleteEndpoint, ListPlatformApplications, the SMS-sandbox family); the three below are not named by any of them.

Cause 1 of 3: ConfirmSubscription is a stub.
AWS: Smithy ConfirmSubscription(TopicArn, Token, AuthenticateOnUnsubscribe) returns a SubscriptionArn and models InvalidParameterException (400, code InvalidParameter) for a token that is not a valid confirmation token, NotFoundException (404) for an unknown topic.
Emulator: NotImplemented — "operation ... is scaffolded but not implemented in this emulator".
Why: no http/https or email subscription can ever be confirmed, so every non-SQS subscription stays PendingConfirmation forever; a client handling the documented InvalidParameter path instead sees an unknown error code.
Fix: implement the ConfirmSubscription handler (confirm_subscription.rs) against the subscription/token state.
Traces: sns.op-confirm-subscription-invalid-token-rejected.

Cause 2 of 3: ListEndpointsByPlatformApplication is a stub.
AWS: refuses an unknown platform-application ARN with NotFoundException (404, code NotFound).
Emulator: NotImplemented.
Why: mobile-push code paths get an unrecognizable error instead of the documented NotFound.
Fix: implement list_endpoints_by_platform_application.rs (and its state).
Traces: sns.list-endpoints-by-nonexistent-application-fails.

Cause 3 of 3: the platform-application attribute read is a stub.
AWS: a nonexistent platform application is reported with NotFoundException (404, code NotFound).
Emulator: NotImplemented.
Why: same as above — the "does this application exist?" probe cannot be answered.
Fix: implement the platform-application read handler (get_platform_application_attributes.rs).
Traces: sns.platform-application-not-found.

## Evidence
- sns.op-confirm-subscription-invalid-token-rejected: AssertionError: expected InvalidParameter for a bogus token, got NotImplemented
- sns.list-endpoints-by-nonexistent-application-fails: AssertionError: ListEndpointsByPlatformApplication: expected NotFound, got NotImplemented
- sns.platform-application-not-found: AssertionError: expected NotFound for nonexistent platform application, got NotImplemented

## Steps to Reproduce
1. CreateTopic, then ConfirmSubscription(TopicArn=<that topic>, Token="bogus") -> NotImplemented (AWS: 400 InvalidParameter).
2. ListEndpointsByPlatformApplication(PlatformApplicationArn="arn:aws:sns:us-west-2:000000000000:app/GCM/does-not-exist") -> NotImplemented (AWS: 404 NotFound).
3. GetPlatformApplicationAttributes on the same bogus ARN -> NotImplemented (AWS: 404 NotFound).
