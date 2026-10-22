Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the AppConfigData emulator

_Filed: 2026-10-21T21:57:44Z_
_Operation: service-com-amazonaws-appconfigdata-appconfigdata | Class: stub_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The whole crate is a fresh scaffold: both bound operations (`src/ops/start_configuration_session.rs`, `src/ops/get_latest_configuration.rs`) still carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and answer HTTP 501 `NotImplemented`, holding zero state. AppConfigData has only two operations, so the bootstrap happy path IS the whole configuration-polling loop and must work end-to-end, statefully.

### Happy path to implement (in order)

1. **StartConfigurationSession** — `POST /configurationsessions -> 201`. Input: `ApplicationIdentifier` (required, len 1..128), `EnvironmentIdentifier` (required, len 1..128), `ConfigurationProfileIdentifier` (required, len 1..128), `RequiredMinimumPollIntervalInSeconds` (optional, range 15..86400). Output: `InitialConfigurationToken` (string matching `^\S{1,8192}$`). Must mint and PERSIST a session record keyed by that opaque token — the token is not an echo of the inputs and must be resolvable later by GetLatestConfiguration. Store the triple of identifiers and the requested minimum poll interval on the session.

2. **GetLatestConfiguration (first poll)** — `GET /configuration?configuration_token=...  -> 200`. Input: `ConfigurationToken` (required `@httpQuery(configuration_token)`, pattern `^\S{1,8192}$`). Output headers/payload: `NextPollConfigurationToken` (`Next-Poll-Configuration-Token`), `NextPollIntervalInSeconds` (`Next-Poll-Interval-In-Seconds`, required), `ContentType` (`Content-Type`), `VersionLabel` (`Version-Label`), and `Configuration` as the `@httpPayload` blob. The first poll of a session must return the stored configuration content for the session's (application, environment, profile) with the matching content type, plus a FRESH `NextPollConfigurationToken` that is itself a valid, resolvable session token.

3. **GetLatestConfiguration (second poll, token rotation + unchanged semantics)** — polling again with the token returned in step 2 must succeed, and because the deployed configuration has not changed since the last poll, it must return an EMPTY `Configuration` payload (zero-length body) while still returning a new `NextPollConfigurationToken` and `NextPollIntervalInSeconds`. This is the defining AppConfigData behaviour: content is returned only when it differs from what the session last received. Each returned token must also be single-use: re-using a consumed token must not be silently accepted as a fresh poll.

4. **GetLatestConfiguration (changed configuration)** — if the emulator's stored configuration content for the session's profile is mutated (via test seeding / a cross-service AppConfig deployment hook, whichever the repo offers), the next poll with the current token must return the NEW content and its version label, not the cached/empty response.

5. **Error path** — `GetLatestConfiguration` with an unknown/garbage-but-pattern-valid token must return the modeled `BadRequestException` (HTTP 400); a token for an application/environment/profile that does not exist must return `ResourceNotFoundException` (HTTP 404). `StartConfigurationSession` with a missing/over-length identifier or `RequiredMinimumPollIntervalInSeconds` outside 15..86400 must return `BadRequestException` (HTTP 400), not a 501 and not a 200.

Every one of these responses must be COMPUTED FROM PERSISTED STATE in the shared engine (`crate::state::service_state(&*self.engine)`), never canned, echoed or randomly generated without storage. In particular: a minted token that is not stored, or a `Configuration` payload that is a fixed literal regardless of the session, is a failure of this issue.

## State Model

In `src/state.rs`, on `ComAmazonawsAppconfigdataAppconfigdataState`:

- `sessions: HashMap<String /* configuration token */, Session>` where
  `Session { application_identifier: String, environment_identifier: String, configuration_profile_identifier: String, required_minimum_poll_interval_in_seconds: Option<i32>, /* content hash/version the client has already been served */ last_served_version: Option<String>, consumed: bool, created_at: SystemTime }`
- `configurations: HashMap<(String /* app */, String /* env */, String /* profile */), DeployedConfiguration>` where
  `DeployedConfiguration { content: Vec<u8>, content_type: String, version_label: Option<String>, version: String /* monotonically bumped on change */ }`

Token rotation: each successful `GetLatestConfiguration` removes/marks-consumed the incoming token entry and inserts a new token entry pointing at a `Session` that carries the updated `last_served_version`, so the "unchanged ⇒ empty payload" decision is a comparison of `Session.last_served_version` against `DeployedConfiguration.version` read out of state. `NextPollIntervalInSeconds` is derived from the session's `required_minimum_poll_interval_in_seconds` (defaulting to the service's 60s) — not a hardcoded 0.

There are no other operations in this service, so this slice is the entire bootstrap; anything left incomplete should be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-appconfigdata-appconfigdata/OPERATIONS.md` lists exactly 2 operations with the routes, members and errors quoted above: `StartConfigurationSession` (`POST /configurationsessions -> 201`, required ApplicationIdentifier/EnvironmentIdentifier/ConfigurationProfileIdentifier len 1..128, optional RequiredMinimumPollIntervalInSeconds range 15..86400, output `InitialConfigurationToken`) and `GetLatestConfiguration` (`GET /configuration -> 200`, required `ConfigurationToken` `@httpQuery(configuration_token)`, outputs `Next-Poll-Configuration-Token` / `Next-Poll-Interval-In-Seconds` (required) / `Content-Type` / `Version-Label` headers and the `Configuration` `@httpPayload` blob), both declaring `BadRequestException (400)`, `ResourceNotFoundException (404)`, `ThrottlingException (429)`, `InternalServerException (500)`. `grep -n 'SCAFFOLDED, NOT IMPLEMENTED' src/ops/` matches both `src/ops/start_configuration_session.rs:4` and `src/ops/get_latest_configuration.rs:4`, i.e. 2/2 operations are unmodified scaffolds answering 501 with no state. The token-rotation and "empty payload when unchanged" rules are well-established AppConfigData semantics (each poll returns a new single-use token; content is returned only when it has changed since the last poll). Wire protocol `restJson1`, signing name `appconfig`, endpoint prefix `appconfigdata` per README/OPERATIONS.md.

## Steps to Reproduce

1. POST /configurationsessions with {"ApplicationIdentifier":"MyApp","EnvironmentIdentifier":"Prod","ConfigurationProfileIdentifier":"MyProfile"} — expect HTTP 201 with a non-empty InitialConfigurationToken; actual: HTTP 501 NotImplemented.
2. Seed a deployed configuration for (MyApp, Prod, MyProfile) with content `{"feature":"on"}` and Content-Type `application/json`.
3. GET /configuration?configuration_token=<InitialConfigurationToken> — expect HTTP 200, body `{"feature":"on"}`, Content-Type `application/json`, a non-empty Next-Poll-Configuration-Token and a Next-Poll-Interval-In-Seconds > 0; actual: HTTP 501.
4. GET /configuration?configuration_token=<Next-Poll-Configuration-Token from previous step> — expect HTTP 200 with an EMPTY body (configuration unchanged) and yet another fresh Next-Poll-Configuration-Token.
5. Change the deployed configuration content to `{"feature":"off"}`, then poll again with the latest token — expect HTTP 200 with body `{"feature":"off"}` and the updated Version-Label.
6. GET /configuration?configuration_token=not-a-real-token — expect HTTP 400 BadRequestException.
7. POST /configurationsessions with RequiredMinimumPollIntervalInSeconds=5 — expect HTTP 400 BadRequestException (below the modeled range 15..86400).
