Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5bfe48ddd26ea9415acafc303bdb706a28d090a8

# Intune connector's OpenIdConfiguration is synthesized by echoing the request: Audience is the caller's Entra tenant ID and Issuer is the SCEP URL

_Filed: 2026-10-24T07:31:41Z_
_Operation: GetConnector | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5bfe48ddd26ea9415acafc303bdb706a28d090a8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

For a connector created with an `IntuneConfiguration`, `GetConnector` (and `ListConnectors`) must report the OIDC federation parameters the service publishes so the caller can paste them into a Microsoft Entra ID app registration's federated credential, as the `OpenIdConfiguration` doc instructs ("Contains OpenID Connect (OIDC) parameters for use with Connector for SCEP for Microsoft Intune").

Expected (real service): `OpenIdConfiguration.Audience` is the Entra token-exchange audience the AWS-issued token must carry for Microsoft Entra workload identity federation (`api://AzureADTokenExchange`), `Issuer` is an AWS-hosted OIDC issuer URL (an OIDC discovery/issuer endpoint, not the connector's SCEP service URL), and `Subject` is the subject identifier AWS mints inside that issuer's namespace. All three are service-published values the caller never supplied.

Actual (emulator): all three members are derived from the request itself — `Audience` is set verbatim to the request's `IntuneConfiguration.AzureApplicationId`, `Issuer` to the connector's SCEP endpoint URL, and `Subject` to the connector's own ARN (`src/state.rs::open_id_configuration`). Because `AzureApplicationId` is documented as "The directory (tenant) ID from your Microsoft Entra ID app registration", the reported audience is a caller-supplied tenant ID — which is never a valid OIDC audience for the federation this integration configures — and the issuer is the SCEP data-plane URL rather than an OIDC issuer. The response is therefore an echo of input and the values cannot be used to configure Intune at all.

## Evidence

Smithy model: `Connector$OpenIdConfiguration` / `ConnectorSummary$OpenIdConfiguration` doc = "Contains OpenID Connect (OIDC) parameters for use with Connector for SCEP for Microsoft Intune"; `IntuneConfiguration$AzureApplicationId` doc = "The directory (tenant) ID from your Microsoft Entra ID app registration" (so it is caller input, not a service-minted audience). Handler/state: `services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/state.rs` `open_id_configuration()` returns `issuer: endpoint`, `subject: connector_arn`, `audience: intune.azure_application_id`, called from `src/ops/create_connector.rs` (~line 280) and surfaced unchanged by `StoredConnector::to_connector()` / `to_summary()` which `src/ops/get_connector.rs` and `src/ops/list_connectors.rs` return. Well-established behavior: Microsoft Entra workload identity federation requires the exchanged token's audience to be `api://AzureADTokenExchange`, and the AWS Private CA "Using Connector for SCEP for Microsoft Intune" flow has the customer copy the connector's published Issuer/Subject/Audience into the Entra federated credential. Test coverage: `tests/all/connector_lifecycle.rs` (created_connector_is_read_back_from_state) only asserts that the three members round-trip the values the emulator itself derived — no test asserts service-published OIDC values, so this is uncovered.

## Steps to Reproduce

1. POST /connectors with {"CertificateAuthorityArn": "arn:aws:acm-pca:us-east-1:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555", "MobileDeviceManagement": {"Intune": {"AzureApplicationId": "12345678-abcd-1234-abcd-123456789012", "Domain": "example.onmicrosoft.com"}}} -> 202 with a ConnectorArn.
2. GET /connectors/{ConnectorArn} and read Connector.OpenIdConfiguration.
3. Observe Audience == the AzureApplicationId sent in step 1 (the Entra tenant ID) instead of the Entra token-exchange audience, Issuer == the connector's SCEP endpoint URL instead of an AWS OIDC issuer URL, and Subject == the connector ARN.
4. GET /connectors (ListConnectors) and observe the same echoed values in the ConnectorSummary.
