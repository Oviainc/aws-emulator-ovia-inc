Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__logs.deep-apigateway-1-apigw-methodsetting-logging-level-and-data-trace-gate-execution-logs
Issue-Key: apigateway__dynamodb__logs|GetAccount|NotImplemented
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# apigateway__dynamodb__logs: the method-setting execution-log gating check aborts because apigateway GetAccount answers 501 NotImplemented

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccount

## Description
Cause 1 of 1: GetAccount answers 501 NotImplemented, so the check cannot read (or establish) the account's CloudWatch role before driving traffic, and the execution-log gating assertions never run.
AWS: Smithy: GetAccount, http GET /account, success 200 returning the Account representation (cloudwatchRoleArn, throttleSettings, features, apiKeyVersion); its modeled errors are BadRequestException 400, NotFoundException 404, TooManyRequestsException 429, UnauthorizedException 401.
Emulator: HTTP 501 with error code `NotImplemented` and message "operation GetAccount is scaffolded but not implemented in this emulator".
Why: `NotImplemented` is not in GetAccount's modeled error set, so no client error branch matches it; and because the account-level CloudWatch role is unreadable, every flow that must confirm execution logging is wired (stage loggingLevel / dataTrace gating of delivered log lines) is blocked at its first call.
Fix: the GetAccount handler, services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/get_account.rs. Open issue 20261023T125454Z-apigateway-getaccount-stub.md names that file but does not carry this replay key.
Traces: apigateway__dynamodb__logs.deep-apigateway-1-apigw-methodsetting-logging-level-and-data-trace-gate-execution-logs.

## Evidence
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetAccount operation: operation GetAccount is scaffolded but not implemented in this emulator. Expected: HTTP 200 with the Account representation.

## Steps to Reproduce
1. Call apigateway get_account.
2. Observe HTTP 501 NotImplemented instead of 200 with the Account representation.
3. Or replay apigateway__dynamodb__logs.deep-apigateway-1-apigw-methodsetting-logging-level-and-data-trace-gate-execution-logs.
