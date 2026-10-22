Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the WorkSpaces Web (workspacesweb AWSErmineControlPlaneService) emulator

_Filed: 2026-10-21T21:18:34Z_
_Operation: service-com-amazonaws-workspacesweb-awserminecontrolplaneservice | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 74 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and stores nothing, so no client workflow works at all. Implement a FIRST, SMALL stateful slice: the web-portal lifecycle plus one real association, so a client can create a portal, give it user settings, read it back, list it, and delete it.

Happy path to implement, in order (restJson1, sigv4 signing name `workspaces-web`):

1. **CreatePortal** — `POST /portals`. Inputs: `displayName` (1..64), `authenticationType` (`Standard` | `IAM_Identity_Center`, default `Standard`), `instanceType` (`standard.regular|standard.large|standard.xlarge`), `maxConcurrentSessions` (1..5000), `customerManagedKey`, `additionalEncryptionContext`, `portalCustomDomain`, `tags`, `clientToken` (@idempotencyToken). Must mint and PERSIST a new portal: generate a UUIDv4 portal id, build `portalArn` = `arn:aws:workspaces-web:<region>:<account>:portal/<uuid>` (must satisfy the modeled ARN pattern `^arn:...:[a-zA-Z]+(\/[a-fA-F0-9\-]{36})+$`), and a `portalEndpoint` hostname matching the modeled pattern (e.g. `<uuid>.workspaces-web.<region>.amazonaws.com`). Return the required `portalArn` + `portalEndpoint` read back out of the stored record. Record `creationDate` (now) and `portalStatus` (`Incomplete` until network+user settings are associated, then `Active` — see PortalStatus in the model). Repeating the same `clientToken` must return the SAME portalArn, not a second portal.

2. **CreateUserSettings** — `POST /userSettings`. Required inputs: `copyAllowed`, `pasteAllowed`, `downloadAllowed`, `uploadAllowed`, `printAllowed` (each `Enabled`|`Disabled`); optional `disconnectTimeoutInMinutes` (1..600), `idleDisconnectTimeoutInMinutes` (0..60), `cookieSynchronizationConfiguration`, `toolbarConfiguration`, `deepLinkAllowed`, `webAuthnAllowed`, `tags`, `clientToken`. Persist the full record under a freshly minted `userSettingsArn` (`.../userSettings/<uuid>`) and return that ARN.

3. **AssociateUserSettings** — `PUT /portals/{portalArn+}/userSettings?userSettingsArn=...`. Both ARNs are required. Must look BOTH resources up in state and return `ResourceNotFoundException` (404) if either is absent; on success store `userSettingsArn` on the portal record and echo back the stored `{portalArn, userSettingsArn}` pair.

4. **GetPortal** — `GET /portals/{portalArn+}`. Must return the `portal` structure assembled from the stored record: `portalArn`, `displayName`, `creationDate`, `portalStatus`, `portalEndpoint`, `authenticationType`, `instanceType`, `maxConcurrentSessions`, `customerManagedKey`, `additionalEncryptionContext`, `portalCustomDomain`, plus the association ARNs (`userSettingsArn` populated by step 3; `browserSettingsArn`, `networkSettingsArn`, `trustStoreArn`, `ipAccessSettingsArn`, `dataProtectionSettingsArn`, `userAccessLoggingSettingsArn`, `sessionLoggerArn` absent until associated). Unknown ARN → `ResourceNotFoundException` (404). No echoed inputs: every field comes from the store.

5. **ListPortals** — `GET /portals`. Must return one `PortalSummary` per stored portal (same fields as `Portal` minus `statusReason`/encryption context), honouring `maxResults` (>=1) and the opaque `nextToken` round-trip (@paginated inputToken/outputToken=nextToken): page deterministically (e.g. by creationDate then arn), emit `nextToken` only when more remain, and resume exactly after the token's position. An invalid/unparsable token → `ValidationException` (400).

6. **DeletePortal** — `DELETE /portals/{portalArn+}`. Remove the portal from state; an immediately following `GetPortal` must return `ResourceNotFoundException` (404) and `ListPortals` must no longer include it. Empty body on success. Deleting user settings still associated with a portal should be rejected with `ConflictException` (409) — model the association both ways.

Across all six: responses MUST be computed from persisted state held in the single shared engine store reached via `crate::state::service_state(&*self.engine)` (never a fresh `InMemoryEngine`). No canned bodies, no echoed inputs, no random-ARN-without-storage. Also enforce the modeled constraints the happy path depends on (`displayName` 1..64, `maxConcurrentSessions` 1..5000, ARN length/pattern, enum membership) with `ValidationException` (400).

## State Model

In `ComAmazonawsWorkspaceswebAwserminecontrolplaneserviceState`:

- `portals: HashMap<String /*portalArn*/, Portal>` where `Portal { portal_arn, portal_endpoint, display_name, creation_date: SystemTime, portal_status: PortalStatus, authentication_type, instance_type, max_concurrent_sessions: Option<i32>, customer_managed_key: Option<String>, additional_encryption_context: HashMap<String,String>, portal_custom_domain: Option<String>, status_reason: Option<String>, renderer_type, browser_type, browser_settings_arn: Option<String>, network_settings_arn: Option<String>, user_settings_arn: Option<String>, trust_store_arn: Option<String>, ip_access_settings_arn: Option<String>, data_protection_settings_arn: Option<String>, user_access_logging_settings_arn: Option<String>, session_logger_arn: Option<String>, tags: Vec<Tag> }`
- `user_settings: HashMap<String /*userSettingsArn*/, UserSettings>` where `UserSettings { user_settings_arn, copy_allowed, paste_allowed, download_allowed, upload_allowed, print_allowed, disconnect_timeout_in_minutes: Option<i32>, idle_disconnect_timeout_in_minutes: Option<i32>, cookie_synchronization_configuration: Option<..>, toolbar_configuration: Option<..>, deep_link_allowed: Option<..>, web_authn_allowed: Option<..>, associated_portal_arns: Vec<String>, tags: Vec<Tag> }`
- `idempotency: HashMap<String /*clientToken*/, String /*created resource arn*/>` so a replayed `clientToken` returns the first-created ARN.

Everything else (browser/network/trust-store/IP-access/data-protection/user-access-logging settings, identity providers, sessions, session loggers, tagging) stays 501 for now and will be filed as individual gap issues later.

## Evidence

All 74 files under `services/service-com-amazonaws-workspacesweb-awserminecontrolplaneservice/src/ops/*.rs` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker on line 4 and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501; `grep -rl` over that directory matches every operation file, so the service holds zero state. Contracts cited from the crate's `OPERATIONS.md` (restJson1, SDK id `WorkSpaces Web`, sigv4 name `workspaces-web`): CreatePortal `POST /portals` → required `portalArn`+`portalEndpoint` (lines 263-268); GetPortal `GET /portals/{portalArn+}` → `portal` (Portal) with ResourceNotFoundException 404 (lines 270-276); UpdatePortal/DeletePortal `PUT|DELETE /portals/{portalArn+}` with ConflictException 409 (lines 278-292); ListPortals `GET /portals` @paginated(inputToken=nextToken, outputToken=nextToken) with `maxResults`/`nextToken` (lines 294-300); AssociateUserSettings `PUT /portals/{portalArn+}/userSettings` with required `userSettingsArn` query param and output `{portalArn, userSettingsArn}` (lines 360-363); CreateUserSettings `POST /userSettings` with five required Enabled/Disabled permission fields and required `userSettingsArn` output (lines 568-573). The `Portal`/`PortalSummary` member lists and ARN/endpoint patterns above are from `smithy_query shape=Portal,PortalSummary` on the service model.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `workspaces-web` at it.
2. `CreatePortal {displayName: "demo-portal", authenticationType: "Standard", instanceType: "standard.regular", maxConcurrentSessions: 10, clientToken: "tok-1"}` → 200 with a well-formed `portalArn` and `portalEndpoint`.
3. Repeat the identical CreatePortal with `clientToken: "tok-1"` → same `portalArn` returned, and ListPortals still shows exactly one portal.
4. `CreateUserSettings {copyAllowed: "Enabled", pasteAllowed: "Enabled", downloadAllowed: "Disabled", uploadAllowed: "Disabled", printAllowed: "Enabled", idleDisconnectTimeoutInMinutes: 15}` → 200 with `userSettingsArn`.
5. `AssociateUserSettings {portalArn, userSettingsArn}` → 200 echoing the stored pair; the same call with a bogus portalArn → 404 ResourceNotFoundException.
6. `GetPortal {portalArn}` → 200 and `portal.displayName == "demo-portal"`, `portal.maxConcurrentSessions == 10`, `portal.userSettingsArn` equals the ARN from step 4, `portal.creationDate` present, `portal.portalEndpoint` equal to the value returned at create.
7. `ListPortals {maxResults: 1}` after creating a second portal → exactly one PortalSummary plus a `nextToken`; passing that token returns the other portal and no token; the two pages do not overlap.
8. `DeletePortal {portalArn}` → 200, then `GetPortal {portalArn}` → 404 ResourceNotFoundException and `ListPortals` no longer lists it.
9. Every step above must return a modeled 2xx body computed from state (not 501, not an echo of the request).
