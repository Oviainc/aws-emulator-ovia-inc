Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# RegisterSubscriptionProvider refuses every request, so the subscription-provider resource has no create path

_Filed: 2026-10-24T01:03:15Z_
_Operation: RegisterSubscriptionProvider | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler unconditionally returns HTTP 400 `ValidationException` ("The subscription provider token cannot be validated: this emulator holds no subscription-provider trust material…") and writes nothing to state — it never looks at the input and never touches the provider store.

Expected: a `RegisterSubscriptionProvider` call carrying a required `SubscriptionProviderSource` (`RedHat`) and a pattern-valid `SecretArn` creates the subscription-provider resource and answers HTTP 200 with a minted `SubscriptionProviderArn`, the echoed `SubscriptionProviderSource`, and a `SubscriptionProviderStatus` describing the registration action (`PENDING` while License Manager validates the access token, `ACTIVE` once it is accepted, `INVALID` when it cannot be validated — in which case `GetRegisteredSubscriptionProvider` reports that status together with a `SubscriptionProviderStatusMessage`). The `Tags` map supplied at registration must be stored and be readable through `ListTagsForResource`.

Actual: every registration is refused, nothing is persisted, and the provider store can only be written in-process by this crate's own test harness (`tests/all/harness.rs::seed_provider`). Consequence for any wire client: `ListRegisteredSubscriptionProviders` always returns an empty list, `GetRegisteredSubscriptionProvider` and `DeregisterSubscriptionProvider` always answer `ResourceNotFoundException`, and `TagResource`/`UntagResource`/`ListTagsForResource` always answer 404 `NotFoundException` — the whole create→read→list→tag→deregister lifecycle of the service's primary resource is unreachable.

A registration whose token cannot be established as genuine should be *recorded as a provider in a non-ACTIVE state* rather than erased: that is the only way the modelled `INVALID`/`PENDING` statuses and `SubscriptionProviderStatusMessage` can ever be observed.

## Evidence

Smithy model (`RegisterSubscriptionProvider`): output members `SubscriptionProviderArn` (String), `SubscriptionProviderSource`, `SubscriptionProviderStatus` (enum ACTIVE | INVALID | PENDING), input `Tags` (map, len 0..50) documented as "The metadata tags to assign to your registered Linux subscription provider resource"; `GetRegisteredSubscriptionProvider` additionally returns `SubscriptionProviderStatusMessage` and `LastSuccessfulDataRetrievalTime` — members that presuppose a registration that EXISTS while/after token validation. `DeregisterSubscriptionProvider` is modelled as the delete half of that lifecycle.

Handler: `src/ops/register_subscription_provider.rs` lines ~170-194 — `let _ = (&input, ctx, &self.engine);` followed by an unconditional `Err(RegisterSubscriptionProviderErrorModeled::ValidationException(... TOKEN_NOT_VALIDATED_MESSAGE ...))`. No `crate::state::service_state` access at all; `src/state.rs` comments confirm the provider store is only populated by in-process seeding.

Test coverage: `tests/all/error_taxonomy.rs::a_registration_is_refused_and_registers_nothing` pins the blanket refusal, and `tests/all/subscription_providers.rs` / `tests/all/resource_tags.rs` work around it by seeding records through `engine.state::<…>()`. So the current behaviour is asserted, but no test exercises a registration over the wire producing a readable resource. This is NOT the previously-fixed cause ("accepts any token and reports ACTIVE"): that was over-permissive; this is the opposite failure — the create path was removed entirely instead of being modelled as PENDING/INVALID.

## Steps to Reproduce

1. Start the emulator and call POST /subscription/RegisterSubscriptionProvider with body {"SubscriptionProviderSource":"RedHat","SecretArn":"arn:aws:secretsmanager:us-east-1:000000000000:secret:redhat-token-abc123"} and a few Tags.
2. Observe HTTP 400 ValidationException instead of HTTP 200 carrying SubscriptionProviderArn / SubscriptionProviderSource / SubscriptionProviderStatus.
3. Call POST /subscription/ListRegisteredSubscriptionProviders — the list is empty: nothing was registered.
4. Call POST /subscription/GetRegisteredSubscriptionProvider with any ARN shaped like a provider ARN — ResourceNotFoundException; and GET /tags/{thatArn} — 404 NotFoundException. No wire sequence can ever produce a provider resource.
