Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 87ef04f300fd59d35c2e48965ae7f4a968416820

# A repeat CreateLicenseServerEndpoint silently drops the LicenseServerSettings it carries and answers 200 with the endpoint minted from the earlier settings

_Filed: 2026-10-29T07:15:56Z_
_Operation: CreateLicenseServerEndpoint | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `87ef04f300fd59d35c2e48965ae7f4a968416820`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler's idempotency resolution keys ONLY on `(IdentityProviderArn, ServerType)` and returns before the request's `LicenseServerSettings` is read at all. `LicenseServerSettings$ServerSettings` is `@required` and carries the credentials provider for license-server user administration (`RdsSalSettings$RdsSalCredentialsProvider`, also `@required`), yet on the repeat path that member is neither compared with the stored endpoint's nor stored: the call answers HTTP 200 with the existing `LicenseServerEndpointArn` while `license_server_endpoints[arn].settings` keeps whatever the create that MINTED the record supplied. A client that re-creates the endpoint with a different (e.g. rotated) secret is told it worked and nothing changed.

This contradicts the rule this crate states in ONE place for every resolution of one of its own resources (`crate::state::names_other`: every member a request names is part of WHICH resource it names, never "a comment on the resource"), and it contradicts the sibling handler for the very same resource: `DeleteLicenseServerEndpoint` cross-checks the one other member its request carries (`ServerType`) against the stored endpoint and fails closed when it differs. The create handler's own comment on the branch admits only the `Tags` consequence and is silent about `ServerSettings`, so the required credentials payload is dropped with no rule stated for it. Four already-fixed issues in this crate established the same rule for the sibling resolutions (DeregisterIdentityProvider/DisassociateUser/StopProductSubscription "ignores X when the request also carries the ARN").

## Evidence

The resolution reads two members and returns — src/ops/create_license_server_endpoint.rs:195-206:
```
        // `@idempotent`: a repeat create for the same (identity provider, server type)
        // answers with the endpoint already in state instead of minting a second one.
        // The endpoint's tags are therefore whatever the create that MINTED the record
        // supplied; a repeat request's `Tags` map does not re-tag it, because the repeat
        // creates nothing (use `TagResource` to change an existing endpoint's tags).
        let key = (input.identity_provider_arn.clone(), server_type.clone());
        if let Some(existing) = state.license_server_endpoint_index.get(&key) {
            return Ok(CreateLicenseServerEndpointOutput {
                identity_provider_arn: Some(input.identity_provider_arn.clone()),
                license_server_endpoint_arn: Some(existing.clone()),
            });
        }
```
The only place the payload is persisted is the mint path, below that early return — src/ops/create_license_server_endpoint.rs:242: `settings: input.license_server_settings.clone(),` (stored as documented in src/state.rs:639-641: "The license-server settings exactly as created, so nothing the caller supplied is lost between operations.").

The model makes the dropped member required and says what it carries — `smithy_query(query="shape", shape="LicenseServerSettings")`: `"ServerSettings": {"target": "ServerSettings", "required": true, "doc": "The ServerSettings resource contains the settings for your server."}`; `smithy_query(query="shape", shape="RdsSalSettings")`: `"RdsSalCredentialsProvider": {"target": "CredentialsProvider", "required": true, "doc": "The CredentialsProvider resource contains a reference to the credentials provider that's used for RDS license server user administration."}`; and `smithy_query(query="doc", shape="CreateLicenseServerEndpointRequest", member="LicenseServerSettings")`: "The LicenseServerSettings resource to create for the endpoint. The settings include the type of license server and the Secrets Manager secret that enables administrators to add or remove users associated with the license server."

The crate's own rule for a member a request names — src/state.rs:2364-2376:
```
/// Whether a member a request NAMES is not the stored resource's.
///
/// Every operation here addresses a resource either by its ARN or by the members that
/// identify it, and a request may supply BOTH. Each member it names is part of which
/// resource the caller asked for ... so a named value that is not the stored one means the
/// request names no resource state holds, however the rest of it addressed one.
/// ... The ONE place that comparison is made, so no resolution can read a member the caller
/// named as a comment on the resource rather than as part of naming it.
```
The sibling handler for the same resource applies it — src/ops/delete_license_server_endpoint.rs:173-180:
```
        // Fail closed: an ARN state holds no endpoint for, or one whose `ServerType` is
        // not the stored endpoint's, is refused without disclosing which it was.
        let arn = state
            .license_server_endpoints
            .get(&input.license_server_endpoint_arn)
            .filter(|record| record.server_type == input.server_type)
```
Test coverage: tests/all/license_server_endpoints.rs and tests/all/license_servers_behind_an_endpoint.rs exercise create/list; no row varies `LicenseServerSettings.ServerSettings` across two creates for one `(IdentityProviderArn, ServerType)` pair, so the dropped member is untested.

## Steps to Reproduce

1. RegisterIdentityProvider for a directory and product, taking the returned IdentityProviderArn.
2. POST /license-server/CreateLicenseServerEndpoint with {"IdentityProviderArn": <arn>, "LicenseServerSettings": {"ServerType": "RDS_SAL", "ServerSettings": {"RdsSalSettings": {"RdsSalCredentialsProvider": {"SecretsManagerCredentialsProvider": {"SecretId": "secret-one"}}}}}} and note the LicenseServerEndpointArn.
3. POST the same operation again for the same IdentityProviderArn and ServerType but with SecretId "secret-two".
4. Observe HTTP 200 carrying the SAME LicenseServerEndpointArn: the second payload's ServerSettings is neither rejected nor stored — src/ops/create_license_server_endpoint.rs:201 returns before line 242, the only place `input.license_server_settings` is persisted.
5. Compare with src/ops/delete_license_server_endpoint.rs:173-180, where a request whose ServerType is not the stored endpoint's is refused rather than served against a record it does not name.
