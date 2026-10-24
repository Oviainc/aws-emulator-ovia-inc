Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.concurrent-association-operations appsync.merged-api-wrong-target-type-rejected
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.concurrent-association-operations|run#14|AssertionError|assert msg == assoc_messages[code], f"unexpected {code} message: {msg!r}"
Issue-Key: appsync.merged-api-wrong-target-type-rejected|run#8|AssertionError|assert message == f"Invalid API type, expected api to be type MERGED for api : {not_merged_id}", message
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# AppSync merged-API association rejections return non-AWS message text for ConcurrentModificationException and the wrong-api-type BadRequestException (2 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateMergedGraphqlApi

## Description
Two distinct root causes in the source-API-association code path of the AppSync control plane. In both the HTTP status and the error code the emulator returns are CORRECT; what diverges is the message body, and in both cases AWS's message is produced by a known, request-derived template (it embeds the identifier the caller passed), so the template — not the recorded prose — is the requirement. These are NOT covered by the open issue 20261024T032443Z-appsync-control-plane-fidelity-domain-name-operations-unimplemented.md, which is about rejections returning the WRONG error CODE (ValidationException instead of BadRequestException) and about unimplemented domain-name operations; here the codes are already right.

Cause 1 of 2 — ConcurrentModificationException from a losing Associate*GraphqlApi racer does not name the merged API.
AWS: Smithy (com-amazonaws-appsync-awsdeepdishcontrolplaneservice): AssociateMergedGraphqlApi and AssociateSourceGraphqlApi both model ConcurrentModificationException, errorKind client, httpError 409. Recorded behavior: when several Associate{Merged,Source}GraphqlApi calls for the same merged API overlap, the losers are refused 409 ConcurrentModificationException with a message built from the request's mergedApiIdentifier — of the form "SourceApiAssociations for Merged API <mergedApiIdentifier> are currently being updated, please wait and try again." The rule is that this conflict is reported as scoped to the specific merged API named in the request (a losing racer may alternatively be refused 400 BadRequestException, whose message varies and is not graded).
Emulator: returns the correct code/status but a single generic, request-independent sentence for every ConcurrentModificationException: "Another modification is in progress at this time and it must complete before you can make your change." — the merged API identifier never appears, so the message carries no information about WHICH resource is contended.
Why: a client or operator reconciling concurrent merged-API wiring cannot tell from the refusal which merged API is busy, and retry/alerting logic that keys off the association-scoped refusal text (or parses the identifier out of it) silently degrades to a generic retry. It also makes the association-conflict refusal indistinguishable from an unrelated control-plane modification conflict on the same account.
Fix: the AssociateMergedGraphqlApi / AssociateSourceGraphqlApi handlers (and the shared association-conflict error construction they call) must build the ConcurrentModificationException message from the request's mergedApiIdentifier rather than emitting one hard-coded constant for all ConcurrentModificationException sites.
Traces: appsync.concurrent-association-operations — fails at the first associate race, `assert msg == assoc_messages[code]`, AssertionError: unexpected ConcurrentModificationException message: 'Another modification is in progress at this time and it must complete before you can make your change.'

Cause 2 of 2 — wrong-api-type BadRequestException from AssociateMergedGraphqlApi uses emulator-authored wording.
AWS: Smithy: AssociateMergedGraphqlApi models BadRequestException, errorKind client, httpError 400. Recorded behavior: pointing mergedApiIdentifier at an apiType=GRAPHQL (non-MERGED) API is refused 400 BadRequestException with a message built from the request's mergedApiIdentifier, of the form "Invalid API type, expected api to be type MERGED for api : <mergedApiIdentifier>". The rejection also creates no association record (ListSourceApiAssociations on the source stays empty) — that part the emulator gets right.
Emulator: correct 400 / BadRequestException, and correctly creates no association, but the message is re-worded: "API <apiId> is not a Merged API; only an API of type MERGED can be associated with source APIs".
Why: the refusal text is the only place the API-type contract is surfaced to a caller; a client matching AWS's documented refusal shape for this specific pre-condition (as this check does) treats the emulator's refusal as an unknown failure and cannot distinguish "target is not a MERGED API" from any other BadRequestException on the same call.
Fix: the AssociateMergedGraphqlApi handler's api-type validation step — emit AWS's request-derived template instead of locally authored prose; the status, code and no-side-effect behavior already match and need no change.
Traces: appsync.merged-api-wrong-target-type-rejected — `assert message == f"Invalid API type, expected api to be type MERGED for api : {not_merged_id}"`, AssertionError: API gxt3g7obqbwtsff73b6muz5iuu is not a Merged API; only an API of type MERGED can be associated with source APIs.

## Evidence
Cause 1 (appsync.concurrent-association-operations):
  Expected (AWS, template built from the request): code ConcurrentModificationException (HTTP 409), message "SourceApiAssociations for Merged API <mergedApiIdentifier> are currently being updated, please wait and try again."
  Actual (emulator): code ConcurrentModificationException (HTTP 409) — correct — message 'Another modification is in progress at this time and it must complete before you can make your change.' (no identifier, same constant for every CME site).
  Replay failure: check_appsync_concurrent_association_operations.py line 187, `assert msg == assoc_messages[code]` -> AssertionError: unexpected ConcurrentModificationException message.

Cause 2 (appsync.merged-api-wrong-target-type-rejected):
  Expected (AWS, template built from the request): code BadRequestException (HTTP 400), message "Invalid API type, expected api to be type MERGED for api : <mergedApiIdentifier>".
  Actual (emulator): code BadRequestException (HTTP 400) — correct — message "API gxt3g7obqbwtsff73b6muz5iuu is not a Merged API; only an API of type MERGED can be associated with source APIs" (the id shown is an example; it is whatever non-MERGED api was created by the run).
  Replay failure: check_appsync_merged_api_wrong_target_type_rejected.py line 41, `assert message == f"Invalid API type, expected api to be type MERGED for api : {not_merged_id}"`.

Smithy used: com-amazonaws-appsync-awsdeepdishcontrolplaneservice — AssociateMergedGraphqlApi (POST /v1/sourceApis/{sourceApiIdentifier}/mergedApiAssociations) and AssociateSourceGraphqlApi (POST /v1/mergedApis/{mergedApiIdentifier}/sourceApiAssociations), both modelling BadRequestException (400) and ConcurrentModificationException (409).

## Steps to Reproduce
Cause 1:
1. CreateGraphqlApi(authenticationType=API_KEY) -> source API id.
2. CreateGraphqlApi(authenticationType=API_KEY, apiType=MERGED, mergedApiExecutionRoleArn=<role trusting appsync.amazonaws.com>) -> merged API id.
3. Fire 4 concurrent AssociateMergedGraphqlApi(sourceApiIdentifier=<source>, mergedApiIdentifier=<merged>, sourceApiAssociationConfig={mergeType: MANUAL_MERGE}) calls.
4. At least one wins; inspect each loser's error. Losers refused with ConcurrentModificationException must carry a message naming the merged API ("SourceApiAssociations for Merged API <merged> are currently being updated, please wait and try again."). The emulator returns the generic 'Another modification is in progress...' constant instead.
   (Repeat with concurrent AssociateSourceGraphqlApi against the same merged API for the merged-initiated side.)

Cause 2:
1. CreateGraphqlApi(authenticationType=API_KEY) twice -> source id and a plain (apiType defaults to GRAPHQL) id.
2. Call AssociateMergedGraphqlApi(sourceApiIdentifier=<source>, mergedApiIdentifier=<plain id>).
3. Expect 400 BadRequestException with message "Invalid API type, expected api to be type MERGED for api : <plain id>"; the emulator returns the correct status/code with re-worded text.
4. ListSourceApiAssociations(apiId=<source>) is empty in both AWS and the emulator (no divergence there).

Full replay: run_behavioral_suite and re-run checks appsync.concurrent-association-operations and appsync.merged-api-wrong-target-type-rejected.
