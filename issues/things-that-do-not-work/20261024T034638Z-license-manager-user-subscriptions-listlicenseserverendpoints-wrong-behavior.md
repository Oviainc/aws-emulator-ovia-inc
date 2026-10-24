Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ba63228bbc1032132c1fbdbb0b54a8c844ce19f7

# LicenseServerEndpoint.LicenseServers is never populated, even for a PROVISIONED RDS_SAL endpoint

_Filed: 2026-10-24T03:46:38Z_
_Operation: ListLicenseServerEndpoints | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ba63228bbc1032132c1fbdbb0b54a8c844ce19f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every `LicenseServerEndpoint` this service reports is built by `crate::state::license_server_endpoint`, which hardcodes `license_servers: None`. The member is therefore absent from every reply of `ListLicenseServerEndpoints` (and of `DeleteLicenseServerEndpoint`), whatever the endpoint's provisioning status.

Expected: once an RDS_SAL license server endpoint reaches `PROVISIONED`, the endpoint resource reports the license servers reachable through it — `LicenseServers` is an array of `LicenseServer` resources, each carrying `ProvisioningStatus`, `HealthStatus` (HEALTHY | UNHEALTHY | NOT_APPLICABLE) and `Ipv4Address`/`Ipv6Address`. A client that polls the list to decide whether the RDS SAL license servers behind the endpoint are healthy has something to read.

Actual: `LicenseServers` is absent in every reply, at every provisioning status, so the health of the license servers behind a provisioned endpoint is never observable. The emulator already computes the sibling `ServerEndpoint` address from state once the endpoint is PROVISIONED, so the data model has a place for the same transition to materialise license servers; it simply never does.

## Evidence

Spec: `LicenseServerEndpoint$LicenseServers` -> `LicenseServerList` of `LicenseServer` — "An array of LicenseServer resources that represent the license servers that are accessed through this endpoint" (smithy_query shape=LicenseServerEndpoint,LicenseServer), with `LicenseServerHealthStatus` values HEALTHY/UNHEALTHY/NOT_APPLICABLE, i.e. a health report the real service computes for a provisioned endpoint. Handler/state: `src/ops/list_license_server_endpoints.rs` maps every record through `crate::state::license_server_endpoint`, which sets `license_servers: None` unconditionally (`src/state.rs`, the `license_server_endpoint` builder, with the comment "LicenseServers is left absent rather than invented: this emulator provisions no license-server hosts"); `LicenseServerEndpointRecord` holds no license-server field at all. Tests: `tests/all/license_server_endpoints.rs` asserts ARNs, ServerType, provisioning status and the address transition but never looks at `LicenseServers`; `tests/all/resource_resolution_refusals.rs` (around line 1171) asserts `LicenseServers` is ABSENT on the delete reply — i.e. the current behaviour is pinned, and no test asserts the expected report.

## Steps to Reproduce

1. RegisterIdentityProvider for an Active Directory provider with Product=REMOTE_DESKTOP_SERVICES and Settings naming a subnet and a security group.
2. CreateLicenseServerEndpoint with that IdentityProviderArn and LicenseServerSettings.ServerType=RDS_SAL (ServerSettings.RdsSalSettings.RdsSalCredentialsProvider naming a Secrets Manager secret).
3. Poll ListLicenseServerEndpoints until LicenseServerEndpointProvisioningStatus is PROVISIONED — ServerEndpoint.Endpoint is now reported.
4. Inspect the same listed endpoint: LicenseServers is absent. Expected: an array of LicenseServer entries carrying ProvisioningStatus, HealthStatus and an Ipv4Address/Ipv6Address for the servers behind the endpoint.
