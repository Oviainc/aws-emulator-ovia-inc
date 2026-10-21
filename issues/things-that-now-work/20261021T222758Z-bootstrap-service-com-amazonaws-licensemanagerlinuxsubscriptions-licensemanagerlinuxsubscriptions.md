Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 85378260976efce74d968b045ae62c2fe2a03011

# Bootstrap the license-manager-linux-subscriptions emulator

_Filed: 2026-10-21T22:27:58Z_
_Operation: service-com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions | Class: stub_

> _Note: this issue was identified against the code base at commit `85378260976efce74d968b045ae62c2fe2a03011`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 11 operations in `src/ops/` are unmodified scaffolds that answer HTTP 501 `NotImplemented` and hold no state, so no client workflow works at all. Implement a FIRST, SMALL, stateful slice: the subscription-provider registration lifecycle plus the service settings pair (restJson1, sigv4 name `license-manager-linux-subscriptions`).

Happy path, in order:

1. **RegisterSubscriptionProvider** (`POST /subscription/RegisterSubscriptionProvider`) — input `SubscriptionProviderSource` (enum, only `RedHat`, required), `SecretArn` (required, must match the Secrets Manager ARN pattern), optional `Tags` map (0..50). Mint a deterministic `SubscriptionProviderArn` of the form `arn:aws:license-manager-linux-subscriptions:<region>:<account>:subscription-provider/<uuid>`, STORE the provider record (source, secret ARN, status, tags) and return `SubscriptionProviderSource`, `SubscriptionProviderArn`, `SubscriptionProviderStatus` (`ACTIVE`) read back from the stored record. Registering the same `SecretArn`+source again must be idempotent (@idempotent) and return the SAME ARN rather than a second record.
2. **GetRegisteredSubscriptionProvider** (`POST /subscription/GetRegisteredSubscriptionProvider`) — input `SubscriptionProviderArn` (required). Look the record up in state and return `SubscriptionProviderArn`, `SubscriptionProviderSource`, `SecretArn`, `SubscriptionProviderStatus`, `SubscriptionProviderStatusMessage`, `LastSuccessfulDataRetrievalTime` from the record. Unknown ARN -> `ResourceNotFoundException` (HTTP 400); ARN not matching the modeled pattern -> `ValidationException`.
3. **ListRegisteredSubscriptionProviders** (`POST /subscription/ListRegisteredSubscriptionProviders`) — return `RegisteredSubscriptionProviders` as a list of `RegisteredSubscriptionProvider{SubscriptionProviderArn, SubscriptionProviderSource, SecretArn, SubscriptionProviderStatus, SubscriptionProviderStatusMessage, LastSuccessfulDataRetrievalTime}` built from the stored map — every provider registered in step 1 must appear, and only those. Honour the `SubscriptionProviderSources` filter, `MaxResults`, and emit/consume `NextToken` for the @paginated contract (no token when the page is the last one).
4. **DeregisterSubscriptionProvider** (`POST /subscription/DeregisterSubscriptionProvider`) — input `SubscriptionProviderArn` (required). REMOVE the record from state and return an empty 200 body. A subsequent Get must fail with `ResourceNotFoundException` and List must no longer include it. Deregistering an unknown ARN -> `ResourceNotFoundException`.
5. **UpdateServiceSettings** (`POST /subscription/UpdateServiceSettings`) — input `LinuxSubscriptionsDiscovery` (`Enabled`|`Disabled`, required) and `LinuxSubscriptionsDiscoverySettings{SourceRegions (1..100, unique, required), OrganizationIntegration (required)}`, optional `AllowUpdate`. PERSIST these settings (one per-account singleton) and return the persisted values plus `Status` (`Successful`), `StatusMessage`, `HomeRegions`.
6. **GetServiceSettings** (`POST /subscription/GetServiceSettings`) — no input. Return the settings persisted by step 5: `LinuxSubscriptionsDiscovery`, `LinuxSubscriptionsDiscoverySettings`, `Status`, `StatusMessage`, `HomeRegions`. Before any update it must report the default (`Disabled`, empty settings), never a canned "Enabled".

EVERY one of these six must compute its response from persisted state held in the shared engine (reached via `crate::state::service_state(&*self.engine)`). No canned responses, no echoing the input back without storing it, no minting an ARN that is never saved — a register followed by a get/list must return exactly what was registered, and a deregister must be observable by the next caller.

## State Model

In `src/state.rs`:

- `providers: BTreeMap<String /* SubscriptionProviderArn */, SubscriptionProvider>` where `SubscriptionProvider { arn: String, source: String /* "RedHat" */, secret_arn: String, status: String /* ACTIVE | INVALID | PENDING */, status_message: Option<String>, last_successful_data_retrieval_time: Option<String>, tags: BTreeMap<String,String> }` — insertion order preserved for stable list/pagination ordering.
- `secret_index: BTreeMap<(String /* source */, String /* secret_arn */), String /* arn */>` — makes re-registration idempotent.
- `service_settings: ServiceSettings { linux_subscriptions_discovery: String /* default "Disabled" */, source_regions: Vec<String>, organization_integration: String, status: String /* default "Successful" */, status_message: BTreeMap<String,String>, home_regions: Vec<String> }` — the per-account singleton behind Get/UpdateServiceSettings.

Out of scope for this first slice (stay 501, filed separately later): ListLinuxSubscriptions, ListLinuxSubscriptionInstances, TagResource, UntagResource, ListTagsForResource (tags are stored on the provider record by step 1 but the tagging APIs need not be wired yet).

Done = a client script can register a provider, get it, see it in the list, deregister it, see it gone, and round-trip service settings — with correct bodies, not just 2xx.

## Evidence

`services/service-com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions/OPERATIONS.md` lists all 11 operations with these exact routes, required members, enums, patterns, @paginated and @idempotent traits, and the `ResourceNotFoundException`/`ValidationException`/`ThrottlingException`/`InternalServerException` error sets. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 11 handler files (deregister_subscription_provider.rs, get_registered_subscription_provider.rs, get_service_settings.rs, list_linux_subscription_instances.rs, list_linux_subscriptions.rs, list_registered_subscription_providers.rs, list_tags_for_resource.rs, register_subscription_provider.rs, tag_resource.rs, untag_resource.rs, update_service_settings.rs), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` = HTTP 501. The shapes `RegisteredSubscriptionProvider` and `LinuxSubscriptionsDiscoverySettings{SourceRegions, OrganizationIntegration}` were read from the Smithy model. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point the AWS SDK/CLI at it with `--endpoint-url`.
2. `aws license-manager-linux-subscriptions register-subscription-provider --subscription-provider-source RedHat --secret-arn arn:aws:secretsmanager:us-east-1:123456789012:secret:rhel-token` -> currently HTTP 501; expected 200 with a persisted SubscriptionProviderArn and SubscriptionProviderStatus=ACTIVE.
3. `get-registered-subscription-provider --subscription-provider-arn <arn from step 2>` -> expected 200 echoing the stored source/secret ARN/status; currently 501.
4. `list-registered-subscription-providers` -> expected the registered provider in RegisteredSubscriptionProviders; currently 501.
5. `deregister-subscription-provider --subscription-provider-arn <arn>` -> expected empty 200; then repeat the get -> expected ResourceNotFoundException (HTTP 400), and the list -> expected empty.
6. `update-service-settings --linux-subscriptions-discovery Enabled --linux-subscriptions-discovery-settings 'SourceRegions=[us-east-1],OrganizationIntegration=Disabled'` then `get-service-settings` -> expected the persisted Enabled/us-east-1 settings returned; currently both 501.
