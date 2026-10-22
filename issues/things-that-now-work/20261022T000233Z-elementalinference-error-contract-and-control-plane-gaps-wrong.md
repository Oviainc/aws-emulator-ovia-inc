Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elementalinference.create-feed-malformed-access-role-arn-rejected elementalinference.get-feed-not-found elementalinference.delete-feed-removes-it-regardless-of-status elementalinference.create-dictionary-invalid-name-validation elementalinference.disassociate-feed-never-associated-rejected elementalinference.search-fixtures-date-window-exceeds-seven-days-rejected elementalinference.associate-feed-concurrent-race-single-winner
Issue-Shape-Id: com.amazonaws.elementalinference#ElementalInference
Issue-Key: elementalinference.create-feed-malformed-access-role-arn-rejected|run#4|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code!r}"
Issue-Key: elementalinference.get-feed-not-found|run#10|AssertionError|assert message == "Resource not found", message
Issue-Key: elementalinference.delete-feed-removes-it-regardless-of-status|run#11|AssertionError|assert message == "Resource not found", message
Issue-Key: elementalinference.create-dictionary-invalid-name-validation|run#7|AssertionError|assert message == expected, message
Issue-Key: elementalinference.disassociate-feed-never-associated-rejected|run#7|AssertionError|assert code == "ValidationException", code
Issue-Key: elementalinference.search-fixtures-date-window-exceeds-seven-days-rejected|run#6|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: elementalinference.associate-feed-concurrent-race-single-winner|run#11|AssertionError|assert len(winners) == 1, f"expected exactly 1 winner, got {len(winners)}: {results}"
Issue-Commit: 668d747e62a7a896ef2863c29b158e984b097a40

# ElementalInference error contract and control-plane gaps: wrong error class for a bad accessRoleArn, wrong ResourceNotFound/validation message rules, and three operations still answering NotImplemented

> _Note: this issue was identified against the code base at commit `668d747e62a7a896ef2863c29b158e984b097a40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFeed, GetFeed, DeleteFeed, CreateDictionary, DisassociateFeed, SearchFixtures, AssociateFeed

## Description
Six distinct root causes in the elementalinference emulator, found by replaying the behavioral suite (3/30 checks passed). They are numbered below; each must be closed before this issue is submitted. Causes 4-6 are about operations that answer the scaffold `NotImplemented` error — separate auto-filed stub issues exist for `AssociateFeed`, `CreateDictionary`, `GetFixture` and `SearchFixtures` and cover standing the handlers up; the causes below additionally name the specific validation and state semantics these checks require, which merely un-stubbing the handler does not supply.

Cause 1 of 6: CreateFeed rejects a malformed / cross-account accessRoleArn with the wrong error class.
AWS: recorded behavior only — CreateFeed does not resolve `accessRoleArn` with an offline pattern check; it evaluates the role by attempting to pass it, so an ARN that names a different account surfaces as a pass-role authorization refusal. The recorded response is HTTP 403, error code `AccessDeniedException`, with the message `Cross-account pass role is not allowed.` (message rule: the request supplies the account id in the role ARN, so the refusal is derived from the request). Smithy: `AccessDeniedException` is a service-common error of ElementalInference with `@httpError 403`; `CreateFeedRequest.accessRoleArn` targets `IamRoleArn` with `@length min 32 max 255` and `@pattern ^arn:aws[a-z\-]*:iam::[0-9]{12}:role/.+$`.
Emulator: returns error code `ValidationException` (modeled `@httpError 400`) for `accessRoleArn="arn:aws:iam::123456789012:notarole"`. Hypothesis: the handler enforces the member's `@pattern` ahead of (instead of) the pass-role authorization check.
Why: a client that distinguishes "I passed a role I'm not allowed to pass" (403, retry with a role in my own account / fix the trust policy) from "my request was malformed" (400, fix the field) takes the wrong remediation path, and IAM-failure handling paths keyed on `AccessDeniedException` never fire.
Fix: CreateFeed's handler/validation ordering in the elementalinference service — the accessRoleArn authorization check must classify a role ARN naming another account as 403 `AccessDeniedException`.
Traces: elementalinference.create-feed-malformed-access-role-arn-rejected.

Cause 2 of 6: ResourceNotFoundException carries a per-resource prose message instead of the service's fixed message.
AWS: recorded behavior only — ElementalInference renders `ResourceNotFoundException` with the fixed message `Resource not found`, invariant across operations and across different ids (observed on both GetFeed and DeleteFeed with two different feed ids), i.e. the message does not render the requested id. Smithy: `ResourceNotFoundException` is `errorKind: client`, `@httpError 404`, with a single required `message` member, documented as "The resource specified in the action doesn't exist."
Emulator: the error code is correct (`ResourceNotFoundException`) but the message is a per-request rendering that discloses the id, e.g. `No feed with id 53prtya0vie7788nfh5 exists.` / `No feed with id zzzzzzzzzzzzzzzzzzz exists.`
Why: clients and test harnesses that assert on the service's not-found message, and log scrubbers that do not expect resource ids in error text, diverge; the emulator also leaks the probed id back to the caller where AWS does not.
Fix: the shared ResourceNotFoundException construction/serializer for the elementalinference service (used by GetFeed and DeleteFeed) must emit the service's fixed message rather than formatting the looked-up id.
Traces: elementalinference.get-feed-not-found, elementalinference.delete-feed-removes-it-regardless-of-status.

Cause 3 of 6: the validation-error message renders the pattern with its regex anchors.
AWS: the `ValidationException` for a constraint violation is built from the request and names the member and its constraint, so the rule is known: `1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>` — and AWS renders `<pattern>` with the leading `^` and trailing `$` stripped. Smithy: `CreateDictionaryRequest.name` targets `ResourceName`, whose `@pattern` is `^[a-zA-Z0-9]([a-zA-Z0-9-_]{0,126}[a-zA-Z0-9])?$`; recorded AWS text reports it as `[a-zA-Z0-9]([a-zA-Z0-9-_]{0,126}[a-zA-Z0-9])?`. Error code `ValidationException`, `@httpError 400` — both of which the emulator already gets right.
Emulator: emits the modeled pattern verbatim including the anchors: `... Member must satisfy regular expression pattern: ^[a-zA-Z0-9]([a-zA-Z0-9-_]{0,126}[a-zA-Z0-9])?$`.
Why: the message is machine-consumed in practice (clients surface it to users and tests match it); rendering the anchors makes every constraint-violation message for every anchored pattern member in this service diverge, not just CreateDictionary.name.
Fix: the shared constraint-violation message formatter used by the elementalinference input validation — strip the anchors when rendering a `@pattern` into the message.
Traces: elementalinference.create-dictionary-invalid-name-validation.

Cause 4 of 6: DisassociateFeed on a never-associated feed is not rejected — the operation answers NotImplemented.
AWS: DisassociateFeed on a feed that is still `AVAILABLE` (never associated) is rejected with `ValidationException`. Smithy: `DisassociateFeed` (POST /v1/feed/{id}/disassociate) models `ValidationException` (`@httpError 400`), `ConflictException` and `ResourceNotFoundException`; the operation is documented as releasing the resource currently associated with the feed. Recorded message example: `You can't disassociate this feed. It has status AVAILABLE which indicate that it's not currently associated with a resource.`
Emulator: returns error code `NotImplemented` (scaffold stub) for any DisassociateFeed call, so the state precondition is never evaluated and the feed never reaches `ARCHIVED` on the valid path either.
Why: a client cannot release a feed at all, and the negative path is unprotected: nothing distinguishes a feed that may be disassociated from one that may not.
Fix: the DisassociateFeed handler and its feed-status precondition in the elementalinference service state (reject when the feed is not currently associated; drive ACTIVE -> ARCHIVED when it is).
Traces: elementalinference.disassociate-feed-never-associated-rejected.

Cause 5 of 6: SearchFixtures does not enforce the seven-day search window — the operation answers NotImplemented.
AWS: a `startDate`..`endDate` window wider than seven days is rejected with `ValidationException`. Smithy: `SearchFixturesRequest.endDate` is documented "The window from startDate through endDate must not exceed seven days"; `SearchFixtures` (POST /v1/fixtures) models `ValidationException` (`@httpError 400`). A 0-day (same-day) window is accepted and returns a `fixtures` list. Recorded message example: `The search window must not span more than 7 days. Narrow the range between startDate and endDate.`
Emulator: returns error code `NotImplemented` for SearchFixtures, so neither the window check nor the accepted same-day search works. The auto-filed `SearchFixtures` stub issue covers standing the handler up; the window constraint is the additional requirement here.
Fix: the SearchFixtures handler plus its input validation (date-window span check) in the elementalinference service.
Traces: elementalinference.search-fixtures-date-window-exceeds-seven-days-rejected.

Cause 6 of 6: concurrent AssociateFeed on one feed produces no winner — both calls answer NotImplemented.
AWS: two concurrent AssociateFeed calls against the same feed resolve to exactly one winner; the loser is rejected with `ValidationException` (this service classifies "already associated" as a validation failure for the race, not a conflict). Smithy: `AssociateFeed` (POST /v1/feed/{id}/associate) models `ValidationException`, `ConflictException`, `ResourceNotFoundException` and `ServiceQuotaExceededException`.
Emulator: both concurrent calls return error code `NotImplemented`, so there are 0 winners and 0 ValidationException losers. The auto-filed `AssociateFeed` stub issue covers standing the handler up; the single-winner serialization of concurrent associations on one feed is the additional requirement here.
Why: a client that relies on the race being arbitrated server-side (one association wins, the other is told why) sees two failures and may double-associate once the handler exists without the mutual exclusion.
Fix: the AssociateFeed handler and the feed association record in the elementalinference service state — the association must be claimed atomically per feed, with the losing claim rejected as `ValidationException`.
Traces: elementalinference.associate-feed-concurrent-race-single-winner.

## Evidence
Suite: simparity-behavioral-aws-v1, elementalinference 3/30 (10%), 27 failed. Checks whose failures are NOT covered by the auto-filed scaffold-stub issues:

1. create-feed-malformed-access-role-arn-rejected [mismatch]
   expected: code AccessDeniedException (HTTP 403), message "Cross-account pass role is not allowed."
   actual:   AssertionError: expected AccessDeniedException, got 'ValidationException'

2. get-feed-not-found [mismatch]
   expected: code ResourceNotFoundException, message "Resource not found"
   actual:   code ok, message "No feed with id zzzzzzzzzzzzzzzzzzz exists."
   delete-feed-removes-it-regardless-of-status [mismatch]
   expected: code ResourceNotFoundException, message "Resource not found"
   actual:   code ok, message "No feed with id 53prtya0vie7788nfh5 exists."
   (same fixed message expected from two different operations and two different ids)

3. create-dictionary-invalid-name-validation [mismatch]
   expected: "1 validation error detected: Value '<name> bad!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9]([a-zA-Z0-9-_]{0,126}[a-zA-Z0-9])?"
   actual:   "...Member must satisfy regular expression pattern: ^[a-zA-Z0-9]([a-zA-Z0-9-_]{0,126}[a-zA-Z0-9])?$"

4. disassociate-feed-never-associated-rejected [mismatch]
   expected: code ValidationException
   actual:   AssertionError: NotImplemented

5. search-fixtures-date-window-exceeds-seven-days-rejected [mismatch]
   expected: code ValidationException for an 11-day window
   actual:   AssertionError: expected ValidationException, got NotImplemented

6. associate-feed-concurrent-race-single-winner [mismatch]
   expected: exactly 1 winner, 1 loser with code ValidationException
   actual:   AssertionError: expected exactly 1 winner, got 0: [('NotImplemented', 'sim2real-ei-race-a-...'), ('NotImplemented', 'sim2real-ei-race-b-...')]

Model evidence (services/service-com-amazonaws-elementalinference-elementalinference/smithy-model/com-amazonaws-elementalinference-elementalinference.json): restJson1, sigv4; common errors AccessDeniedException(403)/InternalServerErrorException/TooManyRequestException; ResourceNotFoundException @httpError 404; ValidationException @httpError 400; ResourceName @pattern ^[a-zA-Z0-9]([a-zA-Z0-9-_]{0,126}[a-zA-Z0-9])?$; CreateFeedRequest.accessRoleArn @length 32..255 @pattern ^arn:aws[a-z\-]*:iam::[0-9]{12}:role/.+$; SearchFixturesRequest.endDate doc "The window from startDate through endDate must not exceed seven days".

## Steps to Reproduce
Boot the emulator and point a botocore elementalinference client at it, then:

Cause 1: create_feed(name=<valid>, outputs=[{name:'sub1', outputConfig:{subtitling:{language:'eng'}}, status:'ENABLED'}], accessRoleArn='arn:aws:iam::123456789012:notarole') -> observe ValidationException instead of 403 AccessDeniedException.

Cause 2: create_feed(...); get_feed(id='zzzzzzzzzzzzzzzzzzz') -> observe ResourceNotFoundException whose message names the id instead of "Resource not found". Same for delete_feed(id=<deleted feed id>) and get_feed on a deleted feed.

Cause 3: create_dictionary(name='<valid-prefix> bad!', language='eng', entries='[{"content": "test"}]') -> observe the ValidationException message rendering the pattern with ^ and $.

Cause 4: create_feed(...); disassociate_feed(id=<feed id>, associatedResourceName='x') on the still-AVAILABLE feed -> observe NotImplemented instead of ValidationException.

Cause 5: search_fixtures(sport='basketball', startDate=<today-10d>, endDate=<today>) -> observe NotImplemented instead of ValidationException; also search_fixtures with startDate==endDate -> expected to succeed with a fixtures list.

Cause 6: create_feed(...); from two threads concurrently call associate_feed(id=<same feed id>, associatedResourceName=<distinct names>, outputs=[]) -> observe both returning NotImplemented, so 0 winners instead of exactly 1 winner and 1 ValidationException loser.

Replay: the check ids in trace_ids.

## Known divergence left after the fix (cause 1)

Cause 1's rule is implemented: a role ARN whose account is not the one the emulator
serves is answered as a pass-role denial — error code `AccessDeniedException` with the
message `Cross-account pass role is not allowed.` — rather than as a malformed request.
The HTTP STATUS of that answer still diverges for one class of input, and that class is
the recorded one:

* a role ARN that satisfies `IamRoleArn`'s `@length`/`@pattern` reaches the handler,
  which returns the modelled `AccessDeniedException` with its modelled **HTTP 403**;
* a role ARN that does NOT satisfy them (the recorded
  `arn:aws:iam::123456789012:notarole` has no `role/` resource) is refused BEFORE any
  handler runs, by the framework's boundary validator. The emulator answers it with the
  pass-role code and message, but at **HTTP 400**, because the status of a pre-handler
  refusal is the protocol's: `aws-emulator-smithy-server-runtime`'s
  `pre_dispatch::json` picks it from the `ProtocolError` variant, and the service hook
  that rewords such a refusal (`Service::override_refusal` -> `Refused`) carries only a
  code and a message.

Closing that gap means giving `Refused` an optional modelled HTTP status that
`protocol_error_response_with` honours — a change in the read-only `vendor/` tree, which
cannot be made from inside the emulator. Until that lands, the emulator's answer for a
cross-account role ARN that is also syntactically invalid is `AccessDeniedException` at
HTTP 400 instead of HTTP 403. The integration test
(`tests/all/error_contract.rs::a_role_in_another_account_is_denied_rather_than_called_malformed`)
asserts the modelled 403 wherever the handler decides, and for this class asserts only
the code, the message and that the status is a 4xx — so the divergence is recorded here
rather than pinned as expected behaviour.
