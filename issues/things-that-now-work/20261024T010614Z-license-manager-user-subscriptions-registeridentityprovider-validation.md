Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# RegisterIdentityProvider rejects a request that omits the model-optional Settings member

_Filed: 2026-10-24T01:06:14Z_
_Operation: RegisterIdentityProvider | Class: validation_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler refuses any registration with no `Settings` payload: `let Some(settings) = input.settings.clone() else { return Err(validation("Settings is required to register an identity provider ...")) }`.

Expected: `Settings` is NOT `@required` in `RegisterIdentityProviderRequest` (the model marks only `IdentityProvider` and `Product` required), so a request that omits it must be accepted. This matters for the RDS SAL flow the service added `CreateLicenseServerEndpoint` for: a `REMOTE_DESKTOP_SERVICES` registration whose licensing goes through a license server endpoint needs no subnets/security group for activation-server VPC endpoints, which is exactly why the member is optional.

Actual: HTTP 400 `ValidationException` for every Settings-less registration, so the documented register -> CreateLicenseServerEndpoint workflow cannot be driven without inventing subnet and security-group values the caller does not have. The emulator's justification (the output member `IdentityProviderSummary$Settings` is `@required`) is an internal modelling problem, not a reason to refuse a request the input model accepts; the record should persist whatever the caller supplied (defaulting the stored settings) instead of rejecting the call.

## Evidence

Spec: smithy_query(query="shape", shape="RegisterIdentityProviderRequest") shows `Settings` with NO `required: true` (only `IdentityProvider` and `Product` are required), doc "The registered identity provider's product related configuration settings such as the subnets to provision VPC endpoints." Handler: `src/ops/register_identity_provider.rs` lines 166-173. Test coverage: `tests/all/identity_provider_subscriptions.rs::a_registration_without_settings_is_refused` currently PINS the over-rejection, so the divergence is deliberate and asserted — that test would need to change with the behaviour.

## Steps to Reproduce

1. Call RegisterIdentityProvider with IdentityProvider.ActiveDirectoryIdentityProvider.DirectoryId=d-9f8e7d6c5b and Product=REMOTE_DESKTOP_SERVICES, and NO Settings member.
2. Observe HTTP 400 ValidationException ('Settings is required to register an identity provider ...').
3. Expected: HTTP 200 with an IdentityProviderSummary for the new registration (the model declares Settings optional), after which CreateLicenseServerEndpoint for that IdentityProviderArn can be driven.
