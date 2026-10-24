Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# Environments are stored and looked up without a Region, so a listing leaks environments created in other Regions

_Filed: 2026-10-24T05:25:40Z_
_Operation: ListEnvironments | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

MWAA is a regional service: an environment lives in the Region it was created in, its ARN carries that Region, and `ListEnvironments` in another Region returns only that Region's environments (a name-addressed lookup such as `GetEnvironment`/`UpdateEnvironment`/`DeleteEnvironment` for an environment that lives elsewhere answers 404 `ResourceNotFoundException`).

Expected: create 'RegionalEnv' with a request signed for us-east-1, then call `ListEnvironments` with a request signed for eu-west-1 -> `Environments: []`; `GetEnvironment(Name='RegionalEnv')` in eu-west-1 -> 404 `ResourceNotFoundException`.

Actual: the store keys environments by NAME alone, with no Region in the key and no Region filter on any read, so every Region sees every environment. The eu-west-1 listing names 'RegionalEnv', and a eu-west-1 `GetEnvironment` returns it with `Arn`/`WebserverUrl`/`CeleryExecutorQueue`/VPC-endpoint-service values that all carry `us-east-1` — a payload the real service can never produce for a eu-west-1 request. The same name-keyed lookup makes `UpdateEnvironment`, `DeleteEnvironment`, `CreateCliToken`, `CreateWebLoginToken` and `InvokeRestApi` cross-Region as well, and it makes a name that is legitimately reusable across two Regions collide: creating 'RegionalEnv' in eu-west-1 after us-east-1 is refused with `ValidationException` ("An environment named RegionalEnv already exists") where AWS creates a second, independent environment. (The `/tags/{ResourceArn}` operations are not affected: they resolve through the ARN, which carries the Region.)

## Evidence

Grounding: well-established AWS semantics — MWAA environments are per-account per-Region resources and `EnvironmentArn`'s own modelled pattern `^arn:aws(-[a-z]+)?:airflow:[a-z0-9\-]+:\d{12}:environment/\w+` carries the Region segment; the emulator already reads the request's Region off its SigV4 credential scope (src/state.rs:525-529, `request_region`) and stamps it into every minted ARN/host.

Handler/state: src/state.rs:430 `pub environments: BTreeMap<String, EnvironmentRecord>` is keyed by name only, and `EnvironmentRecord.region` (src/state.rs:297) is used solely to render ARNs/hosts — grep shows its only read is the log-group ARN at src/state.rs:368. src/ops/list_environments.rs:163 collects `store.environments.keys()` with no Region predicate; src/ops/get_environment.rs:152, src/ops/update_environment.rs:265, src/ops/delete_environment.rs:150, src/ops/create_cli_token.rs:166, src/ops/create_web_login_token.rs:178 and src/ops/invoke_rest_api.rs:185 all look up `store.environments` by `input.name` alone; src/ops/create_environment.rs:352 treats a name stored for any Region as taken. Sibling services in this repo do scope resolution to the request Region (e.g. services/service-com-amazonaws-aiops-aiops/src/state.rs:213-227 filters candidates by `request_region`), so this is a gap rather than a framework limitation.

Tests: every suite under services/service-com-amazonaws-mwaa-amazonmwaa/tests signs for one Region (`const REGION: &str = "us-east-1"`, tests/all/environment_lifecycle.rs:32, tests/all/rejection_contract.rs:52); no test exercises two Regions, so nothing asserts (or pins) the current cross-Region behaviour.

## Steps to Reproduce

1. Boot the emulator.
2. Send CreateEnvironment (PUT /environments/RegionalEnv) with an Authorization credential scope for us-east-1 and the required members -> 200 with Arn 'arn:aws:airflow:us-east-1:000000000000:environment/RegionalEnv'.
3. Send ListEnvironments (GET /environments) with an Authorization credential scope for eu-west-1. Expected: Environments: []. Observed: Environments: ['RegionalEnv'].
4. Send GetEnvironment (GET /environments/RegionalEnv) signed for eu-west-1. Expected: 404 ResourceNotFoundException. Observed: 200 with an Environment whose Arn, WebserverUrl and CeleryExecutorQueue all name us-east-1.
5. Send CreateEnvironment for the same name 'RegionalEnv' signed for eu-west-1. Expected: 200 and a second, independent environment with a eu-west-1 ARN. Observed: 400 ValidationException 'An environment named RegionalEnv already exists'.
