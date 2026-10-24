Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: aeaddd52642475574d853bf8e089296dd7f2f86c

# CreateCliToken's not-found message uses the control-plane wording instead of the env endpoint's fixed "Environment not found"

_Filed: 2026-10-24T00:24:30Z_
_Operation: CreateCliToken | Class: error-response_

> _Note: this issue was identified against the code base at commit `aeaddd52642475574d853bf8e089296dd7f2f86c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateCliToken is hosted on the environment data endpoint (`@endpoint` hostPrefix `env.`), the same front door as CreateWebLoginToken and InvokeRestApi. For those env.*-hosted operations the not-found refusal carries the endpoint's FIXED wording, which names no environment and ends without a period: "Environment not found". Only the control-plane operations (GetEnvironment, UpdateEnvironment, DeleteEnvironment) build the request-derived sentence "Environment <Name> not found.".

Expected: CreateCliToken for a name matching no environment -> HTTP 404, error code ResourceNotFoundException, message "Environment not found".

Actual: the handler calls the shared control-plane message builder, so the message is "Environment <Name> not found." — a different sentence for the same condition, and the requested environment name leaks into a message the service keeps name-free. Status (404) and wire code (ResourceNotFoundException) already match.

## Evidence

Spec: smithy_query shows CreateCliToken with `endpoint.hostPrefix = "env."` and its single modeled error ResourceNotFoundException (404) — the same host prefix as CreateWebLoginToken and InvokeRestApi. The env-endpoint wording rule is established by recorded behavior for the sibling operations: the open issue 20261023T180451Z (cause 3) requires CreateWebLoginToken to answer the fixed 'Environment not found', and src/state.rs:501 already holds that constant (`ENVIRONMENT_ENDPOINT_NOT_FOUND_MESSAGE`) used by src/ops/invoke_rest_api.rs:199. Handler: src/ops/create_cli_token.rs:160-164 uses `crate::state::environment_not_found_message(&input.name)` instead, and src/state.rs:482-487 explicitly lists CreateCliToken among the operations wired to the control-plane builder. No existing issue mentions CreateCliToken, and no test asserts the env-endpoint wording for it.

## Steps to Reproduce

1. Boot the emulator and point a botocore mwaa client at it.
2. mwaa.create_cli_token(Name='mwaa-missing-env') for a name that was never created.
3. Expected: ClientError HTTP 404, Error.Code 'ResourceNotFoundException', Message 'Environment not found'.
4. Observed: HTTP 404 / ResourceNotFoundException with Message 'Environment mwaa-missing-env not found.'
