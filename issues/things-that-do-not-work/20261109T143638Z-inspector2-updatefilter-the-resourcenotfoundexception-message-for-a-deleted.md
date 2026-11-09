Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: inspector2.filter-create-list-delete-lifecycle inspector2.update-filter-action-and-criteria-persist
Issue-Shape-Id: com.amazonaws.inspector2#Inspector2
Issue-Key: inspector2.filter-create-list-delete-lifecycle|run#20|AssertionError|assert message == expected, f"expected the message {expected!r}, got {message!r}"
Issue-Key: inspector2.update-filter-action-and-criteria-persist|run#26|AssertionError|assert message == expected, f"expected the message {expected!r}, got {message!r}"
Issue-Commit: 53cd3feb2c3f3c67eaa2a19cd2ddf09d89eca5f3

# inspector2 UpdateFilter: the ResourceNotFoundException message for a deleted filter ARN is emulator-worded and omits AWS's "(Service: MandoFindings, Status Code: 404, Request ID: …) (SDK Attempt Count: …)" envelope

> _Note: this issue was identified against the code base at commit `53cd3feb2c3f3c67eaa2a19cd2ddf09d89eca5f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFilter

## Description
ONE root cause, grounded in two failing behavioral checks. Related open issue 20261102T121628Z-inspector2-updatefilter-the-resourcenotfoundexception-message-for-a-well.md describes the same wording defect, but the replay suite does not report these two checks as owned by it (their assertion/replay key has moved), so they are filed here; close them together if they are in fact one work item.

Cause 1 of 1: UpdateFilter on a syntactically well-formed filter ARN whose filter was created and then deleted correctly answers 404 ResourceNotFoundException, but with an emulator-authored message instead of AWS's.

AWS: Smithy (services/service-com-amazonaws-inspector2-inspector2/smithy-model/com-amazonaws-inspector2-inspector2.json): UpdateFilter POST /filters/update, input member filterArn required, target FilterArn, @length min 1 max 128; modeled error ResourceNotFoundException, errorKind client, httpError 404. The model does not state the message text, so the wording is recorded behavior only: AWS answers 404 ResourceNotFoundException with
    "<filterArn> does not exist (Service: MandoFindings, Status Code: 404, Request ID: <request-id>) (SDK Attempt Count: <n>)"
Fixed text, which is the requirement: the ARN leads the message with NO noun-phrase prefix, the sentence is " does not exist" with NO trailing period, and it is followed by the Inspector2 backend's downstream-exception envelope " (Service: MandoFindings, Status Code: 404, Request ID: " + id + ") (SDK Attempt Count: " + n + ")". Variable parts: <filterArn> is echoed verbatim from the request; <request-id> is a per-response UUID and <n> a per-response attempt counter — the checks match those two with [0-9a-f-]{36} and \\d+, so no recorded id or count is being required, only the envelope's fixed text and the 404 status.

Emulator: answers 404 ResourceNotFoundException (code and status correct) with the message "The filter <filterArn> does not exist." — it prepends "The filter ", ends with a period, and omits the entire "(Service: MandoFindings, Status Code: 404, Request ID: …) (SDK Attempt Count: …)" envelope.

Why: clients and operators that classify or parse Inspector2 not-found text (error-classification rules keyed on the backend name or the embedded "Status Code: 404", log scrapers, suppression/retry policies, support tooling that extracts the server-side Request ID for escalation) get no match at all; the envelope is the only place that request id appears in this error, so a client recording it for correlation records nothing. The extra "The filter " prefix and trailing period also break exact-text comparisons in regression suites and runbooks.

Fix: the UpdateFilter handler's not-found path in the inspector2 crate (services/service-com-amazonaws-inspector2-inspector2/src/ops/update_filter.rs and the shared ResourceNotFoundException constructor it calls) — emit the ARN-first "<filterArn> does not exist" text plus the "(Service: MandoFindings, Status Code: 404, Request ID: <uuid>) (SDK Attempt Count: <n>)" envelope with a freshly generated request id per response, and drop the "The filter "/period wording.

Traces: inspector2.filter-create-list-delete-lifecycle, inspector2.update-filter-action-and-criteria-persist.

## Evidence
Both checks complete a full lifecycle first — CreateFilter (action NONE), ListFilters paging shows the new arn, DeleteFilter, ListFilters no longer shows it, and in update-filter-action-and-criteria-persist a successful UpdateFilter whose action SUPPRESS and criteria severity EQUALS HIGH do persist — then call UpdateFilter on the deleted filter's ARN.

Expected (AWS, as pinned by both checks): code ResourceNotFoundException, HTTP 404, message equal to
  "<arn> does not exist (Service: MandoFindings, Status Code: 404, Request ID: " + [0-9a-f-]{36} + ") (SDK Attempt Count: " + \\d+ + ")"

Actual (emulator), both checks:
  AssertionError: expected the message 'arn:...:owner/<account>/filter/<id> does not exist (Service: MandoFindings, Status Code: 404, Request ID: ?) (SDK Attempt Count: ?)', got 'The filter arn:aws:inspector2:us-west-2:000000000000:owner/000000000000/filter/0000000000000001 does not exist.'
(The '?' placeholders appear because the regex capturing the request id and attempt count found nothing to capture — the envelope is absent entirely.)

Assertion sites: check_inspector2_filter_create_list_delete_lifecycle.py line 60 and check_inspector2_update_filter_action_and_criteria_persist.py line 77, both `assert message == expected`.

The region, account id and filter id in the actual message are this environment's values filling the template; the divergence is the fixed text ("The filter " prefix, trailing period) and the missing Service/Status Code/Request ID/SDK Attempt Count envelope.

## Steps to Reproduce
1. Build and boot the emulator and replay the behavioral suite (simparity-behavioral-aws-v1).
2. CreateFilter with action=NONE, a unique name and filterCriteria {"severity":[{"comparison":"EQUALS","value":"HIGH"}]}; note the returned arn.
3. DeleteFilter with that arn and confirm ListFilters no longer reports it.
4. Call UpdateFilter with filterArn = that arn, action=SUPPRESS and any valid filterCriteria.
5. Observe code ResourceNotFoundException / HTTP 404 (correct) but Error.Message = "The filter <arn> does not exist." instead of "<arn> does not exist (Service: MandoFindings, Status Code: 404, Request ID: <uuid>) (SDK Attempt Count: <n>)".
6. Equivalent paths exercised by inspector2.filter-create-list-delete-lifecycle and inspector2.update-filter-action-and-criteria-persist.
