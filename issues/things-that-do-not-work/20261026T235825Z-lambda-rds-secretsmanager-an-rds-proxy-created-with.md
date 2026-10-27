Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__rds__secretsmanager.deep-rds-3-proxy-require-tls-plaintext-rejected
Issue-Key: lambda__rds__secretsmanager.deep-rds-3-proxy-require-tls-plaintext-rejected|_retry_delete#13|Timeout|time.sleep(interval_s)
Issue-Commit: 6402e991ee1ffb1f37749ece0148ecf3009a760a

# lambda__rds__secretsmanager: an RDS Proxy created with RequireTLS=True never enforces TLS on its MySQL endpoint — the check hangs to the 60 s cap, so the proxy data plane is ungraded (1 cause)

> _Note: this issue was identified against the code base at commit `6402e991ee1ffb1f37749ece0148ecf3009a760a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBProxy

## Description
Cause 1 of 1: An RDS Proxy's `RequireTLS` setting is stored and echoed back, but the proxy endpoint has no MySQL data plane behind it, so the setting enforces nothing and the whole positive/negative/break-then-restore sequence that grades it never completes.

AWS: Smithy (services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json) — `CreateDBProxy.RequireTLS` (Boolean) and `ModifyDBProxy.RequireTLS` (BooleanOptional) are documented as "Specifies whether Transport Layer Security (TLS) encryption is required for connections to the proxy. By enabling this setting, you can enforce encrypted TLS connections to the proxy." The enforcement itself is not an API response — it is spoken in the MySQL wire protocol at the proxy's `DBProxy.Endpoint` on port 3306, so for that part the requirement is recorded behavior only: against real AWS a proxy with `RequireTLS=True` answers a bare TCP connection with a MySQL protocol-version-10 server greeting whose capability flags advertise CLIENT_SSL (0x0800), and then refuses the non-TLS handshake with a SERVER-sent error — MySQL error number 3159 (`ER_SECURE_TRANSPORT_REQUIRED`, "Connections using insecure transport are prohibited while --require_secure_transport=ON."), surfaced to pymysql as `OperationalError` with errno 3159 — never a client-side CR_* code in the 2000–2999 range. The fixed text of that refusal is the MySQL server message for error 3159; it carries no request- or state-derived values. The same proxy, same backend, same credentials, with a certificate-verified TLS handshake, admits the login and runs SQL, and the client socket reports a TLSv1.x protocol version. After `ModifyDBProxy(RequireTLS=False)` and the proxy returning to `available` with `RequireTLS=false`, the identical plaintext login is admitted and runs SQL with no TLS session at all. `CreateDBProxy` returns 200 either way and `DescribeDBProxies` merely echoes `RequireTLS`, which is why this contract is only observable from inside the VPC.

Emulator: the check produces no assertion failure and no diverging response — it never reaches any of its graded assertions. It is cut off by the harness with `Error: Check timed out after 60.0s` while still inside its long-poll/handshake sequence, so none of beats 3 (verified TLS login), 4 (plaintext refusal) or 5 (RequireTLS=False admits plaintext) is ever evaluated. The control plane is implemented — services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_proxy.rs records `require_tls: input.require_tls.unwrap_or(false)` and mints an endpoint host via `crate::state::mint_endpoint_host(mint)`, and services/service-com-amazonaws-rds-amazonrdsv19/src/ops/describe_db_proxy_targets.rs projects registered targets — but nothing in the emulator ever listens on that minted endpoint host, so there is no MySQL greeting to read and no TLS requirement to enforce. Hypothesis (neither the model nor the recording states which hop stalls): the probe stalls on the TCP/handshake path to that endpoint — the socket to the minted proxy host never completes a MySQL greeting — and/or on the surrounding Lambda/VPC convergence waits, so the poll loops spin until the cap. Note the open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md sits on the same path for the probe Lambda; the replay report did NOT mark this check as owned by it, and that issue is about Invoke answering NotImplemented, not about the proxy having no MySQL data plane, so it does not cover this cause.

Why: a client that uses `RequireTLS=True` as its guarantee that no credential ever crosses the wire in plaintext gets the opposite of the AWS guarantee from the emulator. An integration test that asserts "a plaintext login is refused" cannot even be written against the emulator: instead of a 3159 refusal it gets a connection that never answers, so the test hangs rather than failing loudly, and a regression that drops the TLS requirement is invisible. Equally, a client that verifies its TLS path works (certificate-verified login, socket reports TLSv1.x) can get no such confirmation, so TLS-related application code is never exercised.

Fix: the RDS DB-proxy data plane. The minted proxy endpoint (`crate::state::mint_endpoint_host`, set in services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_proxy.rs) must be backed by a listener that speaks the MySQL handshake for `EngineFamily=MYSQL`: answer a protocol-version-10 greeting advertising CLIENT_SSL, then consult the proxy record's stored `require_tls` (services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs, `DbProxyRecord.require_tls`, which `ModifyDBProxy` already flips) and reject a non-TLS handshake with the server-sent MySQL error 3159, or accept it when the flag is false. The behavior belongs with the proxy record's state and its endpoint listener, not with the CreateDBProxy/ModifyDBProxy request handlers, which already store the flag correctly.

Traces: lambda__rds__secretsmanager.deep-rds-3-proxy-require-tls-plaintext-rejected (verdict mismatch; `Error: Check timed out after 60.0s`).

## Evidence
Failing check: lambda__rds__secretsmanager.deep-rds-3-proxy-require-tls-plaintext-rejected
Verdict: mismatch
Replay detail: `Error: Check timed out after 60.0s`

Expected (AWS), as the check's own assertions state them:
  - beat 3, verified TLS through the proxy: probe returns ok=True, echo == the SQL marker, and tls_version starts with "TLSv1".
  - beat 4, same login with ssl_disabled=True through the RequireTLS=True proxy:
      greeting == {"proto": 10, "ssl_capable": True}   (the proxy answered TCP with a MySQL v10 greeting advertising SSL)
      ok == False
      (cls, errno) == ("OperationalError", 3159)       (a SERVER-sent error, never a client-side CR_* code 2000-2999)
    Fixed text of the refusal = the MySQL server message for error 3159 (ER_SECURE_TRANSPORT_REQUIRED); it has no
    request- or state-derived variable parts.
  - beat 5, after ModifyDBProxy(RequireTLS=False) and the proxy returning Status=available with RequireTLS=false:
      the identical plaintext login returns ok=True, echo == the new marker, tls_version is None.

Actual (emulator): none of the above was ever observed. No assertion failed and no response diverged — the check was
terminated by the harness after 60.0s while still polling, so beats 3, 4 and 5 are all ungraded. The control plane
accepted everything: CreateDBProxy stores require_tls and mints an endpoint host
(services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_proxy.rs lines 307 and 318) and nothing in the
emulator serves MySQL on that host, so no greeting and no 3159 refusal can exist.

Related but non-covering open issue: 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md
(the probe Lambda's Invoke). The replay report did not mark this check as OWNED by it.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite (run_behavioral_suite); check
   lambda__rds__secretsmanager.deep-rds-3-proxy-require-tls-plaintext-rejected reports
   `mismatch` / `Error: Check timed out after 60.0s`.
2. Or drive it by hand against the emulator:
   a. Create a VPC with two private subnets in two AZs and one self-referencing security group open on TCP 3306;
      create an RDS DB subnet group over those subnets.
   b. rds create-db-instance --engine mysql (default orderable version, a small class), master user s2radmin, in that
      subnet group and security group; wait for DBInstanceStatus=available.
   c. secretsmanager create-secret with SecretString {"username":"s2radmin","password":"<pw>"}; create an IAM role
      trusted by rds.amazonaws.com allowing secretsmanager:GetSecretValue on that secret.
   d. rds create-db-proxy --engine-family MYSQL --require-tls --auth AuthScheme=SECRETS,SecretArn=<arn>,
      IAMAuth=DISABLED,ClientPasswordAuthType=MYSQL_NATIVE_PASSWORD --role-arn <role> --vpc-subnet-ids <subnets>
      --vpc-security-group-ids <sg>; wait for Status=available and confirm DescribeDBProxies reports RequireTLS=true.
   e. rds register-db-proxy-targets --db-instance-identifiers <instance>; wait for the RDS_INSTANCE target's
      TargetHealth.State=AVAILABLE.
   f. From inside the same VPC (e.g. a Lambda with vendored pymysql), open a bare TCP socket to the proxy's
      DBProxy.Endpoint on port 3306 and read the first MySQL packet.
      Expected: a protocol-version-10 server greeting with the CLIENT_SSL capability bit (0x0800) set.
      Observed: nothing is served on that host, so the read never yields a greeting.
   g. Connect with pymysql using ssl_disabled=True (plaintext).
      Expected: pymysql.err.OperationalError with errno 3159 (the MySQL server's ER_SECURE_TRANSPORT_REQUIRED message).
      Observed: never reached.
   h. Connect with pymysql using a default SSL context (check_hostname=True, CERT_REQUIRED), run `SELECT <marker>`.
      Expected: the query echoes the marker and the socket reports a TLSv1.x version. Observed: never reached.
   i. rds modify-db-proxy --no-require-tls; wait for Status=available with RequireTLS=false, then repeat (g).
      Expected: the plaintext login now succeeds and runs SQL with no TLS session. Observed: never reached.
