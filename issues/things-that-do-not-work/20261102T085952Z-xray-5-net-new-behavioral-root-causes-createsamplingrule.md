Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: xray.concurrent-create-sampling-rule-duplicate-name xray.filter-expression-operand-type-mismatch-rejected-on-both-surfaces xray.get-service-graph-over-a-6-hour-window-rejected xray.get-trace-summaries-over-a-24-hour-window-rejected xray.segment-origin-types-the-service-so-same-name-ecs-and-ec2-frames-are-distinct-nodes
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray.concurrent-create-sampling-rule-duplicate-name|run#12|AssertionError|assert all((m or "").startswith(prefix) for m in messages), messages
Issue-Key: xray|CreateGroup|InvalidRequestException
Issue-Key: xray.get-service-graph-over-a-6-hour-window-rejected|_error#9|AssertionError|raise AssertionError(f"the call succeeded; real AWS rejects it, got {result!r}")
Issue-Key: xray.get-trace-summaries-over-a-24-hour-window-rejected|_error#10|AssertionError|raise AssertionError(f"the call succeeded; real AWS rejects it, got {result!r}")
Issue-Key: xray.segment-origin-types-the-service-so-same-name-ecs-and-ec2-frames-are-distinct-nodes|run#7|AssertionError|assert types == sorted([origin, "client"]), (
Issue-Commit: fec48c0e4a4ef614c94c9aeb73915e609cba5726

# xray: 5 net-new behavioral root causes — CreateSamplingRule duplicate-name and filter-expression operand-type refusal messages diverge, GetServiceGraph/GetTraceSummaries never enforce their time-range bounds, and TraceSummary.ServiceIds omits the synthesised client caller

> _Note: this issue was identified against the code base at commit `fec48c0e4a4ef614c94c9aeb73915e609cba5726`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTraceSummaries

## Description
FIVE distinct net-new root causes in the X-Ray service. All five failing checks carry SERVICES = ["xray"], the xray service is bootstrapped and answering (130/159 xray checks pass in this run), so these are fidelity defects, not a bootstrap gap. None of the five checks is marked ALREADY FILED or OWNED in the replay report.

RELATED, NOT COVERING: 20261029T190848Z-xray-refusal-messages-diverge-from-aws-on-four.md covers the ValidationException missing-value rendering, the Default sampling-rule delete message, the UpdateSamplingRule priority message, the PutEncryptionConfig missing-KeyId message and the KMS alias/key ARN — it does NOT name the CreateSamplingRule duplicate-name message (Cause 1) nor any filter-expression type message (Cause 2). 20261029T190848Z-xray-gettracesummaries-filter-expression-grammar-rejects-two-documented.md covers the unary annotation[key] bracket predicate and the ! negation operator being rejected outright — it does NOT cover the MESSAGE emitted for a type-mismatched operand that is already (correctly) rejected. 20261022T045628Z-xray-x-ray-account-config-surfaces-are-overly-permissive.md is about account-config surfaces, not trace-query windows. 20261025T043615Z-xray-3-net-new-behavioral-root-causes-getservicegraph.md names GetServiceGraph not projecting a submitted service and GetTraceSummaries not deriving ResponseTimeRootCauses; in this run both of those projections ARE produced (the 6 h graph and the 24 h summaries below show the node, its client peer and a ResponseTimeRootCause), and neither of its causes is the missing window-bound validation (Causes 3/4) or the missing client entry in a summary's ServiceIds (Cause 5).

Cause 1 of 5: CreateSamplingRule's duplicate-RuleName refusal message diverges from AWS.
AWS: Smithy (services/service-com-amazonaws-xray-awsxray/smithy-model/com-amazonaws-xray-awsxray.json) models CreateSamplingRule as POST /CreateSamplingRule (success 200) with errors InvalidRequestException (400), RuleLimitExceededException (400) and ThrottledException (429). Recorded behavior for the duplicate-name race: of N concurrent creates of the identical RuleName exactly one wins and every loser is refused 400 InvalidRequestException with the message `A resource with the same resourceName but a different internalId already exists: <internalId>`. Fixed text: `A resource with the same resourceName but a different internalId already exists: `; the single variable part is the winning rule's internalId, taken from the emulator's own state (no response exposes it, so only the prefix is a requirement).
Emulator: the status (400) and the code (InvalidRequestException) are right and exactly one create wins, but the message is the fixed string `Sampling rule already exists` — it carries neither AWS's fixed text nor the internalId of the resource that already exists.
Why: a client that distinguishes "this name is already taken by another rule" from the other InvalidRequestException causes of CreateSamplingRule (bad priority, bad ARN, bad attribute map) does so on the message, because the code is shared. Against the emulator that branch never matches, so an idempotent create-or-reuse path cannot recognise the collision and the identifier of the colliding resource is lost.
Fix: the CreateSamplingRule handler's duplicate-name guard in the xray crate (services/service-com-amazonaws-xray-awsxray/src/ops/create_sampling_rule.rs) — render AWS's template with the existing rule's internalId.
Traces: xray.concurrent-create-sampling-rule-duplicate-name.

Cause 2 of 5: the filter-expression operand-type-mismatch refusal message diverges from AWS, on both CreateGroup and GetTraceSummaries.
AWS: Smithy models CreateGroup (POST /CreateGroup) and GetTraceSummaries (POST /TraceSummaries) with FilterExpression and error InvalidRequestException (400); the expression grammar and its keyword types are documented at https://docs.aws.amazon.com/xray/latest/devguide/xray-console-filters.html. Recorded behavior: comparing a NUMBER keyword against a quoted operand is refused 400 InvalidRequestException with `You can't compare a non-numeric value of type [<operand data type>] against property [<keyword>] for the <operator> operator.`, and using a numeric comparison operator on a STRING keyword is refused with `You can't compare a non-numeric property [<keyword>] for the <operator> operator.` Fixed text is everything outside the brackets; the variable parts are the operand's data type name, the keyword as written in the expression and the operator symbol, all taken from the submitted expression. Both surfaces must emit the same message for the same expression.
Emulator: refuses with the right status (400) and code (InvalidRequestException) but a different message template — for `responsetime > "abc"` it answers `You can't compare property [responsetime] against the String data type.` instead of `You can't compare a non-numeric value of type [String] against property [responsetime] for the > operator.`; the template drops the `non-numeric value of type` clause and the `for the <operator> operator.` tail and reorders the remaining parts. The second probe (`http.url > 5`, expected `You can't compare a non-numeric property [http.url] for the > operator.`) is never reached because the check stops at the first message mismatch, so that arm must be re-verified after the fix.
Why: the type-mismatch text is the only thing that tells a caller WHICH operand of a rejected expression was mistyped and under which operator; a console, linter or SDK wrapper that surfaces or matches AWS's wording shows an unknown message, and a client cannot tell a type error from a syntax error since both are InvalidRequestException/400.
Fix: the filter-expression validator shared by the CreateGroup and GetTraceSummaries handlers in the xray crate (services/service-com-amazonaws-xray-awsxray/src/ops/create_group.rs and src/ops/get_trace_summaries.rs, or the filter-expression parser they both call) — emit AWS's two type-mismatch templates.
Traces: xray.filter-expression-operand-type-mismatch-rejected-on-both-surfaces.

Cause 3 of 5: GetServiceGraph does not enforce its 6-hour time-range bound — a longer window is answered 200 instead of refused.
AWS: Smithy models GetServiceGraph as POST /ServiceGraph with required StartTime/EndTime and errors InvalidRequestException (400) and ThrottledException (429). Recorded behavior: a window of exactly 6 h is accepted and answered 200; a window one minute longer is refused 400 InvalidRequestException with the message `Time range cannot be longer than 6 hours` — entirely fixed text, no value from the request.
Emulator: the over-long window is ACCEPTED. GetServiceGraph answered HTTP 200 with a full graph payload (echoing StartTime/EndTime 6 h 1 min apart and the Services list containing the frame's root node and its synthesised client peer). No refusal of any kind is emitted.
Why: this is the overly-permissive direction — a client that queries a window production refuses gets a plausible answer from the emulator and a 400 in AWS, so the window-splitting logic every service-map consumer needs (paginating a day of topology into 6 h slices) is never exercised and ships broken.
Fix: the GetServiceGraph handler's input validation in the xray crate (services/service-com-amazonaws-xray-awsxray/src/ops/get_service_graph.rs) — reject EndTime - StartTime > 6 h with InvalidRequestException and AWS's fixed message, before any projection work.
Traces: xray.get-service-graph-over-a-6-hour-window-rejected.

Cause 4 of 5: GetTraceSummaries does not enforce its 24-hour time-range bound — a longer window is answered 200 instead of refused. This is a different bound from Cause 3 and a different handler, so it needs its own fix.
AWS: Smithy models GetTraceSummaries as POST /TraceSummaries with required StartTime/EndTime and errors InvalidRequestException (400) and ThrottledException (429). Recorded behavior: a window of exactly 24 h is accepted and answered 200; a window one minute longer is refused 400 InvalidRequestException with the message `Time range cannot be longer than 24 hours` — entirely fixed text.
Emulator: the 24 h + 1 min window is ACCEPTED, answered HTTP 200 with TraceSummaries, ApproximateTime and TracesProcessedCount for the published frame. No refusal is emitted.
Why: same overly-permissive direction as Cause 3, on the primary trace-query read: callers that build long-range queries see them work against the emulator and fail in AWS, and the two different bounds (6 h for the graph, 24 h for the summaries) — a real asymmetry of the API that clients must encode — are both invisible.
Fix: the GetTraceSummaries handler's input validation in the xray crate (services/service-com-amazonaws-xray-awsxray/src/ops/get_trace_summaries.rs) — reject EndTime - StartTime > 24 h with InvalidRequestException and AWS's fixed message.
Traces: xray.get-trace-summaries-over-a-24-hour-window-rejected.

Cause 5 of 5: a trace summary's ServiceIds omits the synthesised `client` caller of the root segment, so an origin-typed root frame is reported with one ServiceId instead of two.
AWS: Smithy models TraceSummary.ServiceIds -> ServiceIds of ServiceId{Name, Names, AccountId, Type}. Recorded behavior: for a root segment submitted with an `origin` (e.g. `AWS::ECS::Container`, `AWS::EC2::Instance`), GetTraceSummaries reports that trace's ServiceIds with TWO entries named for the service — one typed with the segment's `origin` (the root node) and one typed `client` (the synthesised caller of that root) — which is the same pair GetServiceGraph draws for the frame. The requirement is the pair of Types for a root-named service, not any one run's generated service name.
Emulator: the summary lists only the origin-typed entry. Observed ServiceIds for the ECS frame: `[{'Name': '<service>', 'Names': ['<service>'], 'Type': 'AWS::ECS::Container'}]`, i.e. `['AWS::ECS::Container']` where `['AWS::ECS::Container', 'client']` is required. Note the emulator's GetServiceGraph DOES synthesise the client node for the same submission (its Services list carries a second node of `'Type': 'client'` with an edge to the root), so the projection exists in the graph path and is simply not carried into the per-trace summary. Because the check stops here, its later arms — that `service(id(name: "<svc>", type: "<origin>"))` selects only the frame submitted with that origin, and that two same-name frames with different origins yield two distinct typed root nodes — are unverified and must be re-checked after the fix.
Why: a client reading a trace summary to learn which services a trace touched, and of what type, cannot see the caller edge at all, and cannot distinguish same-named frames published from different host types (ECS vs EC2) — exactly what `origin` exists to express. Anything keying on ServiceIds Type (grouping, filtering by `id(name:, type:)`, console colouring) silently under-reports.
Fix: the trace-summary projection in the GetTraceSummaries handler / the summary-building state of the xray crate (services/service-com-amazonaws-xray-awsxray/src/ops/get_trace_summaries.rs) — derive ServiceIds from the same node set the service-graph projection uses, including the segment's `origin` as the root entry's Type and the synthesised `client` caller entry.
Traces: xray.segment-origin-types-the-service-so-same-name-ecs-and-ec2-frames-are-distinct-nodes.

## Evidence
Cause 1 — xray.concurrent-create-sampling-rule-duplicate-name (verdict mismatch). 5 concurrent CreateSamplingRule calls with one identical RuleName; exactly 1 winner and all 4 loser codes == 'InvalidRequestException' (those assertions passed).
  expected each loser Message to start with: "A resource with the same resourceName but a different internalId already exists: " (suffix = the winning rule's internalId)
  actual: ['Sampling rule already exists', 'Sampling rule already exists', 'Sampling rule already exists', 'Sampling rule already exists']

Cause 2 — xray.filter-expression-operand-type-mismatch-rejected-on-both-surfaces (verdict mismatch). CreateGroup(GroupName=<fresh>, FilterExpression='responsetime > "abc"'); status 400 and code InvalidRequestException were correct.
  expected Message: "You can't compare a non-numeric value of type [String] against property [responsetime] for the > operator."
  actual Message:   "You can't compare property [responsetime] against the String data type."
  not reached (must be re-verified): the same expression on GetTraceSummaries must carry the identical message, and 'http.url > 5' must be refused with "You can't compare a non-numeric property [http.url] for the > operator." on both surfaces; the well-typed control 'responsetime > 5 AND http.url CONTAINS "/probe"' must keep succeeding on both.

Cause 3 — xray.get-service-graph-over-a-6-hour-window-rejected (verdict mismatch). PutTraceSegments accepted the frame (UnprocessedTraceSegments == []) and the 6 h graph did contain the service exactly once; then get_service_graph(StartTime=EndTime-6h-1min, EndTime=now+1min):
  expected: ClientError, code 'InvalidRequestException', Message 'Time range cannot be longer than 6 hours'
  actual:   HTTP 200, {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'StartTime': <end-6h01m>, 'EndTime': <end>, 'Services': [{'ReferenceId': 0, 'Name': '<svc>', 'Root': True, 'State': 'active', 'SummaryStatistics': {...}}, {'ReferenceId': 1, 'Name': '<svc>', 'Root': False, 'Type': 'client', 'State': 'unknown', 'Edges': [...]}]} — the call succeeded (harness: "the call succeeded; real AWS rejects it").

Cause 4 — xray.get-trace-summaries-over-a-24-hour-window-rejected (verdict mismatch). PutTraceSegments accepted the frame and the 24 h window returned exactly it; then get_trace_summaries(StartTime=EndTime-24h-1min, EndTime=now+1min, FilterExpression='service("<svc>")'):
  expected: ClientError, code 'InvalidRequestException', Message 'Time range cannot be longer than 24 hours'
  actual:   HTTP 200 with {'TraceSummaries': [{'Id': '1-<8hex>-<24hex>', 'Duration': 21.0, 'ResponseTime': 21.0, 'ServiceIds': [...], 'ResponseTimeRootCauses': [...], ...}], 'ApproximateTime': <now>, 'TracesProcessedCount': 1} — the call succeeded.

Cause 5 — xray.segment-origin-types-the-service-so-same-name-ecs-and-ec2-frames-are-distinct-nodes (verdict mismatch). Two root segments submitted under one service name, one with origin 'AWS::ECS::Container' and one with 'AWS::EC2::Instance' (both accepted, UnprocessedTraceSegments == []); both summarised under service("<svc>").
  expected for the ECS frame: sorted Types of the ServiceIds entries named for the service == ['AWS::ECS::Container', 'client'] (and ['AWS::EC2::Instance', 'client'] for the EC2 frame)
  actual:   [{'Name': '<svc>', 'Names': ['<svc>'], 'Type': 'AWS::ECS::Container'}] -> ['AWS::ECS::Container'] only; the 'client' entry is absent
  contrast within the same run: GetServiceGraph for an equivalent submission does emit the client peer — 'Services' contains {'ReferenceId': 1, 'Name': '<svc>', 'Root': False, 'Type': 'client', 'State': 'unknown', 'Edges': [{'ReferenceId': 0, ...}]}.

## Steps to Reproduce
Prerequisite: git_clone, then run_behavioral_suite (builds and boots the emulator); bootstrap_signal is false and xray answers (130/159 xray checks pass).

Cause 1:
1. Build a valid sampling rule {RuleName: '<fresh name>', ResourceARN: '*', Priority: <1..9999>, FixedRate: 0.05, ReservoirSize: 1, ServiceName: '*', ServiceType: '*', Host: '*', HTTPMethod: '*', URLPath: '*', Version: 1}.
2. Fire 5 concurrent xray.create_sampling_rule(SamplingRule=<that rule>, Tags=[...]) from a thread pool. Observe exactly one 200 and four 400 InvalidRequestException — then read each loser's Message: it must begin 'A resource with the same resourceName but a different internalId already exists: ' and currently reads 'Sampling rule already exists'.
3. get_sampling_rules() must list the rule exactly once (it does).

Cause 2:
4. xray.create_group(GroupName='<fresh>', FilterExpression='responsetime > 5 AND http.url CONTAINS "/probe"') — accepted, echoed verbatim (control).
5. xray.create_group(GroupName='<fresh2>', FilterExpression='responsetime > "abc"') — observe 400 InvalidRequestException with the wrong Message (above); no group may be created (it is not).
6. xray.get_trace_summaries(StartTime=now-10m, EndTime=now+1m, FilterExpression='responsetime > "abc"', Sampling=False) — must carry the identical AWS message.
7. Repeat 5 and 6 with FilterExpression='http.url > 5' — message must be "You can't compare a non-numeric property [http.url] for the > operator."

Cause 3:
8. xray.put_trace_segments with one 21 s root segment named '<svc>' (subsegments InternalPipeline 2 s and QuartzAPI 19 s); confirm UnprocessedTraceSegments == [].
9. xray.get_service_graph(StartTime=end-6h, EndTime=end) where end = now+1min — accepted, and '<svc>' appears once as a root node.
10. xray.get_service_graph(StartTime=end-6h-1min, EndTime=end) — observe HTTP 200 with a graph instead of 400 InvalidRequestException / 'Time range cannot be longer than 6 hours'.

Cause 4:
11. With the same published frame, xray.get_trace_summaries(StartTime=end-24h, EndTime=end, FilterExpression='service("<svc>")') — accepted and returns the frame.
12. xray.get_trace_summaries(StartTime=end-24h-1min, EndTime=end, FilterExpression='service("<svc>")') — observe HTTP 200 with TraceSummaries instead of 400 InvalidRequestException / 'Time range cannot be longer than 24 hours'.

Cause 5:
13. xray.put_trace_segments with TWO root segments sharing one name '<svc>', different trace ids, one 'origin': 'AWS::ECS::Container' and one 'origin': 'AWS::EC2::Instance', each with an http request/response block and a QuartzAPI subsegment; confirm both accepted.
14. xray.get_trace_summaries(StartTime=now-15m, EndTime=now+1m, FilterExpression='service("<svc>")') and read each summary's ServiceIds — each must hold both the origin-typed entry and a 'client'-typed entry named '<svc>'; currently only the origin-typed entry is present.
15. Then xray.get_trace_summaries(FilterExpression='service(id(name: "<svc>", type: "AWS::ECS::Container"))') must select only the ECS frame (and likewise for the EC2 origin), and a paginated xray.get_service_graph over the same window must show two Root nodes named '<svc>', one per origin type.

16. Re-replay the five check ids in trace_ids to confirm the fixes.
