Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatchomni-domain-validation-and-notfound-errors
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatchomni-domain-validation-and-notfound-errors cloudwatchomni-space-and-access-control-errors cloudwatchomni-view-alert-dashboard-notfound-and-validation
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni|GetDomain|4xx->4xx|$.message
Issue-Key: cloudwatchomni|CreateSpace|4xx->4xx|$.message
Issue-Key: cloudwatchomni|GetAccessGrant|4xx->4xx|$.message
Issue-Key: cloudwatchomni|GetView|4xx->4xx|$.message
Issue-Key: cloudwatchomni|GetAlert|4xx->4xx|$.message
Issue-Commit: 17c4643c966c9683f16ddaee3534ae6f6497ef31

# cloudwatchomni ResourceNotFoundException messages do not match AWS wording

> _Note: this issue was identified against the code base at commit `17c4643c966c9683f16ddaee3534ae6f6497ef31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDomain

## Description
The emulator emits its own generic not-found sentence instead of AWS's per-resource message text. Status (404) and error code (ResourceNotFoundException) are correct in every case; only the `message` member diverges. This is ONE shared fault — a single generic "<Resource> '<id>' does not exist." template used for every resource type instead of the per-operation AWS wording — with FIVE causes, each needing its own message template.

AWS (common): Smithy com.amazonaws.cloudwatchomni#ResourceNotFoundException, errorKind client, @httpError 404, required member `message`, doc "The specified resource does not exist." The model does NOT specify message text, so the message wording below is recorded behavior only, taken from the recorded responses. The error message is a requirement, so each operation must reproduce its own fixed text and fill the variable parts from the request.

Cause 1 of 5 — GetDomain (fixture cloudwatchomni-domain-validation-and-notfound-errors, step task#3).
  AWS: recorded behavior only — 404 ResourceNotFoundException, `message` is the constant "Domain not found". Entirely fixed text: it has NO variable part and in particular does not echo the requested domainId.
  Emulator: "Domain 'd-<id>' does not exist." — wrong fixed text plus an interpolated id AWS never includes.
  Why: clients and assertions that match on the not-found message for a missing domain (log scrapers, user-facing surfacing of `message`, contract tests) see text AWS never produces; and the emulator discloses the queried id in a message AWS keeps opaque.
  Fix: the GetDomain handler's not-found error construction.

Cause 2 of 5 — CreateSpace, parent-domain lookup (fixture cloudwatchomni-space-and-access-control-errors, step task#1).
  AWS: recorded behavior only — 404 ResourceNotFoundException, `message` is the same constant "Domain not found" when the domain named by the request's domainId does not exist. Fixed text, no variables.
  Emulator: "Domain 'd-<id>' does not exist."
  Why: as cause 1; also shows the domain-missing message must be identical across operations, which the emulator's per-site formatting does not guarantee.
  Fix: the CreateSpace handler's parent-domain validation (shared domain-not-found error construction).

Cause 3 of 5 — GetAccessGrant (fixture cloudwatchomni-space-and-access-control-errors, step task#6).
  AWS: recorded behavior only — 404 ResourceNotFoundException, `message` is the constant "AccessGrant not found". Fixed text, no variables; note the resource word is the modeled CamelCase "AccessGrant", not "Access grant".
  Emulator: "Access grant '<grantId>' does not exist." — different wording, different casing, and an interpolated id.
  Fix: the GetAccessGrant handler's not-found error construction.

Cause 4 of 5 — GetView (fixture cloudwatchomni-view-alert-dashboard-notfound-and-validation, step task#2).
  AWS: recorded behavior only — 404 ResourceNotFoundException with message template "View not found: <name>" where the fixed text is "View not found: " and the single variable is the view name taken verbatim from the request (recorded run: view.sim2realprobecwomissingf8c7f1cc — a per-run value, not part of the requirement).
  Emulator: "View '<name>' does not exist." — wrong fixed text and wrong punctuation/quoting of the variable.
  Fix: the GetView handler's not-found error construction.

Cause 5 of 5 — GetAlert (fixture cloudwatchomni-view-alert-dashboard-notfound-and-validation, step task#3).
  AWS: recorded behavior only — 404 ResourceNotFoundException with message template "No alert '<alertId>' in space '<spaceId>'" — fixed text "No alert '", "' in space '", "'", with both variables taken from the request (alertId and spaceId) and NO trailing period.
  Emulator: "Alert '<alertId>' does not exist in space '<spaceId>'." — same two variables, but the fixed text and the trailing period differ.
  Fix: the GetAlert handler's not-found error construction.

Not covered by any existing open issue: the two open cloudwatchomni issues are unimplemented-stub reports for CreateIntegration and ListDomains.

Traces: cloudwatchomni-domain-validation-and-notfound-errors#task#3 (GetDomain; cluster also covers #task#6); cloudwatchomni-space-and-access-control-errors#task#1 (CreateSpace), #task#6 (GetAccessGrant); cloudwatchomni-view-alert-dashboard-notfound-and-validation#task#2 (GetView), #task#3 (GetAlert).

## Evidence
Cause 1 GetDomain — status 404 both sides; $.message expected 'Domain not found', got "Domain 'd-7e4c6b310d8812180140e5' does not exist." (cluster of 2 steps: task#3, task#6).
Cause 2 CreateSpace — status 404 both sides; $.message expected 'Domain not found', got "Domain 'd-ad2ba2d4f2bb6f9cf6e008' does not exist."
Cause 3 GetAccessGrant — status 404 both sides; $.message expected 'AccessGrant not found', got "Access grant '00000000-0000-4000-8000-496d9e19c704' does not exist."
Cause 4 GetView — status 404 both sides; $.message expected 'View not found: view.sim2realprobecwomissingf8c7f1cc', got "View 'view.sim2realprobecwomissingf8c7f1cc' does not exist." Fixed text is 'View not found: '; the name is the request variable.
Cause 5 GetAlert — status 404 both sides; $.message expected "No alert 'e70d59a037802d0f10853b79dc813857' in space '00000000-0000-4000-8000-c5a4f0a9ee2f'", got "Alert 'e70d59a037802d0f10853b79dc813857' does not exist in space '00000000-0000-4000-8000-c5a4f0a9ee2f'." The ids are request variables; the surrounding words and the absent trailing period are the fixed text.
Smithy: ResourceNotFoundException @httpError 404 with required `message`; the model does not fix the text, so the wording above is recorded behavior only.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Inspect fixture cloudwatchomni-domain-validation-and-notfound-errors step task#3, fixture cloudwatchomni-space-and-access-control-errors steps task#1 and task#6, and fixture cloudwatchomni-view-alert-dashboard-notfound-and-validation steps task#2 and task#3.
4. Each step requests a non-existent domain, space parent, access grant, view or alert. The emulator answers 404 ResourceNotFoundException (correct status and code) but with its generic "<Resource> '<id>' does not exist." message instead of AWS's per-operation text.
