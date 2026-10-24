Issue-Severity: LOW
Issue-Gap-Class: parameter-ignored
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# CreateDomain drops webAppSetupConfiguration.idcInstanceId: the Identity Center instance a domain is wired to is never persisted or observable

_Filed: 2026-10-24T14:32:39Z_
_Operation: CreateDomain | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateWebAppConfiguration.idcInstanceId` is @required in the model ("The Identity Center instance ID to use for creating the application"), but the handler never stores it and nothing derived from it is reported. The handler copies `ehrRole` and `idcRegion` onto the stored record and mints an unrelated `idcApplicationId` of the form `apl-<16 hex>` from a SHA-256 of a counter, so two CreateDomain calls that name two DIFFERENT Identity Center instances produce indistinguishable `webAppConfiguration` payloads, and neither CreateDomain nor GetDomain lets a caller see which instance the domain's web application actually lives in. Expected: the instance the caller named governs the application identifier the service reports — in IAM Identity Center an application is identified within its instance (ids/ARNs of the form arn:aws:sso::<account>:application/<instanceId>/apl-<suffix>), so the reported `idcApplicationId` is derived from the supplied instance and a domain created against instance A is distinguishable from one created against instance B. Actual: the required input member is silently discarded at the boundary of the state model.

## Evidence

Spec: smithy_query shape=CreateWebAppConfiguration marks `idcInstanceId` (String, len 1..256) @required; shape=WebAppConfiguration declares the @required `idcApplicationId` the service reports back. Handler: `src/ops/create_domain.rs:284-291` builds `crate::state::WebAppConfigurationRecord { ehr_role, idc_application_id: store.mint_idc_application_id(), idc_region }` — `setup.idc_instance_id` is never read; `src/state.rs:230-235` (`WebAppConfigurationRecord`) has no field for it and `src/state.rs:673-677` (`mint_idc_application_id`) mints `apl-` + 16 hex independent of any input. Test coverage: the web-app projection tests assert only that the minted application id is non-empty/stable (`tests/all/domain_response_projection.rs`), none ties it to the requested instance, so no test asserts this.

## Steps to Reproduce

1. POST /domain with {"name":"a","webAppSetupConfiguration":{"ehrRole":"arn:aws:iam::123456789012:role/ehr","idcInstanceId":"ssoins-1111111111111111","idcRegion":"us-east-1"}}.
2. POST /domain again with the same body but "idcInstanceId":"ssoins-2222222222222222".
3. Compare the two `webAppConfiguration` objects in the 201 responses (and via GET /domain/{domainId}): both carry an `apl-...` id unrelated to the instance supplied, and nothing in any response distinguishes the two instances — the required `idcInstanceId` is unobservable.
