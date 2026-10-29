Issue-Source: trace-sweep-divergence
Issue-Trace: savingsplans-create-plan-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: savingsplans-create-plan-error-paths savingsplans-describe-rates-unknown-plan-empty-result savingsplans-filtered-offering-rates-and-plans savingsplans-queued-plan-delete-lifecycle
Issue-Shape-Id: com.amazonaws.savingsplans#AWSSavingsPlan
Issue-Key: savingsplans|CreateSavingsPlan|4xx->4xx|$.message
Issue-Key: savingsplans|DescribeSavingsPlanRates|2xx->4xx|$.savingsPlanId
Issue-Key: savingsplans|DescribeSavingsPlanRates|2xx->4xx|$.searchResults
Issue-Key: savingsplans|DescribeSavingsPlanRates|2xx->4xx|status
Issue-Key: savingsplans|DescribeSavingsPlansOfferings|2xx->2xx|$.nextToken
Issue-Key: savingsplans|ListTagsForResource|2xx->2xx|$.tags.aws
Issue-Key: savingsplans|DescribeSavingsPlanRates|2xx->2xx|$.searchResults[*]
Issue-Key: savingsplans|DescribeSavingsPlans|2xx->2xx|$.savingsPlans[*]
Issue-Key: savingsplans|DeleteQueuedSavingsPlan|4xx->4xx|$.message
Issue-Commit: 355c5d16add239c66c8799ffa26377be788e0f43

# savingsplans handlers diverge from recorded AWS answers in error text, status, pagination, tags and rate cards

> _Note: this issue was identified against the code base at commit `355c5d16add239c66c8799ffa26377be788e0f43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSavingsPlan

## Description
Replay of the savingsplans wire corpus (simparity-wire-aws-v1, 6 fixtures / 27 steps, 8 steps mismatched) shows SEVEN distinct causes in the service-com-amazonaws-savingsplans-awssavingsplan crate. All seven must be closed before this issue is submitted; each is numbered below with its own fixture and step.

Cause 1 of 7: CreateSavingsPlan — 404 ResourceNotFoundException message is the crate's own sentence, not AWS's.
AWS: Smithy: CreateSavingsPlan errors ResourceNotFoundException (httpError 404), input savingsPlanOfferingId @required; the model does not state the message text, so recorded behavior only. For an offering id that does not exist AWS answers HTTP 404 ResourceNotFoundException with the FIXED message 'Offering ID not found.  Insert a valid offering ID and try again.' (note the two spaces after the first sentence). The message has NO variable parts — it never echoes the submitted offering id.
Emulator: 404 ResourceNotFoundException with 'no Savings Plan offering with id '<offeringId>'' — a different sentence, and it interpolates the request's offering id into text AWS keeps fixed.
Why: a client (or test) that branches on, logs or asserts the service's error message for an unknown offering never sees the sentence AWS sends; the operator-facing remediation hint ('Insert a valid offering ID and try again.') is lost.
Fix: the CreateSavingsPlan handler's offering-lookup failure path — the ResourceNotFoundException it constructs must carry AWS's fixed sentence.
Traces: savingsplans-create-plan-error-paths#task#2.

Cause 2 of 7: DescribeSavingsPlanRates — a well-formed but unknown savingsPlanId is answered 404 instead of an empty 200 result.
AWS: Smithy: DescribeSavingsPlanRates input savingsPlanId @required, output { savingsPlanId, searchResults, nextToken }; ResourceNotFoundException is modeled (404) but recorded behavior shows AWS does NOT use it for a syntactically valid plan id that the account does not hold: it answers HTTP 200 echoing the requested savingsPlanId and an EMPTY searchResults list. Hypothesis: AWS resolves the rate card as a search, so no match is an empty search rather than a missing resource.
Emulator: HTTP 404 ResourceNotFoundException ('no Savings Plan with id '<id>''), with no savingsPlanId and no searchResults in the body.
Why: a client enumerating rate cards for plan ids it holds a stale list of gets a hard client error instead of an empty page, so the call fails rather than returning 'nothing to price'; SDK retry/error handling escalates what AWS treats as a successful empty search.
Fix: the DescribeSavingsPlanRates handler's plan lookup — an unknown-but-well-formed plan id must produce a 200 with the echoed savingsPlanId and an empty searchResults rather than the modeled 404.
Traces: savingsplans-describe-rates-unknown-plan-empty-result#task#1.

Cause 3 of 7: DescribeSavingsPlansOfferings — a nextToken is minted for an answer that has no further results.
AWS: Smithy: DescribeSavingsPlansOfferings output nextToken (PaginationToken, len 0..1024) documented as 'This value is null when there are no more results to return.' Recorded behavior: for a filtered search whose single page already holds every match, AWS answers HTTP 200 with nextToken as the EMPTY string '' — i.e. no continuation.
Emulator: HTTP 200 with a non-empty opaque cursor (a hex-encoded '<pageKind>:<offset>' value) on the first, unresumed answer, even though the page returned every match and the walk reached offset 0.
Why: the standard SDK/CLI paginator loops while nextToken is non-empty, so it issues at least one extra call per search and, because the minted cursor can point back at the same offset, a client that trusts the cursor to advance can loop without making progress.
Fix: the shared search-cursor rule used by the offerings search (the paginator helper the DescribeSavingsPlansOfferings handler calls) — emit no continuation (empty/absent nextToken) when the served page exhausted the matches, instead of always carrying a cursor on the first answer. Related but distinct open issue: 20261029T084556Z-savingsplans-describesavingsplanrates-pagination.md (which is about cursor spellings decode_page_token accepts); it does not cover this over-emission.
Traces: savingsplans-filtered-offering-rates-and-plans#task#2.

Cause 4 of 7: ListTagsForResource — a tag key carrying the reserved 'aws:' prefix is not stored/returned.
AWS: Smithy: ListTagsForResource output tags (TagMap); recorded behavior only. AWS returns the plan's tag set verbatim, including a key with the 'aws:' prefix (the recording shows an 'aws:tag:<name>' key with its value) — the key is preserved exactly as written on the plan.
Emulator: HTTP 200 but the tags map omits the 'aws:'-prefixed entry entirely (expected value present, got None); the write that set it returned success, so the tag is silently lost. Hypothesis: the tag-ingestion path drops or filters keys with the reserved 'aws:' prefix; crate-side check_tags (state.rs) only enforces count/length, so the drop happens before merge.
Why: a client that tags a plan and reads the tags back sees a tag it successfully wrote disappear — tag-driven cost allocation, filtering and authorization all mis-evaluate, and the write reports success for state that was never kept.
Fix: the tag write path (the TagResource handler and CreateSavingsPlan's tags ingestion into the plan record's tag set) — store every supplied key verbatim, including reserved-prefix keys, or refuse the write outright instead of accepting and discarding it.
Traces: savingsplans-queued-plan-delete-lifecycle#task#5 (same cause recurs at savingsplans-queued-plan-delete-lifecycle#task#7).

Cause 5 of 7: DescribeSavingsPlanRates — the rate card of a purchased plan is one synthetic line item instead of the plan's full per-product-description card.
AWS: Smithy: DescribeSavingsPlanRates output searchResults (SavingsPlanRateList of SavingsPlanRate{rate, currency, unit, productType, serviceCode, usageType, operation, properties}); content is recorded behavior only. The recording shows a card with 32 entries for one EC2 plan: one line item per (productDescription, operation-code) pair — operation values are the usage-operation codes ('RunInstances' for Linux/UNIX, 'RunInstances:0006' for Windows with SQL Server Standard, 'RunInstances:0200' for Linux with SQL Server Web, and so on) — each with usageType spelled WITHOUT a region prefix ('BoxUsage:<instanceType>'), rate rendered at three decimal places, and properties limited to tenancy, instanceType, region, productDescription.
Emulator: HTTP 200 with a SINGLE entry: usageType region-prefixed ('<region>-BoxUsage:<instanceType>'), operation always the bare 'RunInstances', rate rendered at eight decimal places, and a properties list that adds an instanceFamily property AWS does not emit for a plan rate. Entries 1..31 of the expected card are absent.
Why: a client pricing covered usage can only match the one Linux/UNIX line item; every other product description (Windows, SQL Server variants) resolves to no rate, and the region-prefixed usageType does not join against usage records whose usageType AWS reports unprefixed — so cost calculations are wrong and silently incomplete.
Fix: the plan rate-card catalog and its projection (the rate line-item catalog in the service state plus the DescribeSavingsPlanRates projection) — enumerate one line item per product description/operation code the plan's commitment covers, spell usageType without the region prefix, emit only the four properties AWS reports, and render rate at AWS's precision.
Traces: savingsplans-queued-plan-delete-lifecycle#task#8.

Cause 6 of 7: DescribeSavingsPlans — the projected plan drops its tags, collapses returnableUntil onto start, and ends the term one second late.
AWS: Smithy: DescribeSavingsPlans output savingsPlans (SavingsPlan{..., start, end, returnableUntil, tags, termDurationInSeconds}); timestamp relations are recorded behavior only. The recording shows, for a plan in 'queued-deleted' state: tags carry the plan's tag set (the 'aws:tag:<name>' entry is present); end = start + termDurationInSeconds - 1 second (the last second of the term, not the first second after it); returnableUntil = start + 7 days (the documented 168-hour return window close), reported even for a plan whose queued purchase was cancelled.
Emulator: HTTP 200 but tags is an empty map; returnableUntil equals start exactly (the record's has_return_window excludes the queued-deleted state, so returnable_until_millis falls back to start_millis — state.rs returnable_until_millis/has_return_window); and end = start + termDurationInSeconds, one second past AWS's value. The absolute start/end instants also differ between recording and replay, which is per-run clock and NOT part of this finding — only the relations above are.
Why: a client reading a plan's tags after a successful tag write sees none (same loss as cause 4, on a second operation); a client computing the remaining return window from returnableUntil concludes the window already closed at the plan's start; and term-boundary arithmetic (billing periods, renewal) is off by one second.
Fix: the DescribeSavingsPlans projection and the plan record's derived instants in the service state — include the stored tag set, report the return-window close for a cancelled queued plan, and derive end as start + termDurationInSeconds - 1s.
Traces: savingsplans-queued-plan-delete-lifecycle#task#11.

Cause 7 of 7: DeleteQueuedSavingsPlan — the 400 refusal message is the crate's own sentence, not AWS's.
AWS: Smithy: DeleteQueuedSavingsPlan errors ValidationException (httpError 400); message text is recorded behavior only. Deleting a plan that is not in the queued state (here one already queued-deleted) answers HTTP 400 ValidationException with the FIXED message 'Savings Plan is not in Queued State.' — no variable parts; the current state is never interpolated.
Emulator: HTTP 400 ValidationException with 'Savings Plans in <state> state cannot be deleted.' — different fixed text, plus the plan's state substituted from emulator state where AWS substitutes nothing.
Why: callers that make the double-delete / wrong-state case idempotent by matching the service's message see a sentence AWS never sends, so the refusal is reported as an unknown failure.
Fix: the DeleteQueuedSavingsPlan handler's state guard — construct the ValidationException with AWS's fixed sentence.
Traces: savingsplans-queued-plan-delete-lifecycle#task#12.

## Evidence
Replay summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, steps_total=27, passed=19, mismatched=8, errored=0, blocked=0, score_pct=70.4, divergence_count=7. No cluster carried a stub_path or owned_by, so none is already filed.

Cause 1 — savingsplans-create-plan-error-paths#task#2, CreateSavingsPlan, status 404 expected / 404 actual: $.message expected 'Offering ID not found.  Insert a valid offering ID and try again.', got "no Savings Plan offering with id '<offeringId>'". Fixed text = the whole AWS sentence; the emulator's variable part (the offering id from the request) does not belong in it.

Cause 2 — savingsplans-describe-rates-unknown-plan-empty-result#task#1, DescribeSavingsPlanRates: status expected 200, got 404; $.savingsPlanId missing (expected the requested plan id, got None); $.searchResults missing (expected [], got None).

Cause 3 — savingsplans-filtered-offering-rates-and-plans#task#2, DescribeSavingsPlansOfferings, status 200/200: $.nextToken expected '' (empty string), got a non-empty opaque cursor that hex-decodes to '<pageKind>:0'.

Cause 4 — savingsplans-queued-plan-delete-lifecycle#task#5 (and #task#7), ListTagsForResource, status 200/200: $.tags['aws:tag:<name>'] missing (expected the written value, got None).

Cause 5 — savingsplans-queued-plan-delete-lifecycle#task#8, DescribeSavingsPlanRates, status 200/200: expected 32 searchResults entries, got 1. Expected entries differ in operation (usage-operation codes such as 'RunInstances:0006', 'RunInstances:0200', bare 'RunInstances'), usageType ('BoxUsage:<instanceType>' vs emulator '<region>-BoxUsage:<instanceType>'), rate precision (3 vs 8 decimal places) and properties (tenancy/instanceType/region/productDescription vs an extra instanceFamily); entries [1]..[31] came back None.

Cause 6 — savingsplans-queued-plan-delete-lifecycle#task#11, DescribeSavingsPlans, status 200/200: $.savingsPlans[0] diverges — tags expected to contain the 'aws:tag:<name>' entry, got {}; returnableUntil expected start + 7 days, got a value equal to start; end expected start + termDurationInSeconds - 1s, got start + termDurationInSeconds. (The absolute start/end values also differ because recording and replay ran at different clocks; that part is excluded.)

Cause 7 — savingsplans-queued-plan-delete-lifecycle#task#12, DeleteQueuedSavingsPlan, status 400/400: $.message expected 'Savings Plan is not in Queued State.', got 'Savings Plans in queued-deleted state cannot be deleted.'

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; replay the simparity-wire-aws-v1 wire corpus (6 savingsplans fixtures, 27 steps).
2. Cause 1: replay savingsplans-create-plan-error-paths and compare step task#2 (CreateSavingsPlan with an offering id the catalog does not hold) — check the 404 body's message.
3. Cause 2: replay savingsplans-describe-rates-unknown-plan-empty-result step task#1 (DescribeSavingsPlanRates for a well-formed plan id that was never purchased) — expect 200 with the echoed savingsPlanId and searchResults [].
4. Cause 3: replay savingsplans-filtered-offering-rates-and-plans step task#2 (DescribeSavingsPlansOfferings with filters whose matches fit one page) — expect nextToken ''.
5. Causes 4, 5, 6, 7: replay savingsplans-queued-plan-delete-lifecycle and compare steps task#5 and task#7 (ListTagsForResource tag set), task#8 (DescribeSavingsPlanRates full rate card), task#11 (DescribeSavingsPlans projection: tags, returnableUntil, end) and task#12 (DeleteQueuedSavingsPlan on a plan already queued-deleted).
6. Re-replay all six fixtures; every step above must pass before submitting.
