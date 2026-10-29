Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: license-manager-linux-subscriptions.register-duplicate-source-rejected license-manager-linux-subscriptions.register-invalid-secret-arn-pattern-rejected license-manager-linux-subscriptions.register-subscription-provider-pending-status license-manager-linux-subscriptions.tag-resource-exceeds-limit-rejected
Issue-Shape-Id: com.amazonaws.licensemanagerlinuxsubscriptions#LicenseManagerLinuxSubscriptions
Issue-Key: license-manager-linux-subscriptions.register-duplicate-source-rejected|run#12|AssertionError|assert valid_source == _TOKEN_REJECTED, (
Issue-Key: license-manager-linux-subscriptions.register-invalid-secret-arn-pattern-rejected|run#11|AssertionError|assert token_rejected == _TOKEN_REJECTED, (
Issue-Key: license-manager-linux-subscriptions.register-subscription-provider-pending-status|run#8|AssertionError|assert (code, message) == _TOKEN_REJECTED, (
Issue-Key: license-manager-linux-subscriptions.tag-resource-exceeds-limit-rejected|run#7|AssertionError|assert (control_code, control_message) == _TOKEN_REJECTED, (
Issue-Commit: c1b8e21acf710dd592fea1840f63adefca5c3a99

# RegisterSubscriptionProvider's token-validation refusal carries an emulator-authored message instead of AWS's "The subscription provider token is invalid or expired"

> _Note: this issue was identified against the code base at commit `c1b8e21acf710dd592fea1840f63adefca5c3a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterSubscriptionProvider

## Description
REOPENS 20261024T112314Z-license-manager-linux-subscriptions-registersubscriptionprovider-accepts-a-secret-holding-a-non-genuine.md: its fix left the call refused. That issue required RegisterSubscriptionProvider to refuse a secret that does not hold a genuine Red Hat offline token, and the emulator now does refuse with HTTP 400 / ValidationException — but it refuses with a message it writes itself rather than the service's wording, so every check that uses the token refusal to tell one rejection class from another now mismatches. This is not a new defect, it is the wording half of that fix left undone.

Cause 1 of 1: the Red Hat offline-token refusal message is emulator-authored.

Failing check ids: license-manager-linux-subscriptions.register-duplicate-source-rejected, license-manager-linux-subscriptions.register-invalid-secret-arn-pattern-rejected, license-manager-linux-subscriptions.register-subscription-provider-pending-status, license-manager-linux-subscriptions.tag-resource-exceeds-limit-rejected

AWS: Smithy (services/service-com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions) — RegisterSubscriptionProvider, POST /subscription/RegisterSubscriptionProvider, @idempotent; input SubscriptionProviderSource (required, enum) and SecretArn (required, @pattern ^arn:[a-z0-9-\.]{1,63}:secretsmanager:[a-z0-9-\.]{0,63}:[a-z0-9-\.]{0,63}:secret:[^/]{1,1023}$); modeled errors InternalServerException (500), ThrottlingException (400), ValidationException (400, client). The model does not state the credential check or its text, so for the message: recorded behavior only — when SubscriptionProviderSource is the one supported value "RedHat" and SecretArn names a well-formed, existing Secrets Manager secret whose SecretString is not a genuine Red Hat offline token, the recording shows HTTP 400, Error.Code "ValidationException" and Error.Message exactly "The subscription provider token is invalid or expired". That message is entirely FIXED text: it has no variable part — no secret name, no ARN, no source value, no limit is interpolated into it. The recordings show this same refusal reached from four independent request shapes (the enum-domain check's positive control, the ARN-pattern check's positive control, the pending-status path, and the tag-limit check's control), i.e. it is the single refusal AWS gives once the request shape and the enum value are accepted and only the credential is bad.

Emulator: the same requests are refused with HTTP 400 and the correct Error.Code "ValidationException", but Error.Message is "The secret referenced by SecretArn does not contain a Red Hat offline token: it is not a JSON Web Token (three '.'-separated base64url segments). Store your Red Hat offline token in that secret and try your request again." That sentence appears nowhere in AWS's responses; it also leaks an implementation rule (the emulator decides the token is bad because it is not three '.'-separated base64url segments) that the service does not publish, and it names a cause ("does not contain") narrower than the service's verdict ("invalid or expired"). All four failing checks assert on (Code, Message) and report this one string.

Why: a client cannot distinguish refusal classes on this operation. The three distinct refusals the service gives here — the enum-domain violation ("1 validation error detected: Value 'Ubuntu' at 'subscriptionProviderSource' failed to satisfy constraint: Member must satisfy enum value set: [RedHat]"), the SecretArn pattern violation, and the credential refusal — all share error code ValidationException, so the message is the only discriminator. Client code and tests that branch on "The subscription provider token is invalid or expired" (re-prompt the operator for a fresh offline token, versus fix the request) take the wrong branch or fall through to a generic handler against the emulator, and conversely anything written against the emulator's sentence breaks on AWS. Four checks that exercise unrelated behaviors (enum domain, ARN pattern, provider status, tag limit) all use this refusal as their positive control, so the wrong wording also masks whatever those checks were meant to prove.

Fix: the operation's validation in the RegisterSubscriptionProvider handler — the credential/offline-token rejection path must emit the service's fixed sentence "The subscription provider token is invalid or expired" as the ValidationException Message (status 400 and code ValidationException are already correct), and must not substitute a diagnosis of its own token-shape heuristic. The enum-domain and SecretArn-pattern refusals on the same operation must keep their own distinct messages so the three classes stay separable.

Traces: license-manager-linux-subscriptions.register-duplicate-source-rejected (positive control: source "RedHat" + fake-token secret, after 'Ubuntu' is correctly rejected for the enum); license-manager-linux-subscriptions.register-invalid-secret-arn-pattern-rejected (positive control: well-formed ARN + fake token); license-manager-linux-subscriptions.register-subscription-provider-pending-status (syntactically valid secret holding a non-genuine token); license-manager-linux-subscriptions.tag-resource-exceeds-limit-rejected (control call with valid-sized Tags + non-genuine token).

## Evidence
All four checks fail on the same (Code, Message) comparison against RegisterSubscriptionProvider.

Expected (recorded AWS behavior), identical in all four: HTTP 400, Error.Code == "ValidationException", Error.Message == "The subscription provider token is invalid or expired" (fixed text, no interpolated values).

Actual (emulator), identical in all four: HTTP 400, Error.Code == "ValidationException", Error.Message == "The secret referenced by SecretArn does not contain a Red Hat offline token: it is not a JSON Web Token (three '.'-separated base64url segments). Store your Red Hat offline token in that secret and try your request again."

Replay assertions:
1) license-manager-linux-subscriptions.register-duplicate-source-rejected, check line 62:
   AssertionError: expected ('ValidationException', 'The subscription provider token is invalid or expired') for a fake RedHat token (a token rejection, not the enum-constraint one), got ('ValidationException', "The secret referenced by SecretArn does not contain a Red Hat offline token: ...")
2) license-manager-linux-subscriptions.register-invalid-secret-arn-pattern-rejected, check line 53:
   AssertionError: expected the token-validation rejection ('ValidationException', 'The subscription provider token is invalid or expired'), got ('ValidationException', "The secret referenced by SecretArn does not contain a Red Hat offline token: ...")
3) license-manager-linux-subscriptions.register-subscription-provider-pending-status, check line 43:
   AssertionError: expected ('ValidationException', 'The subscription provider token is invalid or expired') for a syntactically-valid secret holding a non-genuine RedHat token (the token named as the problem, distinct from an ARN-pattern or request-shape rejection), got code='ValidationException' message="The secret referenced by SecretArn does not contain a Red Hat offline token: ..."
4) license-manager-linux-subscriptions.tag-resource-exceeds-limit-rejected, check line 38:
   AssertionError: control call (valid-sized Tags, non-genuine token) expected ('ValidationException', 'The subscription provider token is invalid or expired'), got ('ValidationException', "The secret referenced by SecretArn does not contain a Red Hat offline token: ...")

Positive controls inside these checks passed before the failing leg: the account onboarded (LinuxSubscriptionsDiscovery Enabled, Status not InProgress), ListRegisteredSubscriptionProviders answered a well-formed list, and the enum-domain rejection of SubscriptionProviderSource="Ubuntu" matched AWS exactly — so the account is healthy and the divergence is confined to this one refusal message.

Model citation: Smithy RegisterSubscriptionProvider — errors InternalServerException (server, 500), ThrottlingException (client, 400), ValidationException (client, 400); SecretArn @pattern as quoted; http POST /subscription/RegisterSubscriptionProvider, @idempotent. The model does not specify refusal message text; the sentence above is taken from the recorded response.

## Steps to Reproduce
1. Onboard the account: GetServiceSettings; if LinuxSubscriptionsDiscovery is not Enabled with a settled Status, create the service-linked role for license-manager-linux-subscriptions.amazonaws.com and call UpdateServiceSettings(LinuxSubscriptionsDiscovery="Enabled", LinuxSubscriptionsDiscoverySettings={OrganizationIntegration:"Disabled", SourceRegions:[<region>]}, AllowUpdate=true); poll GetServiceSettings until Status != "InProgress".
2. Create a Secrets Manager secret whose SecretString is any non-genuine value (e.g. "fake-redhat-offline-token-duplicate-source-test"), tagged LicenseManagerLinuxSubscriptions=enabled. Note its ARN.
3. POST /subscription/RegisterSubscriptionProvider with {"SubscriptionProviderSource":"RedHat","SecretArn":"<that-arn>"}.
4. Observed: HTTP 400, Error.Code "ValidationException", Error.Message "The secret referenced by SecretArn does not contain a Red Hat offline token: it is not a JSON Web Token (three '.'-separated base64url segments). Store your Red Hat offline token in that secret and try your request again."
   Expected: HTTP 400, Error.Code "ValidationException", Error.Message "The subscription provider token is invalid or expired".
5. Repeat step 3 with a SecretString that IS three '.'-separated base64url segments but is still not a genuine offline token (e.g. "eyJhbGciOiJub25lIn0.eyJzdWIiOiJ4In0.sig"): AWS still answers the same "The subscription provider token is invalid or expired" refusal, so the emulator's JWT-shape heuristic is not the service's rule.
6. Confirm the discriminating refusals on the same operation are still distinct: POST with {"SubscriptionProviderSource":"Ubuntu","SecretArn":"arn:aws:secretsmanager:<region>:<account>:secret:<name>"} must keep answering "1 validation error detected: Value 'Ubuntu' at 'subscriptionProviderSource' failed to satisfy constraint: Member must satisfy enum value set: [RedHat]", and a SecretArn violating the modeled pattern must keep its own pattern message.
7. POST /subscription/ListRegisteredSubscriptionProviders with {"SubscriptionProviderSources":["RedHat"]} and confirm the count is unchanged (no provider may be left behind by the refused call) — this part already holds.
