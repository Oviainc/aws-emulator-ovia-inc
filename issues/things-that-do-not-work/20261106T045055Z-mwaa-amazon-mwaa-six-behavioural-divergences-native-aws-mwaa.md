Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mwaa.cli-token-native-airflow-cli-round-trip mwaa.concurrent-create-environment-same-name-single-winner mwaa.create-environment-lifecycle-reaches-available mwaa.delete-environment-lifecycle-reaches-deleted mwaa.update-environment-during-creation-rejected mwaa.update-environment-scales-workers-lifecycle
Issue-Shape-Id: com.amazonaws.mwaa#AmazonMWAA
Issue-Key: mwaa.cli-token-native-airflow-cli-round-trip|run#14|AssertionError|raise AssertionError(
Issue-Key: mwaa.concurrent-create-environment-same-name-single-winner|run#12|AssertionError|assert len(successes) == 1, f"expected exactly 1 winner, got {len(successes)}: {results}"
Issue-Key: mwaa.create-environment-lifecycle-reaches-available|run#19|AssertionError|assert env["WebserverUrl"] and "://" not in env["WebserverUrl"], env["WebserverUrl"]
Issue-Key: mwaa.delete-environment-lifecycle-reaches-deleted|run#17|AssertionError|assert mutate_code == "ValidationException", (
Issue-Key: mwaa.update-environment-during-creation-rejected|run#12|AssertionError|assert message == (
Issue-Key: mwaa.update-environment-scales-workers-lifecycle|run#17|AssertionError|assert seen_updating, "update settled on AVAILABLE without ever observing UPDATING status"
Issue-Commit: d1d0ef69dc509afeb35cfd83bb98f419ab648ebe

# Amazon MWAA: six behavioural divergences — native aws_mwaa/cli endpoint unserved, duplicate-name CreateEnvironment accepted, WebserverUrl carries a scheme, delete/update refusal class and wording, and an unobservable UPDATING phase

> _Note: this issue was identified against the code base at commit `d1d0ef69dc509afeb35cfd83bb98f419ab648ebe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCliToken, CreateEnvironment, GetEnvironment, DeleteEnvironment, UpdateEnvironment

## Description
Six net-new root causes in the Amazon MWAA emulator, all grounded in failing behavioural checks of the `mwaa` service. None of them is covered by the open issues on this service (20261024T153328Z-mwaa-updateenvironment-validation--r2 covers UpdateEnvironment's AirflowVersion-downgrade and dependency-resolution validation only; the other open MWAA issues are mwaa-serverless). Each cause below must be closed before this issue is submitted.

=== Cause 1 of 6: the CLI token CreateCliToken mints is unusable — the Airflow web server's native `aws_mwaa/cli` endpoint is not served at all ===
AWS:      Smithy: `CreateCliToken` is POST /clitoken/{Name} with `@endpoint` hostPrefix `env.`, output `CliToken` (Token, @sensitive) + `WebServerHostname` (Hostname, len 1..255, pattern `^(([a-zA-Z0-9]|[a-zA-Z0-9][a-zA-Z0-9\-]*[a-zA-Z0-9])\.)*([A-Za-z0-9]|[A-Za-z0-9][A-Za-z0-9\-]*[A-Za-z0-9])$`); error ResourceNotFoundException (@httpError 404). The endpoint the token is FOR is not an AWS-protocol operation and is therefore not in the model — recorded behavior only: a POST to `https://<WebServerHostname>/aws_mwaa/cli` carrying `Authorization: Bearer <CliToken>`, `Content-Type: text/plain` and an Apache Airflow CLI command as the raw body answers HTTP 200 with a JSON document whose `stdout` (and `stderr`, when non-empty) members are base64-encoded CLI output; the web server may first answer 307 to the same path, which keeps method and body. `dags list` prints the Airflow dag table (a `dag_id` header, or "No data found").
Emulator: the request never reaches any handler. The router answers HTTP 404 with body `{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}` — an emulator-internal error type that is not an AWS error shape. The service claims requests by resolving a route key from the SigV4 credential scope / X-Amz-Target / Host (`src/service.rs` `can_handle`), and a bearer-token request addressed to the environment's web-server host resolves to nothing; the environment data-endpoint front door (`src/service/env_endpoint.rs`) wraps only the modeled `InvokeRestApi` operation, so no route serves `/aws_mwaa/cli`.
Why:      CreateCliToken exists solely to authenticate the Apache Airflow CLI. Every client that mints a token and then drives DAGs over `aws_mwaa/cli` (the documented MWAA CLI pattern, and what CI scripts and the `mwaa` wrappers do) gets a 404 `UnknownService` instead of CLI output, so no DAG can be listed, triggered or backfilled against this emulator even though CreateCliToken reports success and hands back a token and a hostname.
Fix:      the service's request-claiming seam and the environment data-endpoint front door: `src/service.rs` (`can_handle` / `override_operation`) must claim a bearer-authenticated POST addressed to a stored environment's web-server host, and `src/service/env_endpoint.rs` must serve `/aws_mwaa/cli` — verify the bearer token against the one minted by `crate::state::mint_airflow_token`, execute the command against the environment's stored Apache Airflow metadata, and serialize base64 `stdout`/`stderr`.
Traces:   mwaa.cli-token-native-airflow-cli-round-trip.

=== Cause 2 of 6: CreateEnvironment accepts a duplicate environment name instead of refusing it ===
AWS:      Smithy: `CreateEnvironment` is PUT /environments/{Name}, `@idempotent`, and declares ValidationException (@httpError 400). Recorded behavior: an environment name is unique per account per Region, and a create of a name that already exists is refused with HTTP 400 `ValidationException` and the message "An environment with name <Name> already exists. Please use other name." — "An environment with name " / " already exists. Please use other name." is fixed text and `<Name>` is the request's own Name. Four identical racing CreateEnvironment calls for one name therefore yield exactly ONE 200 and three such refusals; GetEnvironment then reports the winner's ExecutionRoleArn/SourceBucketArn.
Emulator: all four racers answered 200 (`[('ok', None), ('ok', None), ('ok', None), ('ok', None)]`). `src/ops/create_environment.rs:443-466` reads the `@idempotent` trait as "a replay of the SAME create answers the stored ARN and writes nothing", so a repeat whose spec and tags compare equal (`EnvironmentRecord::matches_create`, `src/state.rs:1222`) is a success; only a create that differs in a recorded member is refused, and then with the emulator's own sentence "An environment named <name> already exists" (no "with name", no trailing "Please use other name.").
Why:      a client that uses CreateEnvironment to claim a name — the standard create-or-fail pattern, and the only way to detect that a teammate or a concurrent pipeline already owns the name — is told it succeeded. Provisioning code that branches on the ValidationException never takes that branch, and a racing fleet believes every member won. Clients that match the refusal message see the wrong sentence even in the non-identical case.
Fix:      `src/ops/create_environment.rs` — the existing-name gate (an environment stored under that name in that Region is a refusal, not an idempotent replay) and the wording of its ValidationException message.
Traces:   mwaa.concurrent-create-environment-same-name-single-winner.

=== Cause 3 of 6: Environment.WebserverUrl is reported with an `https://` scheme ===
AWS:      Smithy doc for `Environment$WebserverUrl`: "The Apache Airflow web server HOST NAME for the Amazon MWAA environment." Recorded behavior: the value is a bare host name with no scheme (the check asserts `"://" not in WebserverUrl`). MODEL CONFLICT, stated plainly so the fixer can weigh it: the `WebserverUrl` shape itself carries `@length(1, 256)` and `@pattern("^https://.+$")`, i.e. the shape's pattern demands the scheme the recording does not show. The recorded response and the member documentation are the requirement here; the pattern is the conflicting signal.
Emulator: `crate::state::webserver_url` (`src/state.rs:1759-1762`) builds `https://{id}.c2.{region}.airflow.amazonaws.com` (explicitly to satisfy that `^https://.+$` pattern) and GetEnvironment reports it verbatim, e.g. `https://<uuid>.c2.us-west-2.airflow.amazonaws.com`.
Why:      a client that composes the Airflow UI/API address from this member (`https://{WebserverUrl}/...`, or passing it as a host to an HTTP client, a DNS lookup or a Host header) gets `https://https://...` or a host name that is not one. The generated id, region and host suffix are environment-specific and are NOT the requirement — only the absence of a scheme is.
Fix:      the representation of the stored record on the read path (`src/ops/get_environment.rs` / `list_environments` via the `Environment` serializer) and/or `crate::state::webserver_url` in `src/state.rs`, keeping `crate::state::webserver_hostname` (used by CreateCliToken) consistent with it.
Traces:   mwaa.create-environment-lifecycle-reaches-available.

=== Cause 4 of 6: a repeat DeleteEnvironment on an already-deleted environment answers ResourceNotFoundException (404) instead of ValidationException (400) ===
AWS:      Smithy: `DeleteEnvironment` is DELETE /environments/{Name}, `@idempotent`, and declares BOTH ResourceNotFoundException (@httpError 404) and ValidationException (@httpError 400). Recorded behavior: once the teardown settles, GetEnvironment answers 404 `ResourceNotFoundException` "Environment <Name> not found." (the emulator matches this; that assertion passed), but MWAA keeps the DELETED record behind that not-found read, so a SECOND DeleteEnvironment on the same name is refused with HTTP 400 `ValidationException` and the message "Environments with DELETED status cannot be deleted." — entirely fixed text, with no value taken from the request or the state.
Emulator: `ResourceNotFoundException`. `ComAmazonawsMwaaAmazonmwaaState::reap_deleted_environments` (`src/state.rs:1518-1535`) DROPS the record and its ARN index entry the instant `STATUS_DELETED` lands, so the name is free again and DeleteEnvironment takes the name-not-found path (`crate::state::environment_not_found_message`).
Why:      a client cannot tell "this name never existed" (404) from "this environment was already torn down" (400 + DELETED status) — the distinction a retrying deleter and any idempotent teardown step branch on. Error-class–driven retry logic that treats 400 as terminal and 404 as a lookup miss takes the wrong branch, and the name reads as free before AWS considers it free.
Fix:      the store's retirement of a deleted record plus the delete handler's status gate: `src/state.rs` (`reap_deleted_environments` / the DELETED record's lifetime) and `src/ops/delete_environment.rs` (refuse by DELETED status with the fixed sentence above before falling through to not-found).
Traces:   mwaa.delete-environment-lifecycle-reaches-deleted.

=== Cause 5 of 6: UpdateEnvironment's not-AVAILABLE refusal carries the wrong message ===
AWS:      Smithy: `UpdateEnvironment` (PATCH /environments/{Name}) declares ValidationException (@httpError 400); `EnvironmentStatus` includes CREATING. Recorded behavior: an UpdateEnvironment against an environment still in CREATING is refused with HTTP 400 `ValidationException` (the emulator's code is correct) and the message "Environments with <STATUS> status must complete previous operation before initiating a new operation." — fixed text apart from `<STATUS>`, which is the environment's stored status (CREATING here); AWS names NO environment in this sentence.
Emulator: "Environment <name> is in status CREATING. An environment can only be updated while its status is AVAILABLE.", built by `crate::state::update_not_allowed_message` (`src/state.rs:976-981`) from the request's Name and the stored status. Different fixed text, and it leaks a name AWS keeps out of this refusal.
Why:      a client that matches on the refusal message to distinguish a transient state conflict (retry when the create settles) from a permanent input error (do not retry) does not match. The effect-absent part of the behavior is correct — MaxWorkers stayed at 1 — so only the wording diverges, which is exactly what a message-matching client sees.
Fix:      `crate::state::update_not_allowed_message` in `src/state.rs` (the single place the status-conflict sentence is worded), used by `src/ops/update_environment.rs`.
Traces:   mwaa.update-environment-during-creation-rejected.

=== Cause 6 of 6: an accepted UpdateEnvironment never becomes observable as UPDATING ===
AWS:      Smithy: `EnvironmentStatus` models UPDATING as a non-terminal status and `LastUpdate$Status` PENDING -> SUCCESS. Recorded behavior: an accepted UpdateEnvironment drives AVAILABLE -> UPDATING -> AVAILABLE, and the UPDATING phase lasts minutes — far longer than any client's poll interval — so a poller that reads GetEnvironment repeatedly after the call observes UPDATING at least once before the new MaxWorkers settles.
Emulator: the poller only ever sees AVAILABLE, already carrying the new MaxWorkers, and LastUpdate.Status SUCCESS; UPDATING is never observed. `src/state.rs:42-63` sets TRANSITION_SPAN = 500ms, split into two UPDATE_PHASE_SPAN = 250ms phases (CREATING_SNAPSHOT then UPDATING), and `settle()` materialises every due transition on the next read — so the whole update is already due by the time a client polls at any realistic interval (2s in the check). The transient status exists in the state machine but is unobservable through the API.
Why:      a client that waits for the status to LEAVE AVAILABLE to confirm its update was accepted never fires and hangs or concludes the call was a no-op; a client that asks "is an update in flight on this environment?" (before issuing a second update, or before a deploy gate) can never observe one, and code paths guarded on UPDATING are never exercised. This is a representation defect, not a timing preference: the emulator reports a transition it never makes visible.
Fix:      the state clock's transition schedule for an accepted update — `crate::state::TRANSITION_SPAN` / `UPDATE_PHASE_SPAN` and `EnvironmentRecord::begin_update` / `settle` in `src/state.rs` — so each non-terminal phase of an update is observable to a client polling at a normal interval.
Traces:   mwaa.update-environment-scales-workers-lifecycle.

## Evidence
All six are mismatches from the replayed behavioural suite (mwaa 13/19 passed, 6 failed; no checks errored, skipped or unloadable, and none of the six is marked ALREADY FILED or OWNED).

1. mwaa.cli-token-native-airflow-cli-round-trip
   Expected: POST https://<WebServerHostname>/aws_mwaa/cli, `Authorization: Bearer <CliToken>`, body `dags list` -> HTTP 200, JSON with base64 `stdout` containing the dag table (`dag_id` / "No data found").
   Actual:  AssertionError: aws_mwaa/cli returned HTTP 404 (Location=None): b'{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}'

2. mwaa.concurrent-create-environment-same-name-single-winner
   Expected: 1 success + 3 x HTTP 400 ValidationException "An environment with name <Name> already exists. Please use other name."
   Actual:  AssertionError: expected exactly 1 winner, got 4: [('ok', None), ('ok', None), ('ok', None), ('ok', None)]
   Source:  src/ops/create_environment.rs:443-466 returns the stored ARN for an identical repeat create; the non-identical case is refused with "An environment named <name> already exists".

3. mwaa.create-environment-lifecycle-reaches-available
   Expected: GetEnvironment Environment.WebserverUrl is a host name containing no "://" (check comment: "WebserverUrl is the webserver's host name, with no scheme (MWAA API reference, Environment)").
   Actual:  AssertionError: https://495870d3-db1e-5472-a29f-6bcb3217ad78.c2.us-west-2.airflow.amazonaws.com (the id/region here are per-run values, not requirements; only the scheme is).
   Source:  src/state.rs:1759-1762. Conflicting model signal recorded in the description: shape WebserverUrl has @pattern ^https://.+$ while the member doc says "host name".

4. mwaa.delete-environment-lifecycle-reaches-deleted
   Expected: repeat DeleteEnvironment -> HTTP 400 ValidationException "Environments with DELETED status cannot be deleted."
   Actual:  AssertionError: expected a repeat delete on the deleted environment to be refused, got 'ResourceNotFoundException'
   (The preceding GetEnvironment assertion — 404 ResourceNotFoundException "Environment <Name> not found." — passed, so only the repeat-delete class/message diverges.)
   Source:  src/state.rs:1518-1535 reaps the record as soon as STATUS_DELETED lands.

5. mwaa.update-environment-during-creation-rejected
   Expected: ValidationException with message "Environments with CREATING status must complete previous operation before initiating a new operation."
   Actual:  AssertionError: Environment sim2real-mwaa-env-<id> is in status CREATING. An environment can only be updated while its status is AVAILABLE.  (the environment name is a per-run value; the divergence is the fixed text and the fact a name appears at all)
   Source:  src/state.rs:976-981 (update_not_allowed_message). The error CODE and the effect-absent assertion (MaxWorkers unchanged at 1) were correct.

6. mwaa.update-environment-scales-workers-lifecycle
   Expected: after UpdateEnvironment(MaxWorkers=2) on an AVAILABLE mw1.small environment, a poller observes Status=UPDATING before it settles on AVAILABLE with MaxWorkers=2.
   Actual:  AssertionError: update settled on AVAILABLE without ever observing UPDATING status (LastUpdate.Status SUCCESS and MaxWorkers=2 were already correct on the first poll).
   Source:  src/state.rs:42-63 — TRANSITION_SPAN 500ms, UPDATE_PHASE_SPAN 250ms, materialised by settle() on the next read; shorter than any client poll interval.

## Steps to Reproduce
Boot the emulator and, for each cause, drive the MWAA surface as the named check does (an execution role, an S3 bucket ARN and a VPC with subnets + a security group are provisioned first in every case).

Cause 1 — CreateCliToken / native CLI:
1. CreateEnvironment 'Env1'; poll GetEnvironment until Status=AVAILABLE.
2. CreateCliToken(Name='Env1'); keep CliToken and WebServerHostname.
3. POST to https://<WebServerHostname>/aws_mwaa/cli with headers `Authorization: Bearer <CliToken>` and `Content-Type: text/plain` and body `dags list` (follow a 307 to the same path, preserving method and body).
4. Expect HTTP 200 and a JSON body with base64 `stdout` containing the Airflow dag table header. Observe HTTP 404 `{"__type":"UnknownService",...}`.

Cause 2 — duplicate name:
1. Issue four identical CreateEnvironment calls for one new Name concurrently (4 threads).
2. Expect exactly one HTTP 200 and three HTTP 400 ValidationException "An environment with name <Name> already exists. Please use other name.". Observe four HTTP 200s.
3. Also: CreateEnvironment the same Name with a DIFFERENT MaxWorkers; expect that same AWS sentence, observe "An environment named <Name> already exists".

Cause 3 — WebserverUrl:
1. CreateEnvironment 'Env3'; poll until AVAILABLE; call GetEnvironment.
2. Expect Environment.WebserverUrl to be a bare host name (no "://"). Observe it prefixed with "https://".

Cause 4 — repeat delete:
1. CreateEnvironment 'Env4'; wait for AVAILABLE; DeleteEnvironment('Env4').
2. Poll GetEnvironment until it answers 404 ResourceNotFoundException "Environment Env4 not found." (this part already matches).
3. Call DeleteEnvironment('Env4') again. Expect HTTP 400 ValidationException "Environments with DELETED status cannot be deleted."; observe HTTP 404 ResourceNotFoundException.

Cause 5 — update during CREATING:
1. CreateEnvironment 'Env5' with MaxWorkers=1; immediately confirm GetEnvironment reports Status in (CREATING, PENDING).
2. Call UpdateEnvironment(Name='Env5', MaxWorkers=5). Expect HTTP 400 ValidationException "Environments with CREATING status must complete previous operation before initiating a new operation."; observe "Environment Env5 is in status CREATING. An environment can only be updated while its status is AVAILABLE."

Cause 6 — UPDATING observability:
1. CreateEnvironment 'Env6' with EnvironmentClass='mw1.small'; wait for AVAILABLE.
2. Call UpdateEnvironment(Name='Env6', MaxWorkers=2), then poll GetEnvironment every ~2s until Status=AVAILABLE and MaxWorkers=2.
3. Expect at least one poll to report Status='UPDATING'. Observe that the first poll already reports AVAILABLE with MaxWorkers=2 and LastUpdate.Status=SUCCESS, so UPDATING is never seen.
