Issue-Source: trace-sweep-divergence
Issue-Trace: resiliencehub-app-lifecycle
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resiliencehub-app-lifecycle resiliencehub-app-version-draft-composition resiliencehub-resiliency-policy-lifecycle
Issue-Shape-Id: com.amazonaws.resiliencehub#AwsResilienceHub
Issue-Key: resiliencehub|CreateApp|4xx->4xx|$.message
Issue-Key: resiliencehub|DescribeApp|4xx->4xx|$.message
Issue-Key: resiliencehub|DescribeResiliencyPolicy|4xx->4xx|$.message
Issue-Key: resiliencehub|DeleteResiliencyPolicy|4xx->4xx|$.message
Issue-Related: 20261029T195027Z-resiliencehub-accepts-invalid-app-version-and-policy-input.md
Issue-Commit: 5b71589754f95f05f88c1a0ee9ccd691bbc5f701

# resiliencehub refusal messages diverge from AWS wording across app, resource and policy errors

> _Note: this issue was identified against the code base at commit `5b71589754f95f05f88c1a0ee9ccd691bbc5f701`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApp

## Description
Seven causes sharing one root fault: resiliencehub returns the correct HTTP status and error type but its own paraphrased `message` text instead of AWS's. The message is a requirement of each error class, so each of these is a client-visible divergence. The open issue 'resiliencehub: PublishAppVersion mints numbered versions instead of the "release" alias, and both ConflictException refusal messages diverge from AWS wording' covers the PublishAppVersion empty-draft text and the DeleteResiliencyPolicy IN-USE (409) text; none of the seven causes below is one of those.

=== Cause 1 of 7: CreateApp duplicate-name ConflictException message ===
AWS: creating an application whose name is already taken is refused with ConflictException / HTTP 409 (Smithy: CreateApp errors include ConflictException, httpError 409). Recorded message: `App with name <name> already exists.` — fixed text `App with name `, ` already exists.`, with <name> the request's `name`. Note the capitalised `App`, the sentence-final period, and that the message names the NAME, not an ARN.
Emulator: `an application named <name> already exists` — different fixed text, lower case, no period.
Why: clients branch on this message to distinguish an idempotent re-create from other 409s this operation models (ConflictException is also documented for concurrent-write conflicts); a paraphrase makes the duplicate-name case indistinguishable and breaks any create-if-absent wrapper matching AWS's text.
Fix: the duplicate-name error construction in the CreateApp handler.
Traces: resiliencehub-app-lifecycle#task#7.

=== Cause 2 of 7: CreateApp missing-policy ResourceNotFoundException message ===
AWS: CreateApp with a policyArn that names no policy is refused with ResourceNotFoundException / HTTP 404. Recorded message: `Cannot attach policy <policyArn> to app <appName>. Policy <policyArn> does not exist.` — fixed text `Cannot attach policy `, ` to app `, `. Policy `, ` does not exist.`; variable parts are the request's policyArn (twice) and the request's app `name`. The AWS message states the ATTACH intent and names the application being created; the emulator's does neither.
Emulator: `resiliency policy <policyArn> does not exist` — different fixed text and it drops the app name entirely.
Why: the message is the only place the failure is tied to the application the client was creating; without it, a client batching several CreateApp calls cannot tell which one failed from the error alone.
Fix: the policy-resolution error construction in the CreateApp handler (it must interpolate the requested app name alongside the policy ARN).
Traces: resiliencehub-app-lifecycle#task#8.

=== Cause 3 of 7: CreateApp constraint-violation ValidationException message uses the generic framework format ===
AWS: `name` is constrained by EntityName, pattern ^[A-Za-z0-9][A-Za-z0-9_\-]{1,59}$; a violating value is refused with ValidationException / HTTP 400. Recorded message: `[ECMA 262 regex "<pattern>" does not match input string "<value>"]` — resiliencehub's own constraint-report format: a bracketed list whose entries are `ECMA 262 regex "` + the member's pattern + `" does not match input string "` + the rejected value + `"`. Variable parts: the pattern of the violated member and the submitted value.
Emulator: the generic AWS-framework phrasing `1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>` — a completely different template (it names the member and counts errors; AWS's resiliencehub format does neither).
Why: resiliencehub does not use the common `N validation error detected` format, so a client or test matching the service's documented/recorded constraint report never matches, and a client extracting the offending value from the bracketed report reads the wrong substring.
Fix: this service's ValidationException constraint-report formatter (shared by every pattern/length-constrained member), not the CreateApp handler alone.
Traces: resiliencehub-app-lifecycle#task#9.

=== Cause 4 of 7: DescribeApp unknown-app ResourceNotFoundException message ===
AWS: DescribeApp on an unknown appArn is refused with ResourceNotFoundException / HTTP 404. Recorded message: `App not found for appArn <appArn>` — fixed text `App not found for appArn `, with the request's appArn appended; no trailing period.
Emulator: `application <appArn> does not exist` — different fixed text.
Why: this is the canonical not-found probe a client uses to test for existence (and the same text is expected after a delete); a paraphrase defeats message-based classification and any log-scraping that keys on `App not found for appArn`.
Fix: the not-found error construction in the DescribeApp handler (shared app-lookup helper).
Traces: resiliencehub-app-lifecycle#task#10 (cluster also covers resiliencehub-app-lifecycle#task#12).

=== Cause 5 of 7: CreateAppVersionResource duplicate-resource ConflictException message ===
AWS: adding a resource whose id is already present in the draft template is refused with ConflictException / HTTP 409. Recorded message: `Resource with id <resourceId> exists in the current app template. Please update the the app template to remove the duplicated resource.` — fixed text `Resource with id `, ` exists in the current app template. Please update the the app template to remove the duplicated resource.` (including AWS's literal doubled `the the`), with the duplicated resource id from the request.
Emulator: `a resource with logicalResourceId <logicalResourceId> already exists in the draft version of application <appArn>` — different fixed text, different variable set (it interpolates the appArn, which AWS's message does not contain), and it names a different id than the one AWS reports.
Why: the remediation sentence is what tells the client to edit its template; a client matching AWS's wording treats this as an unrelated conflict and retries with backoff instead of de-duplicating.
Fix: the duplicate-resource error construction in the CreateAppVersionResource handler.
Traces: resiliencehub-app-version-draft-composition#task#9.

=== Cause 6 of 7: DescribeResiliencyPolicy constraint-violation ValidationException message ===
AWS: `policyArn` is constrained by the Arn pattern; a non-ARN value is refused with ValidationException / HTTP 400 carrying the same bracketed `[ECMA 262 regex "<pattern>" does not match input string "<value>"]` report as Cause 3, with the Arn pattern and the submitted value.
Emulator: again the generic `1 validation error detected: Value '<value>' at 'policyArn' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>`.
Why: same as Cause 3 — and it confirms the defect is in the shared formatter rather than one operation, so fixing only CreateApp would leave this path diverging.
Fix: the shared ValidationException constraint-report formatter (same component as Cause 3).
Traces: resiliencehub-resiliency-policy-lifecycle#task#8.

=== Cause 7 of 7: DeleteResiliencyPolicy unknown-policy ResourceNotFoundException message ===
AWS: deleting a policy ARN that names no policy is refused with ResourceNotFoundException / HTTP 404. Recorded message: `ResiliencyPolicy <policyArn> not found` — fixed text `ResiliencyPolicy ` and ` not found`, with the request's policyArn; note the CamelCase entity name and the trailing `not found`.
Emulator: `resiliency policy <policyArn> does not exist` — different fixed text.
Why: a delete-if-exists wrapper that swallows AWS's `not found` text sees an unmatched message and propagates the error; this is the 404 path and is distinct from the in-use 409 message the open PublishAppVersion/DeleteResiliencyPolicy issue describes.
Fix: the not-found error construction in the DeleteResiliencyPolicy handler.
Traces: resiliencehub-resiliency-policy-lifecycle#task#10.

## Evidence
Every step below returned the EXPECTED status and error type; only `$.message` diverges.

Cause 1 — CreateApp 409, resiliencehub-app-lifecycle#task#7:
  expected 'App with name <name> already exists.'
  actual   'an application named <name> already exists'
Cause 2 — CreateApp 404, resiliencehub-app-lifecycle#task#8:
  expected 'Cannot attach policy <policyArn> to app <appName>. Policy <policyArn> does not exist.'
  actual   'resiliency policy <policyArn> does not exist'
Cause 3 — CreateApp 400, resiliencehub-app-lifecycle#task#9:
  expected '[ECMA 262 regex "^[A-Za-z0-9][A-Za-z0-9_\\-]{1,59}$" does not match input string "<value>"]'
  actual   "1 validation error detected: Value '<value>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9][A-Za-z0-9_\\-]{1,59}$"
Cause 4 — DescribeApp 404, resiliencehub-app-lifecycle#task#10 (and #task#12):
  expected 'App not found for appArn <appArn>'
  actual   'application <appArn> does not exist'
Cause 5 — CreateAppVersionResource 409, resiliencehub-app-version-draft-composition#task#9:
  expected 'Resource with id <resourceId> exists in the current app template. Please update the the app template to remove the duplicated resource.'
  actual   'a resource with logicalResourceId <id> already exists in the draft version of application <appArn>'
Cause 6 — DescribeResiliencyPolicy 400, resiliencehub-resiliency-policy-lifecycle#task#8:
  expected '[ECMA 262 regex "<Arn pattern>" does not match input string "<value>"]'
  actual   "1 validation error detected: Value '<value>' at 'policyArn' failed to satisfy constraint: Member must satisfy regular expression pattern: <Arn pattern>"
Cause 7 — DeleteResiliencyPolicy 404, resiliencehub-resiliency-policy-lifecycle#task#10:
  expected 'ResiliencyPolicy <policyArn> not found'
  actual   'resiliency policy <policyArn> does not exist'

Smithy citations: ValidationException (httpError 400), ResourceNotFoundException (httpError 404), ConflictException (httpError 409), each with a `message` member of String500; CreateApp.name / UpdateApp / DescribeResiliencyPolicy.policyArn patterns as quoted above. The app names, policy ARNs, resource ids and rejected strings in the recordings are per-run values filling each template — the fixed text and the identity of each variable slot are the requirement.

## Steps to Reproduce
1. CreateApp(name=<name>); CreateApp(name=<name>) again -> HTTP 409, message 'an application named <name> already exists' instead of 'App with name <name> already exists.' (Cause 1).
2. CreateApp(name=<other>, policyArn=arn:aws:resiliencehub:<region>:<account>:resiliency-policy/00000000-0000-0000-0000-000000000000) -> HTTP 404 with the paraphrased policy message, missing the 'Cannot attach policy ... to app <name>.' framing (Cause 2).
3. CreateApp(name="sim2real probe rh bad name") (spaces violate the EntityName pattern) -> HTTP 400 with the generic 'N validation error detected' text instead of the bracketed ECMA 262 report (Cause 3).
4. DescribeApp(appArn=<arn of a non-existent app>) -> HTTP 404 'application <arn> does not exist' instead of 'App not found for appArn <arn>' (Cause 4).
5. CreateAppVersionResource twice with the same logicalResourceId on one draft -> HTTP 409 with the paraphrased duplicate text (Cause 5).
6. DescribeResiliencyPolicy(policyArn="not-an-arn") -> HTTP 400 with the generic validation text (Cause 6).
7. DeleteResiliencyPolicy(policyArn=<arn of a non-existent policy>) -> HTTP 404 'resiliency policy <arn> does not exist' instead of 'ResiliencyPolicy <arn> not found' (Cause 7).
Full replay: replay_wire_corpus() and re-check resiliencehub-app-lifecycle#task#7, #task#8, #task#9, #task#10, resiliencehub-app-version-draft-composition#task#9, resiliencehub-resiliency-policy-lifecycle#task#8, #task#10.
