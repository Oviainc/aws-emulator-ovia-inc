Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatchomni.omni-dashboard-body-must-be-a-panels-document
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni.omni-dashboard-body-must-be-a-panels-document|run#14|AssertionError|assert message == (
Issue-Commit: e0e096f7847ddbab99498abf6c574ac0da8fbc09

# CreateOmniDashboard: dashboard-body ValidationException message is prose and additionalProperties is not enforced

> _Note: this issue was identified against the code base at commit `e0e096f7847ddbab99498abf6c574ac0da8fbc09`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateOmniDashboard

## Description
One net-new root cause.

Cause 1 of 1: `CreateOmniDashboard` rejects a non-conforming dashboard `body` with its own hand-written prose message instead of AWS's dashboard-body-schema violation message, and it validates only the presence/shape of `panels` — it never enforces that the body has no other top-level members, so only the FIRST violation is ever reported and `additionalProperties` violations are not reported at all. Failing check: cloudwatchomni.omni-dashboard-body-must-be-a-panels-document.

AWS: Smithy (services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/smithy-model/com-amazonaws-cloudwatchomni-cloudwatchomnifrontend.json) models `CreateOmniDashboardInput.body` only as a required String with @length 1..1048576 (doc: "The dashboard definition, as a JSON document. Maximum 1 MiB."), and declares `ValidationException` (HTTP 400) for the operation; the document schema itself is NOT in the model, so for the body schema this is recorded behavior only. The recording shows AWS validating the body against a JSON-schema for the dashboard document and returning HTTP 400 `ValidationException` whose message AGGREGATES every violated constraint, one clause per violation, joined by "; ", each clause of the fixed form `<json-pointer-or-member>: does not satisfy the '<constraint>' constraint of the dashboard body schema`. For the body `{"widgets": []}` that is two clauses — the missing required member and the extra top-level member:
  "panels: does not satisfy the 'required' constraint of the dashboard body schema; $: does not satisfy the 'additionalProperties' constraint of the dashboard body schema"
Fixed text: ": does not satisfy the '", "' constraint of the dashboard body schema", the "; " separator, and the document root token "$". Variable parts, taken from the request's body: the location of each violation (the member name `panels` for the missing required member; `$` for a violation at the document root) and the constraint name (`required`, `additionalProperties`). The recording also shows the schema is closed at the top level: a body carrying any top-level member other than `panels` is refused, and a conforming `{"panels": []}` body is accepted and round-trips verbatim through CreateOmniDashboard/GetOmniDashboard/ListOmniDashboards.

Emulator: returns `ValidationException` (correct code) with the message "The dashboard body must contain a 'panels' array." — none of AWS's fixed text, no per-violation clause, no "; " aggregation, and no mention of the second violation. `crate::state::dashboard_body_rejection` returns an `Option<String>` of hand-written prose and short-circuits on the first problem it finds (not valid JSON / not an object / no `panels` / `panels` not an array / a non-object `panels[i]`), so at most one violation is ever reported; it inspects only the `panels` member and ignores every other top-level member, so a body such as `{"panels": [], "widgets": []}` is ACCEPTED and persisted where AWS refuses it for `additionalProperties`.

Why: a client that parses the ValidationException message to tell the caller which part of the dashboard document is wrong gets text in a shape it cannot parse and learns about only one of several violations, so a fix-and-retry loop needs one round trip per violation instead of one. Worse, a body with extra top-level members succeeds against the emulator and is refused by real CloudWatchOmni, so a dashboard definition validated locally fails in production — the emulator is more permissive than AWS on the same input.

Fix: the body-schema validation for this operation — `crate::state::dashboard_body_rejection` (the shared checker both CreateOmniDashboard and UpdateOmniDashboard call) must validate the body against the closed dashboard-body schema, COLLECT all violations rather than returning on the first, and render them as AWS's clause list; the handler then carries that text into the modeled ValidationException.

Traces: cloudwatchomni.omni-dashboard-body-must-be-a-panels-document (body `{"widgets": []}` refused with the wrong message).

## Evidence
Check cloudwatchomni.omni-dashboard-body-must-be-a-panels-document, [mismatch]:

  AssertionError: unexpected message "The dashboard body must contain a 'panels' array."
  (check_cloudwatchomni_omni_dashboard_body_must_be_a_panels_document.py line 71)

Request: CreateOmniDashboard(spaceId=<a space created in this run>, name=<unique>, body='{"widgets": []}').

Expected (recorded AWS): HTTP 400, Error.Code `ValidationException`, Error.Message
  "panels: does not satisfy the 'required' constraint of the dashboard body schema; $: does not satisfy the 'additionalProperties' constraint of the dashboard body schema"

Actual (emulator): Error.Code `ValidationException` (matches), Error.Message
  "The dashboard body must contain a 'panels' array."

Source: services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/state.rs lines 672-704 (`dashboard_body_rejection`) returns a single prose string and returns early at the first problem; it reads only the `panels` member and never rejects other top-level members. services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/create_omni_dashboard.rs lines 182-191 passes that string through as the ValidationException message.

Because the assertion fires on the refusal, the rest of the check (the conforming `{"panels": []}` round-trip through Create/Get/ListOmniDashboards and the gone-after-delete ResourceNotFoundException) was never reached and remains unverified.

## Steps to Reproduce
1. Boot the emulator and point a botocore `cloudwatchomni` client at it.
2. iam.create_role(...) to get a role ARN; cwo.create_domain(name=<unique>, identityProviders=["IAM"]); cwo.create_space(name=<unique>, domainId=<domainId>, dataAccessRoleArn=<roleArn>).
3. Call cwo.create_omni_dashboard(spaceId=<spaceId>, name=<unique>, body=json.dumps({"widgets": []})).
4. Observe ValidationException with message "The dashboard body must contain a 'panels' array." instead of the two-clause schema message "panels: does not satisfy the 'required' constraint of the dashboard body schema; $: does not satisfy the 'additionalProperties' constraint of the dashboard body schema".
5. Also call cwo.create_omni_dashboard(spaceId=<spaceId>, name=<unique2>, body=json.dumps({"panels": [], "widgets": []})): the emulator ACCEPTS and persists it, where AWS refuses it with the `additionalProperties` clause.
6. Replay check cloudwatchomni.omni-dashboard-body-must-be-a-panels-document to confirm the fix (it also covers the conforming-body round-trip that the current failure hides).
