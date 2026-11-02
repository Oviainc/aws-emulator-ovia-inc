Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__sqs.events-pattern-language-nested-object-pattern-matches-the-leaf-path
Issue-Key: events__sqs.events-pattern-language-nested-object-pattern-matches-the-leaf-path|_drain_all#1|Timeout|with ThreadPoolExecutor(max_workers=max(1, len(queues))) as pool:
Issue-Commit: 9010f89773c9fbf121fd2271b9167862206a1831

# events__sqs: EventBridge nested-object event patterns never match — a rule selecting on a nested detail path delivers nothing to its SQS target and the check hangs to the 60s cap

> _Note: this issue was identified against the code base at commit `9010f89773c9fbf121fd2271b9167862206a1831`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One root cause. NOTE: this is the nested-object sibling of the already-open issue 20261102T021130Z-events-sqs-eventbridge-array-element-pattern-matching-is.md (array-element matching) and of 20261029T182228Z-events-sqs-eventbridge-puttargets-accepts-conflicting-invalid-input-forms-the.md (inert SQS delivery). Neither carries this check's replay key — the replay reports it as unowned — so the cause is filed here. Both named issues are OPEN, so nothing is being reopened.

Cause 1 of 1: a rule whose event pattern selects on a NESTED object path inside `detail` never delivers the matching events to its SQS target, and neither does the source-only control rule on the same custom bus, so EventBridge's path-flattening matching semantics cannot be exercised at all.

AWS: an EventBridge event pattern is matched against the event by PATH, not by leaf-key search. A pattern {"detail":{"o":{"p":{"q":["v"]}}}} matches an event whose detail is {"o":{"p":{"q":"v"}}} and must NOT match {"o":{"q":"v"}} (a skipped level) or {"q":"v"} (the leaf at top level). AWS flattens the event the same way in two further respects: an object inside an array is traversed, so the same deep pattern matches detail {"o":[{"p":{"q":"v"}}]}; and a literal member name containing a dot is indistinguishable from the path it spells, so a pattern {"detail":{"o":{"p":["v"]}}} matches both detail {"o":{"p":"v"}} and detail {"o.p":"v"}. A value that differs from the pattern's ({"q":"w"} for pattern value ["v"]) matches nothing. The verdict TestEventPattern returns and the verdict live delivery produces are two surfaces of ONE matcher and must agree. Smithy (services/service-com-amazonaws-eventbridge-awsevents/smithy-model/com-amazonaws-eventbridge-awsevents.json): TestEventPattern input EventPattern (required, @length min 0 max 4096) + Event (required), output Result:Boolean, errors InvalidEventPatternException (client, HTTP 400) / InternalException (server, HTTP 500); PutRule input EventPattern (@length 0..4096) with InvalidEventPatternException (400); PutEvents input Entries (required, @length 1..10) output FailedEntryCount/Entries. The matching semantics themselves are AWS-documented event-pattern behavior and are not expressed in the model, so for the match table this is recorded behavior only (the check states the verdicts were probed with TestEventPattern and confirmed by live delivery against real AWS before authoring).

Emulator: the check ends "Error: Check timed out after 60.0s". Every control-plane call on the path succeeds — CreateEventBus, PutRule (two pattern rules plus a source-only control rule), SetQueueAttributes with an events.amazonaws.com sqs:SendMessage Allow scoped by ArnEquals aws:SourceArn to the rule ARN, PutTargets (the check asserts FailedEntryCount == 0) and PutEvents (no entry carries ErrorCode) — but across up to 16 PutEvents rounds, each followed by a 5-second drain of every queue, no queue ever reached its per-rule warm barrier, so the harness never advanced to the confirmed round and was cut off at the cap. Nothing is delivered — not even to the control rule, whose pattern constrains only source and detail-type.

Why: a client that routes on a nested detail path — the normal shape for any structured event detail (object members under `detail.o.p`, objects carried inside arrays, flattened dotted keys) — receives no events at all and gets no error anywhere: PutEvents reports success, the rule and target exist, the queue stays empty. The caller also cannot fall back to TestEventPattern to predict routing, because the check never reached the point of comparing the two surfaces, so the emulator's TEP verdicts for nested paths are unverified. An emulator that instead matched a leaf key at ANY depth would be wrong in the opposite direction (it would deliver the "skipped-level" and "top-level" events), which is exactly what the absence legs of this check exist to catch.

Fix: the EventBridge rule-matching engine's object-path handling (full-path matching, no leaf-key search at the wrong depth, traversal into objects nested in arrays, a dotted member name treated as the path it spells) and the SQS target invoker that delivers a matched event to a queue on a custom bus; TestEventPattern must return the same verdict as live delivery for the same pattern and event.

Traces: events__sqs.events-pattern-language-nested-object-pattern-matches-the-leaf-path

## Evidence
Failing check: events__sqs.events-pattern-language-nested-object-pattern-matches-the-leaf-path — verdict [mismatch], "Error: Check timed out after 60.0s".

Expected (the verdicts real AWS returns, probed with TestEventPattern and confirmed by live delivery when the check was authored). Rules, each additionally scoped to the run's own source and detail-type:
  rule "deep" ({"detail":{"o":{"p":{"q":["v"]}}}}) -> delivers {deep, deep-in-array}
  rule "mid"  ({"detail":{"o":{"p":["v"]}}})       -> delivers {mid, dotted-key}
  control rule (source + detail-type only)           -> delivers every case
against the detail cases: deep {"o":{"p":{"q":"v"}}}, deep-wrong-value {"o":{"p":{"q":"w"}}}, skipped-level {"o":{"q":"v"}}, top-level {"q":"v"}, deep-in-array {"o":[{"p":{"q":"v"}}]}, mid {"o":{"p":"v"}}, dotted-key {"o.p":"v"}. TestEventPattern must return exactly those verdicts for the same patterns and events.

Actual: no queue — including the source-only control queue — ever received the cases of a single PutEvents round, so the differential barrier never warmed and the run was cut at the 60-second cap. The assertions that discriminate the matcher were never reached: "a leaf at the wrong depth matched the nested pattern: ...", "path flattening: mid=... deep=...", "TestEventPattern disagreed with the verdicts real AWS returns ...", "TestEventPattern and live delivery disagree -- two surfaces of one matcher: ...".

All control-plane calls on the path succeeded: CreateEventBus, PutRule x3, SetQueueAttributes (queue policy allowing events.amazonaws.com sqs:SendMessage, ArnEquals aws:SourceArn = rule ARN), PutTargets (asserted FailedEntryCount == 0) and PutEvents (no entry carried ErrorCode).

## Steps to Reproduce
1. CreateEventBus with a fresh custom bus name.
2. CreateQueue three times; for each queue SetQueueAttributes with a Policy allowing Principal {"Service":"events.amazonaws.com"}, Action sqs:SendMessage on the queue ARN, Condition ArnEquals aws:SourceArn = the rule ARN created in step 3.
3. PutRule on that bus three times, each scoped to one source S and detail-type DT: (a) control, pattern {"source":[S],"detail-type":[DT]}; (b) "deep", {"source":[S],"detail-type":[DT],"detail":{"o":{"p":{"q":["v"]}}}}; (c) "mid", {"source":[S],"detail-type":[DT],"detail":{"o":{"p":["v"]}}}. PutTargets each rule with its queue (FailedEntryCount comes back 0).
4. TestEventPattern each pattern against each of the seven events in Evidence and compare with the AWS verdicts listed there.
5. PutEvents one batch on that bus carrying all seven cases (every entry is accepted).
6. ReceiveMessage on each queue. Observed: every queue stays empty, including the control queue, so the run burns its 16 warm rounds and ends "Check timed out after 60.0s". Expected: the control queue receives all seven cases, the "deep" queue receives exactly {deep, deep-in-array}, the "mid" queue receives exactly {mid, dotted-key}, and TestEventPattern agrees with that delivery for every pattern/event pair.
