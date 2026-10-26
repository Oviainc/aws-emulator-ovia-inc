Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: license-manager-linux-subscriptions.register-subscription-provider-concurrent-race
Issue-Shape-Id: com.amazonaws.licensemanagerlinuxsubscriptions#LicenseManagerLinuxSubscriptions
Issue-Key: license-manager-linux-subscriptions.register-subscription-provider-concurrent-race|run#11|AssertionError|assert len(winners) == 0, f"expected zero winners (fake tokens), got {winners}"
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# RegisterSubscriptionProvider accepts a secret holding a non-genuine Red Hat offline token, minting providers AWS refuses with ValidationException

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterSubscriptionProvider

## Description
Cause 1 of 1: RegisterSubscriptionProvider performs no validation of the referenced secret's subscription-provider credential, so every syntactically well-formed request succeeds and a provider resource is left registered — including the concurrent case, where several simultaneous registrations for the same source all succeed instead of all being refused.

Failing check ids: license-manager-linux-subscriptions.register-subscription-provider-concurrent-race

AWS: RegisterSubscriptionProvider (Smithy: input SubscriptionProviderSource required, enum SubscriptionProviderSource; SecretArn required with @pattern ^arn:[a-z0-9-\.]{1,63}:secretsmanager:[a-z0-9-\.]{0,63}:[a-z0-9-\.]{0,63}:secret:[^/]{1,1023}$; operation is @idempotent; modeled errors InternalServerException (500), ThrottlingException (400), ValidationException (400, client)). The Smithy model does not state the credential check itself, so for that part: recorded behavior only — the recording shows that registering source "RedHat" with a Secrets Manager secret whose SecretString is not a genuine Red Hat offline token is rejected synchronously with HTTP 400 / error code ValidationException, and that this holds uniformly when four such registrations are fired concurrently for the same source (no race lets one through). After the rejections, ListRegisteredSubscriptionProviders(SubscriptionProviderSources=["RedHat"]) reports the same number of registered providers as before the attempt — no provider resource is created. Hypothesis: the real service validates the offline token against the provider before admitting the registration, which is why the outcome is independent of concurrency.

Emulator: all four concurrent RegisterSubscriptionProvider calls returned 200 with distinct SubscriptionProviderArns (replay shows four ('ok', arn:aws:license-manager-linux-subscriptions:<region>:<account>:subscription-provider/<uuid>) results and zero errors), so zero calls were rejected and four provider resources remained registered. The check's positive control passed first — ListRegisteredSubscriptionProviders returned a well-formed list — so the account was healthy and onboarded, and the divergence is the missing rejection, not a broken service. No ValidationException is raised for an unusable credential at all; the request is admitted purely on ARN shape.

Why: a client that uses RegisterSubscriptionProvider to verify a BYOL credential before storing it reads success and believes the token is good. Error-handling paths that catch ValidationException and prompt for a new token are never exercised against the emulator, and tests asserting that a bad token leaves no provider behind fail against real AWS instead of the emulator. Under concurrency the divergence is worse: the emulator both accepts and multiplies resources that AWS would have produced none of, so cleanup logic written against the emulator leaks on AWS and vice versa.

Fix: the operation's validation in the RegisterSubscriptionProvider handler — the secret referenced by SecretArn must be resolved and its credential checked before any provider record is created, and the failure must surface as HTTP 400 with error code ValidationException; the registration must be rejected before the state mutation so the concurrent case leaves the provider store unchanged. Related but distinct open issues, neither of which covers this missing synchronous rejection: 20261024T034312Z-license-manager-linux-subscriptions-registersubscriptionprovider-wrong-behavior--r2 (registration identity keyed on (source, secret)) and 20261024T034312Z-license-manager-linux-subscriptions-getregisteredsubscriptionprovider-wrong-behavior (a registered provider can never reach ACTIVE). The second shows the emulator defers the credential verdict to a post-registration status instead of refusing the call.

Traces: license-manager-linux-subscriptions.register-subscription-provider-concurrent-race (group concurrency, axis:rejection) — four concurrent registrations of source RedHat, each with its own secret containing a fake Red Hat offline token.

## Evidence
Check: license-manager-linux-subscriptions.register-subscription-provider-concurrent-race [mismatch]

Expected (recorded AWS behavior): all 4 concurrent RegisterSubscriptionProvider calls fail with HTTP 400 / Error.Code == "ValidationException"; ListRegisteredSubscriptionProviders(SubscriptionProviderSources=["RedHat"]) count unchanged before vs. after.

Actual (emulator): 4 successes, 0 failures. Replay assertion:
  AssertionError: expected zero winners (fake tokens), got [('ok', 'arn:aws:license-manager-linux-subscriptions:us-west-2:000000000000:subscription-provider/00000000-0000-4000-8000-000000000001'), ('ok', '...-0002'), ('ok', '...-0003'), ('ok', '...-0004')]
(the region, account and provider UUIDs above are environment-specific values, not requirements; the requirement is that there be zero successful registrations.)

Positive control in the same check passed: ListRegisteredSubscriptionProviders returned a well-formed RegisteredSubscriptionProviders list, and _ensure_onboarded saw LinuxSubscriptionsDiscovery=Enabled / Status=Completed, so the uniform rejection AWS gives is not explainable by a generally failing account.

Model citation: Smithy RegisterSubscriptionProvider — errors include ValidationException (client, httpError 400); SecretArn @pattern as quoted in the description; http POST /subscription/RegisterSubscriptionProvider, @idempotent.

## Steps to Reproduce
1. Ensure the account is onboarded: GetServiceSettings; if LinuxSubscriptionsDiscovery is not Enabled/Completed, create the service-linked role for license-manager-linux-subscriptions.amazonaws.com and call UpdateServiceSettings(LinuxSubscriptionsDiscovery="Enabled", LinuxSubscriptionsDiscoverySettings={OrganizationIntegration:"Disabled", SourceRegions:[<region>]}, AllowUpdate=true); wait for Status != InProgress.
2. POST /subscription/ListRegisteredSubscriptionProviders with {"SubscriptionProviderSources":["RedHat"]} and record the count (baseline).
3. Create 4 Secrets Manager secrets, each with SecretString set to an arbitrary non-genuine value (e.g. "fake-redhat-offline-token-race-0" .. "-3") and tag LicenseManagerLinuxSubscriptions=enabled. Note each ARN.
4. From 4 threads simultaneously, POST /subscription/RegisterSubscriptionProvider with {"SubscriptionProviderSource":"RedHat","SecretArn":"<secret-arn-i>"}.
5. Observed: all 4 return HTTP 200 with distinct SubscriptionProviderArn values. Expected: all 4 return HTTP 400 with Error.Code "ValidationException".
6. POST /subscription/ListRegisteredSubscriptionProviders with {"SubscriptionProviderSources":["RedHat"]}. Observed: 4 more providers than the baseline. Expected: the baseline count, unchanged.
