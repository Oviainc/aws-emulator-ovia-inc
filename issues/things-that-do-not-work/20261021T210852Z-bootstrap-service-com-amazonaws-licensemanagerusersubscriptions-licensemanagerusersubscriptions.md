Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the license-manager-user-subscriptions emulator

_Filed: 2026-10-21T21:08:52Z_
_Operation: service-com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 17 operations in this crate is an unmodified scaffold answering HTTP 501 `NotImplemented` with zero persisted state, so no client workflow works at all. Implement a FIRST, SMALL stateful slice: the identity-provider registration + user product-subscription loop, which is the service's primary workflow.

Happy path to implement, in order (restJson1, sigv4 name `license-manager-user-subscriptions`):

1. **RegisterIdentityProvider** — `POST /identity-provider/RegisterIdentityProvider`. Input: `IdentityProvider` (union, required — currently only the `ActiveDirectoryIdentityProvider` member with `DirectoryId` matching `^(d|sd)-[0-9a-f]{10}$`, `ActiveDirectorySettings`, `ActiveDirectoryType`, `IsSharedActiveDirectory`), `Product` (required), `Settings` (`Subnets` min 1 + `SecurityGroupId` matching `^sg-(([0-9a-z]{8})|([0-9a-z]{17}))$`), `Tags`. Must mint and store a real `IdentityProviderArn` (`arn:aws:license-manager-user-subscriptions:<region>:<account>:identity-provider/<id>`) and return `IdentityProviderSummary{IdentityProvider, Settings, Product, Status, IdentityProviderArn}` built from what was stored — `Status` should progress to `REGISTERED` (an initial `REGISTERING` is acceptable only if a later read transitions it). Re-registering the same (IdentityProvider, Product) pair must return `ConflictException`.
2. **ListIdentityProviders** — `POST /identity-provider/ListIdentityProviders`. Must return the registrations created in step 1 from state in `IdentityProviderSummaries` (required member, so always present, empty list when nothing registered), honoring `MaxResults` + opaque `NextToken` paging.
3. **StartProductSubscription** — `POST /user/StartProductSubscription`. Input: `Username` (required), `IdentityProvider` (required), `Product` (required), `Domain`, `Tags`. Must resolve the identity provider from state (unknown provider/product → `ResourceNotFoundException`), create a product-user record with a minted `ProductUserArn`, and return `ProductUserSummary{Username, Product, IdentityProvider, Status (SUBSCRIBED), ProductUserArn, Domain, SubscriptionStartDate}`. Subscribing the same user to the same product twice → `ConflictException`.
4. **ListProductSubscriptions** — `POST /user/ListProductSubscriptions`. Input: `IdentityProvider` (required), `Product`, `MaxResults`, `Filters`, `NextToken`. Must return exactly the product users recorded in step 3 for that provider/product (`ProductUserSummaries`), with `MaxResults`/`NextToken` paging and at minimum `Username`/`Status` filter support via `Filter{Attribute, Operation, Value}`.
5. **StopProductSubscription** — `POST /user/StopProductSubscription`. Addressable either by `ProductUserArn` or by (`Username`, `IdentityProvider`, `Product`, `Domain`). Must remove/terminate the stored subscription and return the `ProductUserSummary` with `Status` = `UNSUBSCRIBED` and a `SubscriptionEndDate`; a second stop of the same user → `ResourceNotFoundException`. After this, ListProductSubscriptions must no longer report the user as subscribed.
6. **DeregisterIdentityProvider** — `POST /identity-provider/DeregisterIdentityProvider`. Addressable by `IdentityProviderArn` or (`IdentityProvider`, `Product`). Must delete the stored registration and return the final `IdentityProviderSummary` with `Status` = `DEREGISTERING`/`DEREGISTERED`; deregistering an unknown provider → `ResourceNotFoundException`, and after it the provider must be gone from ListIdentityProviders.

Rules for this slice:
- NEVER STUBS: every one of the six operations must compute its response from persisted state. No hardcoded summaries, no echoing the input back without storing it, no minting an ARN that is never recorded. A register followed by a list must return what was registered; a stop followed by a list must reflect the removal.
- Modeled errors must use the real shapes/statuses from the model: `ValidationException` (400) for missing required members or pattern violations (bad `DirectoryId`, bad `SecurityGroupId`, empty `Subnets`), `ResourceNotFoundException` (404) for unknown ARNs/providers/users, `ConflictException` for duplicate register/subscribe.
- Idempotency: RegisterIdentityProvider, DeregisterIdentityProvider and the license-server ops are `@idempotent` in the model — identical repeated register of an already-identical registration should not create a duplicate record.

## State Model

Store in `ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState` (reached via `crate::state::service_state(&*self.engine)`, one shared engine):

- `identity_providers: HashMap<String /* IdentityProviderArn */, IdentityProviderRecord>` where
  `IdentityProviderRecord { arn: String, identity_provider: IdentityProvider /* union; directory_id for the AD variant */, product: String, settings: Option<Settings{subnets: Vec<String>, security_group_id: String}>, status: String, failure_message: Option<String>, owner_account_id: String, tags: HashMap<String,String>, created_at: SystemTime }`
  plus an index `provider_key: HashMap<(String /* directory_id or union key */, String /* product */), String /* arn */>` so lookups by (IdentityProvider, Product) and by ARN both resolve.
- `product_users: HashMap<String /* ProductUserArn */, ProductUserRecord>` where
  `ProductUserRecord { arn: String, username: String, product: String, identity_provider_arn: String, status: String, status_message: Option<String>, domain: Option<String>, subscription_start_date: Option<String /* ISO8601 */>, subscription_end_date: Option<String>, license_expiration_date: Option<String>, tags: HashMap<String,String> }`
  plus index `(identity_provider_arn, product, username) -> arn`.
- `instance_users: HashMap<String /* InstanceUserArn */, InstanceUserRecord { arn, username, instance_id, identity_provider_arn, status, status_message, domain, association_date, disassociation_date }>` — reserved for the AssociateUser/DisassociateUser/ListUserAssociations slice (out of scope here, left stubbed).
- `tags: HashMap<String /* ResourceArn */, HashMap<String,String>>` — written by RegisterIdentityProvider/StartProductSubscription's `Tags` so a later TagResource/ListTagsForResource slice reads the same store.
- Opaque pagination cursors for the list operations (e.g. an encoded offset or last-seen ARN) so `NextToken` round-trips.

All other operations (AssociateUser, DisassociateUser, ListUserAssociations, ListInstances, UpdateIdentityProviderSettings, TagResource, UntagResource, ListTagsForResource, Create/Delete/ListLicenseServerEndpoints) stay stubbed at 501 for now and will be filed as individual gap issues.

## Evidence

All 17 handler files under `services/service-com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions/src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501 (grep over src/ops confirms 17/17). Contracts come from the crate's `OPERATIONS.md` (routes, required members, declared errors, @paginated/@idempotent traits) and from the Smithy model shapes `IdentityProvider` (union with `ActiveDirectoryIdentityProvider`), `IdentityProviderSummary` (required `IdentityProvider`, `Settings`, `Product`, `Status`), `Settings` (required `Subnets` min 1, `SecurityGroupId` pattern `^sg-...$`), `ProductUserSummary` (required `Username`, `Product`, `IdentityProvider`, `Status`), and `Filter`. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `license-manager-user-subscriptions` at it.
2. `RegisterIdentityProvider` with IdentityProvider.ActiveDirectoryIdentityProvider.DirectoryId=d-0123456789, Product=VISUAL_STUDIO_ENTERPRISE, Settings={Subnets:[subnet-0123456789abcdef0], SecurityGroupId: sg-0123456789abcdef0} — expect 200 with an IdentityProviderSummary carrying a real IdentityProviderArn; today: HTTP 501 NotImplemented.
3. `ListIdentityProviders` — expect the registration from step 2 echoed from state in IdentityProviderSummaries.
4. `StartProductSubscription` with Username=alice, the same IdentityProvider and Product — expect 200 with ProductUserSummary{Status: SUBSCRIBED, ProductUserArn, SubscriptionStartDate}.
5. `ListProductSubscriptions` with that IdentityProvider and Product — expect alice present with the same ProductUserArn as step 4.
6. `StopProductSubscription` by that ProductUserArn — expect 200 with Status UNSUBSCRIBED; re-run ListProductSubscriptions and expect alice no longer subscribed.
7. `DeregisterIdentityProvider` for that IdentityProviderArn — expect 200, then ListIdentityProviders returns an empty IdentityProviderSummaries list.
8. Repeat step 7 — expect ResourceNotFoundException (404), not 501 or a hollow 200.
