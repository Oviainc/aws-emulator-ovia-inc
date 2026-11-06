Issue-Source: trace-sweep-divergence
Issue-Trace: savingsplans-queued-plan-delete-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: savingsplans-queued-plan-delete-lifecycle
Issue-Shape-Id: com.amazonaws.savingsplans#AWSSavingsPlan
Issue-Key: savingsplans|ListTagsForResource|2xx->2xx|$.tags.aws:tag:domain
Issue-Commit: 85bf1a921c90e8ec0578295db955b3f20c9762fc

# savingsplans ListTagsForResource omits the reserved aws:-prefixed tag key from a plan's tag set

> _Note: this issue was identified against the code base at commit `85bf1a921c90e8ec0578295db955b3f20c9762fc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
Replay of the savingsplans wire corpus (simparity-wire-aws-v1, 6 fixtures / 27 steps, 8 steps mismatched, score 70.4%) leaves ONE divergence cluster that the replay report marks neither stub-blocked nor owned by an open issue. It is a single cause.

Cause 1 of 1: ListTagsForResource drops a tag key carrying the reserved 'aws:' prefix from the tag set it reports for a Savings Plan.

AWS: Smithy: ListTagsForResource input resourceArn (SavingsPlanArn, @required, pattern ^arn:aws([-a-z]*)?:[a-z]+:(...):(\d{12}):savingsplan/<uuid>$), output tags -> TagMap (key TagKey, value TagValue; neither key nor value carries a length or pattern constraint, and the model states no reserved-prefix rule), errors InternalServerException 500 / ResourceNotFoundException 404 / ValidationException 400. The tag CONTENT rule is recorded behavior only: for a Savings Plan whose tag set includes a key spelled with the reserved 'aws:' prefix ('aws:tag:<name>'), AWS answers HTTP 200 and the tags map carries that entry with its value, exactly as spelled on the plan. AWS reports a plan's stored tag set verbatim; a key is never filtered out of the read, and whatever happened to it on the write (a reserved-prefix key is kept, and is not removable by an ordinary untag) it is still present on the subsequent read.

Emulator: HTTP 200 with the expected status and shape, but the tags map OMITS the reserved-prefix entry entirely — the replayed body has no 'aws:tag:<name>' member at all (expected the written value, got None), while every other asserted field matches. The same omission recurs on the second ListTagsForResource of the same plan later in the lifecycle, so it is a stable state property of the plan rather than a one-off read.

Why: a client that writes a tag and reads it back sees a tag it wrote (and that the service accepted with a success response) silently disappear. TagResource's response body is empty, so the write itself gives the client no signal — the loss is only observable on the read, which is exactly the call cost-allocation, inventory and tag-based authorization tooling uses. Those consumers conclude the plan is untagged for that key and mis-attribute, mis-filter or mis-authorize it; an idempotent tag-then-verify loop retries forever because the verify never converges.

Fix: the savingsplans crate's tag read/write path for the plan record — the tag set a plan holds must keep every supplied key verbatim, including reserved-prefix keys, and ListTagsForResource must project that set unfiltered (or the write must be refused outright instead of accepted and discarded). Hypothesis (the mechanism is not visible in the shared state helpers, so it is labelled as such): neither crate::state::check_tags nor SavingsPlan::merge_tags applies any prefix rule — check_tags enforces only key/value length and the 50-tag allowance — and SavingsPlan::remove_tags deletes any key handed to it, so the candidate sites are (a) an operation-level filter on the ingestion side (TagResource / CreateSavingsPlan tags) that discards 'aws:'-prefixed keys after reporting success, or (b) remove_tags honouring an untag of a reserved key that AWS keeps. Whichever it is, the observable requirement is the one above: the key is present on the read.

Note: the open issue 20261029T203239Z-savingsplans-handlers-diverge-from-recorded-aws-answers-in.md describes this same omission as its Cause 4 and lists the fixture in its Issue-Trace-Ids, but its recorded replay key for the cluster is normalized to 'savingsplans|ListTagsForResource|2xx->2xx|$.tags.aws' while the cluster replays under '$.tags.aws:tag:domain', so this replay reports the cluster as UNOWNED (owned_by empty) and it is filed here so the gap is not closed unrun. A fixer closing either issue should re-replay the steps named below.

## Evidence
Replay summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true (health 200, data listener accepts), bootstrap_signal=false, steps_total=27, passed=19, mismatched=8, errored=0, blocked=0, score_pct=70.4, divergence_count=7. Six of the seven clusters carry owned_by=20261029T203239Z-savingsplans-handlers-diverge-from-recorded-aws-answers-in.md; this one carries owned_by=[] and stub_path="".

Cluster: divergence_key service=savingsplans|ListTagsForResource|status=200->200|keys=$.tags.aws:tag:domain, verdict=mismatch, expected_status=200, actual_status=200, affected_count=2.
Detail verbatim: "$.tags.aws:tag:domain: missing (expected 'prod', got None)".
Representative step: savingsplans-queued-plan-delete-lifecycle#task#5; the same cause recurs at savingsplans-queued-plan-delete-lifecycle#task#7.

What is a requirement and what is not: the requirement is that the tags map returned by ListTagsForResource contains an entry for every key stored on the plan, including a reserved 'aws:'-prefixed key, with the value as written. The particular key spelling and value in the recording ('aws:tag:domain' -> 'prod') are the fixture's request values filling that template, not part of it; the HTTP status (200) and the output shape (tags: TagMap) already match and are not in dispute.

Code read while grounding the Fix (citations only, not a patch): services/service-com-amazonaws-savingsplans-awssavingsplan/src/state.rs lines 635-660 (check_tags: key length / value length / 50-tag allowance only, no prefix rule), lines 1866-1875 (SavingsPlan::merge_tags: clone, extend, check_tags, store), lines 1883-1887 (SavingsPlan::remove_tags: removes any key given); services/service-com-amazonaws-savingsplans-awssavingsplan/src/ops/tag_resource.rs lines 139-183 (ARN check, plan lookup, merge_tags, empty 200 output — no key filtering); services/service-com-amazonaws-savingsplans-awssavingsplan/src/ops/create_savings_plan.rs lines 185-204 (tags collected from input and bounded by check_tags).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (6 savingsplans fixtures, 27 steps).
3. Replay fixture savingsplans-queued-plan-delete-lifecycle and compare step task#5 (ListTagsForResource on the purchased plan's ARN): the 200 body's tags map must contain the reserved-prefix key the lifecycle wrote onto the plan, with its value — the emulator's map omits that entry.
4. Compare step task#7 of the same fixture (the second ListTagsForResource of the same plan): same omission, confirming the key is absent from stored state rather than from one projection.
5. After the fix, re-replay savingsplans-queued-plan-delete-lifecycle; task#5 and task#7 must both pass.
