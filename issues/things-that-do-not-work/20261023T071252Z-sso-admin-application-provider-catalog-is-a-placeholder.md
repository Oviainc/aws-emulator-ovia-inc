Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-catalog-list-application-providers
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-catalog-list-application-providers sso-admin-application-provider-describe-round-trip sso-admin-application-provider-not-found-describe-rejected
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|ListApplicationProviders|2xx->2xx|$.ApplicationProviders[*]
Issue-Key: sso-admin|ListApplicationProviders|2xx->2xx|$.NextToken
Issue-Key: sso-admin|DescribeApplicationProvider|2xx->2xx|$.ApplicationProvider
Issue-Key: sso-admin|DescribeApplicationProvider|2xx->2xx|$.ApplicationProviderArn
Issue-Key: sso-admin|DescribeApplicationProvider|2xx->2xx|$.DisplayData.Description
Issue-Key: sso-admin|DescribeApplicationProvider|2xx->2xx|$.DisplayData.DisplayName
Issue-Key: sso-admin|DescribeApplicationProvider|2xx->2xx|$.DisplayData.IconUrl
Issue-Key: sso-admin|DescribeApplicationProvider|2xx->2xx|$.FederationProtocol
Issue-Key: sso-admin|DescribeApplicationProvider|4xx->4xx|status
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# sso-admin application provider catalog is a placeholder: wrong providers, missing DisplayData, no pagination, 404 on unknown ARN

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListApplicationProviders

## Description
The sso-admin application-provider surface (ListApplicationProviders / DescribeApplicationProvider) is backed by a tiny hand-written placeholder catalog instead of the IAM Identity Center provider catalog, so list contents, per-ARN lookup, DisplayData completeness, pagination and the not-found status all diverge. FIVE distinct causes are bundled here; all five must be closed.

Cause 1 of 5 — ListApplicationProviders returns a 3-entry stand-in catalog instead of the real provider catalog, and every entry is missing DisplayData.Description and DisplayData.IconUrl.
AWS: Smithy ListApplicationProviders -> ListApplicationProvidersResponse.ApplicationProviders: ApplicationProviderList of ApplicationProvider{ApplicationProviderArn (required, pattern ^arn:aws(-[a-z]{1,5}){0,3}:sso::aws:applicationProvider/[a-zA-Z0-9-/]+$), FederationProtocol, DisplayData, ResourceServerConfig}; DisplayData declares DisplayName, IconUrl and Description. The model does not enumerate the catalog entries, so the catalog itself is recorded behavior only: the recording returns a large AWS-managed catalog (indices 0..393 are asserted) whose ARNs are mostly of the form arn:aws:sso::aws:applicationProvider/catalog/<Name> (e.g. .../catalog/Statuspage, .../catalog/DuoNetworkGateway, .../catalog/DruvaInSync) alongside service providers such as .../glue, and each entry carries DisplayName, Description and IconUrl, with ResourceServerConfig.Scopes on the providers that expose scopes.
Emulator: returns only three entries — arn:aws:sso::aws:applicationProvider/athena (OAUTH, DisplayName 'Amazon Athena'), .../custom (SAML, 'Custom SAML 2.0 application'), .../emr (OAUTH, 'Amazon EMR') — in an order unrelated to the recording, and no entry carries Description or IconUrl.
Why: a client enumerating providers to pick one to federate (console-style provider pickers, or any code that resolves a provider ARN by display name) finds neither the catalog/* providers nor the descriptions and icons it renders, and cannot proceed past provider selection. The absent Description/IconUrl are modeled members, so clients that read them see null where AWS always supplies a value.
Fix: the ApplicationProvider catalog seed/state behind the ListApplicationProviders handler in the ssoadmin service crate — it needs the catalog/* provider set with full DisplayData per entry, not a three-element literal.
Traces: sso-admin-catalog-list-application-providers#task#1 (expected 200/200, 394 asserted entries diverge).

Cause 2 of 5 — ListApplicationProviders never emits NextToken.
AWS: the operation is @paginated{inputToken: NextToken, outputToken: NextToken, pageSize: MaxResults, items: ApplicationProviders}; output NextToken is a Token (length 0..2048, pattern ^[-a-zA-Z0-9+=/_]*$). Recorded behavior: the response carries a NextToken (its value is shape-asserted only, so the token text is not a requirement — its presence is).
Emulator: NextToken is absent from the response.
Why: a paginating client stops after the first page and silently sees a truncated provider catalog; with the real catalog size, a client that honors MaxResults can never reach the remaining pages.
Fix: ListApplicationProviders handler/serializer — page the catalog and emit a continuation token whenever more entries remain.
Traces: sso-admin-application-provider-describe-round-trip#task#1 ($.NextToken missing).

Cause 3 of 5 — DescribeApplicationProvider ignores the requested ApplicationProviderArn and answers with an unrelated provider.
AWS: Smithy DescribeApplicationProvider.ApplicationProviderArn is @required; the response echoes ApplicationProviderArn (@required) for the provider that was asked for, plus FederationProtocol, DisplayData and ResourceServerConfig.
Emulator: asked for arn:aws:sso::aws:applicationProvider/catalog/Statuspage it returns ApplicationProviderArn arn:aws:sso::aws:applicationProvider/athena with DisplayName 'Amazon Athena' and FederationProtocol OAUTH (recording: 'Statuspage', SAML), and DisplayData.Description/IconUrl are absent. Asked for arn:aws:sso::aws:applicationProvider/custom it returns DisplayName 'Custom SAML 2.0 application' / FederationProtocol SAML where the recording has 'SSO Custom Application Provider' / OAUTH, again with no Description or IconUrl. The echoed ARN not matching the requested one shows the lookup is positional (first/arbitrary catalog entry) rather than keyed on the input ARN.
Why: a client that describes a provider before creating an application against it gets another provider's federation protocol and scopes, so it configures the application for the wrong protocol — a silent, data-corrupting wrong answer rather than an error.
Fix: DescribeApplicationProvider handler — resolve the provider by the input ApplicationProviderArn against the catalog state and serialize that entry's full DisplayData.
Traces: sso-admin-application-provider-describe-round-trip#task#2; sso-admin-application-provider-not-found-describe-rejected#task#2.

Cause 4 of 5 — DescribeApplicationProvider on an unknown provider ARN answers HTTP 404 where the wire answers HTTP 400.
AWS: ResourceNotFoundException is modeled with @httpError(404), but the recorded sso-admin wire response for an unknown application provider ARN is HTTP 400 (the error code itself matched on replay, so only the status diverges). Recorded behavior: status 400 with the not-found error code.
Emulator: HTTP 404.
Why: awsJson1_1 clients and SDK retry/error classifiers key off the HTTP status as well as __type; a 404 where AWS sends 400 is classified differently (and by some HTTP intermediaries treated as a routing failure), so error handling branches that AWS would take are never taken.
Fix: the ssoadmin error serializer / status mapping for ResourceNotFoundException on this surface — emit the status the service actually returns rather than the model's @httpError default.
Traces: sso-admin-application-provider-not-found-describe-rejected#task#1 (status: expected 400, got 404).

Cause 5 of 5 — DisplayData is serialized with DisplayName only, on both the list and the describe path.
AWS: Smithy DisplayData{DisplayName (length 1..100), IconUrl (length 1..768, pattern ^(http|https)://.*$), Description (length 1..128)}; the recording populates all three on every provider (e.g. Description 'Statuspage Application', an IconUrl under the sso static host — the host and digest in the recorded URL are environment detail, the presence of an https IconUrl and a Description are the requirement).
Emulator: emits DisplayData{DisplayName} and omits IconUrl and Description everywhere.
Why: portal-rendering and inventory clients that read Description/IconUrl get nulls for members AWS always fills, and any code that validates the response against the model's length constraints on those members has nothing to validate.
Fix: the ApplicationProvider -> DisplayData serializer (shared by both handlers) plus the catalog seed that must carry the values.
Traces: sso-admin-catalog-list-application-providers#task#1; sso-admin-application-provider-describe-round-trip#task#2; sso-admin-application-provider-not-found-describe-rejected#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (emulator_up: true, /__admin/health 200, data listener :44033; state_reset_failures: 0, so none of these ran against a prior fixture's state).

1) sso-admin-catalog-list-application-providers#task#1 ListApplicationProviders — expected 200, got 200; $.ApplicationProviders[0..393] all diverge. Expected [0] {'ApplicationProviderArn': 'arn:aws:sso::aws:applicationProvider/catalog/Statuspage', 'DisplayData': {'Description': 'Statuspage Application', 'DisplayName': 'Statuspage', 'IconUrl': 'https://static.global.sso.amazonaws.com/app-d77638588eff9057/icons/default.png'}, 'FederationProtocol': 'SAML'}; got {'ApplicationProviderArn': 'arn:aws:sso::aws:applicationProvider/athena', 'FederationProtocol': 'OAUTH', 'DisplayData': {'DisplayName': 'Amazon Athena'}}. Expected [3] is the glue provider carrying ResourceServerConfig.Scopes['glue:read_write']; the emulator has no fourth entry.

2) sso-admin-application-provider-describe-round-trip#task#1 ListApplicationProviders — expected 200, got 200; $.NextToken: missing (expected a shaped token, got None).

3) sso-admin-application-provider-describe-round-trip#task#2 DescribeApplicationProvider — expected 200, got 200; $.ApplicationProviderArn: value_mismatch (expected 'arn:aws:sso::aws:applicationProvider/catalog/Statuspage', got 'arn:aws:sso::aws:applicationProvider/athena'); $.DisplayData.DisplayName: expected 'Statuspage', got 'Amazon Athena'; $.FederationProtocol: expected 'SAML', got 'OAUTH'; $.DisplayData.Description and $.DisplayData.IconUrl: missing.

4) sso-admin-application-provider-not-found-describe-rejected#task#2 DescribeApplicationProvider — expected 200, got 200; $.DisplayData.DisplayName: expected 'SSO Custom Application Provider', got 'Custom SAML 2.0 application'; $.FederationProtocol: expected 'OAUTH', got 'SAML'; $.DisplayData.Description/$.DisplayData.IconUrl: missing.

5) sso-admin-application-provider-not-found-describe-rejected#task#1 DescribeApplicationProvider — status: expected 400, got 404 (error code matched; only the status diverged).

Model consulted: Smithy com.amazonaws.ssoadmin — DescribeApplicationProvider (input ApplicationProviderArn @required; errors ResourceNotFoundException @httpError 404, ValidationException 400), ListApplicationProviders (@paginated inputToken/outputToken NextToken, items ApplicationProviders), shapes ApplicationProvider and DisplayData{DisplayName, IconUrl, Description}.

## Steps to Reproduce
1. git clone the repo at mainline and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1.
3. Fixture sso-admin-catalog-list-application-providers, step task#1: call ListApplicationProviders with no NextToken — observe 3 entries with DisplayName only, instead of the recorded catalog of 394 entries with Description and IconUrl.
4. Fixture sso-admin-application-provider-describe-round-trip, step task#1: call ListApplicationProviders — observe no NextToken in the response body.
5. Same fixture, step task#2: call DescribeApplicationProvider with ApplicationProviderArn=arn:aws:sso::aws:applicationProvider/catalog/Statuspage — observe the response echoes .../athena with 'Amazon Athena'/OAUTH and no Description/IconUrl.
6. Fixture sso-admin-application-provider-not-found-describe-rejected, step task#1: call DescribeApplicationProvider with an application-provider ARN that is not in the catalog — observe HTTP 404 where the recording has HTTP 400.
7. Same fixture, step task#2: call DescribeApplicationProvider for arn:aws:sso::aws:applicationProvider/custom — observe the wrong DisplayName/FederationProtocol and the absent DisplayData.Description/IconUrl.
