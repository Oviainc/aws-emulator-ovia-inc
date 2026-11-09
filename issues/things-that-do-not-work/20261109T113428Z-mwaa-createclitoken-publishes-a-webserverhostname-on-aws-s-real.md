Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mwaa.cli-token-native-airflow-cli-round-trip
Issue-Shape-Id: com.amazonaws.mwaa#AmazonMWAA
Issue-Key: mwaa.cli-token-native-airflow-cli-round-trip|run#13|PublishedUrlRejected|url = f"{ctx.endpoint_url(hostname)}/aws_mwaa/cli"
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# CreateCliToken publishes a WebServerHostname on AWS's real airflow.amazonaws.com domain, which no client of the emulator can reach

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCliToken

## Description
One net-new root cause in the Amazon MWAA emulator, grounded in the failing behavioural check mwaa.cli-token-native-airflow-cli-round-trip.

RELATION TO AN OPEN ISSUE (named, not covered by it): 20261106T045055Z-mwaa-amazon-mwaa-six-behavioural-divergences-native-aws-mwaa.md lists this same check among its trace ids, but the suite does NOT mark this failure OWNED by it, because the failure is no longer the one that issue describes. That issue's Cause 1 is "a POST to the published host reaches no handler and answers 404 UnknownService" (an AssertionError raised inside the check after the HTTP call), and its Cause 3 is "Environment.WebserverUrl carries an https:// scheme". The failure replayed here happens STRICTLY EARLIER and on a DIFFERENT member: the request is never made at all, because the host CreateCliToken publishes in its own output member WebServerHostname is not an address of the emulator. Fixing the aws_mwaa/cli route (Cause 1) cannot clear this check, and neither can stripping a scheme from Environment.WebserverUrl (Cause 3) — the defect is the HOST that is published.

=== Cause 1 of 1: CreateCliToken's WebServerHostname names a real-AWS host instead of the host the emulator serves the environment's Airflow web server on ===
AWS:      Smithy (com.amazonaws.mwaa#AmazonMWAA): CreateCliToken is POST /clitoken/{Name} with @endpoint hostPrefix "env.", input Name (EnvironmentName, @httpLabel, required, @length 1..80, @pattern ^[a-zA-Z][0-9a-zA-Z-_]*$), output CliToken (Token, @sensitive) + WebServerHostname (Hostname, @length 1..255, @pattern ^(([a-zA-Z0-9]|[a-zA-Z0-9][a-zA-Z0-9\-]*[a-zA-Z0-9])\.)*([A-Za-z0-9]|[A-Za-z0-9][A-Za-z0-9\-]*[A-Za-z0-9])$); error ResourceNotFoundException (@httpError 404). The model constrains the SHAPE of the value but not which host it must be; for the host's meaning this is recorded behavior only: WebServerHostname is the address of the Apache Airflow web server THAT SERVES THIS ENVIRONMENT, and the whole point of the operation is that a client can compose https://<WebServerHostname>/aws_mwaa/cli, POST an Airflow CLI command with `Authorization: Bearer <CliToken>` and `Content-Type: text/plain`, and receive HTTP 200 with a JSON document carrying base64 stdout (and stderr when non-empty). On real AWS the host that is published is the host that answers. The requirement is therefore relational, not literal: whatever host the responder is actually reachable at is the host it must publish.
Emulator: CreateCliToken answers 200 and publishes a host on AWS's production Airflow domain — of the form <uuid>.c2.<region>.airflow.amazonaws.com (built by crate::state::webserver_hostname / webserver_url, the same builder the open issue above cites for Environment.WebserverUrl). The emulator does not serve that host and that host does not resolve to it, so the published address is unusable: the replay refuses it before any request leaves the client (simparity.published.PublishedUrlRejected — "its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener"). The specific uuid, region, loopback address and port in the replay are per-run values and are NOT the requirement; the requirement is that the published host be one the emulator answers on.
Why:      a client that does exactly what CreateCliToken exists for — mint a token, then drive the Apache Airflow CLI over the published web-server host, which is the documented MWAA CLI pattern used by CI scripts and the mwaa CLI wrappers — is handed an address it cannot reach. Best case it fails DNS or connect and no DAG can ever be listed, triggered or backfilled against the emulator; worst case the name resolves in an environment with egress and the client silently leaves the emulator and talks to real AWS with an emulator-minted bearer token. Either way the 200 response is a false success: the token is reported as issued together with a host that cannot honour it.
Fix:      the data-plane address the environment publishes, and the one place it is built: crate::state::webserver_hostname / webserver_url in src/state.rs, as consumed by src/ops/create_cli_token.rs (and kept consistent with Environment.WebserverUrl on the read path). The published host must be the emulator's own serving host/port for the environment data-plane front door (src/service/env_endpoint.rs), so that the address handed to a client is the address the emulator is reached on, while still satisfying the Hostname shape's @length/@pattern constraints.
Traces:   mwaa.cli-token-native-airflow-cli-round-trip.

## Evidence
Replayed behavioural suite: mwaa 13/19 passed, 6 failed. Five of the six are marked OWNED by 20261106T045055Z-mwaa-amazon-mwaa-six-behavioural-divergences-native-aws-mwaa.md and are skipped; this one is not owned and is filed here.

mwaa.cli-token-native-airflow-cli-round-trip  [mismatch]
  Check flow: provision bucket + execution role + VPC/subnets/SG, CreateEnvironment, poll GetEnvironment to AVAILABLE, CreateCliToken(Name=env), then
    url = f"{ctx.endpoint_url(hostname)}/aws_mwaa/cli"   # hostname = CreateCliToken's WebServerHostname
    POST url, headers Authorization: Bearer <CliToken>, Content-Type: text/plain, body b"dags list"
  Expected: HTTP 200 and a JSON body whose base64 `stdout` contains the Airflow dag table (a `dag_id` header, or "No data found").
  Actual:   the POST is never issued. Resolving the published host fails:
            File "context.py", line 572, in endpoint_route
              return self._published_target(url, "endpoint_url"), {}
            File "context.py", line 586, in _published_target
              raise PublishedUrlRejected(...)
            simparity.published.PublishedUrlRejected: endpoint_url was given
              https://dca68993-f11a-c310-febf-09098eaf42b8.c2.us-west-2.airflow.amazonaws.com
              (published by the emulator or built by the check): its host is not one of the
              emulator's own hosts; a customer of the emulator needs
              http(s)://127.0.0.1:33987/<resource path>, or http(s)://<own host>:<port>/<path>
              on a per-resource listener
  The uuid `dca68993-...`, the region segment, `127.0.0.1` and port `33987` are per-run values, not requirements. The divergence is that the host published in WebServerHostname belongs to AWS's production airflow.amazonaws.com domain rather than to the emulator.
  Source: crate::state::webserver_hostname / webserver_url in src/state.rs build "<id>.c2.<region>.airflow.amazonaws.com"; src/ops/create_cli_token.rs reports it verbatim.
  Note: the CliToken itself was issued and the call answered 200 — only the host is unusable, so a client cannot discover the problem from the response.

## Steps to Reproduce
1. Boot the emulator. Provision an S3 bucket with a `dags/` key, an MWAA execution role, and a VPC with two subnets and a security group.
2. CreateEnvironment with that role ARN, bucket ARN, DagS3Path="dags" and the network configuration; poll GetEnvironment until Status=AVAILABLE.
3. Call CreateCliToken(Name=<that environment>). Observe HTTP 200 with a CliToken and WebServerHostname.
4. Inspect WebServerHostname. Expected: a host the emulator itself answers on (its own host/port for the environment data plane). Observed: `<uuid>.c2.<region>.airflow.amazonaws.com`, a host on AWS's production Airflow domain that the emulator does not serve.
5. Compose https://<WebServerHostname>/aws_mwaa/cli and POST body `dags list` with headers `Authorization: Bearer <CliToken>` and `Content-Type: text/plain` (following a 307 to the same path, preserving method and body). Expected: HTTP 200 with base64 `stdout` containing the dag table. Observed: the address cannot be reached at all — the replay harness refuses the published URL (PublishedUrlRejected) because its host is not one of the emulator's own hosts, so the request is never made.
