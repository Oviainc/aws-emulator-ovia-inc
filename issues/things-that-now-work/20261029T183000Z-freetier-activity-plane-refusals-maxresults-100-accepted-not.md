Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: freetier.list-account-activities-max-results-upper-bound-rejected freetier.get-account-activity-not-found-for-unknown-id freetier.get-account-activity-resource-not-found freetier.get-account-activity-wrong-length-rejected freetier.get-account-activity-invalid-language-code-rejected freetier.get-account-activity-language-code-case-sensitive-rejected freetier.list-account-activities-invalid-language-code-error freetier.list-account-activities-invalid-next-token-rejected freetier.list-account-activities-invalid-status-filter-rejected freetier.list-account-activities-invalid-status-rejected
Issue-Shape-Id: com.amazonaws.freetier#AWSFreeTierService
Issue-Key: freetier.list-account-activities-max-results-upper-bound-rejected|_client_error#2|AssertionError|raise AssertionError(f"expected ListAccountActivities to reject maxResults=101, got {result!r}")
Issue-Key: freetier.get-account-activity-not-found-for-unknown-id|run#7|AssertionError|assert error["Message"] == "Activity not found", error
Issue-Key: freetier.get-account-activity-resource-not-found|run#7|AssertionError|assert error["Message"] == "Activity not found", error
Issue-Key: freetier.get-account-activity-wrong-length-rejected|run#9|AssertionError|assert missing["Message"] == "Activity not found", missing
Issue-Key: freetier.get-account-activity-invalid-language-code-rejected|run#5|AssertionError|assert error["Message"] == (
Issue-Key: freetier.get-account-activity-language-code-case-sensitive-rejected|run#5|AssertionError|assert error["Message"] == (
Issue-Key: freetier.list-account-activities-invalid-language-code-error|run#5|AssertionError|assert error.get("Message") == _EXPECTED_MESSAGE, error.get("Message")
Issue-Key: freetier.list-account-activities-invalid-next-token-rejected|run#6|AssertionError|assert error["Message"] == "Invalid nextToken", error["Message"]
Issue-Key: freetier.list-account-activities-invalid-status-filter-rejected|run#6|AssertionError|assert error["Message"] == _EXPECTED_MESSAGE, error["Message"]
Issue-Key: freetier.list-account-activities-invalid-status-rejected|run#6|AssertionError|assert error["Message"] == _EXPECTED_MESSAGE, error["Message"]
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# freetier activity-plane refusals: maxResults>100 accepted, not-found and validation messages diverge from AWS (5 causes)

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAccountActivities

## Description
FIVE distinct causes on the activity plane (ListAccountActivities + GetAccountActivity). One is an input AWS refuses and the emulator accepts; the other four are refusals with the right code and the wrong message text.

Cause 1 of 5: maxResults above 100 is accepted instead of refused.
AWS: Smithy: ListAccountActivitiesRequest.maxResults targets MaxResults with @default 10 and @range min 1 / max 1000, but the live service enforces a lower ceiling than the model declares. Recorded behavior: maxResults=101 is refused with ValidationException (HTTP 400) and message "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100" — fixed text with the limit 100 as its only embedded constant and, unlike the sibling messages, NO rendering of the submitted value; maxResults=100 is accepted. So the real upper bound is 100, not the modeled 1000.
Emulator: answers maxResults=101 with HTTP 200 and the full activity catalog (5 activities), i.e. it enforces no upper bound below the modeled 1000 on this member.
Why: the emulator is over-permissive on a documented input bound. A client developed against it ships a page size the real service rejects outright, so the first call against AWS fails with an error the client has never seen; and a client that tests its own clamp-to-100 logic against the emulator gets no signal that the clamp is required.
Fix: the input validation for ListAccountActivities (services/service-com-amazonaws-freetier-awsfreetierservice/src/ops/list_account_activities.rs) must reject maxResults > 100 with ValidationException before paging, rather than taking the modeled @range max of 1000.
Traces: freetier.list-account-activities-max-results-upper-bound-rejected.

Cause 2 of 5: GetAccountActivity not-found message.
AWS: Smithy: GetAccountActivity models ResourceNotFoundException ("thrown when the requested resource cannot be found"). Recorded behavior: the message for a well-formed but nonexistent activityId is the fixed string "Activity not found" — no variable parts at all; it does not echo the id and does not mention the account.
Emulator: "no activity with id <activityId> exists for this account", an emulator-authored sentence that interpolates the requested id. Code is ResourceNotFoundException either side.
Why: the message is the only human-readable discriminator on this error, and a client or test pinning AWS's text never matches; the emulator also leaks the requested id into a message AWS keeps opaque.
Fix: the not-found branch of services/.../src/ops/get_account_activity.rs.
Traces: freetier.get-account-activity-not-found-for-unknown-id, freetier.get-account-activity-resource-not-found, freetier.get-account-activity-wrong-length-rejected (this last one reaches the same divergence on its well-formed-but-missing control leg; its 20-character length rejection already matches AWS).

Cause 3 of 5: the enum-value-set listing inside languageCode validation messages is in the wrong order.
AWS: Smithy: LanguageCode is an enum whose declared values are [en-US, en-GB, id-ID, de-DE, es-ES, fr-FR, ja-JP, it-IT, pt-PT, ko-KR, zh-CN, zh-TW, tr-TR]; an out-of-enum value is refused with ValidationException. Recorded behavior: AWS renders the permitted set in its own stable, non-declaration order — "[zh-TW, en-US, zh-CN, tr-TR, pt-PT, it-IT, fr-FR, es-ES, de-DE, id-ID, ko-KR, ja-JP, en-GB]" — and does so identically for GetAccountActivity and ListAccountActivities. Fixed text: "1 validation error detected: Value '<value>' at 'languageCode' failed to satisfy constraint: Member must satisfy enum value set: [<that set, in that order>]"; the only variable part is the rejected value. Matching is case-sensitive, so "EN-US" is rejected by the same path as "xx-XX". Hypothesis: AWS's order is its validator's hash-set iteration order rather than the model's member order; it is nonetheless stable and identical across both operations in the recordings.
Emulator: emits the set in model-declaration order, "[en-US, en-GB, id-ID, de-DE, es-ES, fr-FR, ja-JP, it-IT, pt-PT, ko-KR, zh-CN, zh-TW, tr-TR]". Everything else in the message matches, and the case-sensitivity itself is correctly enforced.
Why: the permitted-value list is the actionable content of this message — tooling that parses it to populate a retry, a prompt or a fixture gets a differently ordered set than AWS emits, and any assertion on the message text fails.
Fix: the shared enum-validation message builder used by both activity operations (services/.../src/ops/get_account_activity.rs and .../src/ops/list_account_activities.rs, or the common validation helper they call) must render AWS's order for LanguageCode.
Traces: freetier.get-account-activity-invalid-language-code-rejected, freetier.get-account-activity-language-code-case-sensitive-rejected, freetier.list-account-activities-invalid-language-code-error.

Cause 4 of 5: unissued nextToken message on ListAccountActivities.
AWS: Smithy: ListAccountActivitiesRequest.nextToken targets NextPageToken (@length 1..8192); a token the service never issued is refused with ValidationException. Recorded behavior: the message is the fixed string "Invalid nextToken" — no variable parts, and notably NOT the bracketed "[Message = The next token is invalid.]" form its GetFreeTierUsage sibling uses, so the two listings do not share one string.
Emulator: "nextToken: the pagination token is not one ListAccountActivities issued".
Why: a paginating client that detects a stale cursor by its message never recognizes the condition, and a shared-string assumption across the two listings is wrong in both directions.
Fix: the nextToken decode-failure branch of services/.../src/ops/list_account_activities.rs (today it raises that string around crate::state::decode_page_token). Distinct from the open issue 20261028T211319Z-freetier-listaccountactivities-pagination.md, which is about an offset-past-the-end token being answered HTTP 200 instead of refused at all; this cause is the wording of the refusal that IS raised.
Traces: freetier.list-account-activities-invalid-next-token-rejected.

Cause 5 of 5: filterActivityStatuses enum failure is reported per member instead of as the collection-level aggregate.
AWS: Smithy: ListAccountActivitiesRequest.filterActivityStatuses targets the list FilterActivityStatuses whose member is the enum ActivityStatus with declared values [NOT_STARTED, IN_PROGRESS, COMPLETED, EXPIRING]. Recorded behavior: AWS reports the failure against the LIST, rendering the whole submitted list as the offending value and nesting the member constraint: "1 validation error detected: Value '[<list, comma-joined, in brackets>]' at 'filterActivityStatuses' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [IN_PROGRESS, COMPLETED, NOT_STARTED, EXPIRING]]". Fixed text: everything but the bracketed rendering of the submitted list; note the permitted set is again in AWS's own order (IN_PROGRESS, COMPLETED, NOT_STARTED, EXPIRING), not the model's.
Emulator: reports the single bad member with an indexed path and the model's order: "1 validation error detected: Value 'BOGUS_STATUS' at 'filterActivityStatuses.1.member' failed to satisfy constraint: Member must satisfy enum value set: [NOT_STARTED, IN_PROGRESS, COMPLETED, EXPIRING]" — wrong member path shape, wrong value rendering, missing the nested "Member must satisfy constraint: [...]" wrapper, wrong set order.
Why: the member path is the machine-readable part of the message; a client that maps 'filterActivityStatuses' back to the field it must correct instead receives a sub-path with a 1-based index AWS never emits.
Fix: list-member validation aggregation for ListAccountActivities (services/.../src/ops/list_account_activities.rs and the shared validation-message builder). Same aggregation defect as the Dimensions.Values cause filed against GetFreeTierUsage, but a different operation and a different member, so it is listed here as its own cause.
Traces: freetier.list-account-activities-invalid-status-filter-rejected, freetier.list-account-activities-invalid-status-rejected.

Every check behind causes 2-5 carries a positive control that passes on the emulator (an unfiltered or valid-filter ListAccountActivities, a valid languageCode, a single valid status), so the operations are reachable and only these behaviors diverge. The not-found-message and enum-order causes survive the earlier fixed issues 20261024T000352Z-freetier-getaccountactivity-error-response.md and 20261024T011932Z-freetier-listaccountactivities-parameter-ignored.md: those made the status/code and languageCode handling correct, and the refusals they created now carry non-AWS text.

## Evidence
Cause 1 — ListAccountActivities(maxResults=101):
  expected: ClientError Code=ValidationException, HTTP 400, Message="1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"
  actual:   HTTP 200 with ResponseMetadata.HTTPStatusCode=200 and a full 'activities' list of 5 records (activityId a1b2c3d4e5f60718293a4b5c6d7e8f90, 0f1e2d3c4b5a69788796a5b4c3d2e1f0, 1122334455667788990011223344aabb, deadbeefcafef00d0123456789abcdef, ABCDEF0123456789abcdef0123456789). The check's own helper raised "expected ListAccountActivities to reject maxResults=101". maxResults=100 is accepted by both.

Cause 2 — GetAccountActivity(activityId=<32 chars, absent>), confirmed absent via ListAccountActivities first:
  expected: Code=ResourceNotFoundException, Message="Activity not found"
  actual:   Code=ResourceNotFoundException, Message="no activity with id 24030a6560324f1ea50f9e37712a45af exists for this account"
  same divergence for id "f"*32 -> "no activity with id ffffffffffffffffffffffffffffffff exists for this account" and for id "a"*32 -> "no activity with id aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa exists for this account". The interpolated id is a request value filling the emulator's template; AWS's message has no variable part.

Cause 3 — GetAccountActivity(activityId="z"*32, languageCode="xx-XX") and (activityId="q"*32, languageCode="EN-US") and ListAccountActivities(languageCode="xx-XX"); all Code=ValidationException:
  expected Message: "1 validation error detected: Value 'xx-XX' at 'languageCode' failed to satisfy constraint: Member must satisfy enum value set: [zh-TW, en-US, zh-CN, tr-TR, pt-PT, it-IT, fr-FR, es-ES, de-DE, id-ID, ko-KR, ja-JP, en-GB]"
  actual   Message: "1 validation error detected: Value 'xx-XX' at 'languageCode' failed to satisfy constraint: Member must satisfy enum value set: [en-US, en-GB, id-ID, de-DE, es-ES, fr-FR, ja-JP, it-IT, pt-PT, ko-KR, zh-CN, zh-TW, tr-TR]"
  (identical divergence with Value 'EN-US' for the wrong-cased variant; only the bracketed set's order differs, and it differs on all three checks.)

Cause 4 — ListAccountActivities(nextToken="not-a-real-pagination-token"), Code=ValidationException both sides:
  expected Message: "Invalid nextToken"
  actual   Message: "nextToken: the pagination token is not one ListAccountActivities issued"

Cause 5 — ListAccountActivities(filterActivityStatuses=["BOGUS_STATUS"]), Code=ValidationException both sides:
  expected Message: "1 validation error detected: Value '[BOGUS_STATUS]' at 'filterActivityStatuses' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [IN_PROGRESS, COMPLETED, NOT_STARTED, EXPIRING]]"
  actual   Message: "1 validation error detected: Value 'BOGUS_STATUS' at 'filterActivityStatuses.1.member' failed to satisfy constraint: Member must satisfy enum value set: [NOT_STARTED, IN_PROGRESS, COMPLETED, EXPIRING]"
  ListAccountActivities(filterActivityStatuses=["COMPLETED"]) succeeds and returns the same set before and after, so the rejected call has no side effect — only its wording diverges.

## Steps to Reproduce
1. Boot the emulator with the freetier service mounted and build a botocore freetier client against it.
2. Cause 1: call ListAccountActivities(maxResults=101). Observe HTTP 200 with the activity catalog; AWS raises ValidationException "...Member must have value less than or equal to 100". Then call ListAccountActivities(maxResults=100) and confirm it succeeds (the boundary itself must stay accepted).
3. Cause 2: call ListAccountActivities(maxResults=50) to learn the real activity ids, pick a 32-character [a-zA-Z0-9] id that is not among them (e.g. "f"*32), call GetAccountActivity(activityId=<that id>) and compare Error.Message with "Activity not found".
4. Cause 3: call GetAccountActivity(activityId="z"*32, languageCode="xx-XX"), then GetAccountActivity(activityId="q"*32, languageCode="EN-US"), then ListAccountActivities(languageCode="xx-XX"); compare each Error.Message's bracketed enum set with AWS's order [zh-TW, en-US, zh-CN, tr-TR, pt-PT, it-IT, fr-FR, es-ES, de-DE, id-ID, ko-KR, ja-JP, en-GB].
5. Cause 4: call ListAccountActivities() (succeeds), then ListAccountActivities(nextToken="not-a-real-pagination-token") and compare Error.Message with "Invalid nextToken".
6. Cause 5: call ListAccountActivities(filterActivityStatuses=["COMPLETED"]) (succeeds), then ListAccountActivities(filterActivityStatuses=["BOGUS_STATUS"]) and compare Error.Message with the collection-level aggregate above.
7. Re-replay the checks in trace_ids to confirm all five causes are closed.
