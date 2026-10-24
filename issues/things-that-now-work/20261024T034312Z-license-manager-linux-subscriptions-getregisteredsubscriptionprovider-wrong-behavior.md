Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ba63228bbc1032132c1fbdbb0b54a8c844ce19f7

# A registered provider can never reach SubscriptionProviderStatus ACTIVE; it always settles INVALID with emulator-authored text

_Filed: 2026-10-24T03:43:12Z_
_Operation: GetRegisteredSubscriptionProvider | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ba63228bbc1032132c1fbdbb0b54a8c844ce19f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a provider registered with `RegisterSubscriptionProvider` reports `SubscriptionProviderStatus: PENDING` while the token is validated and then `ACTIVE` once the registration is usable — that is the status a client script/console polls for before expecting third-party subscription data, and `LastSuccessfulDataRetrievalTime` is then reported. `SubscriptionProviderStatusMessage` is service text (and absent for a healthy registration).

Actual: every registration created through the API transitions to `INVALID` 500 ms after it was created (`state.rs` `settle()`), carrying a long emulator-authored `SubscriptionProviderStatusMessage` (`TOKEN_NOT_VALIDATED_MESSAGE`: "The subscription provider access token was not validated: this emulator holds no subscription-provider trust material ...") — a string no AWS response ever contains. `ACTIVE` is unreachable over the wire (the crate's own test harness has to seed an `ACTIVE` record directly into state to exercise the read side), and `LastSuccessfulDataRetrievalTime` is never populated. A client that registers a provider and waits for `ACTIVE` never finishes, and one that branches on `INVALID` takes a failure path for a registration AWS would consider healthy.

## Evidence

Spec: `SubscriptionProviderStatus` = ACTIVE | INVALID | PENDING; `GetRegisteredSubscriptionProvider$SubscriptionProviderStatus` is "The status of the Linux subscription provider access token from the last successful subscription data request" and `LastSuccessfulDataRetrievalTime` is "The timestamp from the last time License Manager retrieved subscription details from your registered third-party Linux subscription provider" — both presuppose a registration that becomes usable. Handler/state: `src/state.rs:174-184` (`settle()` sets `PROVIDER_STATUS_INVALID` + `TOKEN_NOT_VALIDATED_MESSAGE` unconditionally after `TOKEN_VALIDATION_DELAY`); `src/ops/get_registered_subscription_provider.rs:177` settles before reporting. Tests: `tests/all/subscription_provider_registration.rs:268-308` asserts the emulator's INVALID outcome and that ACTIVE is never reported, and `tests/all/harness.rs:273-300` seeds ACTIVE records out of band — i.e. the divergence is locked in by tests rather than covered by them.

## Steps to Reproduce

1. POST /subscription/RegisterSubscriptionProvider with {"SubscriptionProviderSource":"RedHat","SecretArn":"arn:aws:secretsmanager:us-east-1:123456789012:secret:redhat-token"} — note the returned SubscriptionProviderArn and PENDING status
2. Wait ~1 s and POST /subscription/GetRegisteredSubscriptionProvider with that ARN
3. Observe SubscriptionProviderStatus = INVALID plus a long emulator-specific SubscriptionProviderStatusMessage, and no LastSuccessfulDataRetrievalTime — instead of ACTIVE with no status message
4. POST /subscription/ListRegisteredSubscriptionProviders — the same INVALID status is reported there
