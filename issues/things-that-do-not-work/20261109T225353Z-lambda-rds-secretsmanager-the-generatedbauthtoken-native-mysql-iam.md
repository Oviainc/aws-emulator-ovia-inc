Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__rds__secretsmanager.lambda-rds-generate-db-auth-token-positive
Issue-Key: lambda__rds__secretsmanager.lambda-rds-generate-db-auth-token-positive|_invoke#20|Timeout|resp = lam.invoke(FunctionName=name, Payload=json.dumps(payload).encode(), InvocationType="RequestResponse")
Issue-Commit: 424434639e8b1acdb0579cf5577ef0e9dbf90686

# lambda__rds__secretsmanager: the GenerateDBAuthToken -> native MySQL IAM-auth round trip from an in-VPC Lambda reaches no verdict — the check hangs to the 60 s harness cap

> _Note: this issue was identified against the code base at commit `424434639e8b1acdb0579cf5577ef0e9dbf90686`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GenerateDBAuthToken

## Description
Cause 1 of 1: the IAM-database-authentication trajectory driven from an in-VPC Lambda never completes, so the check is cut off by the harness cap with no verdict.

AWS: recorded behavior only — GenerateDBAuthToken is a client-side signing helper (no wire operation), so the Smithy model states nothing about it. The recording shows the full trajectory succeeding: rds.CreateDBInstance(Engine="mysql", EnableIAMDatabaseAuthentication=True) reaches "available" and publishes Endpoint.Address/Port; a Lambda in the DB's VPC, whose execution role allows rds-db:connect on arn:aws:rds-db:<region>:<account>:dbuser:<DbiResourceId>/iamuser, calls boto3 rds.generate_db_auth_token(DBHostname, Port, DBUsername="iamuser") and uses the token as the MySQL password over TLS; the server authenticates the AWSAuthenticationPlugin user and the session answers SELECT 1 with 1 and SELECT CURRENT_USER() with the granted `iamuser` identity.

Emulator: the check produces no response at all — it is terminated by the harness with `Check timed out after 60.0s`. Nothing in the trajectory reaches a verdict: no error code, no status, no reply. The emulator stalls somewhere between creating the IAM-auth-enabled MySQL instance, having it become available with a reachable published endpoint, bringing up the in-VPC Lambda, and serving the native MySQL IAM-token login. Hypothesis: the same unreachable-published-endpoint and inert-native-login behavior that blocks the sibling RDS endpoint checks leaves a connect attempt hanging rather than being refused, so the step never returns.

Why: a client cannot tell success from failure — an operation that AWS answers promptly (either authenticating the token or refusing it with a definite MySQL access-denied error) instead hangs, so callers and orchestrators that rely on a bounded response block indefinitely, and the whole IAM-database-authentication feature is unverifiable. A hang is strictly worse than a wrong answer: there is no code to branch on.

Fix: the RDS native MySQL endpoint's IAM-authentication login path and the DB-instance endpoint publication it depends on, plus the in-VPC Lambda invoke path that drives it — each step must return a bounded response (authenticate, or refuse with the MySQL access-denied error AWS returns for a bad/expired token) instead of blocking. Related but NOT the same work item: open issue 20261109T135215Z-lambda-rds-secretsmanager-db-proxy-endpoints-are-published.md covers DB *proxy* endpoints being published on rds.amazonaws.com hosts; this check uses the instance's own endpoint and is not marked owned by it. Precedent for filing a harness-cap hang as an emulator defect: 20261109T171151Z-firehose-lambda-s3-the-httpendpointdestination-retry-then-persist.md.

Traces: lambda__rds__secretsmanager.lambda-rds-generate-db-auth-token-positive.

## Evidence
Expected (AWS, per the recording): the in-VOC Lambda's "setup" invocation creates `iamuser` IDENTIFIED WITH AWSAuthenticationPlugin AS 'RDS' and grants SELECT; the "connect" invocation generates an auth token with boto3 and returns {"ok": true, "value": 1, "current_user": "iamuser@..."} — a bounded, successful response.

Actual (emulator): no response of any kind; the suite reports

  #### [mismatch] lambda__rds__secretsmanager.lambda-rds-generate-db-auth-token-positive
  Error: Check timed out after 60.0s

No HTTP status, no error code and no MySQL error is produced — the trajectory simply does not terminate within the harness cap.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. rds.CreateDBInstance(Engine="mysql", DBInstanceClass="db.t3.micro", MasterUsername="admin", EnableIAMDatabaseAuthentication=True, PubliclyAccessible=False, with a DB subnet group over the default VPC's subnets and a VPC security group); wait for "available" and read Endpoint.Address/Port and DbiResourceId.
3. Create an IAM role allowing rds-db:connect on arn:aws:rds-db:<region>:<account>:dbuser:<DbiResourceId>/iamuser plus VPC access, and a Lambda function in those subnets using it.
4. Invoke the function with action="setup" to create the AWSAuthenticationPlugin user, then with action="connect" so it calls generate_db_auth_token and logs in to MySQL over TLS with the token as the password.
5. Observe that no step returns — the check is killed at the 60 s harness cap with no verdict.
6. Replay lambda__rds__secretsmanager.lambda-rds-generate-db-auth-token-positive to confirm.
