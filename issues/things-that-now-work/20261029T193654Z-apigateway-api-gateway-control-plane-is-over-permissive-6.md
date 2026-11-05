Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.create-domain-name-malformed-certificate-arn-rejected apigateway.delete-domain-name-with-mapping-rejected apigateway.delete-stage-referenced-by-base-path-mapping-conflict apigateway.delete-stage-with-base-path-mapping-conflict apigateway.domain-name-unicode-basepath-round-trip apigateway.usage-plan-invalid-quota-offset-rejected apigateway.apigw-import-rest-api-fail-on-warnings-rejects-and-leaves-no-api-behind
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.create-domain-name-malformed-certificate-arn-rejected|_error#9|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: apigateway.delete-domain-name-with-mapping-rejected|_error#18|AssertionError|raise AssertionError("real AWS refuses this call, but it SUCCEEDED")
Issue-Key: apigateway.delete-stage-referenced-by-base-path-mapping-conflict|_client_error#14|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: apigateway.delete-stage-with-base-path-mapping-conflict|_client_error#12|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: apigateway.domain-name-unicode-basepath-round-trip|_error#21|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: apigateway.usage-plan-invalid-quota-offset-rejected|_client_error#10|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: apigateway.apigw-import-rest-api-fail-on-warnings-rejects-and-leaves-no-api-behind|run#9|AssertionError|assert message == expected_message, (
Issue-Commit: c8d80ea15f1454d07f388eb2b932b3a5430349f4

# API Gateway control plane is over-permissive: 6 root causes where the emulator accepts a request real AWS refuses

> _Note: this issue was identified against the code base at commit `c8d80ea15f1454d07f388eb2b932b3a5430349f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomainName

## Description
This issue bundles 6 NET-NEW root causes where the emulator ACCEPTS a control-plane request that real AWS refuses. An over-permissive emulator lets a negative test pass that must fail. A fixer must close all 6 before submitting.

Cause 1 of 6: CreateDomainName accepts a malformed certificateArn.
AWS: certificateArn must be an ACM certificate ARN; a value that is not a well-formed ARN is refused with BadRequestException 'Invalid ARN specified in the request' (fixed text, no variable part). Smithy: CreateDomainName (POST /domainnames, 201) models certificateArn as String and declares BadRequestException (400).
Emulator: returns 201 and stores the domain with certificateArn 'not-a-well-formed-acm-arn', reporting domainNameStatus PENDING.
Why: a client gets a domain name that could never exist on AWS, and the ARN validation it depends on never runs.
Fix: CreateDomainName's input validation.
Traces: apigateway.create-domain-name-malformed-certificate-arn-rejected

Cause 2 of 6: DeleteDomainName succeeds while a base path mapping still references the domain.
AWS: deleting a domain name that still has base path mappings is refused with BadRequestException whose message names the remaining mapping.
Emulator: the call SUCCEEDED.
Why: the mapping is orphaned and the referential guard clients rely on is absent.
Fix: DeleteDomainName's precondition check against base-path-mapping state.
Traces: apigateway.delete-domain-name-with-mapping-rejected

Cause 3 of 6: DeleteStage succeeds while a base path mapping references the stage.
AWS: refused with BadRequestException naming the referencing base path mapping; the stage survives.
Emulator: returns HTTP 202 with an empty body and deletes the stage.
Why: a custom domain keeps a mapping to a stage that no longer exists.
Fix: DeleteStage's precondition check against base-path-mapping state.
Traces: apigateway.delete-stage-referenced-by-base-path-mapping-conflict, apigateway.delete-stage-with-base-path-mapping-conflict

Cause 4 of 6: CreateBasePathMapping accepts a non-ASCII base path.
AWS: a base path containing non-ASCII characters is refused; the character class for a base path does not admit them.
Emulator: returns 201 with basePath 'café-日本語'.
Why: a mapping is created that AWS would never accept, and URL construction diverges.
Fix: CreateBasePathMapping's basePath validation.
Traces: apigateway.domain-name-unicode-basepath-round-trip

Cause 5 of 6: CreateUsagePlan accepts a nonzero quota offset in the DAY period.
AWS: refused with BadRequestException 'Usage Plan quota offset must be zero in the DAY period' (fixed text, no variable part).
Emulator: returns 201 with quota {'limit': 50, 'offset': 50, 'period': 'DAY'}.
Why: a usage plan is created whose metering window AWS rejects outright.
Fix: CreateUsagePlan quota validation.
Traces: apigateway.usage-plan-invalid-quota-offset-rejected

Cause 6 of 6: ImportRestApi with failOnWarnings does not refuse with AWS's wording naming the warning.
AWS: with failOnWarnings=true an unresolvable schema reference refuses the import with BadRequestException whose message names the warning it refused to tolerate, in AWS's wording, and leaves no API behind (the check asserts the exact text).
Emulator: refuses with the message 'Warnings found during import:\n\tUnable to resolve reference '#/components/schemas/Item'. Ignoring.' — fixed text 'Warnings found during import:' plus the emulator's own warning line; the reference pointer comes from the submitted document.
Why: a client that matches on the refusal text to report which part of its OpenAPI document failed cannot do so.
Fix: ImportRestApi's failOnWarnings refusal message.
Traces: apigateway.apigw-import-rest-api-fail-on-warnings-rejects-and-leaves-no-api-behind

## Evidence
Representative replay output (verbatim):
- 'AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {... 'certificateArn': 'not-a-well-formed-acm-arn', ... 'domainNameStatus': 'PENDING' ...}' (Cause 1)
- 'AssertionError: real AWS refuses this call, but it SUCCEEDED' (Cause 2)
- 'AssertionError: expected a ClientError, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 202, ...}}' and 'expected the call to be rejected, but it succeeded and returned {... 202 ...}' (Cause 3)
- 'AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {... 'basePath': 'café-日本語', ...}' (Cause 4)
- 'AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {... 'quota': {'limit': 50, 'offset': 50, 'period': 'DAY'} ...}'; the check asserts ('BadRequestException', 'Usage Plan quota offset must be zero in the DAY period') (Cause 5)
- 'AssertionError: the rejection must name the warning it refused to tolerate, got "Warnings found during import:\n\tUnable to resolve reference '#/components/schemas/Item'. Ignoring."' (Cause 6)

## Steps to Reproduce
1. CreateDomainName with certificateArn='not-a-well-formed-acm-arn' — it returns 201 instead of BadRequestException 'Invalid ARN specified in the request' (Cause 1).
2. Create a domain name and a base path mapping, then DeleteDomainName (Cause 2) and DeleteStage on the mapped stage (Cause 3).
3. CreateBasePathMapping with basePath='café-日本語' (Cause 4).
4. CreateUsagePlan with quota={'limit':50,'offset':50,'period':'DAY'} (Cause 5).
5. ImportRestApi with failOnWarnings=true and a document containing an unresolvable $ref, and read the refusal message (Cause 6).
