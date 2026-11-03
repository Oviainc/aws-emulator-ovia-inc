Issue-Source: trace-sweep-divergence
Issue-Trace: devicefarm-project-lifecycle
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: devicefarm-project-lifecycle
Issue-Shape-Id: com.amazonaws.devicefarm#DeviceFarm_20150623
Issue-Key: devicefarm|GetProject|4xx->4xx|$.message
Issue-Commit: ab6490681e1ab26d5567ff83a1d290924eb5dab0

# DeviceFarm NotFoundException message wording diverges from AWS for unknown project ARN

> _Note: this issue was identified against the code base at commit `ab6490681e1ab26d5567ff83a1d290924eb5dab0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetProject

## Description
Single cause (Cause 1 of 1): the message DeviceFarm returns in its NotFoundException for GetProject against an ARN that does not resolve to an existing project is worded differently by the emulator than by AWS.

AWS: GetProject takes a required `arn` member (Smithy: GetProject.arn -> AmazonResourceName, required, @length min 32 max 1011, @pattern ^arn:aws:devicefarm:.+$) and models NotFoundException (errorKind client, httpError 400, member `message`, doc "The specified entity was not found."). The model does not fix the message text, so the message is recorded behavior only: for a syntactically valid project ARN with no matching project, AWS answers HTTP 400 with error code NotFoundException and the message template "No project was found for arn <arn>" — the fixed text is "No project was found for arn " and the only variable part is the project ARN echoed verbatim from the request's `arn` member. The ARN value seen in the recording is a per-environment value (account id and project id differ in every environment) and is NOT part of the requirement.

Emulator: the HTTP status (400) and the error code (NotFoundException) are correct, but the message is "No project found for arn <arn>" — it drops the word "was", so the fixed text of the message does not match AWS's wording. The replayed step reports `$.message: value_mismatch (expected 'No project was found for arn arn:aws:devicefarm:us-west-2:000000000000:project:0000000000000001-emulated', got 'No project found for arn arn:aws:devicefarm:us-west-2:000000000000:project:0000000000000001-emulated')`, i.e. the divergence is entirely in the message's fixed prefix, not in the interpolated ARN.

Why: an error message is part of the API contract. Clients, SDK retry/branching helpers, integration test suites and log-scraping tooling that assert on or parse the DeviceFarm not-found message string will not match the emulator's text, so a caller that correctly handles the real AWS not-found message silently falls through to a generic error path when run against the emulator. Any snapshot or golden-file comparison of the error body also fails for a reason unrelated to the behavior being tested.

Fix: the GetProject handler's not-found path in the devicefarm service crate — specifically the place that constructs the NotFoundException message for an unresolvable project ARN. The template must be "No project was found for arn {arn}" with {arn} taken verbatim from the request's `arn` input member; status 400 and error code NotFoundException are already correct and should not change. The same wording should be used wherever the project lookup helper raises not-found so sibling project operations stay consistent.

Traces: fixture devicefarm-project-lifecycle, step task#6 (GetProject on a project ARN that no longer/never existed) — the single member of this divergence cluster. No other open issue covers this operation (no open devicefarm issues exist).

## Evidence
Expected (AWS, from the recording): HTTP 400, error code NotFoundException, message "No project was found for arn <request arn>".
Actual (emulator): HTTP 400, error code NotFoundException, message "No project found for arn <request arn>".
Replay detail: "$.message: value_mismatch (expected 'No project was found for arn arn:aws:devicefarm:us-west-2:000000000000:project:0000000000000001-emulated', got 'No project found for arn arn:aws:devicefarm:us-west-2:000000000000:project:0000000000000001-emulated')".
Verdict: mismatch; divergence_key service=devicefarm|GetProject|status=400->400|keys=$.message. Only the message's fixed text differs — status, error code and the interpolated ARN all agree.

## Steps to Reproduce
1. Replay fixture devicefarm-project-lifecycle against the emulator and stop at step task#6.
2. Equivalently, by hand: call devicefarm GetProject with an `arn` that matches ^arn:aws:devicefarm:.+$ and is 32..1011 chars but names a project that does not exist in emulator state (e.g. a project ARN that was just deleted by the preceding step of the fixture).
3. Inspect the 400 response body's `message` field: the emulator returns "No project found for arn <arn>" where AWS returns "No project was found for arn <arn>".
