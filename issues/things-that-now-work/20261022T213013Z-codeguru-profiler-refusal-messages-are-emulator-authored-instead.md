Issue-Source: trace-sweep-divergence
Issue-Trace: codeguruprofiler-error-paths
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeguruprofiler-error-paths codeguruprofiler-profiling-group-lifecycle codeguruprofiler-read-only-analysis-paths
Issue-Shape-Id: com.amazonaws.codeguruprofiler#CodeGuruProfiler
Issue-Key: codeguru-profiler|CreateProfilingGroup|4xx->4xx|$.message
Issue-Key: codeguru-profiler|DescribeProfilingGroup|4xx->4xx|$.message
Issue-Key: codeguru-profiler|DeleteProfilingGroup|4xx->4xx|$.message
Issue-Key: codeguru-profiler|GetProfile|4xx->4xx|$.message
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# codeguru-profiler refusal messages are emulator-authored instead of the service's request-derived text

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProfilingGroup, DescribeProfilingGroup, DeleteProfilingGroup, GetProfile

## Description
Five wire-replay causes in how codeguru-profiler builds the `message` of a refusal. In every one the HTTP status and the error code ALREADY MATCH AWS; what diverges is the message, and for each the rule producing it is known — it is either a constraint-violation message rendered from the request and the model's own constraint, or a service text parameterised by the request's identifiers (or, in Cause 5, a fixed service constant observed twice in the corpus). All five causes must be closed before this issue is resubmitted.

=== Cause 1 of 5: @pattern violation message keeps the model's regex anchors ===
AWS: Smithy CreateProfilingGroup.profilingGroupName targets ProfilingGroupName with `pattern: "^[\\w-]+$"`, length 1..255, and the operation models ValidationException (400). Recorded behavior: AWS renders the standard constraint message with the regex UNANCHORED — "1 validation error detected: Value '<v>' at 'profilingGroupName' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w-]+". The rule is the rendering: member path + constraint, with the pattern stripped of the model's ^/$ anchors.
Emulator: emits the model string verbatim — "... pattern: ^[\\w-]+$". Status 400 and code ValidationException match.
Why: a client or test that reads the accepted alphabet out of the constraint message, or compares it, sees a pattern AWS never emits; the anchors are an artifact of the Smithy source, not of the service's message.
Fix: the shared request-validation layer that formats @pattern violations — strip a leading ^ and trailing $ when rendering the pattern into the message.
Traces: codeguruprofiler-error-paths#task#3.

=== Cause 2 of 5: ConflictException message for a duplicate profiling group is emulator-authored ===
AWS: CreateProfilingGroup models ConflictException (@error client, httpError 409, required `message`). Recorded behavior: the text is built from the request and the caller's account — "ProfilingGroup (name: <profilingGroupName>, accountId: <accountId>) already exists." Both substitutions come from the call, so the format is the requirement.
Emulator: "a profiling group named <profilingGroupName> already exists" — status 409 and ConflictException match, but the account qualifier and the canonical phrasing are absent.
Why: AWS's message tells the caller WHICH group in WHICH account collided, which is the actionable part when the same name exists in another account; the emulator's text drops that identifier.
Fix: CreateProfilingGroup's duplicate-name conflict path — render the name/accountId format.
Traces: codeguruprofiler-error-paths#task#2.

=== Cause 3 of 5: DescribeProfilingGroup's 404 text is emulator-authored ===
AWS: DescribeProfilingGroup models ResourceNotFoundException (@error client, httpError 404, required `message`). Recorded behavior: the text names the requested group in the service's own phrasing — "CodeGuru Profiler can\u2019t complete the operation because <profilingGroupName> was not found." (typographic apostrophe), parameterised by the request's profilingGroupName.
Emulator: "no profiling group named <profilingGroupName>". Status 404 and the error code match.
Why: the 404 classifies correctly, but a client surfacing or matching the service text gets emulator prose; since the text is request-derived it is a reproducible rule, not an environment artifact.
Fix: the not-found path of the profiling-group read handler (and the shared helper that builds its message).
Traces: codeguruprofiler-error-paths#task#4 (cluster also covers codeguruprofiler-profiling-group-lifecycle#teardown#8).

=== Cause 4 of 5: DeleteProfilingGroup's 404 uses a DIFFERENT AWS format that the emulator also does not reproduce ===
AWS: DeleteProfilingGroup models ResourceNotFoundException (404). Recorded behavior: for this operation the text is the name/account form — "ProfilingGroup (name: <profilingGroupName>, accountId: <accountId>) not found." — distinct from Cause 3's phrasing for the read path.
Emulator: the same generic "no profiling group named <profilingGroupName>" as every other operation, so the two distinct, request-derived AWS formats are collapsed into one and NEITHER matches.
Why: the emulator cannot be made correct by fixing one shared string — the delete path needs its own name/accountId format. A client keying on the delete-path text never sees it.
Fix: DeleteProfilingGroup's not-found path — build the name/accountId message rather than deferring to the shared generic text.
Traces: codeguruprofiler-error-paths#task#5.

=== Cause 5 of 5: GetProfile's no-data 404 does not use the service's fixed message ===
AWS: GetProfile models ResourceNotFoundException (404). Recorded behavior only: when the group exists but no aggregated profile covers the requested range, the message is a FIXED constant independent of the request — "No profiles found for the requested time range!" — and the same constant appears in a second fixture of this corpus (codeguruprofiler-agent-report-workflow#task#5), which is what identifies it as a service constant rather than one recording's text.
Emulator: a per-request sentence instead — "no profile has been posted to profiling group <name> in the requested time range". Status 404 and the error code match.
Why: both "group not found" and "no data in range" are 404 ResourceNotFoundException, so the message is the only thing distinguishing them; the emulator never emits the constant a client would match on, and instead emits a group-named sentence that reads like the group-missing case.
Fix: GetProfile's no-aggregated-data path — return the fixed service message for an empty time range.
Traces: codeguruprofiler-read-only-analysis-paths#task#2.

Related already-open behavioral issue: "CodeGuru Profiler error-message construction diverges from AWS: anchored @pattern, truncated value, wrong list-member path, unsorted/incomplete enum sets, and emulator-authored not-found/conflict texts" (20261022T185439Z-codeguruprofiler-codeguru-profiler-error-message-construction-diverges-from-aws) reports the same class of fault from the BEHAVIORAL corpus. It does not carry any of the five wire replay keys below — and its not-found cause asserts the read-path text for the delete path too, which Cause 4 shows is a different format — so these causes are filed here with the fixture steps that exercise them.

## Evidence
In all five the status and error code matched; only `message` diverged. Expected = recorded AWS, actual = emulator.

Cause 1 — codeguruprofiler-error-paths#task#3, CreateProfilingGroup, 400==400:
  expected: "1 validation error detected: Value 'bad name with spaces!' at 'profilingGroupName' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w-]+"
  actual:   "1 validation error detected: Value 'bad name with spaces!' at 'profilingGroupName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\w-]+$"

Cause 2 — codeguruprofiler-error-paths#task#2, CreateProfilingGroup, 409==409:
  expected: 'ProfilingGroup (name: sim2real-probe-codeguruprofiler-pg-errors-bd415873, accountId: 000000000000) already exists.'
  actual:   'a profiling group named sim2real-probe-codeguruprofiler-pg-errors-bd415873 already exists'

Cause 3 — codeguruprofiler-error-paths#task#4, DescribeProfilingGroup, 404==404 (affected_count 2, also codeguruprofiler-profiling-group-lifecycle#teardown#8):
  expected: 'CodeGuru Profiler can\u2019t complete the operation because sim2real-probe-codeguruprofiler-pg-does-not-exist-bd415873 was not found.'
  actual:   'no profiling group named sim2real-probe-codeguruprofiler-pg-does-not-exist-bd415873'

Cause 4 — codeguruprofiler-error-paths#task#5, DeleteProfilingGroup, 404==404:
  expected: 'ProfilingGroup (name: sim2real-probe-codeguruprofiler-pg-does-not-exist-bd415873, accountId: 000000000000) not found.'
  actual:   'no profiling group named sim2real-probe-codeguruprofiler-pg-does-not-exist-bd415873'
  (note the expected text differs from Cause 3's for the same missing group — two distinct AWS formats)

Cause 5 — codeguruprofiler-read-only-analysis-paths#task#2, GetProfile, 404==404:
  expected: 'No profiles found for the requested time range!'
  actual:   'no profile has been posted to profiling group sim2real-probe-codeguruprofiler-pg-analysis-84bf9050 in the requested time range'

The group names and the account id above are environment values; the requirement in Causes 2-4 is the message FORMAT and its substitutions, in Cause 1 the unanchored rendering of the model's pattern, and in Cause 5 the fixed constant.

## Steps to Reproduce
Boot the emulator, point a codeguru-profiler client at it, and inspect Error.Message on each refusal (Code and HTTP status already match):
1. Cause 1: CreateProfilingGroup(profilingGroupName='bad name with spaces!', clientToken=uuid4()) — 400 ValidationException whose pattern is rendered anchored.
2. Cause 2: CreateProfilingGroup twice with the same profilingGroupName — 409 ConflictException without the name/accountId format.
3. Cause 3: DescribeProfilingGroup(profilingGroupName=<never created>) — 404 with the generic text.
4. Cause 4: DeleteProfilingGroup(profilingGroupName=<never created>) — 404 with the same generic text instead of the name/accountId form.
5. Cause 5: CreateProfilingGroup, then GetProfile(profilingGroupName=<group>, period='PT5M') with no profile posted — 404 with a per-request sentence instead of the fixed constant.
6. Or replay codeguruprofiler-error-paths step task#3 plus the fixture#step bindings in trace_ids.
