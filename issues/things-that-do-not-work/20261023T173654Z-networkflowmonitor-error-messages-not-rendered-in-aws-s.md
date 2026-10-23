Issue-Source: trace-sweep-divergence
Issue-Trace: networkflowmonitor-error-paths
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: networkflowmonitor-error-paths networkflowmonitor-tag-and-untag-resource-round-trip
Issue-Shape-Id: com.amazonaws.networkflowmonitor#NetworkFlowMonitor
Issue-Key: networkflowmonitor|GetMonitor|4xx->4xx|$.message
Issue-Key: networkflowmonitor|DeleteMonitor|4xx->4xx|$.message
Issue-Key: networkflowmonitor|ListMonitors|4xx->4xx|$.message
Issue-Related: 20261023T173654Z-networkflowmonitor-wire-status-and-error-code-mapping-wrong.md
Issue-Commit: a91eb31cf888327ed8ad916c03309e3893f84f70

# NetworkFlowMonitor error messages not rendered in AWS's known forms (ARN-qualified not-found, sorted enum value set)

> _Note: this issue was identified against the code base at commit `a91eb31cf888327ed8ad916c03309e3893f84f70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMonitor

## Description
Four distinct root causes in service networkflowmonitor, replayed from the wire corpus simparity-wire-aws-v1. In ALL FOUR the HTTP status and the error code already match AWS; what diverges is the error MESSAGE, and in each case the rule that produces AWS's message is known — it is built from the request (the resource ARN, or the offending member/value) or from the model (the enum's value set), so the message form is a requirement and not merely one recording's wording. None of these were marked stub_path or owned_by in the replay report. Model citations are from services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/smithy-model/com-amazonaws-networkflowmonitor-networkflowmonitor.json.

Related open issues (named, not covering): 20261022T213607Z-networkflowmonitor-control-plane-updatescope-answers-501-updatemonitor-skips.md describes the SAME monitor not-found message rule as its behavioral cause 3, and 20261023T132415Z-networkflowmonitor-query-monitor-plane-stopquery-rejects-terminal-queries.md describes the query-id form as its cause 4. Both are behavioral-corpus issues and carry NO wire replay key (no Issue-Trace/Issue-Step), and the replay reported owned_by=[] for every step below, so the wire divergences here are unowned. Causes 1-3 extend that same renderer defect to the scope reader, which neither issue mentions.

=== Cause 1 of 4: GetMonitor's not-found message is not ARN-qualified in AWS's form ===
AWS: Smithy: GetMonitor is GET /monitors/{monitorName} and models ResourceNotFoundException with httpError 404 ('The request specifies a resource that doesn't exist.'). Recorded behavior: the message is BUILT FROM THE REQUEST and names the monitor by its FULL ARN in square brackets with the suffix "doesn't exist" — Monitor [arn:aws:networkflowmonitor:<region>:<account>:monitor/<monitorName>] doesn't exist. The rule is therefore known: fixed prefix 'Monitor [', the request-derived monitor ARN, then '] doesn't exist'.
Emulator: status and code are correct (404 / ResourceNotFoundException); the message is the emulator's generic bare-name form, e.g. 'monitor <monitorName> was not found' — no ARN, no AWS phrasing.
Why: a client or tool that parses the ARN out of a not-found message to identify WHICH resource of a batch was missing (region, account and resource type all come from the ARN) gets nothing usable, and log/alarm rules matching AWS's text do not fire.
Emulator component to fix: the not-found error construction in GetMonitor's handler (src/ops/get_monitor.rs), or the shared monitor not-found helper it calls — render the monitor ARN and AWS's message form.
Traces: networkflowmonitor-error-paths#task#1.

=== Cause 2 of 4: DeleteMonitor's not-found message has the same defect ===
AWS: Smithy: DeleteMonitor is DELETE /monitors/{monitorName}, @idempotent, and models ResourceNotFoundException (404). Recorded behavior: identical rule to cause 1 — Monitor [<full monitor ARN>] doesn't exist.
Emulator: 404 / ResourceNotFoundException with 'monitor <monitorName> was not found'. Identical to cause 1, which confirms the defect is in the shared monitor not-found renderer rather than in one handler's text.
Why: as cause 1 — the ARN a caller needs to identify the missing monitor is absent from the delete path too, so idempotent delete-then-verify flows log an unidentifiable resource.
Emulator component to fix: the not-found error construction in DeleteMonitor's handler (src/ops/delete_monitor.rs) / the shared helper.
Traces: networkflowmonitor-error-paths#task#2.

=== Cause 3 of 4: GetScope's not-found message is not ARN-qualified in AWS's form ===
AWS: Smithy: GetScope is GET /scopes/{scopeId} and models ResourceNotFoundException (404). Recorded behavior: the message is built from the request and names the scope's FULL ARN after a fixed prefix — There's no scope with ARN: [arn:aws:networkflowmonitor:<region>:<account>:scope/<scopeId>]. Note this is a DIFFERENT fixed prefix from the monitor form in causes 1-2, so the renderer must be per-resource-type, not one generic template.
Emulator: 404 / ResourceNotFoundException with 'scope <scopeId> was not found'.
Why: as above, plus the scope reader is the operation a client polls after DeleteScope, so the message a teardown flow logs never identifies the scope by ARN.
Emulator component to fix: the not-found error construction in GetScope's handler (src/ops/get_scope.rs) — render the scope ARN in AWS's prefix form. (Note the companion HIGH issue on this service also changes GetScope's INVALID-identifier path to a 400 ValidationException; this cause is only about the genuine not-found path, which stays 404.)
Traces: networkflowmonitor-tag-and-untag-resource-round-trip#teardown#9.

=== Cause 4 of 4: ListMonitors' enum validation message lists the enum value set in model-declaration order instead of sorted order ===
AWS: Smithy: ListMonitors is GET /monitors with monitorStatus bound to the monitorStatus query string, targeting enum MonitorStatus, and models ValidationException (400). The enum's values are declared in the model in the order PENDING, ACTIVE, INACTIVE, ERROR, DELETING. Recorded behavior: an unmodeled value is refused with AWS's standard constraint-validation message — "1 validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy enum value set: [ACTIVE, DELETING, ERROR, INACTIVE, PENDING]" — i.e. the accepted values are rendered SORTED, not in declaration order. The rule is known: it is a validation error naming the member, the rejected value and the constraint's value set, rendered in sorted order.
Emulator: status and code are correct (400 / ValidationException) and the message is otherwise byte-identical, but the value set is emitted in the model's declaration order: [PENDING, ACTIVE, INACTIVE, ERROR, DELETING].
Why: this message is the machine-readable part of a constraint failure — AWS's ordering is stable and sorted, so a client (or a golden-output test, or a docs generator) that compares the advertised value set against AWS's gets a diff on every enum rejection even though the set itself is identical. Lowest severity of the bundle.
Emulator component to fix: the shared enum-constraint validation message renderer used by ListMonitors' input validation (src/ops/list_monitors.rs and the common validation/error formatting module) — sort the value set before formatting it.
Traces: networkflowmonitor-error-paths#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1: 14 steps, 6 passed, 8 mismatched (score 42.9%); emulator_up=true, bootstrap_signal=false, state_reset_failures=0. In every cause below the replay flagged ONLY $.message (status and x-amzn-errortype matched), so the status/code contract is already right and the message rule is the divergence.

Cause 1 — fixture networkflowmonitor-error-paths, step task#1, operation GetMonitor
  divergence_key: service=networkflowmonitor|GetMonitor|status=404->404|keys=$.message
  expected $.message: "Monitor [arn:aws:networkflowmonitor:us-west-2:000000000000:monitor/sim2real-probe-networkflowmonitor-missing-monitor-d303928e] doesn't exist" (the recorded value additionally carries the SDK-appended 'Service: NetworkFlowMonitor, Status Code: 404, Request ID: ... (SDK Attempt Count: 1)' decoration, which is client-side and NOT a service requirement)
  actual   $.message: 'monitor sim2real-probe-networkflowmonitor-missing-monitor-d303928e was not found'

Cause 2 — fixture networkflowmonitor-error-paths, step task#2, operation DeleteMonitor
  divergence_key: service=networkflowmonitor|DeleteMonitor|status=404->404|keys=$.message
  expected $.message form: "Monitor [<monitor ARN>] doesn't exist"
  actual   $.message: 'monitor sim2real-probe-networkflowmonitor-missing-monitor-d303928e was not found'

Cause 3 — fixture networkflowmonitor-tag-and-untag-resource-round-trip, step teardown#9, operation GetScope
  divergence_key: service=networkflowmonitor|GetScope|status=404->404|keys=$.message
  expected $.message: "There's no scope with ARN: [arn:aws:networkflowmonitor:us-west-2:000000000000:scope/00000000-0000-4000-8000-000000000001]"
  actual   $.message: 'scope 00000000-0000-4000-8000-000000000001 was not found'

Cause 4 — fixture networkflowmonitor-error-paths, step task#5, operation ListMonitors
  divergence_key: service=networkflowmonitor|ListMonitors|status=400->400|keys=$.message
  expected $.message: "1 validation error detected: Value 'NOT_A_REAL_STATUS' at 'monitorStatus' failed to satisfy constraint: Member must satisfy enum value set: [ACTIVE, DELETING, ERROR, INACTIVE, PENDING]"
  actual   $.message: "1 validation error detected: Value 'NOT_A_REAL_STATUS' at 'monitorStatus' failed to satisfy constraint: Member must satisfy enum value set: [PENDING, ACTIVE, INACTIVE, ERROR, DELETING]"
  The only difference is the ordering of the value set; the emulator emits MonitorStatus in model-declaration order, AWS emits it sorted.

## Steps to Reproduce
Boot the emulator and point a boto3 `networkflowmonitor` client at it; take the account id from STS GetCallerIdentity.

1. Cause 1: call get_monitor(monitorName="<a name never created>") and read the error. Observe 404 ResourceNotFoundException with message 'monitor <name> was not found' instead of AWS's "Monitor [arn:aws:networkflowmonitor:<region>:<account>:monitor/<name>] doesn't exist".
2. Cause 2: call delete_monitor(monitorName="<a name never created>") — same 404 ResourceNotFoundException and the same bare-name message, where AWS renders the monitor ARN form.
3. Cause 3: call get_scope(scopeId="00000000-0000-4000-8000-000000000001") — a well-formed scope id that does not exist (e.g. after CreateScope/DeleteScope, or simply never created). Observe 404 ResourceNotFoundException with 'scope <scopeId> was not found' instead of AWS's "There's no scope with ARN: [arn:aws:networkflowmonitor:<region>:<account>:scope/<scopeId>]".
4. Cause 4: call list_monitors(monitorStatus="NOT_A_REAL_STATUS") — observe 400 ValidationException whose message ends 'Member must satisfy enum value set: [PENDING, ACTIVE, INACTIVE, ERROR, DELETING]' instead of AWS's sorted '[ACTIVE, DELETING, ERROR, INACTIVE, PENDING]'.
5. Re-replay to confirm the fix: fixture networkflowmonitor-error-paths steps task#1, task#2 and task#5, and fixture networkflowmonitor-tag-and-untag-resource-round-trip step teardown#9.
