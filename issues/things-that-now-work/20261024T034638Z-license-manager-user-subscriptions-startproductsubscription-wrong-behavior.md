Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ba63228bbc1032132c1fbdbb0b54a8c844ce19f7

# ProductUserSummary.LicenseExpirationDate is always null, including for RDS SAL subscriptions served by a license server endpoint

_Filed: 2026-10-24T03:46:38Z_
_Operation: StartProductSubscription | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ba63228bbc1032132c1fbdbb0b54a8c844ce19f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartProductSubscription` stores `license_expiration_date: None` for every subscription it creates, with no path that ever sets it, so `ProductUserSummary.LicenseExpirationDate` is null in the subscribe reply and in every later `ListProductSubscriptions`/`StopProductSubscription` reply.

Expected: the member is null only "for products that don't use license-based subscriptions"; for a subscription that DOES use a license server endpoint — the model names Remote Desktop Services (RDS) Subscriber Access License (SAL) explicitly — the summary carries the license expiry as an ISO-8601 UTC instant (e.g. `2025-03-15T00:00:00Z`). The emulator supports exactly that configuration: `CreateLicenseServerEndpoint` persists an RDS_SAL endpoint against an identity provider, and `StartProductSubscription` can subscribe a user to `REMOTE_DESKTOP_SERVICES` through that same registration.

Actual: no subscription ever carries an expiry, so a client reading `LicenseExpirationDate` to decide when a SAL license must be renewed cannot distinguish "this product has no license expiry" from "this emulator never computes one". The field should be computed from the registration's license server endpoint (present => an expiry derived from the subscription start), and remain null only for the products that are not license-based.

## Evidence

Spec: `ProductUserSummary$LicenseExpirationDate` — "The expiration date of the license associated with this subscription, in ISO 8601 UTC format (for example, 2025-03-15T00:00:00Z). This field applies only to subscriptions that use license server endpoints, such as Remote Desktop Services (RDS) Subscriber Access License (SAL). It returns null for products that don't use license-based subscriptions." (smithy_query shape=ProductUserSummary). Handler: `src/ops/start_product_subscription.rs` sets `license_expiration_date: None` with the comment "Only a product served by a license server endpoint carries an expiry; this slice creates none" — but the service DOES create license server endpoints (`src/ops/create_license_server_endpoint.rs`), so the premise no longer holds; `crate::state::product_user_summary` just copies the stored `None`, and no writer in `src/state.rs` ever sets the field. Tests: `tests/all/identity_provider_subscriptions.rs` asserts Username/Product/Status/SubscriptionStartDate round trips and never asserts a LicenseExpirationDate for an RDS SAL subscription.

## Steps to Reproduce

1. RegisterIdentityProvider with Product=REMOTE_DESKTOP_SERVICES for an Active Directory provider.
2. CreateLicenseServerEndpoint for the returned IdentityProviderArn with ServerType=RDS_SAL and wait until it is PROVISIONED.
3. Call StartProductSubscription with the same IdentityProvider, Product=REMOTE_DESKTOP_SERVICES and a Username.
4. Inspect ProductUserSummary in the reply, and again via ListProductSubscriptions: LicenseExpirationDate is absent/null. Expected: an ISO-8601 UTC expiry for this license-server-backed (RDS SAL) subscription, null only for products that are not license-based.
