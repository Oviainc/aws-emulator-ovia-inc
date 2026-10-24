Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ba63228bbc1032132c1fbdbb0b54a8c844ce19f7

# Registration identity is keyed on (source, secret), so one account accumulates many providers for the same source

_Filed: 2026-10-24T03:43:12Z_
_Operation: RegisterSubscriptionProvider | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ba63228bbc1032132c1fbdbb0b54a8c844ce19f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: an account holds ONE registration per supported `SubscriptionProviderSource` (the only modelled value is `RedHat`). Re-registering the same source with a different `SecretArn` therefore addresses the SAME subscription-provider resource (its secret is updated) or is refused — `ListRegisteredSubscriptionProviders` never reports two RedHat registrations for one account.

Actual: `register_provider` resolves an existing registration only when BOTH the source and the `SecretArn` match (`registered_arn`), so a second call with the same `SubscriptionProviderSource` and a different `SecretArn` mints a SECOND `SubscriptionProviderArn` and a second record. An account can accumulate up to the emulator's own ceiling of 100 RedHat registrations, all reported side by side by `ListRegisteredSubscriptionProviders`/`ListTagsForResource`, which is a resource topology the real service does not produce.

## Evidence

Spec: `RegisterSubscriptionProvider` is `@idempotent` and takes `SubscriptionProviderSource` (enum with the single value `RedHat`) — "Register the supported third-party subscription provider for your Bring Your Own License (BYOL) subscription". The crate's own state module concedes the divergence: `src/state.rs:487-489` — "AWS publishes its own quota and it is not this number (the real service registers one provider per supported source); this is the emulator's own ceiling". Handler: `src/ops/register_subscription_provider.rs:185` → `src/state.rs:319-363`/`371-380` (identity = source + secret_arn; `MAX_REGISTERED_PROVIDERS = 100`). Tests: `tests/all/subscription_provider_registration.rs:347` only covers a repeat of the SAME (source, secret) pair, and `:548` deliberately registers 100 providers with distinct secrets — so the multi-registration-per-source behaviour is assumed, not validated against AWS.

## Steps to Reproduce

1. POST /subscription/RegisterSubscriptionProvider with {"SubscriptionProviderSource":"RedHat","SecretArn":"arn:aws:secretsmanager:us-east-1:123456789012:secret:token-a"} — note ARN #1
2. POST /subscription/RegisterSubscriptionProvider with {"SubscriptionProviderSource":"RedHat","SecretArn":"arn:aws:secretsmanager:us-east-1:123456789012:secret:token-b"} — a second, different ARN is minted
3. POST /subscription/ListRegisteredSubscriptionProviders with {} — two RedHat registrations are reported for one account, instead of a single registration now pointing at token-b (or a refusal of the second call)
