Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.blueprint-run-timeout-reflected-in-status glue.create-blueprint-invalid-location-rejected
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.blueprint-run-timeout-reflected-in-status|run#10|AssertionError|assert blueprint["Status"] == "FAILED", (
Issue-Key: glue.create-blueprint-invalid-location-rejected|run#9|AssertionError|assert code == "IllegalBlueprintStateException", (
Issue-Commit: 145e568e7ef9b3115be9abe08039cd724717eead

# Glue blueprint registration always settles ACTIVE: a BlueprintLocation that does not resolve in S3 never reaches FAILED and carries no ErrorMessage

> _Note: this issue was identified against the code base at commit `145e568e7ef9b3115be9abe08039cd724717eead`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBlueprint

## Description
ONE net-new root cause (Cause 1 of 1).

Cause 1 of 1: the asynchronous blueprint-registration state machine has no failure path, so every CreateBlueprint settles ACTIVE regardless of whether the ZIP named by BlueprintLocation actually exists / is readable in S3.
Failing check ids for this cause: glue.blueprint-run-timeout-reflected-in-status, glue.create-blueprint-invalid-location-rejected.

AWS: CreateBlueprint is asynchronous. Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json): CreateBlueprint input `BlueprintLocation` (OrchestrationS3Location, @required, len 1..8192, pattern ^s3://([^/]+)/([^/]+/)*([^/]+)$); output carries only `Name`. `Blueprint$Status` targets the enum BlueprintStatus with values CREATING | ACTIVE | UPDATING | FAILED, documented member-by-member as "Creating - The blueprint registration is in progress. / Active - The blueprint has been successfully registered. / Updating - An update to the blueprint registration is in progress. / Failed - The blueprint registration failed." `Blueprint$ErrorMessage` (ErrorString) is "An error message." So AWS fetches the published blueprint archive after the create returns: when that fetch cannot resolve the object, the registration leaves CREATING for the terminal state FAILED and GetBlueprint reports that FAILED status together with an ErrorMessage. Recorded behavior for the message AWS returns in that case: fixed text "Access denied on object at location: " followed by the BlueprintLocation value taken verbatim from the CreateBlueprint request, then a terminating "." — i.e. `Access denied on object at location: <BlueprintLocation>.`; only the location is a request-supplied variable, the rest is fixed text. Consequently StartBlueprintRun against such a non-ACTIVE blueprint is refused with the modeled IllegalBlueprintStateException (HTTP 400) and message `No active definition found for Blueprint: <BlueprintName>` (fixed text plus the requested blueprint name), and GetBlueprintRuns stays empty.

Emulator: the registration unconditionally ripens to ACTIVE on a timer and never consults S3. services/service-com-amazonaws-glue-awsglue/src/ops/create_blueprint.rs documents the rule it implements: "The registration starts out `CREATING` and the clock owes it `ACTIVE` (crate::state::BLUEPRINT_SETTLES_AFTER)" — there is no FAILED branch and no ErrorMessage is ever written. Replay of glue.blueprint-run-timeout-reflected-in-status creates a blueprint whose location points at a key that was deliberately never uploaded and then polls GetBlueprint until Status != CREATING; the emulator answers `ACTIVE` ("expected a Blueprint registered against a nonexistent S3 object to reach FAILED, got ACTIVE"), so the following assertions on the terminal state, on ErrorMessage and on the IllegalBlueprintStateException refusal of StartBlueprintRun are all unreachable. glue.create-blueprint-invalid-location-rejected exercises the same registration against a bucket that does not exist; its first assertion is currently absorbed by the already-filed unimplemented `StartBlueprintRun` scaffold stub (it got error code `NotImplemented` where IllegalBlueprintStateException is modeled — that stub is covered by its own filed issue and is NOT re-filed here), but the same missing FAILED transition is what its later `expected blueprint to terminate FAILED` assertion depends on, so it is listed as a replay target for this cause rather than as a separate defect.

Why: a client that registers a blueprint and polls GetBlueprint to learn whether the upload actually landed is told the registration succeeded when it did not. It then has no ErrorMessage to surface, and it will hand the blueprint to StartBlueprintRun expecting either a RunId (ACTIVE) or IllegalBlueprintStateException (not ACTIVE); against this emulator it can never observe the not-ACTIVE branch at all, so error handling and retry/repair logic for a bad or missing blueprint archive is silently dead code. Orchestration built on blueprints appears healthy while no layout generator exists.

Fix: the blueprint registration state transition in the Glue service state (crate::state BlueprintEntry / BLUEPRINT_SETTLES_AFTER and the settle logic in services/service-com-amazonaws-glue-awsglue/src/state.rs) plus the CreateBlueprint handler that seeds it (services/service-com-amazonaws-glue-awsglue/src/ops/create_blueprint.rs): the settle step must resolve the requested BlueprintLocation against the S3 object store and move the record to FAILED with the ErrorMessage above when it cannot be read, instead of unconditionally ripening to ACTIVE; GetBlueprint (services/service-com-amazonaws-glue-awsglue/src/ops/get_blueprint.rs) must then report that Status and ErrorMessage, and StartBlueprintRun must consult the status so a non-ACTIVE blueprint is refused with IllegalBlueprintStateException and creates no run.

Traces: glue.blueprint-run-timeout-reflected-in-status (mismatch: got ACTIVE, expected FAILED), glue.create-blueprint-invalid-location-rejected (mismatch: same registration path; its leading assertion is blocked by the already-filed StartBlueprintRun stub).

## Evidence
glue.blueprint-run-timeout-reflected-in-status
  Expected: CreateBlueprint(Name=<bp>, BlueprintLocation=s3://<bucket>/blueprints/<bp>.zip) where that object was never uploaded -> GetBlueprint reports Status FAILED and ErrorMessage "Access denied on object at location: s3://<bucket>/blueprints/<bp>.zip."; StartBlueprintRun then fails with code IllegalBlueprintStateException (HTTP 400), message "No active definition found for Blueprint: <bp>", and GetBlueprintRuns returns [].
  Actual:   GetBlueprint reports Status "ACTIVE" and no ErrorMessage.
  Assertion: AssertionError: expected a Blueprint registered against a nonexistent S3 object to reach FAILED, got ACTIVE (check_glue_blueprint_run_timeout_reflected_in_status.py line 62)

glue.create-blueprint-invalid-location-rejected
  Expected: StartBlueprintRun on a blueprint whose location can never resolve -> IllegalBlueprintStateException / "No active definition found for Blueprint: <bp>", no run created, and the blueprint polls to terminal FAILED.
  Actual:   AssertionError: expected IllegalBlueprintStateException, got NotImplemented (check_glue_create_blueprint_invalid_location_rejected.py line 42) — the proximate NotImplemented is the already-filed StartBlueprintRun scaffold stub; the terminal-FAILED expectation behind it is this issue's cause.

Model evidence: BlueprintStatus = [CREATING, ACTIVE, UPDATING, FAILED]; Blueprint$ErrorMessage exists; StartBlueprintRun models IllegalBlueprintStateException (client, HTTP 400).
Code evidence: services/service-com-amazonaws-glue-awsglue/src/ops/create_blueprint.rs lines 8-11 — "The registration starts out `CREATING` and the clock owes it `ACTIVE`" — and handler body lines 167-213, which validate name uniqueness and the store ceiling only and never look at whether BlueprintLocation resolves.

## Steps to Reproduce
1. Create an S3 bucket B (any region-appropriate CreateBucket call).
2. glue.create_blueprint(Name="bp-missing", BlueprintLocation="s3://B/blueprints/bp-missing.zip") WITHOUT ever putting that object.
3. Poll glue.get_blueprint(Name="bp-missing")["Blueprint"] until Status != "CREATING".
   -> AWS: Status == "FAILED", ErrorMessage == "Access denied on object at location: s3://B/blueprints/bp-missing.zip."
   -> Emulator: Status == "ACTIVE", no ErrorMessage.
4. glue.start_blueprint_run(BlueprintName="bp-missing", RoleArn=<a glue-assumable role arn>).
   -> AWS: ClientError IllegalBlueprintStateException (HTTP 400), message "No active definition found for Blueprint: bp-missing"; glue.get_blueprint_runs(BlueprintName="bp-missing")["BlueprintRuns"] == [].
   -> Emulator: NotImplemented (the separately filed StartBlueprintRun stub); the not-ACTIVE refusal is unreachable because step 3 reported ACTIVE.
5. Control: upload a valid blueprint zip (blueprint.cfg + layout.py) to s3://B/blueprints/good.zip and create a blueprint at that location — on AWS this one, and only this one, settles ACTIVE.
