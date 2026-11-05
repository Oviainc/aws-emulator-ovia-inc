Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.reject-domain-name-access-association-invalid-fails apigateway.reject-domain-name-access-association-without-pending-fails apigateway.concurrent-create-domain-name-access-association-race apigateway.concurrent-domain-name-access-association-request-dedup apigateway.concurrent-delete-domain-name-access-association-race apigateway.stage-cache-ttl-out-of-range-rejected apigateway.update-stage-invalid-loglevel-rejected apigateway.update-model-invalid-patch-path-rejected apigateway.update-usage-plan-invalid-quota-period-rejected apigateway.tag-resource-invalid-arn-rejected
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.reject-domain-name-access-association-invalid-fails|run#6|AssertionError|assert message == "Invalid domain name ARN specified.", f"unexpected message: {message!r}"
Issue-Key: apigateway.reject-domain-name-access-association-without-pending-fails|run#8|AssertionError|assert message == "Invalid domain name ARN specified.", f"unexpected message: {message!r}"
Issue-Key: apigateway.concurrent-create-domain-name-access-association-race|run#10|AssertionError|assert err_code == "BadRequestException", (
Issue-Key: apigateway.concurrent-domain-name-access-association-request-dedup|run#16|AssertionError|assert code == "BadRequestException", f"unexpected error code on duplicate association: {code}"
Issue-Key: apigateway.concurrent-delete-domain-name-access-association-race|run#22|AssertionError|assert not unexpected, (
Issue-Key: apigateway.stage-cache-ttl-out-of-range-rejected|run#18|AssertionError|assert message == "Invalid time-to-live setting, must be an integer no greater than 3600", (
Issue-Key: apigateway.update-stage-invalid-loglevel-rejected|run#5|AssertionError|assert err.get("Message") == "Invalid logging level specified: NOT_A_LEVEL", (
Issue-Key: apigateway.update-model-invalid-patch-path-rejected|run#11|AssertionError|assert err.get("Message") == (
Issue-Key: apigateway.update-usage-plan-invalid-quota-period-rejected|run#7|AssertionError|assert (err.get("Code"), err.get("Message")) == ('BadRequestException', 'Invalid Usage Plan quota period specified. Must be one of [DAY, WEEK, MONTH]'), (
Issue-Key: apigateway.tag-resource-invalid-arn-rejected|run#14|AssertionError|assert err.get("Message") == "Invalid ARN specified in the request", (
Issue-Commit: c8d80ea15f1454d07f388eb2b932b3a5430349f4

# API Gateway refusal contracts, part 3: 8 root causes (domain name access association ARN/duplicate/absent contracts, UpdateStage, UpdateModel, UpdateUsagePlan and TagResource messages)

> _Note: this issue was identified against the code base at commit `c8d80ea15f1454d07f388eb2b932b3a5430349f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RejectDomainNameAccessAssociation

## Description
This issue bundles 8 further NET-NEW root causes where an API Gateway refusal diverges from AWS's error code or message. A fixer must close all 8 before submitting.

Cause 1 of 8: RejectDomainNameAccessAssociation with a malformed or unknown ARN.
AWS: a malformed ARN pair is refused with BadRequestException 'Invalid domain name ARN specified.' (fixed text, no variable part); a well-formed but non-pending/nonexistent association is refused with the same BadRequestException text, and a well-formed but nonexistent association/domain pair with NotFoundException "This private custom domain name doesn't have any domain name access associations." Smithy: RejectDomainNameAccessAssociation (POST /rejectdomainnameaccessassociations) takes domainNameAccessAssociationArn and domainNameArn as required httpQuery members and declares BadRequestException (400) and NotFoundException (404).
Emulator: 'Invalid ARN specified in the request not-a-valid-arn' and 'Invalid ARN specified in the request arn:aws:apigateway:us-west-2::/domainnameaccessassociations/domainname/<domain>+missing/vpcesource/<vpceId>' — a generic ARN message with the submitted value appended.
Why: a client cannot distinguish a malformed ARN from a missing association, and the documented text never appears.
Fix: RejectDomainNameAccessAssociation's ARN validation and not-found messages.
Traces: apigateway.reject-domain-name-access-association-invalid-fails, apigateway.reject-domain-name-access-association-without-pending-fails

Cause 2 of 8: a duplicate domain name access association uses the wrong error class.
AWS: creating a second access association for a VPC endpoint already associated with the domain is refused with BadRequestException (400); under a concurrent race the losing racer also gets BadRequestException.
Emulator: ConflictException (409) with the message 'The access association source <vpceId> is already associated with the domain name <domainNameArn>: <associationArn>'. The open issue 20261027T111730Z-apigateway-the-domain-name-access-association-surface-diverges.md covers other checks of this surface but not these two.
Why: a client that retries on 409 and fails fast on 400 takes the wrong branch, and request dedup cannot be observed.
Fix: CreateDomainNameAccessAssociation's duplicate-source refusal (error class and message).
Traces: apigateway.concurrent-create-domain-name-access-association-race, apigateway.concurrent-domain-name-access-association-request-dedup

Cause 3 of 8: DeleteDomainNameAccessAssociation on a domain with no associations.
AWS: NotFoundException "This private custom domain name doesn't have any domain name access associations." (fixed text, no variable part) — so under a concurrent double delete every losing racer gets that message.
Emulator: 'Invalid domain name access association identifier specified', and one racer's delete reports plain success.
Why: the losing racer cannot tell that the domain has no associations left.
Fix: DeleteDomainNameAccessAssociation's not-found message and its delete-race arbitration.
Traces: apigateway.concurrent-delete-domain-name-access-association-race

Cause 4 of 8: UpdateStage with a cache TTL above the maximum.
AWS: BadRequestException 'Invalid time-to-live setting, must be an integer no greater than 3600' (fixed text, no variable part).
Emulator: '/*/*/caching/ttlInSeconds must be between 0 and 3600, inclusive: 3601' — a patch-path-shaped message with the submitted value appended.
Why: the documented refusal text is never produced.
Fix: UpdateStage's methodSettings caching validation message.
Traces: apigateway.stage-cache-ttl-out-of-range-rejected

Cause 5 of 8: UpdateStage with an invalid logging level.
AWS: BadRequestException 'Invalid logging level specified: NOT_A_LEVEL' — fixed text 'Invalid logging level specified: ' plus the submitted value.
Emulator: '/*/*/logging/loglevel takes one of OFF, ERROR, INFO, got "NOT_A_LEVEL"'.
Why: the documented refusal text is never produced.
Fix: UpdateStage's methodSettings logging validation message.
Traces: apigateway.update-stage-invalid-loglevel-rejected

Cause 6 of 8: UpdateModel with an unsupported patch path.
AWS: BadRequestException "Invalid patch path  '/name' specified for op 'replace'. Must be one of: [/description, /schema]" — fixed text as shown including the double space after 'path', with the submitted path, the submitted op and the operation's allowed path list substituted.
Emulator: 'Invalid patch path /name'.
Why: the message does not tell the caller which paths are allowed, and does not match AWS byte for byte.
Fix: the patch-path validation message in UpdateModel (shared patch validator).
Traces: apigateway.update-model-invalid-patch-path-rejected

Cause 7 of 8: UpdateUsagePlan with an invalid quota period.
AWS: BadRequestException whose message names the invalid quota period in AWS's own wording (the check asserts the exact text; the error class BadRequestException already matches).
Emulator: '/quota/period takes one of DAY, MONTH, WEEK, got "FORTNIGHT"' — a patch-path-shaped message with the submitted value appended. Hypothesis: as with the other patch validators above, the emulator reports the JSON-patch path instead of the operation's own refusal wording.
Why: a client matching the documented text cannot classify the refusal.
Fix: UpdateUsagePlan's quota validation message.
Traces: apigateway.update-usage-plan-invalid-quota-period-rejected

Cause 8 of 8: TagResource with a malformed resourceArn appends the ARN to the message.
AWS: BadRequestException 'Invalid ARN specified in the request' — entirely fixed text; the offending ARN is NOT included. Smithy: TagResource (PUT /tags/{resourceArn}, 204) takes resourceArn as a required httpLabel and declares BadRequestException (400).
Emulator: 'Invalid ARN specified in the request sim2real-behav-not-an-arn'.
Why: byte-exact comparison on the documented message fails for every malformed-ARN tag call.
Fix: TagResource's resourceArn validation message.
Traces: apigateway.tag-resource-invalid-arn-rejected

## Evidence
Representative replay output (verbatim):
- 'AssertionError: unexpected message: 'Invalid ARN specified in the request not-a-valid-arn'' against the asserted 'Invalid domain name ARN specified.' (Cause 1)
- 'AssertionError: losing racer did not get BadRequestException, got ConflictException: The access association source vpce-00000000000000038 is already associated with the domain name arn:aws:apigateway:us-west-2::/domainnames/...' and 'unexpected error code on duplicate association: ConflictException' (Cause 2)
- 'AssertionError: expected any losing delete to fail with NotFoundException "This private custom domain name doesn't have any domain name access associations.", got [('ok', None), ('err', ('NotFoundException', 'Invalid domain name access association identifier specified'))]' (Cause 3)
- 'AssertionError: unexpected message: '/*/*/caching/ttlInSeconds must be between 0 and 3600, inclusive: 3601'' against 'Invalid time-to-live setting, must be an integer no greater than 3600' (Cause 4)
- 'AssertionError: expected the exact invalid-logging-level refusal message, got '/*/*/logging/loglevel takes one of OFF, ERROR, INFO, got "NOT_A_LEVEL"'' against 'Invalid logging level specified: NOT_A_LEVEL' (Cause 5)
- 'AssertionError: expected the exact patch-path refusal message, got 'Invalid patch path /name'' against "Invalid patch path  '/name' specified for op 'replace'. Must be one of: [/description, /schema]" (Cause 6)
- 'AssertionError: expected BadRequestException with the quota period message, got {'Message': '/quota/period takes one of DAY, MONTH, WEEK, got "FORTNIGHT"', 'Code': 'BadRequestException'}' (Cause 7)
- 'AssertionError: expected the exact malformed-ARN refusal message, got 'Invalid ARN specified in the request sim2real-behav-not-an-arn'' against 'Invalid ARN specified in the request' (Cause 8)

## Steps to Reproduce
1. RejectDomainNameAccessAssociation with domainNameAccessAssociationArn='not-a-valid-arn' and with a well-formed but nonexistent pair (Cause 1).
2. Create a private custom domain name and a VPC-endpoint access association, then create the same association again, concurrently and serially (Cause 2); delete it twice concurrently (Cause 3).
3. UpdateStage with patchOperations [{'op':'replace','path':'/*/*/caching/ttlInSeconds','value':'3601'}] (Cause 4) and with '/*/*/logging/loglevel' = 'NOT_A_LEVEL' (Cause 5).
4. UpdateModel with patchOperations [{'op':'replace','path':'/name','value':'RenamedThing'}] (Cause 6).
5. UpdateUsagePlan with quota period 'FORTNIGHT' (Cause 7).
6. TagResource with resourceArn='sim2real-behav-not-an-arn' (Cause 8).
