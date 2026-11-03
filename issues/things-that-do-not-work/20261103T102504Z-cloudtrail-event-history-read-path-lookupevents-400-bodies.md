Issue-Source: trace-sweep-divergence
Issue-Trace: cloudtrail-lookup-events-query-mistakes
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudtrail-lookup-events-query-mistakes cloudtrail-lookup-events-and-public-keys
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail|LookupEvents|4xx->4xx|$.Message
Issue-Key: cloudtrail|LookupEvents|2xx->4xx|$.Events
Issue-Key: cloudtrail|LookupEvents|2xx->4xx|status
Issue-Key: cloudtrail|ListPublicKeys|2xx->2xx|$.PublicKeyList[*]
Issue-Commit: a9dbf902d0b1089de5376992e6646091feea50d4

# CloudTrail event-history read path: LookupEvents 400 bodies carry no Message, a lookup AWS answers 200 is refused, and ListPublicKeys serves one synthetic key

> _Note: this issue was identified against the code base at commit `a9dbf902d0b1089de5376992e6646091feea50d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
THREE distinct net-new causes, all in CloudTrail's event-history read path (LookupEvents / ListPublicKeys). All three must be closed before submitting. They are reported by the wire replay of trace set simparity-wire-aws-v1 and none of them is owned by an open issue in the replay report.

Cause 1 of 3: every LookupEvents rejection body is serialized WITHOUT the modelled Message member.
AWS: Smithy (services/service-com-amazonaws-cloudtrail-cloudtrail-20131101) binds InvalidLookupAttributesException, InvalidEventCategoryException, InvalidTimeRangeException, InvalidNextTokenException, InvalidMaxResultsException and OperationNotPermitted/UnsupportedOperation to LookupEvents; each is a client error with httpError 400, and each error structure declares exactly one member, literally named `Message` (target ErrorMessage, @length min 4 max 1000, @pattern '.*', doc "Brief description of the exception returned by the request.") with NO jsonName override — so on the wire the error body MUST carry the key `Message`. The recorded responses confirm it: a LookupAttributes entry whose AttributeKey is not a modelled lookup key is refused 400 with Message "Lookup attribute key cannot be null or empty." — the whole sentence is FIXED text for that error class, with no variable part (it does not echo the rejected key, the list index or a member path).
Emulator: the status and the error class are right (400), but the comparison on $.Message yields None for every one of the four refused steps in this cluster — the body simply has no `Message` key at the top level of the error. Hypothesis: the error serializer emits the member under a different casing (lowercase `message`, the generic JSON-protocol default) or drops it, rather than under CloudTrail's modelled `Message`.
Why: Message is the only populated member of these errors, so a client gets a 400 with an empty description: botocore's `err.response['Error']['Message']` is '', every operator-facing log line and runbook match is blank, and anything that classifies a lookup refusal by its sentence (unknown attribute key vs out-of-domain value vs bad time range) cannot classify it at all. Note the separate open behavioral issue 20261029T173328Z-cloudtrail-lookupevents-rejection-messages-use-a-generic-constraint.md records these messages as present but RE-WORDED; on the wire path exercised here the member is absent entirely, which is a different defect in a different component (the error serializer, not the message builder) and must be fixed for the message fix to be observable.
Fix: the cloudtrail crate's error serializer / the LookupEvents handler's refusal path must emit the modelled `Message` member, spelled exactly as the model spells it, carrying the fixed per-error text for that error class.
Traces: cloudtrail-lookup-events-query-mistakes#task#1 (representative), plus task#4, task#5 and task#6 of the same fixture in this cluster.

Cause 2 of 3: a LookupEvents query AWS ACCEPTS is refused 400, so the empty-result path is unreachable.
AWS: recorded behavior only — the recording shows this LookupEvents request answered with HTTP 200 and the body `{"Events": []}`: a well-formed lookup that simply matches no event in the window is a SUCCESS with an empty Events list (Smithy: LookupEvents.Events targets EventsList, the operation is paginated with items Events, and no modelled error covers 'nothing matched'). An empty list is emitted, not omitted.
Emulator: answers HTTP 400 for the same step (expected 200, got 400) and the response carries no Events member at all ($.Events expected [], got None). Hypothesis: the request-validation layer in front of the handler rejects a bound AWS does not enforce on the request — the same shape of defect the open issue 20261024T114720Z-cloudtrail-control-plane-fidelity-gettrailstatus-omits-its-deprecated.md records as its Cause 3 (LookupEvents refusing MaxResults above the modelled maximum with ValidationException where AWS clamps to the page ceiling and returns 200). That issue does not carry this fixture step's replay key, so the wire divergence is filed here and should be fixed together with it.
Why: a client that paginates event history, or that legitimately polls a window with no matching events, gets a hard 400 from the emulator where AWS returns an empty page — the 'no results yet' branch every poller relies on can never be taken, and a request shape the SDK itself produces is rejected.
Fix: the LookupEvents request-validation seam must stop refusing requests AWS accepts (enforce the page ceiling on the RESPONSE as the handler already clamps), and the handler must answer 200 with an explicitly empty Events list when nothing matches.
Traces: cloudtrail-lookup-events-query-mistakes#task#2.

Cause 3 of 3: ListPublicKeys returns a single synthetic key for the whole requested window instead of one entry per signing-key validity period, and its ValidityEndTime is not a whole second.
AWS: Smithy models ListPublicKeys(StartTime, EndTime, NextToken) -> PublicKeyList of PublicKey{Value (blob, DER PKCS#1), ValidityStartTime (timestamp Date), ValidityEndTime (timestamp Date), Fingerprint (String)}; the operation is paginated over PublicKeyList. Recorded behavior: for the requested time range the response carries FOUR PublicKey entries — one per digest-signing key whose validity period intersects the window, each with its own ValidityStartTime/ValidityEndTime pair spanning that key's own (roughly 30-day) period and each timestamp a whole number of seconds — plus a distinct Fingerprint and Value per entry.
Emulator: returns ONE entry covering the entire window (recorded run: a single key whose ValidityStartTime/ValidityEndTime span the whole multi-month range, with ValidityEndTime carrying a fractional second), so $.PublicKeyList[0] differs and [1], [2], [3] are missing outright. The particular key bytes, fingerprints and epoch values in either side of the recording are per-environment generated values and are NOT the requirement; the requirement is the CARDINALITY (one entry per signing-key period intersecting [StartTime, EndTime], so a window spanning several periods yields several entries), each entry's validity window being that key's own period rather than the request's window, and the timestamps being whole seconds.
Why: digest-file validation walks ListPublicKeys to find the key that was valid at the time each digest was signed; with a single key covering everything a validator either picks the wrong key or cannot discriminate at all, pagination over PublicKeyList is never exercised, and a fractional ValidityEndTime breaks consumers that parse these as second-granularity instants.
Fix: ListPublicKeys' handler/state in the cloudtrail crate must model signing keys as a set of per-period keys and return every key whose validity period overlaps the requested range, emitting each key's own whole-second ValidityStartTime/ValidityEndTime.
Traces: cloudtrail-lookup-events-and-public-keys#task#2.

Count, stated plainly: THREE causes — 1 missing `Message` member on LookupEvents refusals (4 replayed steps), 2 a LookupEvents query AWS answers 200/Events:[] refused with 400, 3 ListPublicKeys serving one key instead of the per-period key list. All three must be closed.

## Evidence
Wire replay of trace set simparity-wire-aws-v1 against the emulator booted from this checkout (emulator_up true, bootstrap_signal false, 179 steps, 152 passed, 10 mismatched, score 84.9%). Three clusters, none carrying stub_path or owned_by:

Cause 1 — divergence_key service=cloudtrail|LookupEvents|status=400->400|keys=$.Message, verdict mismatch, expected_status 400, actual_status 400, affected_count 4:
  detail: "$.Message: missing (expected 'Lookup attribute key cannot be null or empty.', got None)"
  affected: cloudtrail-lookup-events-query-mistakes#task#1, #task#4, #task#5, #task#6
  Expected: 400 with the error body's modelled `Message` member present, carrying that error class's fixed sentence. Actual: 400 with no `Message` at $.Message (None).

Cause 2 — divergence_key service=cloudtrail|LookupEvents|status=200->400|keys=$.Events,status, verdict mismatch, affected_count 1:
  detail: "status: expected 200, got 400" and "$.Events: missing (expected [], got None)"
  affected: cloudtrail-lookup-events-query-mistakes#task#2
  Expected: HTTP 200, body {"Events": []}. Actual: HTTP 400 with no Events member.

Cause 3 — divergence_key service=cloudtrail|ListPublicKeys|status=200->200|keys=$.PublicKeyList[0],$.PublicKeyList[1],$.PublicKeyList[2],$.PublicKeyList[3], verdict mismatch, affected_count 1:
  detail: "$.PublicKeyList[0]: missing (expected {Fingerprint, ValidityEndTime 1706389626.0, ValidityStartTime 1703797626.0, Value ...}, got {Value ..., ValidityStartTime 1702080000, ValidityEndTime 1709855999.999, Fingerprint ...})"; "$.PublicKeyList[1]: missing (expected {... ValidityEndTime 1706389625.0, ValidityStartTime 1703797625.0 ...}, got None)"; "$.PublicKeyList[2]: missing (expected {... ValidityEndTime 1704661644.0, ValidityStartTime 1702069644.0 ...}, got None)"; and the same for $.PublicKeyList[3].
  affected: cloudtrail-lookup-events-and-public-keys#task#2
  Expected: four entries, each with its own whole-second validity window. Actual: one entry whose window spans the whole requested range and whose ValidityEndTime carries a fractional second. The key bytes, fingerprint strings and the specific epoch numbers are per-environment values, not requirements.

Model evidence cited: Smithy LookupEvents errors (InvalidLookupAttributesException / InvalidTimeRangeException / InvalidMaxResultsException ... each: errorKind client, httpError 400, single member `Message` -> ErrorMessage @length 4..1000 @pattern '.*'); LookupEvents output Events -> EventsList, paginated items Events; ListPublicKeys output PublicKeyList -> list of PublicKey{Value blob, ValidityStartTime/ValidityEndTime timestamp Date, Fingerprint String}, paginated over PublicKeyList.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout; point a cloudtrail client at its data listener.
2. Cause 1: replay fixture cloudtrail-lookup-events-query-mistakes, step task#1 (also task#4, task#5, task#6) — a LookupEvents call whose lookup attributes/parameters AWS refuses. Expect HTTP 400 whose JSON error body carries the modelled `Message` member (for the unknown-attribute-key case, exactly "Lookup attribute key cannot be null or empty."); observe 400 with no `Message` key in the body (botocore surfaces Error.Message as empty).
3. Cause 2: replay fixture cloudtrail-lookup-events-query-mistakes, step task#2 — the LookupEvents call the recording shows AWS accepting. Expect HTTP 200 with body {"Events": []}; observe HTTP 400 with no Events member.
4. Cause 3: replay fixture cloudtrail-lookup-events-and-public-keys, step task#2 — ListPublicKeys over the recorded StartTime/EndTime window. Expect one PublicKeyList entry per signing-key period intersecting the window (four in the recording), each with its own whole-second ValidityStartTime/ValidityEndTime; observe a single entry spanning the whole window with a fractional ValidityEndTime and indices 1..3 absent.
5. Re-run replay_wire_corpus and confirm the three clusters above no longer appear (the two cloudtrail GetEventConfiguration/PutEventConfiguration Message clusters are owned by 20261030T134819Z-cloudtrail-never-rejects-an-unknown-trail-lifecycle-and.md and are out of scope here).
