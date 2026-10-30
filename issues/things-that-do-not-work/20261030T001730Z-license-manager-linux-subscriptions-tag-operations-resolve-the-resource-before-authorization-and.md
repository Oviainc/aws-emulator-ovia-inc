Issue-Source: trace-sweep-divergence
Issue-Trace: license-manager-linux-subscriptions-tag-operations-foreign-account-arn-denied
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: license-manager-linux-subscriptions-tag-operations-foreign-account-arn-denied license-manager-linux-subscriptions-tag-resource-empty-tags-map-rejected
Issue-Shape-Id: com.amazonaws.licensemanagerlinuxsubscriptions#LicenseManagerLinuxSubscriptions
Issue-Key: license-manager-linux-subscriptions|TagResource|4xx->4xx|$.message
Issue-Key: license-manager-linux-subscriptions|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-linux-subscriptions|TagResource|4xx->4xx|status
Issue-Key: license-manager-linux-subscriptions|ListTagsForResource|4xx->4xx|$.message
Issue-Key: license-manager-linux-subscriptions|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-linux-subscriptions|ListTagsForResource|4xx->4xx|status
Issue-Key: license-manager-linux-subscriptions|UntagResource|4xx->4xx|$.message
Issue-Key: license-manager-linux-subscriptions|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-linux-subscriptions|UntagResource|4xx->4xx|status
Issue-Commit: 5be2d973ad1b703e2cd0a56e2c9e00ba2fe90954

# Tag operations resolve the resource before authorization and input validation, answering 404

> _Note: this issue was identified against the code base at commit `5be2d973ad1b703e2cd0a56e2c9e00ba2fe90954`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
All three tag-plane operations (TagResource, ListTagsForResource, UntagResource) run the subscription-provider lookup FIRST and let its miss answer the request, so every refusal that AWS raises before (or instead of) that lookup — the cross-account authorization denial and the empty-tags-map input check — is replaced by HTTP 404 with the emulator's "Resource was not found" and error type NotFoundException. Four causes, one shared ordering fault in the tag-plane handlers.

Cause 1 of 4: TagResource against a resourceArn owned by another account.
AWS: refuses with HTTP 403 and an authorization denial, not a not-found. Recorded behavior only (the Smithy model binds TagResource only ResourceNotFoundException and ValidationException and models no AccessDeniedException, and the fixture does not assert x-amzn-errortype). The recording's message follows AWS's standard IAM denial template: fixed text "User: ", then the calling principal's ARN, then the fixed text " is not authorized to access this resource". The principal ARN is a value taken from the caller's credentials (the recording's is arn:aws:iam::000000000000:user/emulator) and is not part of the requirement; the two fixed clauses are. AWS reaches this verdict from the ARN's account field alone — it does not first have to find the resource, because an ARN in a foreign account can never be read by this caller.
Emulator: HTTP 404, $.message "Resource was not found", x-amzn-errortype: NotFoundException. The handler parsed the ARN, looked the provider up in its own account's state, missed, and reported not-found.
Why: a client cannot distinguish "you may not touch this" from "this does not exist". Retry and error-mapping logic keys on the 4xx class: a 403 is a permanent authorization failure that SDK/IAM-aware code surfaces to the operator, while a 404 invites create-then-retry or is swallowed as an idempotent miss. Cross-account access control is therefore untestable against this emulator. The error type also leaks a shape name (NotFoundException) that is not the modeled ResourceNotFoundException.
Fix: the TagResource handler's pre-conditions (services/service-com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions/src/ops/, TagResource) — gate on the resourceArn's account segment against the caller's account before any state lookup, and answer the 403 denial with AWS's message template.
Traces: license-manager-linux-subscriptions-tag-operations-foreign-account-arn-denied#task#2.

Cause 2 of 4: ListTagsForResource against a resourceArn owned by another account.
AWS: same contract as cause 1 — HTTP 403 with "User: <caller principal ARN> is not authorized to access this resource" (recorded behavior only; Smithy binds ListTagsForResource only ResourceNotFoundException and ValidationException). GET /tags/{resourceArn}.
Emulator: HTTP 404, "Resource was not found", x-amzn-errortype NotFoundException — the same lookup-first ordering on the read path.
Why: a read of a foreign resource reports absence instead of denial, so a caller enumerating ARNs across accounts concludes the resources do not exist rather than that it lacks access.
Fix: the ListTagsForResource handler's pre-conditions — apply the same account gate ahead of the provider lookup.
Traces: license-manager-linux-subscriptions-tag-operations-foreign-account-arn-denied#task#3.

Cause 3 of 4: UntagResource against a resourceArn owned by another account.
AWS: same contract — HTTP 403 with "User: <caller principal ARN> is not authorized to access this resource" (recorded behavior only; Smithy binds UntagResource only ResourceNotFoundException). DELETE /tags/{resourceArn}.
Emulator: HTTP 404, "Resource was not found", x-amzn-errortype NotFoundException.
Why: as cause 1, on the delete path — a denied untag looks like an already-clean resource, so a caller treats the failed mutation as a no-op success-by-absence.
Fix: the UntagResource handler's pre-conditions — the same account gate ahead of the lookup.
Traces: license-manager-linux-subscriptions-tag-operations-foreign-account-arn-denied#task#4.

Cause 4 of 4: TagResource with an empty tags map.
AWS: refuses the request body before resolving the resource, with HTTP 400 ValidationException (Smithy: TagResource errors ValidationException, httpError 400 defaulted). The message is the fixed text "Invalid request input, expected non-empty tags map" — it has no variable parts. Note the model does NOT express this bound: TagResourceRequest.tags is @required with @length min 0 max 50, so an empty map is model-legal and the check is recorded behavior only. Because it is an input check, it fires regardless of whether the named ARN resolves.
Emulator: HTTP 404, $.message "Resource was not found", x-amzn-errortype NotFoundException — the handler resolved the ARN first and never evaluated the tags map.
Why: a client sending an empty tag set gets a not-found for a resource it may well own, so the bug in its own request is misattributed to missing state; and once the resource does exist the emulator presumably accepts the empty map, silently diverging on a request AWS always rejects.
Fix: the TagResource handler's input validation — validate the tags map (non-empty, plus the existing size bound) before the subscription-provider lookup, so validation precedes resource resolution on this operation.
Traces: license-manager-linux-subscriptions-tag-resource-empty-tags-map-rejected#task#1.

## Evidence
Fixture license-manager-linux-subscriptions-tag-operations-foreign-account-arn-denied:

  step task#2 (TagResource): expected status 403, got 404.
    expected $.message = 'User: arn:aws:iam::000000000000:user/emulator is not authorized to access this resource'
    actual   $.message = 'Resource was not found'
    $headers.x-amzn-errortype: expected <shaped-not-asserted>, got 'NotFoundException'

  step task#3 (ListTagsForResource): expected status 403, got 404; same expected/actual message pair; got x-amzn-errortype 'NotFoundException'.

  step task#4 (UntagResource): expected status 403, got 404; same expected/actual message pair; got x-amzn-errortype 'NotFoundException'.

Fixture license-manager-linux-subscriptions-tag-resource-empty-tags-map-rejected:

  step task#1 (TagResource, empty tags map): expected status 400, got 404.
    expected $.message = 'Invalid request input, expected non-empty tags map'
    actual   $.message = 'Resource was not found'
    $headers.x-amzn-errortype: expected <shaped-not-asserted>, got 'NotFoundException'

Smithy grounding: TagResource PUT /tags/{resourceArn} errors [ResourceNotFoundException, ValidationException]; ListTagsForResource GET /tags/{resourceArn} errors [ResourceNotFoundException, ValidationException]; UntagResource DELETE /tags/{resourceArn} errors [ResourceNotFoundException]. ValidationException httpError 400. TagResourceRequest.tags is @required, a sensitive map with @length min 0 max 50 — the non-empty rule and the 403 denial are recorded behavior only. No modeled error shape is named NotFoundException.

## Steps to Reproduce
Cross-account denial (causes 1-3):
1. Boot the emulator and replay fixture license-manager-linux-subscriptions-tag-operations-foreign-account-arn-denied.
2. Step task#2: PUT /tags/{resourceArn} where resourceArn's account segment is an account other than the caller's, with a non-empty tags map. Observe HTTP 404 / NotFoundException / 'Resource was not found' instead of HTTP 403 / 'User: <caller ARN> is not authorized to access this resource'.
3. Step task#3: GET /tags/{resourceArn} with the same foreign ARN — same 404 instead of 403.
4. Step task#4: DELETE /tags/{resourceArn} with the same foreign ARN and a tagKeys selection — same 404 instead of 403.

Empty tags map (cause 4):
5. Replay fixture license-manager-linux-subscriptions-tag-resource-empty-tags-map-rejected.
6. Step task#1: PUT /tags/{resourceArn} with "tags": {} . Observe HTTP 404 'Resource was not found' instead of HTTP 400 ValidationException 'Invalid request input, expected non-empty tags map'.
