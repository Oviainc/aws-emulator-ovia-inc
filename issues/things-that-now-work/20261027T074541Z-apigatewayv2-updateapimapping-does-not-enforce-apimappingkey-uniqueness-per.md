Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.concurrent-api-mapping-update-conflict
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.concurrent-api-mapping-update-conflict|run#23|AssertionError|assert len(successes) == 1, f"expected exactly one winner, got {(outcome1, outcome2)!r}"
Issue-Commit: e789e6950867e4c35fee326ce842c6986d4f19e6

# apigatewayv2: UpdateApiMapping does not enforce ApiMappingKey uniqueness per domain — racing key claims yield no ConflictException (1 cause)

> _Note: this issue was identified against the code base at commit `e789e6950867e4c35fee326ce842c6986d4f19e6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateApiMapping

## Description
One net-new root cause in apigatewayv2's api-mapping control plane, grounded in the failing check apigatewayv2.concurrent-api-mapping-update-conflict.

Cause 1 of 1: UpdateApiMapping neither serves the update nor enforces the per-domain uniqueness of ApiMappingKey, so two mappings on one domain can both claim the same mapping key and neither caller is refused.
Failing check ids: apigatewayv2.concurrent-api-mapping-update-conflict

AWS: Smithy (services/service-com-amazonaws-apigatewayv2-apigatewayv2/smithy-model/com-amazonaws-apigatewayv2-apigatewayv2.json): UpdateApiMapping is PATCH /v2/domainnames/{DomainName}/apimappings/{ApiMappingId}, success 200, input members ApiId (required), ApiMappingId (httpLabel, required), DomainName (httpLabel, required), ApiMappingKey (SelectionKey), Stage (StringWithLengthBetween1And128); modelled errors BadRequestException (400), ConflictException (409), NotFoundException (404), TooManyRequestsException (429). ConflictException is documented as "The requested operation would cause a conflict with the current state of a service resource associated with the request. Resolve the conflict before retrying this request. See the accompanying error message for details." An ApiMappingKey identifies ONE mapping under a domain name, so a request that would move a mapping onto a key another mapping of the same domain already holds is the conflict that error exists for: exactly one of two concurrent claims of the same key is applied (HTTP 200, the response echoing ApiId/ApiMappingId/ApiMappingKey/Stage) and the other is refused HTTP 409 with error code ConflictException, leaving the loser's stored ApiMappingKey untouched. The refusal's fixed text is AWS's wording for a mapping-key conflict; its variable parts are the mapping key from the request's ApiMappingKey and the domain from the request's DomainName path label (never a literal id or key from one recording). Hypothesis: AWS serialises the two writes behind the domain's mapping index; the model states only that ConflictException is the modelled outcome, so the single-winner requirement is the recorded behavior the check asserts.

Emulator: both concurrent UpdateApiMapping calls are refused with error code NotImplemented ("operation UpdateApiMapping is scaffolded but not implemented in this emulator"), so there is no winner at all. Replay output: AssertionError: expected exactly one winner, got (('error', 'NotImplemented'), ('error', 'NotImplemented')). Because the operation never runs, no uniqueness check on ApiMappingKey exists anywhere in the mapping writers, and nothing refuses a second mapping that claims a key already held under the same domain.

Why: a client that owns a custom domain uses the mapping key as the routing discriminator for that host. With no 409 and no single-winner guarantee, two mappings can end up claiming one key: a caller that treats its 200 as "this key now routes to my api/stage" is wrong half the time, and a caller written to retry on ConflictException never sees that signal and silently overwrites a peer's routing. Today it is strictly worse — every UpdateApiMapping is refused with an error code (NotImplemented) that this service's model does not declare, so no client can resolve it as a modelled failure or move a mapping key at all.

Fix: the UpdateApiMapping handler and the api-mapping state that indexes mappings by (DomainName, ApiMappingKey) — the update must take the mapping-key claim atomically under the single store lock and refuse the losing claim with the modelled ConflictException (409) rather than applying both writes. Related, NOT duplicate: the open issue 20261027T053233Z-apigatewayv2-updateapimapping-stub.md asks only that UpdateApiMapping be implemented and carries only the replay keys apigatewayv2.api-mapping-lifecycle and apigatewayv2.api-mapping-lifecycle-crud, so implementing a plain happy-path update closes it while leaving this conflict/atomicity requirement unmet; the open issue 20261027T053233Z-apigatewayv2-9-net-new-root-causes-api-mapping.md covers duplicate-key refusals on the CREATE path, and the refusal wording used here should match the one that issue settles on.

Traces: apigatewayv2.concurrent-api-mapping-update-conflict (file and line citations above refer to the checkout only; no recorded id, key or name is part of the requirement).

## Evidence
Failing check: [mismatch] apigatewayv2.concurrent-api-mapping-update-conflict (suite simparity-behavioral-ga-prod-v1).

Expected (AWS): of two concurrent UpdateApiMapping calls that move two different mappings of the SAME domain onto the same ApiMappingKey, exactly one succeeds with HTTP 200 and exactly one is refused HTTP 409 with Error.Code == "ConflictException"; a follow-up GetApiMapping shows the winner's ApiMappingKey set to the contested key and the loser's ApiMappingKey unchanged from the value it was created with.

Actual (emulator): both calls are refused with Error.Code == "NotImplemented" and HTTP 501; nothing is persisted and neither mapping's key changes. Replay assertion:
  File "check_apigatewayv2_concurrent_api_mapping_update_conflict.py", line 80, in run
    assert len(successes) == 1, f"expected exactly one winner, got {(outcome1, outcome2)!r}"
  AssertionError: expected exactly one winner, got (('error', 'NotImplemented'), ('error', 'NotImplemented'))

Model evidence: UpdateApiMapping declares ConflictException with httpError 409; NotImplemented is declared nowhere in com.amazonaws.apigatewayv2.

## Steps to Reproduce
1. Boot the apigatewayv2 emulator.
2. Import a certificate into ACM for a test domain and call CreateDomainName for that domain with a REGIONAL DomainNameConfiguration.
3. Create two HTTP APIs, each with one stage (s1 on api1, s2 on api2).
4. CreateApiMapping(ApiId=api1, DomainName=domain, Stage=s1, ApiMappingKey="race-a") and CreateApiMapping(ApiId=api2, DomainName=domain, Stage=s2, ApiMappingKey="race-b").
5. From two threads simultaneously, call UpdateApiMapping on both mappings with the SAME ApiMappingKey ("race-shared").
6. Observe both calls fail with (NotImplemented, HTTP 501) instead of one HTTP 200 and one HTTP 409 ConflictException.
7. GetApiMapping on both mappings: neither key moved; once the operation is implemented, re-run and confirm the loser's key is still its original value while the winner's is the contested key.
