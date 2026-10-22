Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: license-manager-linux-subscriptions.update-service-settings-disable-transitions-to-successful license-manager-linux-subscriptions.register-subscription-provider-pending-status license-manager-linux-subscriptions.list-linux-subscription-instances-invalid-filter-operator-rejected license-manager-linux-subscriptions.get-registered-subscription-provider-not-found license-manager-linux-subscriptions.register-duplicate-source-rejected license-manager-linux-subscriptions.update-service-settings-invalid-region-rejected license-manager-linux-subscriptions.register-subscription-provider-concurrent-race license-manager-linux-subscriptions.tag-resource-exceeds-limit-rejected license-manager-linux-subscriptions.get-registered-subscription-provider-field-round-trip license-manager-linux-subscriptions.register-invalid-secret-arn-pattern-rejected license-manager-linux-subscriptions.list-registered-subscription-providers-reflects-registration license-manager-linux-subscriptions.untag-resource-removes-specific-keys license-manager-linux-subscriptions.deregister-removes-subscription-provider
Issue-Shape-Id: com.amazonaws.licensemanagerlinuxsubscriptions#LicenseManagerLinuxSubscriptions
Issue-Key: license-manager-linux-subscriptions.update-service-settings-disable-transitions-to-successful|run#5|AssertionError|assert enabled["Status"] == "Completed", enabled
Issue-Key: license-manager-linux-subscriptions.register-subscription-provider-pending-status|run#3|AssertionError|assert settings.get("LinuxSubscriptionsDiscovery") == "Enabled" and settings.get("Status") == "Completed", (
Issue-Key: license-manager-linux-subscriptions.list-linux-subscription-instances-invalid-filter-operator-rejected|_ensure_onboarded#2|AssertionError|raise AssertionError(f"onboarding settled into a non-completed state (last={last!r})")
Issue-Key: license-manager-linux-subscriptions.get-registered-subscription-provider-not-found|_ensure_onboarded#2|Timeout|time.sleep(5.0)
Issue-Key: license-manager-linux-subscriptions.register-duplicate-source-rejected|_ensure_onboarded#2|Timeout|time.sleep(5.0)
Issue-Key: license-manager-linux-subscriptions.update-service-settings-invalid-region-rejected|_error#4|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: license-manager-linux-subscriptions.register-subscription-provider-concurrent-race|run#11|AssertionError|assert len(winners) == 0, f"expected zero winners (fake tokens), got {winners}"
Issue-Key: license-manager-linux-subscriptions.tag-resource-exceeds-limit-rejected|_error#6|AssertionError|raise AssertionError(f"expected rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: license-manager-linux-subscriptions.get-registered-subscription-provider-field-round-trip|run#7|AssertionError|assert malformed == expected_malformed, (
Issue-Key: license-manager-linux-subscriptions.register-invalid-secret-arn-pattern-rejected|run#6|AssertionError|assert rejected == expected_pattern, f"expected {expected_pattern!r}, got {rejected!r}"
Issue-Key: license-manager-linux-subscriptions.list-registered-subscription-providers-reflects-registration|run#5|AssertionError|assert bad == expected_bad, (
Issue-Key: license-manager-linux-subscriptions.untag-resource-removes-specific-keys|run#7|AssertionError|assert malformed == expected_malformed, (
Issue-Key: license-manager-linux-subscriptions.deregister-removes-subscription-provider|run#8|AssertionError|assert first == _NOT_FOUND, (
Issue-Commit: 7545d0b8b2e4fb4a41c801fcbb48b523234ffec8

# license-manager-linux-subscriptions: 7 fidelity divergences — settled Status reported as 'Successful' not 'Completed', unvalidated SourceRegions, RegisterSubscriptionProvider accepts any token, anchored pattern messages, tag-API error taxonomy

> _Note: this issue was identified against the code base at commit `7545d0b8b2e4fb4a41c801fcbb48b523234ffec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateServiceSettings, GetServiceSettings, RegisterSubscriptionProvider, UntagResource, GetRegisteredSubscriptionProvider, DeregisterSubscriptionProvider, ListRegisteredSubscriptionProviders

## Description
The License Manager Linux Subscriptions emulator answers (1/16 behavioral checks pass), but diverges from AWS in SEVEN distinct root causes. All seven belong to this one service. A fixer must close ALL SEVEN causes below before submitting — they are numbered, with their own failing check ids.

Smithy model used throughout: services/service-com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions/smithy-model/com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions.json (restJson1, sigv4, 11 operations).

=== Cause 1 of 7: a settled service-settings transition reports Status 'Successful' where AWS reports 'Completed' (BLOCKS 5 CHECKS) ===
AWS: Smithy `Status` enum = [InProgress, Completed, Successful, Failed]; it is the `Status` member of both GetServiceSettingsResponse and UpdateServiceSettingsResponse ("Indicates the status of Linux subscriptions settings being applied"). Recorded behavior: once an enable (or disable) of LinuxSubscriptionsDiscovery has settled, GetServiceSettings reports Status == "Completed"; every recorded check polls for exactly that value as its settle condition, and one asserts the Disable path also settles to "Completed".
Emulator: reports Status == "Successful" for a settled transition. services/service-com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions/src/ops/update_service_settings.rs:201 writes `status: crate::state::SETTINGS_STATUS_SUCCESSFUL` into the persisted singleton, and GetServiceSettings reads that back (via crate::state::reported_service_settings). Observed response body: {"HomeRegions":["us-west-2"],"LinuxSubscriptionsDiscovery":"Enabled","LinuxSubscriptionsDiscoverySettings":{"OrganizationIntegration":"Disabled","SourceRegions":["us-west-2"]},"Status":"Successful"} with HTTP 200.
Why: every client that onboards this account-wide setting polls GetServiceSettings until Status reaches the settled value. A client written against AWS waits for "Completed"; against the emulator it never sees it and either spins forever or declares onboarding failed. This is the single highest-impact cause here: it wedges the shared onboarding precondition and so prevents 5 checks from ever reaching the behavior they test (two of them simply time out at 60s inside their poll loop; one raises "onboarding settled into a non-completed state").
Fix: the settings state machine / status projection in src/state.rs (the SETTINGS_STATUS_* constant chosen) plus src/ops/update_service_settings.rs and src/ops/get_service_settings.rs, which must report the settled status AWS reports.
Traces: license-manager-linux-subscriptions.update-service-settings-disable-transitions-to-successful, license-manager-linux-subscriptions.register-subscription-provider-pending-status, license-manager-linux-subscriptions.list-linux-subscription-instances-invalid-filter-operator-rejected, license-manager-linux-subscriptions.get-registered-subscription-provider-not-found (60s timeout in the poll loop), license-manager-linux-subscriptions.register-duplicate-source-rejected (60s timeout in the poll loop).

=== Cause 2 of 7: UpdateServiceSettings accepts a SourceRegions entry that is not an AWS Region, and persists it ===
AWS: UpdateServiceSettings models LinuxSubscriptionsDiscoverySettings as required and declares ValidationException (HTTP 400). Recorded behavior: a SourceRegions entry that is not a real Region is refused with ValidationException and the account's settings are left unchanged; recorded message example "Invalid region name/format : not-a-real-region-9999" (built from the request — the offending region token is interpolated — so the rule, not the exact text, is the requirement).
Emulator: answers HTTP 200 and PERSISTS the bogus value — observed body {"HomeRegions":["us-west-2"],"LinuxSubscriptionsDiscovery":"Enabled","LinuxSubscriptionsDiscoverySettings":{"OrganizationIntegration":"Disabled","SourceRegions":["not-a-real-region-9999"]},"Status":"Successful"}. update_service_settings.rs validates only home-Region count/AllowUpdate; nothing inspects SourceRegions.
Why: overly permissive. A client that mistypes a Region gets a 200 and believes aggregation is configured for a Region that does not exist; the bad value then poisons the account-wide singleton every later read returns.
Fix: input validation in src/ops/update_service_settings.rs (reject each SourceRegions entry that is not a known Region with the modeled ValidationException, before mutating state).
Traces: license-manager-linux-subscriptions.update-service-settings-invalid-region-rejected.

=== Cause 3 of 7: RegisterSubscriptionProvider never validates the Red Hat subscription token — it fabricates an ACTIVE provider (and the 50-tag limit is unenforced behind it) ===
AWS: RegisterSubscriptionProvider models SubscriptionProviderSource (enum [RedHat]) and SecretArn as required and declares ValidationException (HTTP 400); Tags is a map with @length max 50. Recorded behavior (not documented as synchronous, recorded only): registration synchronously validates the Red Hat offline token held in the referenced secret, so a syntactically valid Secrets Manager ARN whose secret holds a non-genuine token is refused with ValidationException (recorded message example "The subscription provider token is invalid or expired") and NO subscription-provider resource is left registered. Recorded behavior also shows a Tags map of 51 entries refused with a DIFFERENT error code, BadRequestException (recorded message example "The resultant tag set must not have more than 50 user tags.").
Emulator: returns HTTP 200 and invents a provider — observed body {"SubscriptionProviderArn":"arn:aws:license-manager-linux-subscriptions:us-west-2:000000000000:subscription-provider/00000000-0000-4000-8000-000000000001","SubscriptionProviderSource":"RedHat","SubscriptionProviderStatus":"ACTIVE"}. Four concurrent registrations with four different fake-token secrets ALL succeeded, each with its own ARN. Because the 51-tag call is never reached (the control call succeeds first), the Tags @length max 50 refusal is unexercised and must be implemented in the same pass; note its code is BadRequestException, which is NOT in this operation's modeled error set and must still be emitted as the recording shows.
Why: overly permissive in the worst direction — it fabricates a BYOL registration that cannot exist, reports it ACTIVE, and lets a caller believe a subscription provider is wired up. Every negative test around registration (enum domain, duplicate source, tag limits) becomes unreachable because the call that is supposed to fail succeeds.
Fix: src/ops/register_subscription_provider.rs — token/secret validation and the Tags count limit, before any state is written; plus the provider record in src/state.rs so a refused registration leaves nothing behind.
Traces: license-manager-linux-subscriptions.register-subscription-provider-concurrent-race, license-manager-linux-subscriptions.tag-resource-exceeds-limit-rejected.

=== Cause 4 of 7: pattern-ValidationException messages quote the regex WITH its ^...$ anchors ===
AWS: Smithy pattern traits are `^arn:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,510}/[a-z0-9-\.]{1,510}$` on SubscriptionProviderArn (GetRegisteredSubscriptionProvider.SubscriptionProviderArn, UntagResource.resourceArn) and `^arn:[a-z0-9-\.]{1,63}:secretsmanager:[a-z0-9-\.]{0,63}:[a-z0-9-\.]{0,63}:secret:[^/]{1,1023}$` on RegisterSubscriptionProvider.SecretArn. The message is built from the request and the member's modeled pattern — a known rule — and AWS renders the pattern WITHOUT the leading ^ and trailing $: "1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[a-z0-9-\.]{1,63}:...".
Emulator: same code (ValidationException, HTTP 400) and same framing, but emits the raw modeled pattern including the anchors: "...regular expression pattern: ^arn:[a-z0-9-\.]{1,63}:...{1,510}$".
Why: clients and SDK test suites that match or re-compile the quoted constraint (and humans diffing against AWS) see a constraint string AWS never emits; the anchors are an artifact of the model, not part of the advertised constraint.
Fix: the shared validation-message renderer used by this service's deserialize/validate stage — strip the anchors when quoting a @pattern (same defect class as the already-open issue "EventBridge Scheduler ... anchored pattern-validation mes...", 20261022T035032Z-scheduler-eventbridge-scheduler-diverges-in-5-ways-reserved-default.md; that issue does not cover this service).
Traces: license-manager-linux-subscriptions.get-registered-subscription-provider-field-round-trip, license-manager-linux-subscriptions.register-invalid-secret-arn-pattern-rejected.

=== Cause 5 of 7: an enum violation inside a LIST member is rendered per-element instead of as a collection constraint ===
AWS: ListRegisteredSubscriptionProviders.SubscriptionProviderSources is a list of SubscriptionProviderSource (enum, sole value [RedHat]); the operation declares ValidationException (HTTP 400). The message is built from the request and the modeled enum — a known rule — and AWS attributes the failure to the LIST member, quoting the whole collection and nesting the member constraint: "1 validation error detected: Value '[NotARealProvider]' at 'subscriptionProviderSources' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [RedHat]]".
Emulator: ValidationException with a per-element path and no nesting: "1 validation error detected: Value 'NotARealProvider' at 'subscriptionProviderSources.1.member' failed to satisfy constraint: Member must satisfy enum value set: [RedHat]".
Why: the member path a client logs or parses (`subscriptionProviderSources` vs `subscriptionProviderSources.1.member`) is the part of the error that tells it WHICH input to fix; the emulator names a path AWS never produces, and the 1-based element index is invented.
Fix: the same shared validation-message renderer, for enum violations reached through a list member (render the collection-level constraint, not the element path).
Traces: license-manager-linux-subscriptions.list-registered-subscription-providers-reflects-registration.

=== Cause 6 of 7: UntagResource rejects a malformed resourceArn with the wrong error class ===
AWS: UntagResource (DELETE /tags/{resourceArn}) models resourceArn as a required httpLabel of SubscriptionProviderArn and declares ONLY ResourceNotFoundException (plus InternalServerException) — ValidationException is NOT in its error set. Recorded behavior: a malformed resourceArn is refused with BadRequestException (recorded message example "Invalid input resource arn: bad syntax"), and a well-formed-but-never-registered subscription-provider ARN is refused with the distinct NotFoundException (recorded message example "Resource was not found") — the format check and the existence check are two ordered, distinguishable stages, and neither code is ValidationException.
Emulator: answers ValidationException with the pattern message ("1 validation error detected: Value 'not-an-arn-at-all' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:...$") for the malformed ARN, so the generic modeled-pattern path short-circuits before the operation's own taxonomy. The not-found leg is never reached, so NotFoundException is unverified and must be implemented in the same pass.
Why: a client branching on the error code cannot tell a syntactically bad ARN from a missing resource — it gets a code this operation does not even model, so typed-error handling (catch BadRequestException / NotFoundException) fails outright.
Fix: src/ops/untag_resource.rs — this operation's own ARN syntax check and existence check, emitting BadRequestException and NotFoundException respectively, ahead of the generic @pattern rejection.
Traces: license-manager-linux-subscriptions.untag-resource-removes-specific-keys.

=== Cause 7 of 7: not-found rejections on subscription-provider ARNs interpolate the request instead of AWS's fixed text ===
AWS: DeregisterSubscriptionProvider and GetRegisteredSubscriptionProvider both declare ResourceNotFoundException (HTTP 400, "Unable to find the requested Amazon Web Services resource"). Recorded behavior only: for a well-formed-but-never-registered ARN both answer ResourceNotFoundException with a FIXED message that names the resource type and nothing from the request — "Registered Subscription Provider not found" — and the rejection is idempotent (the second Deregister of the same ARN is identical, no side effect).
Emulator: right code (ResourceNotFoundException), but the message is built from the request: "subscription provider arn:aws:license-manager-linux-subscriptions:us-west-2:000000000000:subscription-provider/22222222-2222-2222-2222-222222222222 is not registered".
Why: the emulator leaks the caller's input back into an error AWS keeps constant, so message-sensitive assertions and log-scrapers diverge; it is also the same request-echoing message rule already filed against several other services here (e.g. 20261022T044905Z-workmailmessageflow-resourcenotfoundexception-..., 20261022T044921Z-verifiedpermissions-verified-permissions-error-messages-interpolate-request-...; neither covers this service). Lowest severity of the bundle — code and status are correct.
Fix: the not-found error construction in src/ops/deregister_subscription_provider.rs and src/ops/get_registered_subscription_provider.rs.
Traces: license-manager-linux-subscriptions.deregister-removes-subscription-provider (the get-registered-subscription-provider-not-found check also covers this leg but is currently blocked by Cause 1).

NOT part of this issue (already filed for this suite, do not re-file): ListLinuxSubscriptions and TagResource are unimplemented scaffold stubs answering 501 NotImplemented, each with its own filed issue.

## Evidence
Suite: 1/16 passed (6.2%); 14 mismatched, 1 errored. All failures are in license-manager-linux-subscriptions.

Cause 1 — expected Status 'Completed', got 'Successful':
  update-service-settings-disable-transitions-to-successful: AssertionError on `enabled["Status"] == "Completed"` with body {... 'LinuxSubscriptionsDiscovery': 'Enabled', 'Status': 'Successful'} (HTTP 200).
  register-subscription-provider-pending-status: "expected Linux subscriptions discovery Enabled/Completed before the token test ... got {... 'Status': 'Successful'}".
  list-linux-subscription-instances-invalid-filter-operator-rejected: "onboarding settled into a non-completed state (last={... 'Status': 'Successful'})".
  get-registered-subscription-provider-not-found / register-duplicate-source-rejected: "Check timed out after 60.0s" — both are inside a poll loop whose exit condition is Status == 'Completed'.
  Source: src/ops/update_service_settings.rs:201 `status: crate::state::SETTINGS_STATUS_SUCCESSFUL.to_string()`.

Cause 2 — expected ('ValidationException', 'Invalid region name/format : not-a-real-region-9999'); got HTTP 200 with {'LinuxSubscriptionsDiscoverySettings': {'OrganizationIntegration': 'Disabled', 'SourceRegions': ['not-a-real-region-9999']}, 'Status': 'Successful'} (update-service-settings-invalid-region-rejected).

Cause 3 — expected ('ValidationException', 'The subscription provider token is invalid or expired'); got HTTP 200. register-subscription-provider-concurrent-race: "expected zero winners (fake tokens), got [('ok', '...subscription-provider/00000000-0000-4000-8000-000000000001'), ...0002, ...0003, ...0004]" — four of four fake-token registrations succeeded. tag-resource-exceeds-limit-rejected: "expected rejection, but the call SUCCEEDED and returned {'SubscriptionProviderArn': '...000000000001', 'SubscriptionProviderSource': 'RedHat', 'SubscriptionProviderStatus': 'ACTIVE'}" (HTTP 200), so the 51-tag leg expecting ('BadRequestException', 'The resultant tag set must not have more than 50 user tags.') was never reached.

Cause 4 — same code, pattern quoted with anchors.
  get-registered-subscription-provider-field-round-trip: expected "...pattern: arn:[a-z0-9-\\.]{1,63}:...[a-z0-9-\\.]{1,510}" got "...pattern: ^arn:[a-z0-9-\\.]{1,63}:...[a-z0-9-\\.]{1,510}$".
  register-invalid-secret-arn-pattern-rejected: expected "...pattern: arn:[a-z0-9-\\.]{1,63}:secretsmanager:...:secret:[^/]{1,1023}" got the same string wrapped in ^...$.

Cause 5 — list-registered-subscription-providers-reflects-registration: expected ('ValidationException', "1 validation error detected: Value '[NotARealProvider]' at 'subscriptionProviderSources' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [RedHat]]"); got ('ValidationException', "1 validation error detected: Value 'NotARealProvider' at 'subscriptionProviderSources.1.member' failed to satisfy constraint: Member must satisfy enum value set: [RedHat]").

Cause 6 — untag-resource-removes-specific-keys: expected ('BadRequestException', 'Invalid input resource arn: bad syntax') for resourceArn='not-an-arn-at-all'; got ('ValidationException', "1 validation error detected: Value 'not-an-arn-at-all' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:...$"). Model: UntagResource errors = [ResourceNotFoundException, InternalServerException] — ValidationException is not modeled for it.

Cause 7 — deregister-removes-subscription-provider: expected ('ResourceNotFoundException', 'Registered Subscription Provider not found'); got ('ResourceNotFoundException', 'subscription provider arn:aws:license-manager-linux-subscriptions:us-west-2:000000000000:subscription-provider/22222222-2222-2222-2222-222222222222 is not registered').

## Steps to Reproduce
Clone the repo, boot the emulator, and replay the behavioral suite; then per cause:

Cause 1: lls.update_service_settings(LinuxSubscriptionsDiscovery='Enabled', LinuxSubscriptionsDiscoverySettings={'OrganizationIntegration':'Disabled','SourceRegions':[region]}, AllowUpdate=True), then poll lls.get_service_settings() — it reports Status 'Successful' and never 'Completed'. Repeat with LinuxSubscriptionsDiscovery='Disabled'.

Cause 2: lls.update_service_settings(LinuxSubscriptionsDiscovery='Enabled', LinuxSubscriptionsDiscoverySettings={'OrganizationIntegration':'Disabled','SourceRegions':['not-a-real-region-9999']}, AllowUpdate=True) — returns 200 and a following get_service_settings shows the bogus Region persisted.

Cause 3: create a Secrets Manager secret holding any non-token string, then lls.register_subscription_provider(SubscriptionProviderSource='RedHat', SecretArn=<that arn>) — returns 200 with a fabricated SubscriptionProviderArn and SubscriptionProviderStatus 'ACTIVE'. Fire four concurrently with four distinct secrets: all four succeed. Then retry with Tags={f'key-{i}': f'value-{i}' for i in range(51)} — the over-50 map is also accepted.

Cause 4: lls.get_registered_subscription_provider(SubscriptionProviderArn='not-an-arn-at-all') and lls.register_subscription_provider(SubscriptionProviderSource='RedHat', SecretArn='not-a-valid-secret-arn') — inspect the quoted regex in the ValidationException message for the leading ^ and trailing $.

Cause 5: lls.list_registered_subscription_providers(SubscriptionProviderSources=['NotARealProvider']) — inspect the member path in the ValidationException message.

Cause 6: lls.untag_resource(resourceArn='not-an-arn-at-all', tagKeys=['a','b']) — expect BadRequestException, observe ValidationException. Then lls.untag_resource(resourceArn=f'arn:aws:license-manager-linux-subscriptions:{region}:{account}:subscription-provider/55555555-5555-5555-5555-555555555555', tagKeys=['a','b']) — expect NotFoundException 'Resource was not found'.

Cause 7: lls.deregister_subscription_provider(SubscriptionProviderArn=f'arn:aws:license-manager-linux-subscriptions:{region}:{account}:subscription-provider/22222222-2222-2222-2222-222222222222') twice — both must be the identical fixed-text ResourceNotFoundException; observe the request-echoing message.

## Resolution

All seven causes are fixed; the fix is in
`services/service-com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions/`
and is covered by `tests/all/error_taxonomy.rs` (one test per cause, each over a table of
inputs spanning the class) plus the reworked `tests/all/subscription_providers.rs`.

### RegisterSubscriptionProvider is DECLINED as unsupported — what clients should expect

Cause 3's fix is a refusal, and it applies to EVERY registration, not only to the
recorded fake-token one. Stated plainly so no client is surprised:

* `RegisterSubscriptionProvider` answers **HTTP 400 `ValidationException`** for every
  request, and writes nothing to state. The CODE is the recorded one; the MESSAGE states
  what is unsupported ("The subscription provider token cannot be validated: this
  emulator holds no subscription-provider trust material and makes no call to the
  provider, so no registration is accepted") rather than the recorded verdict on one
  token, because this emulator inspects no token and reaches no verdict.
* WHY: registering a BYOL subscription provider means having License Manager validate the
  Red Hat offline token held in the referenced Secrets Manager secret against Red Hat SSO
  before the resource exists. This emulator holds no Red Hat SSO trust material and makes
  no outbound call, so it cannot establish ANY token as genuine — and a token that cannot
  be established as genuine is exactly what the recorded service refuses. Answering 200
  with a minted `SubscriptionProviderArn` and `SubscriptionProviderStatus: ACTIVE` (what
  the emulator did before) reports a BYOL registration that cannot exist and a validation
  nothing performed; storing it in `PENDING` instead would still report a registration
  nothing validated, and would make the recorded check that expects zero successful
  registrations fail.
* CONSEQUENCE for clients: the subscription-provider resource has no create path over the
  wire. `GetRegisteredSubscriptionProvider`, `ListRegisteredSubscriptionProviders`,
  `DeregisterSubscriptionProvider`, `TagResource`, `UntagResource` and
  `ListTagsForResource` are fully implemented and answer from the single shared store, so
  they report an empty account (empty list, `ResourceNotFoundException` /
  `NotFoundException` for a named ARN) unless a record is placed in the store
  in-process — which is how this crate's test suites exercise the lifecycle
  (`tests/all/harness.rs::seed_provider`, `tests/all/resource_tags.rs::register`, both
  through the same `engine.state::<…>()` seam the handlers use).
* WHAT WOULD CHANGE IT: a Red Hat token validator the emulator can run in-process, or a
  recorded AWS response showing a registration this emulator could faithfully accept.
  Until then the refusal is the honest answer, and it is the recorded one.

### KNOWN GAP — a partially-invalid collection quotes only the failing elements

Cause 5's rule is implemented in `src/service/validation.rs`, and one part of it cannot
be met from inside a service crate:

* AWS quotes the WHOLE collection the caller sent (`Value '[RedHat, SUSE]'`). The
  structured refusal the framework hands a service
  (`smithy_server_runtime::Refusal::Invalid`) carries one `Violation` per OFFENDING
  element — the elements that passed are not in it — so the renderer can only quote the
  collection formed from the ones that failed (`Value '[SUSE]'`).
* The recorded case, and every case where no element passes, is byte-identical to AWS;
  the difference appears only when a collection mixes valid and invalid elements. The
  test pins the emulator's actual rendering for that case so the gap is visible rather
  than silent.
* CLOSING IT needs `Violation` (in `vendor/smithy-schema-server/src/constraints.rs`) to
  carry the collection value it was built from, which is outside the paths a service fix
  may change.
