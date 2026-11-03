Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-application-provider-describe-round-trip
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-application-provider-describe-round-trip sso-admin-application-provider-not-found-describe-rejected sso-admin-catalog-list-application-providers
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|DescribeApplicationProvider|2xx->2xx|$.ApplicationProvider
Issue-Key: sso-admin|DescribeApplicationProvider|2xx->2xx|$.ApplicationProviderArn
Issue-Key: sso-admin|DescribeApplicationProvider|2xx->2xx|$.DisplayData.Description
Issue-Key: sso-admin|DescribeApplicationProvider|2xx->2xx|$.DisplayData.DisplayName
Issue-Key: sso-admin|DescribeApplicationProvider|2xx->2xx|$.DisplayData.IconUrl
Issue-Key: sso-admin|DescribeApplicationProvider|2xx->2xx|$.FederationProtocol
Issue-Key: sso-admin|ListApplicationProviders|2xx->2xx|$.ApplicationProviders[*]
Issue-Commit: 5dbb2ab27a0402c002f5157a9a56e61b81adc0a2

# Application provider catalog is wrong: Describe ignores the requested ARN, entries carry wrong DisplayData, and List serves a different catalog in a different order

> _Note: this issue was identified against the code base at commit `5dbb2ab27a0402c002f5157a9a56e61b81adc0a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeApplicationProvider

## Description
THREE causes, all rooted in the sso-admin application-provider catalog (its seed data, its lookup and its serializer). A fixer must close all three before submitting.

Cause 1 of 3 - DescribeApplicationProvider answers with a provider other than the one requested (fixture sso-admin-application-provider-describe-round-trip, step task#2).
AWS: the operation takes a required ApplicationProviderArn (Smithy: DescribeApplicationProvider.ApplicationProviderArn @required, @length 10..1224, @pattern ^arn:aws(-[a-z]{1,5}){0,3}:sso::aws:applicationProvider/[a-zA-Z0-9-/]+$) and returns THAT provider's entry: the echoed ApplicationProviderArn (@required), FederationProtocol, DisplayData, and - recorded behavior only, the on-disk DescribeApplicationProviderResponse has no such member - an additional `ApplicationProvider` structure repeating ApplicationProviderArn/FederationProtocol/DisplayData. The AWS catalog contains arn:aws:sso::aws:applicationProvider/catalog/Statuspage, a SAML provider whose DisplayData is DisplayName "Statuspage", Description "Statuspage Application" and an icon path of its own. An ARN naming no provider is refused with the modelled ResourceNotFoundException (Smithy: ResourceNotFoundException, httpError 404).
Emulator: for a request naming .../applicationProvider/catalog/Statuspage it answered 200 describing an unrelated provider - ApplicationProviderArn "arn:aws:sso::aws:applicationProvider/athena", DisplayName "Amazon Athena", Description "Interactive query service for data in Amazon S3", FederationProtocol "OAUTH" - and omitted $.ApplicationProvider entirely (expected an object, got None). Hypothesis: the lookup falls back to the first entry of its catalog instead of matching the requested ARN, so no ARN can ever miss.
Why: a client resolving a provider ARN before CreateApplication/PutApplicationAuthenticationMethod is handed another provider's federation protocol and display data under the ARN it asked for, and a client probing an unknown ARN never sees the 404 ResourceNotFoundException it must handle.
Fix: the DescribeApplicationProvider handler's catalog lookup (match the requested ARN exactly; refuse an unknown ARN with ResourceNotFoundException) and its response serializer (emit the ApplicationProvider envelope beside the flat members).
Traces: sso-admin-application-provider-describe-round-trip#task#2.

Cause 2 of 3 - a provider that IS found is described with the wrong DisplayData (fixture sso-admin-application-provider-not-found-describe-rejected, step task#2).
AWS: for arn:aws:sso::aws:applicationProvider/custom the response's DisplayData.Description is the provider's display name followed by " Application" - "SSO Custom Application Provider Application" - the same rule the catalog/* entries follow, and DisplayData.IconUrl is that provider's own static catalog path of the form https://static.global.sso.amazonaws.com/app-<provider-icon-id>/icons/default.png. Recorded behavior only: the model only constrains DisplayData.Description @length 1..128 and DisplayData.IconUrl @pattern ^(http|https)://.*$. The icon id is fixed per provider in AWS's catalog - it is service data, not a per-run value.
Emulator: 200 whose DisplayData.Description is the bare display name "SSO Custom Application Provider" (the " Application" suffix dropped for the non-catalog providers) and whose IconUrl carries a locally derived app-<id> segment instead of that provider's catalog icon id; $.ApplicationProvider is again absent.
Why: a client that renders or asserts a provider's portal description/icon reads a different string for the same ARN than AWS serves, so display-data round-trips cannot be verified against the emulator.
Fix: the application-provider catalog seed data plus the shared DisplayData builder that composes Description from DisplayName.
Traces: sso-admin-application-provider-not-found-describe-rejected#task#2.

Cause 3 of 3 - ListApplicationProviders enumerates a different catalog, in a different order (fixture sso-admin-catalog-list-application-providers, step task#1).
AWS: ApplicationProviders is returned in the service's own catalog order, which is not an alphabetical sort of the ARNs; the recording's first three members are arn:aws:sso::aws:applicationProvider/catalog/Statuspage, .../catalog/DuoNetworkGateway and .../catalog/DruvaInSync, each SAML with DisplayData.Description = "<DisplayName> Application". Recorded behavior only: ListApplicationProviders carries no ordering trait in the model (Smithy: @paginated items ApplicationProviders, NextToken).
Emulator: 200 whose membership and order both differ at every index - $.ApplicationProviders[0] is "arn:aws:sso::aws:applicationProvider/athena" (OAUTH, a provider the recording's catalog does not list at that position), followed by .../catalog/15Five and .../catalog/AbsorbLMS, i.e. the non-catalog providers hoisted ahead of an alphabetically sorted catalog. All 394 served indices (0..393) diverged from the expected entries.
Why: a client that pages this list or picks providers by index/first-match gets a different provider set and a different first page than AWS returns, and the entry it then passes to DescribeApplicationProvider/CreateApplication does not exist in AWS's catalog.
Fix: the application-provider catalog seed and the ordering ListApplicationProviders pages over - the same catalog the DescribeApplicationProvider lookup in Cause 1 must read.
Traces: sso-admin-catalog-list-application-providers#task#1.

No open sso-admin issue covers these: the nearest neighbours are 20261028T152940Z-sso-admin-listapplications-error-response.md (ListApplications' error status, not the provider catalog) and 20261029T112447Z-sso-admin-listinstances-pagination.md (page-token decoding).

## Evidence
Cause 1 - DescribeApplicationProvider, fixture sso-admin-application-provider-describe-round-trip step task#2, expected 200 / actual 200:
  $.ApplicationProvider: expected {'ApplicationProviderArn': 'arn:aws:sso::aws:applicationProvider/catalog/Statuspage', 'DisplayData': {'Description': 'Statuspage Application', 'DisplayName': 'Statuspage', 'IconUrl': 'https://static.global.sso.amazonaws.com/app-d77638588eff9057/icons/default.png'}, 'FederationProtocol': 'SAML'}, got None
  $.ApplicationProviderArn: expected 'arn:aws:sso::aws:applicationProvider/catalog/Statuspage', got 'arn:aws:sso::aws:applicationProvider/athena'
  $.DisplayData.DisplayName: expected 'Statuspage', got 'Amazon Athena'
  $.DisplayData.Description: expected 'Statuspage Application', got 'Interactive query service for data in Amazon S3'
  $.FederationProtocol: expected 'SAML', got 'OAUTH'

Cause 2 - DescribeApplicationProvider, fixture sso-admin-application-provider-not-found-describe-rejected step task#2, expected 200 / actual 200:
  $.ApplicationProvider: expected {'ApplicationProviderArn': 'arn:aws:sso::aws:applicationProvider/custom', 'DisplayData': {'Description': 'SSO Custom Application Provider Application', 'DisplayName': 'SSO Custom Application Provider', 'IconUrl': 'https://static.global.sso.amazonaws.com/app-efbe0dd582ec4f87/icons/default.png'}, 'FederationProtocol': 'OAUTH'}, got None
  $.DisplayData.Description: expected 'SSO Custom Application Provider Application', got 'SSO Custom Application Provider'
  $.DisplayData.IconUrl: expected '.../app-efbe0dd582ec4f87/icons/default.png', got '.../app-6cdfd271da635d49/icons/default.png'

Cause 3 - ListApplicationProviders, fixture sso-admin-catalog-list-application-providers step task#1, expected 200 / actual 200:
  $.ApplicationProviders[0]: expected catalog/Statuspage (SAML, 'Statuspage Application'), got arn:aws:sso::aws:applicationProvider/athena (OAUTH)
  $.ApplicationProviders[1]: expected catalog/DuoNetworkGateway, got catalog/15Five
  $.ApplicationProviders[2]: expected catalog/DruvaInSync, got catalog/AbsorbLMS
  ... every index through $.ApplicationProviders[393] reported mismatched.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the sso-admin wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture sso-admin-application-provider-describe-round-trip, step task#2 - DescribeApplicationProvider with ApplicationProviderArn=arn:aws:sso::aws:applicationProvider/catalog/Statuspage. Observe the 200 body describes .../applicationProvider/athena and carries no ApplicationProvider member.
3. Cause 2: replay fixture sso-admin-application-provider-not-found-describe-rejected, step task#2 - DescribeApplicationProvider with ApplicationProviderArn=arn:aws:sso::aws:applicationProvider/custom. Observe DisplayData.Description lacks the ' Application' suffix and IconUrl names a different app-<id>.
4. Cause 3: replay fixture sso-admin-catalog-list-application-providers, step task#1 - ListApplicationProviders. Compare $.ApplicationProviders[0..393] against the recorded catalog order.
