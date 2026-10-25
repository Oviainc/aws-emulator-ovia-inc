Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.update-api-key-invalid-patch-bad-request apigateway.update-api-key-value-field-unsupported-rejected
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.update-api-key-invalid-patch-bad-request|run#13|AssertionError|assert err.get("Message") == expected_message, (
Issue-Key: apigateway.update-api-key-value-field-unsupported-rejected|run#12|AssertionError|assert err.get("Message") == expected_message, (
Issue-Commit: 3e2c83ced69a5d134aa927e429a7d731168ed0a2

# UpdateApiKey's invalid-patch-path refusal carries an abbreviated message: it omits the op and the list of supported paths AWS names

> _Note: this issue was identified against the code base at commit `3e2c83ced69a5d134aa927e429a7d731168ed0a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateApiKey

## Description
One net-new root cause in the apigateway service. No open issue carries either of these two replay keys. The nearest open issue, 20261024T043741Z-apigateway-api-gateway-6-control-plane-convergence-causes-updateaccount.md, describes patch-op/path validation defects too, but on UpdateAccount's /cloudwatchRoleArn (accept/refuse behavior), not on UpdateApiKey's refusal MESSAGE, and it does not carry these keys.

Cause 1 of 1: UpdateApiKey refuses an unsupported JSON-Patch path with the right status and the right error code, but with a message that drops the op and the list of legal paths.
AWS: Smithy: UpdateApiKey (PATCH /apikeys/{apiKey}, input apiKey @httpLabel required + patchOperations: ListOfPatchOperation, output ApiKey) models BadRequestException 400, ConflictException 409, LimitExceededException 429, NotFoundException 404, TooManyRequestsException 429, UnauthorizedException 401. Recorded behavior for the BadRequestException raised by an unsupported patch path: HTTP 400, code BadRequestException, message of the form
  Invalid patch path  '<path>' specified for op '<op>'. Must be one of: [/description, /enabled, /name, /customerId]
Its fixed text is "Invalid patch path  '" (note the DOUBLE space after "path"), "' specified for op '", "'. Must be one of: [/description, /enabled, /name, /customerId]". Its variable parts are <path> and <op>, both taken verbatim from the offending entry of the request's patchOperations; the bracketed list is the fixed set of patch paths UpdateApiKey supports and is the same for every rejected path on this operation. Two recordings exercise it: path '/thisFieldDoesNotExist' (a path that does not exist at all) and path '/value' (a real, but immutable, ApiKey member) — both with op 'replace', both producing the same template, which is what shows the list and the op are part of the template and not of one recording.
Emulator: HTTP 400 and code BadRequestException are correct, but the message is only "Invalid patch path <path>" — e.g. 'Invalid patch path /thisFieldDoesNotExist' and 'Invalid patch path /value'. Single space, no quoting of the path, no "specified for op '<op>'" clause, and no "Must be one of: [...]" enumeration.
Why: the message is the only part of this refusal that tells a caller WHICH of several patch operations in the batch was bad and what it could have sent instead. A client, CLI user or IaC tool that surfaces or parses the message gets no legal-path list, so a bad patch path cannot be self-corrected from the response; tooling that asserts on or diffs AWS's message text (and the mutability of /value in particular, which AWS communicates purely through this enumeration) diverges, and the op is unattributable when patchOperations carries more than one entry.
Fix: the UpdateApiKey handler's patch validation / its error construction (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/update_api_key.rs) — build the BadRequestException message from the template above, interpolating the rejected path and its op and emitting UpdateApiKey's supported-path list. Both recordings show the surrounding behavior is already right: the positive controls (replace /description, replace /name) succeed, the rejected patch mutates nothing, the prior valid patch survives, and the key stays deletable — only the message text diverges.
Traces: apigateway.update-api-key-invalid-patch-bad-request, apigateway.update-api-key-value-field-unsupported-rejected.

## Evidence
Cause 1, check apigateway.update-api-key-invalid-patch-bad-request (mismatch):
  AssertionError: expected "Invalid patch path  '/thisFieldDoesNotExist' specified for op 'replace'. Must be one of: [/description, /enabled, /name, /customerId]", got 'Invalid patch path /thisFieldDoesNotExist'
  (the preceding assertion, err.get('Code') == 'BadRequestException', passed — status 400 and code are correct)

Cause 1, check apigateway.update-api-key-value-field-unsupported-rejected (mismatch):
  AssertionError: expected "Invalid patch path  '/value' specified for op 'replace'. Must be one of: [/description, /enabled, /name, /customerId]", got 'Invalid patch path /value'

Diverging requirement, stated as a template: status 400; code BadRequestException; message "Invalid patch path  '<path>' specified for op '<op>'. Must be one of: [/description, /enabled, /name, /customerId]" where <path> and <op> come from the request's rejected patchOperations entry and the bracketed list is fixed for UpdateApiKey. Emulator message: "Invalid patch path <path>" — fixed text truncated after the path, op clause and legal-path enumeration absent, path unquoted, single space.

## Steps to Reproduce
1. apigw.create_api_key(name=<unique>, enabled=True) -> note id.
2. Positive control: apigw.update_api_key(apiKey=<id>, patchOperations=[{'op':'replace','path':'/description','value':'x'}]) -> succeeds (also works for '/name'), proving the rejection below is specific to the path.
3. apigw.update_api_key(apiKey=<id>, patchOperations=[{'op':'replace','path':'/thisFieldDoesNotExist','value':'x'}]) -> observe 400 BadRequestException with message 'Invalid patch path /thisFieldDoesNotExist' instead of "Invalid patch path  '/thisFieldDoesNotExist' specified for op 'replace'. Must be one of: [/description, /enabled, /name, /customerId]".
4. apigw.update_api_key(apiKey=<id>, patchOperations=[{'op':'replace','path':'/value','value':'<any>'}]) -> same divergence with '/value' interpolated.
5. Replay apigateway.update-api-key-invalid-patch-bad-request and apigateway.update-api-key-value-field-unsupported-rejected to confirm.
