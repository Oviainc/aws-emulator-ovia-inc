Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 671706971a680016eadecf6b3438021fa34d8a56

# CreateApplication accepts SAML 2.0 and AWS managed application providers that the operation documents as unsupported

_Filed: 2026-10-24T09:17:49Z_
_Operation: CreateApplication | Class: validation_

> _Note: this issue was identified against the code base at commit `671706971a680016eadecf6b3438021fa34d8a56`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `CreateApplication` creates "an OAuth 2.0 customer managed application" only. Its own documentation states the limit explicitly: "This API does not support creating SAML 2.0 customer managed applications or Amazon Web Services managed applications. ... You can create a SAML 2.0 customer managed application in the Amazon Web Services Management Console only." So a request whose `ApplicationProviderArn` names a provider that federates over SAML (every `catalog/<Name>` entry of the pre-integrated application catalog, which this emulator publishes with `FederationProtocol: SAML`), or an AWS managed service provider whose applications the service creates on the customer's behalf, must be refused with a client error (`ValidationException`) rather than minting an application.

Actual: the handler checks only that the ARN resolves to *some* entry of the published catalog and then stores the application under it, explicitly declining to narrow the create by provider ("Which provider it is does not narrow the create further ... so none is invented here"). As a result `CreateApplication` with `ApplicationProviderArn: arn:aws:sso::aws:applicationProvider/catalog/Slack` answers 200 with a fresh `ApplicationArn`, and `DescribeApplication` then reports a customer managed OAuth-shaped application (with `PortalOptions`/`SignInOptions` and `Status`) sitting under a SAML-only provider — a resource real AWS will not let this API create. The same holds for the AWS managed service providers in the catalog (`athena`, `glue`, `redshift`, `s3`, ...), whose applications are AWS managed ones; `custom` is the provider a customer managed application is created under.

## Evidence

Spec: the `CreateApplication` documentation in the Smithy model (reproduced in the handler doc block, create_application.rs lines 57-63) — "Creates an OAuth 2.0 customer managed application in IAM Identity Center for the given application provider. This API does not support creating SAML 2.0 customer managed applications or Amazon Web Services managed applications." The model also declares `ValidationException` (HTTP 400) on the operation, so the refusal channel exists.

Handler: services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/create_application.rs lines 228-249 — the only provider check is `built_in_application_provider(...).is_none()` -> `ResourceNotFoundException`; the comment at lines 233-237 states the decision not to refuse any particular provider. The catalog these ARNs resolve against is src/state.rs `BUILT_IN_APPLICATION_PROVIDERS` (lines 1532-1718), where `catalog_application(...)` entries are built with `federation_protocol: FEDERATION_SAML` (lines 1493-1504) and `aws_managed_provider(...)` entries with `FEDERATION_OAUTH`; `custom` is the one "SSO Custom Application Provider" entry (line 1534).

Test coverage: no test asserts a refusal for a SAML/AWS-managed provider. The opposite is asserted — tests/all/application_assignments.rs line 49 defines `CATALOG_PROVIDER_ARN = "arn:aws:sso::aws:applicationProvider/catalog/Slack"` and exercises a *successful* create under it "to show the create is not keyed to one entry of the catalog", so that expectation changes with the handler. The only provider refusal covered today is an ARN outside the catalog (application_assignments.rs lines 677-689).

## Steps to Reproduce

1. Start the emulator and read the account's instance ARN with `X-Amz-Target: SWBExternalService.ListInstances`.
2. POST / with `X-Amz-Target: SWBExternalService.CreateApplication` and body `{"InstanceArn":"<instance>","ApplicationProviderArn":"arn:aws:sso::aws:applicationProvider/catalog/Slack","Name":"saml-app"}`.
3. Observe 200 with a minted `ApplicationArn`, where AWS refuses the request because this API does not create SAML 2.0 customer managed applications; confirm the provider is SAML with `DescribeApplicationProvider` on the same ARN (`FederationProtocol: SAML`).
4. Repeat with an AWS managed service provider (e.g. `arn:aws:sso::aws:applicationProvider/glue`) and observe the same unrefused 200 for an Amazon Web Services managed application.
5. Call `DescribeApplication` on the returned ARN and see the stored OAuth-shaped application that could not exist in AWS.
