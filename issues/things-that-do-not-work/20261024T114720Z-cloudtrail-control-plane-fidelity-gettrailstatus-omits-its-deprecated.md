Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail.get-trail-status-delivery-field-absence-and-dual-representation cloudtrail.list-tags-multi-arn-positional-binding cloudtrail.lookup-events-max-results-bound-and-token-stability cloudtrail.event-data-store-federation-and-event-config-reject-unknown-arn cloudtrail.query-ops-reject-unknown-event-data-store cloudtrail.tag-value-unicode-and-empty-round-trip cloudtrail.concurrent-dashboard-update-no-lost-writes-corruption cloudtrail.list-insights-data-permission-denied
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail.get-trail-status-delivery-field-absence-and-dual-representation|run#16|AssertionError|assert field in status, (
Issue-Key: cloudtrail|ListTags|ValidationException
Issue-Key: cloudtrail|LookupEvents|ValidationException
Issue-Key: cloudtrail.event-data-store-federation-and-event-config-reject-unknown-arn|_assert_error_map#10|AssertionError|assert actual == expected, f"error map mismatch: expected={expected} actual={actual}"
Issue-Key: cloudtrail.query-ops-reject-unknown-event-data-store|_assert_error_map#7|AssertionError|assert actual == expected, f"error map mismatch: expected={expected} actual={actual}"
Issue-Key: cloudtrail.tag-value-unicode-and-empty-round-trip|run#22|AssertionError|assert by_key[novalue_key].get("Value") == "", (
Issue-Key: cloudtrail.concurrent-dashboard-update-no-lost-writes-corruption|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: cloudtrail.list-insights-data-permission-denied|_wait_for_error#8|AssertionError|raise AssertionError(f"call never failed with {expected_code} within {timeout_s}s; last outcome {last!r}")
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# CloudTrail control-plane fidelity: GetTrailStatus omits its deprecated String members, ListTags and LookupEvents enforce limits AWS does not, Lake query/event-config refusals use the wrong code, an omitted tag Value is dropped, dashboards never reach UPDATED, and no authorization is evaluated

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTrailStatus

## Description
SEVEN distinct causes in this issue; all 7 must be closed before submitting. They are independent control-plane divergences in one service, each with its own failing check.

Cause 1 of 7 - GetTrailStatus omits the six deprecated String members instead of emitting them as empty strings.
AWS: Smithy GetTrailStatusResponse declares, besides the Date/Boolean members, six String members - LatestDeliveryAttemptTime, LatestNotificationAttemptTime, LatestNotificationAttemptSucceeded, LatestDeliveryAttemptSucceeded, TimeLoggingStarted, TimeLoggingStopped. Recorded behavior (measured by the check against real AWS): on a freshly created, never-started trail the body is {"IsLogging": false, "LatestDeliveryAttemptTime": "", "LatestNotificationAttemptTime": "", "LatestNotificationAttemptSucceeded": "", "LatestDeliveryAttemptSucceeded": "", "TimeLoggingStarted": "", "TimeLoggingStopped": ""} - the String members are always present, carrying '' until they are set, while their Date twins (LatestDeliveryTime, StartLoggingTime, ...) are OMITTED until set. The split between the two representations is the contract.
Emulator: the response carries IsLogging only - the check's status dict is ['IsLogging', 'ResponseMetadata'].
Why: these members exist for older SDK consumers that read them unconditionally; a client that does gets a missing-key failure against the emulator and works on AWS (or vice versa), and nothing can distinguish 'never delivered' from 'not reported'.
Fix: GetTrailStatus's handler/serializer must always populate the six String members, as '' while unset, and keep the Date twins omitted while unset.
Traces: cloudtrail.get-trail-status-delivery-field-absence-and-dual-representation.

Cause 2 of 7 - ListTags refuses a ResourceIdList longer than 20 ARNs.
AWS: Smithy ResourceIdList is a plain list of String with NO length constraint, and ListTags models no limit error. Recorded behavior (measured by the check against real AWS): the documented 20-ARN limit is NOT enforced server-side - 2, 20, 21, 25, 26, 50 and 51 element lists all returned one positional ResourceTag entry per element, duplicates included, with no NextToken and no error.
Emulator: a 25-element list is refused with ValidationException (which ListTags does not model at all): 'ResourceIdList names 25 ARNs; one ListTags call has a limit of 20 ARNs'. The limit is hard-coded - src/ops/list_tags.rs returns Scaffolded::validation when input.resource_id_list.len() > crate::state::LIST_TAGS_RESOURCE_LIMIT.
Why: a client batching tag reads over more than 20 resources gets a 400 that AWS never returns, and the error code is one no SDK branch for ListTags matches.
Fix: ListTags' handler must drop that request-size refusal and keep answering one positional entry per element (file citation: services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/list_tags.rs, the LIST_TAGS_RESOURCE_LIMIT check at the top of handle).
Traces: cloudtrail.list-tags-multi-arn-positional-binding.

Cause 3 of 7 - LookupEvents rejects MaxResults above the modelled maximum instead of clamping the page.
AWS: Smithy LookupEvents.MaxResults targets MaxResults with range {min 1, max 50}, and the operation models InvalidMaxResultsException (httpError 400). Recorded behavior (measured by the check against real AWS): MaxResults=51, 52, 100, 1000 and 1001 were ALL accepted with HTTP 200 and each returned at most 50 events plus a NextToken; InvalidMaxResultsException was not returned for any of them. The ceiling is enforced on the RESPONSE, not on the request.
Emulator: the call is refused with HTTP 400 ValidationException, '1 validation error detected: Value at maxResults failed to satisfy constraint: Member must have value less than or equal to 50'. The handler itself already clamps (src/ops/lookup_events.rs clamps max_results between MAX_RESULTS_MIN and MAX_RESULTS_MAX), so the refusal comes from the centralized range validation that runs before handle. ValidationException is not one of LookupEvents' modelled errors either.
Why: the SDK paginators send the page size a caller configures; a client that asks for a larger page gets a hard 400 from the emulator where AWS pages normally, and the token-stability behavior behind it is unreachable.
Fix: the request-validation layer must not enforce MaxResults' modelled upper bound for LookupEvents (leave the clamp the handler already performs); if a bound is ever enforced it must be the modelled InvalidMaxResultsException, not ValidationException.
Traces: cloudtrail.lookup-events-max-results-bound-and-token-stability.

Cause 4 of 7 - PutEventConfiguration answers EventDataStoreARNInvalidException where AWS answers InvalidParameterException.
AWS: Smithy binds BOTH on PutEventConfiguration (EventDataStoreARNInvalidException httpError 400 / awsQueryError EventDataStoreARNInvalid; InvalidParameterException httpError 400 / awsQueryError InvalidParameter). Recorded behavior: for an EventDataStore value that is a well-formed CloudTrail ARN whose resource id is not an event-data-store id, EnableFederation, DisableFederation and GetEventConfiguration answer EventDataStoreARNInvalidException while PutEventConfiguration answers InvalidParameterException (recorded message, as an example: 'The EventDataStore value must be a valid EventDataStore ARN or ID.'). PutEventConfiguration validates that member itself and classifies it differently from its siblings.
Emulator: PutEventConfiguration answers EventDataStoreARNInvalidException for the same input; the other three operations match. The check's error map differs in exactly that one entry.
Why: a client that branches on InvalidParameterException to correct a malformed EventDataStore argument never takes that branch, and an ARN-format error handler is entered instead.
Fix: PutEventConfiguration's input validation must classify an unusable EventDataStore value as InvalidParameterException.
Traces: cloudtrail.event-data-store-federation-and-event-config-reject-unknown-arn.

Cause 5 of 7 - DescribeQuery and CancelQuery let the deprecated EventDataStore member dominate the QueryId check.
AWS: Smithy marks DescribeQueryRequest.EventDataStore and CancelQueryRequest.EventDataStore deprecated ('EventDataStore is no longer required by DescribeQueryRequest' / '... CancelQueryRequest'), and both operations model QueryIdNotFoundException (httpError 404 / awsQueryError QueryIdNotFound) alongside EventDataStoreNotFoundException (404). Recorded behavior: with a nonexistent event data store AND an unknown QueryId, DescribeQuery, GetQueryResults and CancelQuery all answer QueryIdNotFoundException (recorded message, as an example: 'The QueryId value <id> does not exist or is not in the same region.'), while ListQueries - which genuinely needs the store - answers EventDataStoreNotFoundException naming the ARN. The query id is what these three resolve on; the deprecated store member is not.
Emulator: DescribeQuery and CancelQuery answer EventDataStoreNotFoundException ('The event data store was not found.'); GetQueryResults and ListQueries match AWS. So the precedence is inverted on exactly the two operations whose EventDataStore member is deprecated.
Why: a client cleaning up or polling a query id it holds cannot tell 'no such query' from 'no such store', and retry logic keyed on QueryIdNotFoundException breaks.
Fix: DescribeQuery's and CancelQuery's handlers must resolve the QueryId first and refuse with QueryIdNotFoundException, ignoring the deprecated EventDataStore member for existence purposes.
Traces: cloudtrail.query-ops-reject-unknown-event-data-store.

Cause 6 of 7 - a tag added with no Value comes back with the member missing instead of an empty string.
AWS: Smithy Tag.Value targets TagValue with length {min 1, max 256} and is not required. Recorded behavior (measured by the check against real AWS): AddTags with a tag that omits Value stores it, and ListTags returns that entry with Value: '' - not dropped, not a sentinel. (AWS itself returns a value outside the modelled minimum here, so the recording is the authority.)
Emulator: ListTags returns the entry with no Value member at all, so botocore surfaces None - 'tag Value omitted at creation came back as None instead of ""'. The unicode half of the same check passes, so only the empty case is wrong.
Why: every tag consumer indexes {Key: Value}; a None where AWS gives '' turns a present-but-empty tag into a missing one and breaks cost-allocation, guardrail and reaper logic that compares tag values.
Fix: the tagging state/serializer shared by AddTags/ListTags must normalise an absent Value to '' and always emit the member.
Traces: cloudtrail.tag-value-unicode-and-empty-round-trip.

Cause 7 of 7 - a dashboard never reports Status UPDATED after UpdateDashboard, and no concurrent updater is ever refused.
AWS: Smithy GetDashboardResponse.Status is DashboardStatus with values CREATING, CREATED, UPDATING, UPDATED, DELETING, and UpdateDashboard models ConflictException (httpError 409). Recorded behavior: after an accepted UpdateDashboard the dashboard settles at Status 'UPDATED' with the written value in force; with several UpdateDashboard calls in flight at once, at least one succeeds and any loser is refused with ConflictException.
Emulator: GetDashboard never reports UPDATED. The check fires four concurrent UpdateDashboard calls (all are accepted - no ConflictException is ever produced) and then polls GetDashboard for Status == 'UPDATED' under a 120 s budget; the status never changes, so the poll is still running when the harness's 60 s cap kills it ('Check timed out after 60.0s') and the subsequent termination-protection assertions are never reached.
Why: a client that waits for a dashboard to settle before reading it waits forever, and the lost-write/conflict contract is unobservable. The related open issue 20261023T135552Z-cloudtrail-dashboard-termination-protection-is-dropped-and-putresourcepolicy.md covers termination protection being dropped on create/update but does not carry this check or the Status transition.
Fix: UpdateDashboard's handler/state must advance the stored dashboard's Status (UPDATING -> UPDATED) and take the dashboard's write lock so concurrent losers get ConflictException 409.
Traces: cloudtrail.concurrent-dashboard-update-no-lost-writes-corruption.

Cause 8 of 7 is not present; the eighth failing check belongs to Cause 8 below, which is the seventh distinct cause only if authorization is counted separately - it is, so read the list as EIGHT causes and close all EIGHT:

Cause 8 of 8 - no authorization is evaluated on CloudTrail requests, so a denied principal is served normally.
AWS: recorded behavior only (the check's measured oracle): ListInsightsData called by a principal an explicit deny policy covers is refused with AccessDeniedException; after the deny is removed the same call succeeds. Smithy models no per-operation authorization error - the refusal is the service-common access-denied fault - so the requirement is the code and the 400/403 refusal, not the message text.
Emulator: the call never fails - 'call never failed with AccessDeniedException within 45.0s; last outcome None' - so the positive/negative pair collapses. The open issue 20261023T135552Z-cloudtrail-has-no-updatetrail-concurrency-control-cloudformation-never.md records the same absence for cloudtrail ListTrails (its Cause 3) and 20261022T220446Z-iam-no-iam-authorization-is-evaluated-calls-a-session.md records it for IAM-side operations; neither carries this check's key, so it is filed here and should be fixed with them.
Why: every deny path a client tests is dead, and a least-privilege policy that is wrong in production looks correct against the emulator.
Fix: the request-path authorization seam (the resolved principal's attached/inline policies evaluated before the handler runs, returning the service-common access-denied fault) must cover CloudTrail actions.
Traces: cloudtrail.list-insights-data-permission-denied.

Count, stated plainly: EIGHT causes - 1 GetTrailStatus String members, 2 ListTags 20-ARN limit, 3 LookupEvents MaxResults refusal, 4 PutEventConfiguration error class, 5 DescribeQuery/CancelQuery precedence, 6 empty tag Value, 7 dashboard Status/conflict, 8 CloudTrail authorization. All eight must be closed.

## Evidence
Cause 1: 'GetTrailStatus omitted the deprecated String member LatestDeliveryAttemptTime on a trail that has never logged; real AWS always emits it, as \'\' when it has no value. Omitting it breaks the older SDK consumers it exists for. Got: [\'IsLogging\', \'ResponseMetadata\']'.
Cause 2: botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the ListTags operation: ResourceIdList names 25 ARNs; one ListTags call has a limit of 20 ARNs - raised on ct.list_tags(ResourceIdList=[arn_a, arn_b]*12 + [arn_a]) after the 2-element and duplicate-element calls had succeeded.
Cause 3: botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the LookupEvents operation: 1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 50 - raised on the first over-maximum call, MaxResults=51, after MaxResults=50 had returned 200.
Cause 4: 'error map mismatch: expected={... \'PutEventConfiguration\': (\'InvalidParameterException\', \'The EventDataStore value must be a valid EventDataStore ARN or ID.\')} actual={... \'PutEventConfiguration\': (\'EventDataStoreARNInvalidException\', \'The event data store ARN ...:eventdatastore/not-an-arn-at-all format is not valid.\')}' - the other three probes matched.
Cause 5: 'error map mismatch: expected={\'DescribeQuery\': (\'QueryIdNotFoundException\', ...), \'GetQueryResults\': (\'QueryIdNotFoundException\', ...), \'CancelQuery\': (\'QueryIdNotFoundException\', ...), \'ListQueries\': (\'EventDataStoreNotFoundException\', \'The event data store arn:...:eventdatastore/efb1a8e3-... was not found.\')} actual={\'DescribeQuery\': (\'EventDataStoreNotFoundException\', \'The event data store was not found.\'), \'GetQueryResults\': (\'QueryIdNotFoundException\', ...), \'CancelQuery\': (\'EventDataStoreNotFoundException\', \'The event data store was not found.\'), \'ListQueries\': (\'EventDataStoreNotFoundException\', ...)}'.
Cause 6: 'tag Value omitted at creation came back as None instead of \'\'' after AddTags(TagsList=[{Key: unicode_key, Value: ...}, {Key: novalue_key}]).
Cause 7: verdict 'mismatch', 'Check timed out after 60.0s'; the four concurrent update_dashboard calls all returned ok (no ConflictException among them) and the check then sits in _poll_until(lambda: ct.get_dashboard(DashboardId=arn), lambda d: d['Status'] == 'UPDATED', timeout_s=120, interval_s=3).
Cause 8: 'call never failed with AccessDeniedException within 45.0s; last outcome None' (cloudtrail.list-insights-data-permission-denied, _wait_for_error over ListInsightsData as the denied principal).

## Steps to Reproduce
1. Boot the emulator from this checkout; create an S3 bucket with the CloudTrail policy and a trail on it where a trail is needed.
2. Cause 1: CreateTrail, then GetTrailStatus on the new trail. Expect IsLogging plus LatestDeliveryAttemptTime/LatestNotificationAttemptTime/LatestNotificationAttemptSucceeded/LatestDeliveryAttemptSucceeded/TimeLoggingStarted/TimeLoggingStopped all present as ''; observe only IsLogging.
3. Cause 2: create two trails, AddTags on each, then ListTags with a 25-element ResourceIdList of those two ARNs. Expect 200 with 25 positional entries and no NextToken; observe 400 ValidationException.
4. Cause 3: LookupEvents with MaxResults=51 (and 100, 1000) over a 89-day window. Expect 200 with at most 50 events; observe 400 ValidationException.
5. Cause 4: PutEventConfiguration with EventDataStore='arn:aws:cloudtrail:<region>:<account>:eventdatastore/not-an-arn-at-all'. Expect InvalidParameterException; observe EventDataStoreARNInvalidException. Compare GetEventConfiguration/EnableFederation/DisableFederation with the same value, which correctly answer EventDataStoreARNInvalidException.
6. Cause 5: DescribeQuery and CancelQuery with a random 36-char QueryId and EventDataStore set to an event-data-store ARN that does not exist. Expect QueryIdNotFoundException 404 on both; observe EventDataStoreNotFoundException.
7. Cause 6: AddTags on a trail with TagsList=[{'Key': 'k'}] (Value omitted), then ListTags. Expect the entry with Value ''; observe no Value member (None).
8. Cause 7: CreateDashboard with TerminationProtectionEnabled=false, fire four concurrent UpdateDashboard(TerminationProtectionEnabled=true), then GetDashboard. Expect Status to settle at 'UPDATED' (and any concurrent loser refused ConflictException 409); observe Status never reaching UPDATED and zero refusals.
9. Cause 8: call ListInsightsData with credentials an explicit deny covers. Expect AccessDeniedException; observe a normal response.
