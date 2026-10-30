Issue-Source: trace-sweep-divergence
Issue-Trace: freetier-get-freetier-usage-multiple-match-options-rejected
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: freetier-get-freetier-usage-multiple-match-options-rejected
Issue-Shape-Id: com.amazonaws.freetier#AWSFreeTierService
Issue-Key: freetier|GetFreeTierUsage|4xx->4xx|$.message
Issue-Commit: 2fc08aa0d9b2f1df9d08f4c49d739311c4e863a6

# freetier GetFreeTierUsage MatchOptions cardinality refusal carries emulator-authored prose instead of the service's fixed message

> _Note: this issue was identified against the code base at commit `2fc08aa0d9b2f1df9d08f4c49d739311c4e863a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFreeTierUsage

## Description
GetFreeTierUsage correctly REFUSES a Dimensions filter that carries more than one match option, and it refuses with the right error code and the right HTTP status, but the message text is a sentence this crate wrote itself rather than the text the service emits. The status and code already agree, so the message is the only thing that diverges — and an error message is a requirement, not a cosmetic.

Cause 1 of 1: MatchOptions cardinality refusal message.

AWS: Smithy: GetFreeTierUsage.filter targets Expression; its Dimensions member targets DimensionValues, whose required member MatchOptions is documented as "The match options that you can use to filter your results. You can specify only one of these values in the array." The modeled refusal for a violated input constraint on this operation is ValidationException (errorKind client, @httpError 400 — the operation's modeled errors are InternalServerException/500, ThrottlingException/429, ValidationException/400). Recorded behavior for this error class: the message is the fixed string "[Message = Exactly one match option must be provided]". Every character of that is fixed text — the literal "[Message = " prefix and the closing "]" included. It has NO variable parts: no submitted count, no member path, no echo of the offending values. The rule is the one the model documents (only one value permitted in the array), and AWS words every violation of it with that one string regardless of how many options were submitted or which ones.

Emulator: answers HTTP 400 (matching) but with the message "filter.Dimensions.MatchOptions: exactly one match option is allowed, got 2" — a different sentence that prefixes a member path AWS does not put in this message and appends the submitted option count, which AWS's text does not contain at all. (The trailing count is the emulator varying the message with the request where the service's text is invariant; the specific number in the recording is just the count this one request happened to send, not part of any template.)

Why: the message is the only part of this refusal a client can branch on, since the code and status are shared with every other validation failure on the operation. A client, SDK retry/branching wrapper or test that recognises "too many match options" by matching the service's fixed string never matches against the emulator, and anything that parses the emulator's "<member path>: ... got <n>" shape finds nothing to parse against real AWS. Operators reading emulator logs also see a reason string that never occurs in production.

Fix: the MatchOptions cardinality check in this operation's filter validation must raise the service's single fixed string instead of composing a per-request sentence from the member path and the submitted count. That is the input-validation path of the GetFreeTierUsage handler in the freetier service crate (see Traces for the fixture that exercises it).

Traces: fixture freetier-get-freetier-usage-multiple-match-options-rejected, step task#2.

Related but NOT covering this: the open issue 20261029T183000Z-freetier-getfreetierusage-refusal-messages-are-emulator-authored-prose.md describes the same wording family from the BEHAVIORAL corpus (simparity-behavioral-aws-v1) and lists the behavioral check id freetier.get-freetier-usage-multiple-match-options-rejected among its Issue-Trace-Ids, but it carries no wire replay key for this step — the wire replay reports this step as unowned, so this issue exists to bind the wire fixture/step to the defect. A fixer closing either one should re-replay both. Also distinct: the open issues 20261028T211319Z-freetier-listaccountactivities-pagination.md and 20261029T183000Z-freetier-activity-plane-refusals-maxresults-100-accepted-not.md concern ListAccountActivities pagination and the activity plane, not this operation's filter validation.

## Evidence
Wire replay of the bound corpus (trace_set simparity-wire-aws-v1): 6 steps total, 5 passed, 1 mismatched (score 83.3%). The emulator was up (/__admin/health answered 200 and the data listener accepted), so the refusal path was genuinely reached.

Divergence cluster (verdict: mismatch), service freetier, operation GetFreeTierUsage:
  fixture: freetier-get-freetier-usage-multiple-match-options-rejected
  step:    task#2
  divergence_key: service=freetier|GetFreeTierUsage|status=400->400|keys=$.message
  expected status: 400    actual status: 400   (status agrees)
  diverging field: $.message (value_mismatch)
    expected: "[Message = Exactly one match option must be provided]"
    actual:   "filter.Dimensions.MatchOptions: exactly one match option is allowed, got 2"

Fixed vs variable parts of the required message: the whole of "[Message = Exactly one match option must be provided]" is fixed text, including the "[Message = " wrapper and the closing "]". There are no variable parts — nothing from the request or from emulator state belongs in it. The emulator's extra material (the "filter.Dimensions.MatchOptions" member path and the "got 2" count taken from the request) is precisely what must not appear.

Model backing (Smithy, services/service-com-amazonaws-freetier-awsfreetierservice/smithy-model/com-amazonaws-freetier-awsfreetierservice.json): GetFreeTierUsage errors = {InternalServerException 500, ThrottlingException 429, ValidationException 400}; DimensionValues.MatchOptions doc = "The match options that you can use to filter your results. You can specify only one of these values in the array."

The remaining 5 steps of the corpus passed, so the operation's success path and its other refusals are not implicated by this finding.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from that checkout with the freetier service mounted (the wire replay confirms /__admin/health answers 200 and the data listener accepts).
2. Replay the wire fixture freetier-get-freetier-usage-multiple-match-options-rejected and watch step task#2. Equivalently, drive it by hand: build a freetier client against the emulator endpoint and call GetFreeTierUsage with a Dimensions filter whose MatchOptions array holds more than one option, e.g. filter={"Dimensions": {"Key": "REGION", "Values": ["us-east-1"], "MatchOptions": ["EQUALS", "STARTS_WITH"]}}.
3. Observe HTTP 400 with Code=ValidationException (both correct) and read the response's message field. The emulator returns "filter.Dimensions.MatchOptions: exactly one match option is allowed, got 2"; the required value is exactly "[Message = Exactly one match option must be provided]".
4. Confirm the refusal path is otherwise reached and the surrounding call works by re-issuing the same request with a single option, MatchOptions=["EQUALS"], which succeeds.
5. Vary the number of options (three, four) and confirm the message must NOT change: the service's string is invariant, so no count may appear in it.
6. Re-replay fixture freetier-get-freetier-usage-multiple-match-options-rejected step task#2 to confirm the fix, and re-run the behavioral check freetier.get-freetier-usage-multiple-match-options-rejected named by the related open issue.
