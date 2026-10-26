Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.create-usage-profile-invalid-configuration-rejected glue.delete-usage-profile-in-use-blocked
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.create-usage-profile-invalid-configuration-rejected|_error#5|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: glue.delete-usage-profile-in-use-blocked|run#11|AssertionError|assert code == ("AlreadyExistsException", f"UsageProfile {name} already exists."), code
Issue-Commit: ef9b9c4f59cabcf171f74b1ebb476d6a7b829294

# Glue: 2 net-new root causes in CreateUsageProfile — ProfileConfiguration ranges are never validated (invalid Min>Max accepted 200), and the duplicate-name AlreadyExistsException message omits the profile name

> _Note: this issue was identified against the code base at commit `ef9b9c4f59cabcf171f74b1ebb476d6a7b829294`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUsageProfile

## Description
Two independent defects in the Glue usage-profile control plane, both reached through CreateUsageProfile. Both are net-new: no open issue covers Glue usage profiles (list_existing_issues for 'usage' / 'usage-profile' returns nothing for this resource; the nearest open Glue bundle, 20261025T022820Z-glue-10-net-new-root-causes-identity-center.md, covers Identity Center / table optimizer / dashboard URL / unfiltered-metadata / Iceberg CreateTable / UpdateTable+VersionId, not usage profiles).

=== Cause 1 of 2: CreateUsageProfile never validates the ProfileConfiguration parameter ranges, so an incoherent configuration is accepted with 200 and persisted ===

AWS: Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json): CreateUsageProfile.Configuration targets ProfileConfiguration and is @required; ProfileConfiguration carries JobConfiguration and SessionConfiguration, each a ConfigurationMap of parameter-name -> ConfigurationObject{DefaultValue, AllowedValues, MinValue, MaxValue}. CreateUsageProfile models InvalidInputException (errorKind client, HTTP 400, "The input provided was not valid.") precisely for input the service refuses. Recorded behavior: real AWS cross-validates the numeric members of each ConfigurationObject and refuses a parameter whose DefaultValue/MinValue is above its MaxValue with HTTP 400, error code InvalidInputException and the message template:
  "default value for parameter <parameterName> in <configurationSection> should be less than or equal to max value <MaxValue>."
The FIXED text of that message is "default value for parameter ", " in ", " should be less than or equal to max value " and the trailing ".". The VARIABLE parts all come from the request: <parameterName> is the key in the submitted ConfigurationMap, <configurationSection> is the ProfileConfiguration member the map was sent under (JobConfiguration or SessionConfiguration), and <MaxValue> is that ConfigurationObject's MaxValue as submitted. (In the replayed run the parameter was numberOfWorkers in JobConfiguration with DefaultValue "5", MinValue "10", MaxValue "1", so the recorded wording was "default value for parameter numberOfWorkers in JobConfiguration should be less than or equal to max value 1." — those three tokens are request values filling the template, not part of it.) AWS also creates nothing: a subsequent GetUsageProfile on the rejected name answers EntityNotFoundException with message "UsageProfile <Name> does not exists" (fixed text "UsageProfile " + " does not exists", variable part the requested Name).

Emulator: the call SUCCEEDS. It answers HTTP 200 with body {"Name":"<requested name>"} (content-type application/x-amz-json-1.1) and inserts the profile into the store, so the incoherent configuration becomes readable state. No InvalidInputException is ever produced for a configuration defect — the handler's only refusals are the name collision and the store ceiling.

Why: a client that relies on AWS refusing an incoherent usage profile gets the opposite outcome. Validation harnesses and infra tooling that assert "this bad profile is rejected" see a 200 and treat the input as legal; worse, the emulator now holds a usage profile whose DefaultValue and MinValue sit outside its own MaxValue, so every later read (GetUsageProfile/ListUsageProfiles) hands back a configuration real AWS can never return, and the effect-absence invariant (a refused create leaves nothing behind) is broken because the create was not refused at all.

Fix: the request validation for this operation, in the CreateUsageProfile handler. handle() goes straight from the name-collision check to the store insert without inspecting input.configuration; it needs a per-ConfigurationObject coherence check over both JobConfiguration and SessionConfiguration that returns CreateUsageProfileErrorModeled::InvalidInputException with the message template above before anything is written.

Traces: glue.create-usage-profile-invalid-configuration-rejected (verdict mismatch — "expected a ClientError, but the call SUCCEEDED and returned {'Name': ..., 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}").

=== Cause 2 of 2: the duplicate-name AlreadyExistsException message is the wrong text and deliberately withholds the profile name ===

AWS: Smithy: CreateUsageProfile models AlreadyExistsException (errorKind client, HTTP 400, "A resource to be created or added already exists.") with a single Message member. Recorded behavior: creating a second usage profile under a name already taken is refused with HTTP 400, error code AlreadyExistsException and the message template:
  "UsageProfile <Name> already exists."
FIXED text: "UsageProfile " and " already exists." VARIABLE part: <Name>, echoed verbatim from the request's Name member. (The replayed run's concrete name is a generated per-run value filling that slot, not part of the requirement.) The status and the code the emulator returns are already correct; only the message diverges.

Emulator: returns AlreadyExistsException/400 with the hardcoded message "Usage profile already exists." — different fixed text ("Usage profile" rather than "UsageProfile", no trailing identifier) and no variable part at all. The handler comment states the name is withheld on purpose because no recording pinned the wording; this replay pins it, and the recorded wording echoes the name.

Why: a client that parses or asserts on the refusal message cannot tell WHICH profile name collided — with a batch of creates the emulator's message is identical for every name, so error-handling code that extracts the conflicting identifier from the message (a common Glue pattern, since AlreadyExistsException carries no structured name member) gets nothing. Checks and tooling that compare the message against AWS's wording fail against the emulator even though the operation's status and code are right.

Fix: the error construction in the CreateUsageProfile handler's name-collision branch — build the AlreadyExistsException message from the template, interpolating input.name, instead of the fixed string. The same name-echoing convention should be applied consistently with GetUsageProfile's EntityNotFoundException wording ("UsageProfile <Name> does not exists"), which the same check asserts.

Traces: glue.delete-usage-profile-in-use-blocked (verdict mismatch — AssertionError: ('AlreadyExistsException', 'Usage profile already exists.') where ('AlreadyExistsException', 'UsageProfile <name> already exists.') was expected).

## Evidence
Cause 1 — check glue.create-usage-profile-invalid-configuration-rejected, verdict [mismatch]:
  Request: CreateUsageProfile Name=<generated> Configuration={"JobConfiguration": {"workerType": {"DefaultValue": "G.1X", "AllowedValues": ["G.1X","G.2X"]}, "numberOfWorkers": {"DefaultValue": "5", "MinValue": "10", "MaxValue": "1"}}}
  Expected (AWS): HTTP 400, Code=InvalidInputException, Message="default value for parameter <parameterName> in <section> should be less than or equal to max value <MaxValue>." (recorded run: "...parameter numberOfWorkers in JobConfiguration ... max value 1."), and GetUsageProfile on that name then answers Code=EntityNotFoundException, Message="UsageProfile <Name> does not exists".
  Actual (emulator): HTTP 200, body {"Name": "<requested name>"}; harness reported `AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'Name': 'sim2real-glue-usageprofile-bad-...', 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '54', ...}}}`. The profile is persisted, so the effect-absent leg can never hold.

Cause 2 — check glue.delete-usage-profile-in-use-blocked, verdict [mismatch]:
  Request: CreateUsageProfile with a Name that already exists (second create, different Configuration/Description).
  Expected (AWS): HTTP 400, Code=AlreadyExistsException, Message="UsageProfile <Name> already exists." (fixed text + the request's Name).
  Actual (emulator): HTTP 400, Code=AlreadyExistsException, Message="Usage profile already exists." — harness reported `AssertionError: ('AlreadyExistsException', 'Usage profile already exists.')`.
  Source of the divergence: services/service-com-amazonaws-glue-awsglue/src/ops/create_usage_profile.rs line 176 hardcodes `Some("Usage profile already exists.".to_string())`, with a comment at lines 173-175 stating the requested name is withheld because no recording pinned the wording. The name-collision branch is lines 170-180; the unconditional store insert that Cause 1 reaches is lines 200-206, with no validation of `input.configuration` anywhere in handle() (lines 150-218).

## Steps to Reproduce
Cause 1 (missing ProfileConfiguration validation):
1. glue.create_usage_profile(Name="p-bad", Configuration={"JobConfiguration": {"workerType": {"DefaultValue": "G.1X", "AllowedValues": ["G.1X", "G.2X"]}, "numberOfWorkers": {"DefaultValue": "5", "MinValue": "10", "MaxValue": "1"}}})
2. Observe HTTP 200 and a returned Name instead of HTTP 400 InvalidInputException with message "default value for parameter numberOfWorkers in JobConfiguration should be less than or equal to max value 1."
3. glue.get_usage_profile(Name="p-bad") — the emulator returns the profile; AWS answers EntityNotFoundException "UsageProfile p-bad does not exists".

Cause 2 (AlreadyExistsException wording):
1. glue.create_usage_profile(Name="p1", Configuration={"JobConfiguration": {"numberOfWorkers": {"DefaultValue": "2", "MinValue": "1", "MaxValue": "10"}, "workerType": {"DefaultValue": "G.1X", "AllowedValues": ["G.1X", "G.2X"]}}}, Description="original") — succeeds.
2. Repeat the same call with Name="p1" and a different Configuration/Description.
3. Catch the ClientError: code is AlreadyExistsException (correct, 400) but the message is "Usage profile already exists." instead of "UsageProfile p1 already exists."

Replay both with: run_behavioral_suite, checks glue.create-usage-profile-invalid-configuration-rejected and glue.delete-usage-profile-in-use-blocked.
