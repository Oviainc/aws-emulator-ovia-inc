Issue-Source: trace-sweep-divergence
Issue-Trace: accessanalyzer-policy-check-apis
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: accessanalyzer-policy-check-apis accessanalyzer-policy-generation-workflow
Issue-Shape-Id: com.amazonaws.accessanalyzer#AccessAnalyzer
Issue-Key: access-analyzer|ValidatePolicy|2xx->2xx|$.findings[*].findingDetails
Issue-Key: access-analyzer|CancelPolicyGeneration|4xx->4xx|$.message
Issue-Key: access-analyzer|GetGeneratedPolicy|4xx->4xx|$.message
Issue-Key: access-analyzer|ListPolicyGenerations|4xx->4xx|$.message
Issue-Commit: 600bb9247975905af81e2aa2cda8a015f9396423

# Access Analyzer policy-validation and policy-generation messages are emulator-authored instead of AWS's fixed wording

> _Note: this issue was identified against the code base at commit `600bb9247975905af81e2aa2cda8a015f9396423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ValidatePolicy

## Description
Four net-new wire divergences in the access-analyzer emulator, all one fault: the HTTP status and the error code are right, but the human-readable text the service puts on the wire is prose the emulator authored itself (or prose borrowed from its JSON parser / its generic constraint validator) rather than the fixed per-error-class text Access Analyzer serves. Clients and the SDK surface that text verbatim, and policy-validation findingDetails is a *success*-path response member, so it is part of the normal response contract, not just a diagnostic.

Cause 1 of 4: ValidatePolicy finding text for a malformed policy document.
AWS: Smithy: ValidatePolicy POST /policy/validation, 200, output findings: ValidatePolicyFindingList (required); member findingDetails is modeled as plain text, so the wording itself is recorded behavior only. The recording shows that a policyDocument that is not parseable JSON is answered 200 (not a ValidationException) with a finding whose findingDetails is Access Analyzer's fixed syntax-error sentence: the fixed text is "Fix the JSON syntax error at index <index> line <line> column <column>." and the only variable parts are the index, line and column of the parse failure taken from the submitted document.
Emulator: answers 200 with findingDetails "The policy document is not valid JSON: key must be a string at line 1 column 2. Fix the JSON syntax so that the policy can be parsed." — i.e. the underlying Rust JSON parser's own diagnostic plus an emulator-written sentence. None of that text is AWS's; the position is also reported in the parser's own coordinates rather than as the index/line/column triple AWS reports.
Why: a client that parses or asserts on findingDetails (policy linting in CI, a console-style display, a test that matches AWS's syntax-error sentence) gets text that never appears in real Access Analyzer, and the leaked parser wording changes shape with the parser version.
Fix: the ValidatePolicy handler's finding construction for the unparseable-document case (ops/validate_policy.rs in services/service-com-amazonaws-accessanalyzer-accessanalyzer) must emit AWS's fixed sentence with the index/line/column filled from the parse position, instead of forwarding the parser error string.
Traces: accessanalyzer-policy-check-apis#task#2 ($.findings[0].findingDetails, 200 -> 200).

Cause 2 of 4: CancelPolicyGeneration refusal message for an unknown jobId.
AWS: Smithy: CancelPolicyGeneration PUT /policy/generation/{jobId}, jobId @httpLabel required, errors ValidationException(400) | AccessDeniedException(403) | ThrottlingException(429) | InternalServerException(500). There is no NotFound error on this operation, so a well-formed jobId that names no policy-generation job is refused as a ValidationException. Per the recording the message's fixed text is "Invalid jobId: " ... " does not exist", with the submitted jobId as its only variable part.
Emulator: same 400 and same ValidationException, but message "jobId must name a policy generation job, but was <jobId>" — emulator-authored wording.
Why: callers that distinguish "job gone/expired" from other validation failures must match on the message (no distinct error code exists), so emulator prose makes that branch unreachable and breaks retry/cleanup logic.
Fix: the CancelPolicyGeneration handler's job-lookup refusal in the access-analyzer ops module must build the ValidationException message from AWS's template.
Traces: accessanalyzer-policy-generation-workflow#task#3 ($.message, 400 -> 400).

Cause 3 of 4: GetGeneratedPolicy refusal message for an unknown jobId — the same template, on the read side.
AWS: Smithy: GetGeneratedPolicy GET /policy/generation/{jobId}, jobId @httpLabel required, same error set with no NotFound member, so an unknown job is a ValidationException(400) with the identical fixed text "Invalid jobId: <jobId> does not exist".
Emulator: 400 ValidationException with message "jobId must name a policy generation job, but was <jobId>".
Why: as in cause 2 — a client polling a generation job cannot recognise the terminal "no such job" condition from AWS's message.
Fix: same shared refusal helper as cause 2, applied to the GetGeneratedPolicy handler, so both operations render one AWS-worded template.
Traces: accessanalyzer-policy-generation-workflow#task#4 ($.message, 400 -> 400).

Cause 4 of 4: ListPolicyGenerations serves the generic constraint-violation text instead of Access Analyzer's principalArn message.
AWS: Smithy: ListPolicyGenerations GET /policy/generation, principalArn @httpQuery("principalArn") with @pattern ^arn:[^:]*:iam::[^:]*:(role|user)/.{1,576}$, errors include ValidationException(400). The recording shows Access Analyzer refuses a principalArn that fails that pattern with its own short message: fixed text "Invalid principalArn: " followed by the rejected value from the query string — it does not emit the SDK-style multi-error constraint report.
Emulator: 400 ValidationException whose message is the generic framework text "1 validation error detected: Value '<value>' at 'principalArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:[^:]*:iam::[^:]*:(role|user)/.{1,576}$" — the modeled pattern echoed back as prose.
Why: the message is the only place the offending parameter is identified, so a client that surfaces or matches it shows a message real Access Analyzer never returns; it also leaks the internal regex into a user-visible string.
Fix: the ListPolicyGenerations input validation for principalArn must raise the service's own ValidationException message template rather than falling through to the crate's generic pattern-constraint formatter.
Traces: accessanalyzer-policy-generation-workflow#task#6 ($.message, 400 -> 400).

No open access-analyzer issue covers these: the only open one for this service is "ListFindings is a 501 scaffold stub" (20261023T110741Z-accessanalyzer-iam-listfindings-is-a-501-scaffold-stub-so-no.md), which is a different operation; the earlier message-wording issues for this service are all in the resolved folder and do not cover these four steps.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200): 4 divergence clusters, all service=access-analyzer, all verdict=mismatch with expected status == actual status and the divergence confined to one text member.

1) accessanalyzer-policy-check-apis#task#2 — ValidatePolicy, 200 -> 200.
   $.findings[0].findingDetails
   expected: 'Fix the JSON syntax error at index 1 line 1 column 1.'
   actual:   'The policy document is not valid JSON: key must be a string at line 1 column 2. Fix the JSON syntax so that the policy can be parsed.'
   Fixed text in the expectation: "Fix the JSON syntax error at index <index> line <line> column <column>."; variable parts: index/line/column of the parse failure in the submitted policyDocument (the recorded 1/1/1 is this run's values, not part of the requirement).

2) accessanalyzer-policy-generation-workflow#task#3 — CancelPolicyGeneration, 400 -> 400.
   $.message
   expected: 'Invalid jobId: acef2041-5fb4-4bdd-a201-aee420aa09d0 does not exist'
   actual:   'jobId must name a policy generation job, but was acef2041-5fb4-4bdd-a201-aee420aa09d0'
   Fixed text: "Invalid jobId: " + " does not exist"; variable part: the jobId from the request URI (the UUID shown is this run's value).

3) accessanalyzer-policy-generation-workflow#task#4 — GetGeneratedPolicy, 400 -> 400.
   $.message
   expected: 'Invalid jobId: c65835ff-6499-4784-a219-f7a09b74695d does not exist'
   actual:   'jobId must name a policy generation job, but was c65835ff-6499-4784-a219-f7a09b74695d'
   Same template and same variable part as (2).

4) accessanalyzer-policy-generation-workflow#task#6 — ListPolicyGenerations, 400 -> 400.
   $.message
   expected: 'Invalid principalArn: not-an-arn'
   actual:   "1 validation error detected: Value 'not-an-arn' at 'principalArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:[^:]*:iam::[^:]*:(role|user)/.{1,576}$"
   Fixed text: "Invalid principalArn: "; variable part: the rejected principalArn query value.

Status codes and error codes agree in all four; only the message/detail text diverges. Model citations: ValidatePolicy (POST /policy/validation, 200, findings required), CancelPolicyGeneration (PUT /policy/generation/{jobId}), GetGeneratedPolicy (GET /policy/generation/{jobId}), ListPolicyGenerations (GET /policy/generation, principalArn @pattern ^arn:[^:]*:iam::[^:]*:(role|user)/.{1,576}$) — each with errors ValidationException(400), AccessDeniedException(403), ThrottlingException(429), InternalServerException(500) and no NotFound member.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; confirm /__admin/health answers 200.
2. Replay fixture accessanalyzer-policy-check-apis, step task#2: POST /policy/validation with a policyDocument that is not parseable JSON and a valid policyType. Response is 200; read $.findings[0].findingDetails and compare with AWS's fixed sentence 'Fix the JSON syntax error at index <index> line <line> column <column>.' — the emulator returns its JSON parser's diagnostic plus its own sentence.
3. Replay fixture accessanalyzer-policy-generation-workflow, step task#3: PUT /policy/generation/{jobId} with a well-formed UUID jobId for which no policy-generation job exists. Response is 400 ValidationException; $.message is 'jobId must name a policy generation job, but was <jobId>' instead of 'Invalid jobId: <jobId> does not exist'.
4. Same fixture, step task#4: GET /policy/generation/{jobId} with an unknown well-formed jobId. Response is 400 ValidationException with the same emulator-authored message instead of 'Invalid jobId: <jobId> does not exist'.
5. Same fixture, step task#6: GET /policy/generation?principalArn=not-an-arn (a value failing the modeled principalArn pattern). Response is 400 ValidationException whose $.message is the generic '1 validation error detected: ... Member must satisfy regular expression pattern: ...' report instead of 'Invalid principalArn: not-an-arn'.
6. Re-replay all four steps after the fix; each must match on status, error code and message/detail text.
