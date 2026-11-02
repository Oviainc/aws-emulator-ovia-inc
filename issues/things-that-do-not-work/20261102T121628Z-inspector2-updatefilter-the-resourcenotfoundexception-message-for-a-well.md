Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: inspector2.filter-create-list-delete-lifecycle inspector2.update-filter-action-and-criteria-persist
Issue-Shape-Id: com.amazonaws.inspector2#Inspector2
Issue-Key: inspector2.filter-create-list-delete-lifecycle|run#17|AssertionError|assert re.fullmatch(re.escape(head) + r"[0-9a-f-]{36}\) \(SDK Attempt Count: \d+\)", message), message
Issue-Key: inspector2.update-filter-action-and-criteria-persist|run#23|AssertionError|assert re.fullmatch(re.escape(head) + r"[0-9a-f-]{36}\) \(SDK Attempt Count: \d+\)", message), message
Issue-Commit: f125d0b1eb7b700bf5bc90a9b18e30075b38be6c

# inspector2 UpdateFilter: the ResourceNotFoundException message for a well-formed, deleted filter ARN is emulator-worded and omits AWS's downstream-exception envelope

> _Note: this issue was identified against the code base at commit `f125d0b1eb7b700bf5bc90a9b18e30075b38be6c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFilter

## Description
ONE root cause, grounded in two failing behavioral checks.

Cause 1 of 1: UpdateFilter on a syntactically well-formed filter ARN that no longer exists (the filter was created, then deleted) correctly answers ResourceNotFoundException, but with an emulator-authored message instead of AWS's.

AWS: Smithy (services/service-com-amazonaws-inspector2-inspector2/smithy-model/com-amazonaws-inspector2-inspector2.json): UpdateFilter.filterArn (required, FilterArn, @length min 1 max 128); modeled error ResourceNotFoundException with httpError 404. The model does not state the message text, so the wording is recorded behavior only: for a well-formed filter ARN naming nothing, AWS answers 404 ResourceNotFoundException with the message
    "<filterArn> does not exist (Service: MandoFindings, Status Code: 404, Request ID: <request-id>) (SDK Attempt Count: <n>)"
Fixed text, which is a requirement: the ARN is NOT prefixed by any noun phrase, the sentence is " does not exist" with NO trailing period, and it is followed by the Inspector2 backend's downstream-exception envelope " (Service: MandoFindings, Status Code: 404, Request ID: " + id + ") (SDK Attempt Count: " + n + ")". Variable parts taken from the request and from the serving environment: <filterArn> is echoed verbatim from the request; <request-id> is a per-response UUID and <n> a per-response attempt counter — the checks match those two with [0-9a-f-]{36} and \\d+ respectively, so no recorded id or count is being required here, only the envelope's fixed text and the 404 status.

Emulator: answers 404/ResourceNotFoundException with the message "The filter <filterArn> does not exist." — it prepends "The filter ", terminates with a period, and omits the whole "(Service: MandoFindings, Status Code: 404, Request ID: …) (SDK Attempt Count: …)" envelope. Replay output for both checks: AssertionError: The filter arn:aws:inspector2:<region>:<account>:owner/<account>/filter/<id> does not exist.

Why: clients and operators that parse or match Inspector2 not-found text (log scrapers, error-classification rules, retry/suppression policies keyed on the backend name or the embedded "Status Code: 404", support tooling extracting the server-side Request ID for escalation) get no match at all. The envelope is the only place the server-side request id appears in this error, so a client that records it for correlation records nothing. The leading "The filter " and the trailing period additionally break exact-text comparisons used in regression tests and runbooks.

Fix: the UpdateFilter handler's not-found path in the inspector2 crate (services/service-com-amazonaws-inspector2-inspector2/src/ops/update_filter.rs and whatever shared error constructor it calls for ResourceNotFoundException) — emit the ARN-first "<filterArn> does not exist" text plus the "(Service: MandoFindings, Status Code: 404, Request ID: <uuid>) (SDK Attempt Count: <n>)" envelope, with a freshly generated request id per response, and drop the "The filter "/period wording. Note the related open issue 20261029T171819Z-inspector2-filter-surface-updatefilter-mis-classifies-a-malformed.md: it covers the DIFFERENT case of a MALFORMED filterArn (which must become 400 ValidationException "Invalid Filter Arn <filterArn>") and quotes this same emulator string only as the wrong answer there; it does not state the required message for the genuine, well-formed-ARN not-found path, so it does not cover this cause.

Traces: inspector2.filter-create-list-delete-lifecycle, inspector2.update-filter-action-and-criteria-persist.

## Evidence
Both failing checks reach the end of a full create → list → delete lifecycle (CreateFilter, ListFilters paging, DeleteFilter, and in update-filter-action-and-criteria-persist also a successful UpdateFilter whose action/criteria changes persist), then call UpdateFilter on the deleted filter's ARN.

Expected (AWS, as pinned by both checks): code ResourceNotFoundException, HTTP 404, message fullmatching
  "<arn> does not exist (Service: MandoFindings, Status Code: 404, Request ID: " + [0-9a-f-]{36} + ") (SDK Attempt Count: " + \\d+ + ")"

Actual (emulator): code ResourceNotFoundException (correct), message
  "The filter arn:aws:inspector2:us-west-2:000000000000:owner/000000000000/filter/0000000000000001 does not exist."

Assertion sites:
- check_inspector2_filter_create_list_delete_lifecycle.py line 57: assert re.fullmatch(re.escape(head) + r"[0-9a-f-]{36}\\) \\(SDK Attempt Count: \\d+\\)", message)
- check_inspector2_update_filter_action_and_criteria_persist.py line 74: same assertion.

The account id, region and filter id in the actual message are this environment's values filling the template; the divergence is the fixed text ("The filter " prefix, trailing period) and the entirely missing Service/Status Code/Request ID/SDK Attempt Count envelope.

## Steps to Reproduce
1. Build and boot the emulator and replay the behavioral suite (simparity-behavioral-aws-v1).
2. CreateFilter with action=NONE, a unique name and filterCriteria {"severity":[{"comparison":"EQUALS","value":"HIGH"}]}; note the returned arn.
3. DeleteFilter with that arn (ListFilters no longer reports it).
4. Call UpdateFilter with filterArn = that arn, action=SUPPRESS and any valid filterCriteria.
5. Observe the error: code is ResourceNotFoundException and status 404, but Error.Message is "The filter <arn> does not exist." instead of "<arn> does not exist (Service: MandoFindings, Status Code: 404, Request ID: <uuid>) (SDK Attempt Count: <n>)".
6. Equivalent path exercised by inspector2.filter-create-list-delete-lifecycle and inspector2.update-filter-action-and-criteria-persist.
