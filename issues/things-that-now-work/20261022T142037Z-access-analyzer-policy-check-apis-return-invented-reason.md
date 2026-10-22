Issue-Source: trace-sweep-divergence
Issue-Trace: accessanalyzer-policy-check-apis
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: accessanalyzer-policy-check-apis
Issue-Shape-Id: com.amazonaws.accessanalyzer#AccessAnalyzer
Issue-Key: access-analyzer|CheckAccessNotGranted|2xx->2xx|$.message
Issue-Key: access-analyzer|CheckAccessNotGranted|2xx->2xx|$.reasons[*].description
Issue-Key: access-analyzer|CheckNoPublicAccess|2xx->2xx|$.reasons[*].description
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# Access Analyzer policy-check APIs return invented reason prose instead of the canonical check message and reason description

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CheckAccessNotGranted

## Description
The two policy-check operations return the right HTTP status (200) and the right `result`, but populate the modeled body members `message` and `reasons[].description` with free-form explanatory prose the emulator composes itself, rather than the fixed, short, statement-index-keyed strings AWS emits. These are RESPONSE DATA members of a successful call, not error messages, so their content is part of the wire contract clients read and render. One shared fault: the check evaluator writes its own narrative instead of the service's canonical reason summary. 2 causes.

Cause 1 of 2: CheckAccessNotGranted (fixture accessanalyzer-policy-check-apis, step task#3).
AWS: Smithy: CheckAccessNotGranted http {POST /policy/check-access-not-granted}, code 200; output members result (CheckAccessNotGrantedResult), message (String, "The message indicating whether the specified access is allowed"), reasons (ReasonSummaryList of ReasonSummary{description, statementIndex, statementId}, "A description of the reasoning of the result"). The model does not fix the wording, so: recorded behavior only - for a FAIL the recording shows a message naming only the listed actions ('The policy document grants access to perform one or more of the listed actions.') and a reason description built from the request as a statement-index reference ('One or more of the listed actions in the statement with index: 0.').
Emulator: emits a different message ('The policy document grants access to perform one or more of the listed actions or resources.') and a reason description that is a generated per-statement narrative naming the concrete action instead of the statement index ('This statement may grant access to perform the action s3:GetObject on any resource.').
Why: a client that surfaces or matches on the check's message/reason text (policy-review tooling, CI gates on CheckAccessNotGranted output, anything correlating a reason to a statement by the index the description carries) gets text in a shape AWS never produces: the statement-index reference it expects is absent, so reason-to-statement correlation and any message comparison fail against the emulator while succeeding against AWS.
Fix: the CheckAccessNotGranted handler's result/reason construction (the check evaluator that fills `message` and `reasons[].description`); emit the canonical short forms keyed on statementIndex rather than generated prose.
Traces: accessanalyzer-policy-check-apis#task#3.

Cause 2 of 2: CheckNoPublicAccess (fixture accessanalyzer-policy-check-apis, step task#5).
AWS: Smithy: CheckNoPublicAccess http {POST /policy/check-no-public-access}, code 200; output members result, message, reasons (same ReasonSummary shape). Recorded behavior only - for a FAIL the recording shows reasons[0].description as the statement-index form 'Public access granted in the following statement with index: 0.'.
Emulator: emits a long generated explanation of why the statement is public ('The statement allows every principal ("Principal": "*") and carries no condition that scopes access ... so it can grant public access to the resource.') in the same member. Status and result match.
Why: same as cause 1 - the reason entry no longer references the offending statement in the form AWS uses, so clients that map reasons back to policy statements or display AWS's wording diverge.
Fix: same component for this operation - the CheckNoPublicAccess handler's reason-summary construction.
Traces: accessanalyzer-policy-check-apis#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1:
- accessanalyzer-policy-check-apis#task#3 CheckAccessNotGranted, status 200 == 200, body mismatch: $.message value_mismatch (expected 'The policy document grants access to perform one or more of the listed actions.', got 'The policy document grants access to perform one or more of the listed actions or resources.'); $.reasons[0].description value_mismatch (expected 'One or more of the listed actions in the statement with index: 0.', got 'This statement may grant access to perform the action s3:GetObject on any resource.'). divergence_key service=access-analyzer|CheckAccessNotGranted|status=200->200|keys=$.message,$.reasons[0].description.
- accessanalyzer-policy-check-apis#task#5 CheckNoPublicAccess, status 200 == 200, body mismatch: $.reasons[0].description value_mismatch (expected 'Public access granted in the following statement with index: 0.', got 'The statement allows every principal ("Principal": "*") and carries no condition that scopes access to a known principal, account, organization, calling service or network path, so it can grant public access to the resource.'). divergence_key service=access-analyzer|CheckNoPublicAccess|status=200->200|keys=$.reasons[0].description.
Both clusters carry an empty stub_path and no owning issue. These are 200-response body members, not error messages.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture accessanalyzer-policy-check-apis.
2. Step task#3 POSTs /policy/check-access-not-granted with a policy that grants a listed action; the response is 200 with the expected `result`, but `message` and `reasons[0].description` differ from the recording as quoted above.
3. Step task#5 POSTs /policy/check-no-public-access with a resource policy whose statement allows Principal "*"; the response is 200 with the expected `result`, but `reasons[0].description` is generated prose instead of the recorded statement-index form.
4. Both steps pass once the handlers emit the canonical message and statement-index-keyed reason descriptions.
