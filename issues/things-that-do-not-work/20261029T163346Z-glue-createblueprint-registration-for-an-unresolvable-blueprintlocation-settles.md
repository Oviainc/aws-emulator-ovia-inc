Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.create-blueprint-invalid-location-rejected
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.create-blueprint-invalid-location-rejected|run#17|AssertionError|assert final_status == "FAILED", f"expected blueprint to terminate FAILED, got {final_status}"
Issue-Commit: f30b839d650f592709ba7d30fd0311226ca1f682

# Glue CreateBlueprint registration for an unresolvable BlueprintLocation settles ACTIVE instead of terminal FAILED (no ErrorMessage)

> _Note: this issue was identified against the code base at commit `f30b839d650f592709ba7d30fd0311226ca1f682`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBlueprint

## Description
ONE net-new root cause (Cause 1 of 1).

Cause 1 of 1: the asynchronous blueprint-registration state machine has no failure path, so a blueprint whose BlueprintLocation cannot be resolved in S3 still ripens to ACTIVE on a timer; FAILED is unreachable and no ErrorMessage is ever written.
Failing check ids for this cause: glue.create-blueprint-invalid-location-rejected.

Related already-open issue: 20261028T175030Z-glue-blueprint-registration-always-settles-active-a-blueprintlocation.md describes the same registration path and lists this check among its replay targets, but the replay report does NOT mark this check as OWNED by it (that issue's replay key is pinned to the check's leading `assert code == "IllegalBlueprintStateException"` assertion, which now PASSES — the refusal path has been implemented since). The surviving divergence is the terminal-status one below, so it is filed here and that issue is named rather than assumed to cover it; if a fixer closes both, they are one and the same cause.

AWS: CreateBlueprint is asynchronous — it returns only `Name` (Smithy, services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json: CreateBlueprint input `Name` @required OrchestrationNameString len 1..128, `BlueprintLocation` @required OrchestrationS3Location len 1..8192 pattern ^s3://([^/]+)/([^/]+/)*([^/]+)$; output CreateBlueprintResponse{Name}). Glue then fetches and registers the archive named by BlueprintLocation. `Blueprint$Status` targets enum BlueprintStatus = [CREATING, ACTIVE, UPDATING, FAILED], documented member-by-member ("Creating — the blueprint registration is in progress … Failed — the blueprint registration failed"), and `Blueprint$ErrorMessage` (ErrorString) is "An error message." So when the published object cannot be read (its bucket or key does not exist), the registration leaves CREATING for the TERMINAL state FAILED and GetBlueprint reports that status together with an ErrorMessage; ACTIVE is unreachable for such a blueprint. Recorded behavior for the message AWS returns in that case: fixed text "Access denied on object at location: " + the BlueprintLocation value taken verbatim from the CreateBlueprint request + "." — i.e. `Access denied on object at location: <BlueprintLocation>.`; only the location is a request-supplied variable, the rest is fixed text. The consequent refusal of StartBlueprintRun on a non-ACTIVE blueprint (modeled IllegalBlueprintStateException, HTTP 400, message `No active definition found for Blueprint: <BlueprintName>` — fixed text plus the requested blueprint name) and the empty GetBlueprintRuns are already honored by the emulator and are NOT the divergence here.

Emulator: GetBlueprint polling on a blueprint created against a bucket that does not exist reports `ACTIVE` once the settle timer elapses, with no ErrorMessage. The replay fails on `assert final_status == "FAILED"` with `AssertionError: expected blueprint to terminate FAILED, got ACTIVE`. The settle step never consults the S3 object store, so there is no input that can produce a FAILED registration.

Why: a client that registers a blueprint and polls GetBlueprint to learn whether the published archive actually landed is told the registration succeeded when it did not, and has no ErrorMessage to surface. Its FAILED/repair branch (re-upload, alarm, abort the orchestration) is dead code against this emulator, and blueprint-based orchestration looks healthy while no layout generator exists.

Fix: the blueprint registration settle logic in the Glue service state (services/service-com-amazonaws-glue-awsglue/src/state.rs, the BlueprintEntry settle path / BLUEPRINT_SETTLES_AFTER) together with the handler that seeds it (services/service-com-amazonaws-glue-awsglue/src/ops/create_blueprint.rs): the settle step must resolve the requested BlueprintLocation against the S3 object store and move the record to the terminal FAILED state with the ErrorMessage above when the object cannot be read, instead of unconditionally ripening to ACTIVE; GetBlueprint (services/service-com-amazonaws-glue-awsglue/src/ops/get_blueprint.rs) must then report that Status and ErrorMessage.

Traces: glue.create-blueprint-invalid-location-rejected (mismatch, AssertionError at check_glue_create_blueprint_invalid_location_rejected.py line 70: expected blueprint to terminate FAILED, got ACTIVE).

## Evidence
glue.create-blueprint-invalid-location-rejected [mismatch]
  Expected: glue.create_blueprint(Name=<bp>, BlueprintLocation="s3://<nonexistent-bucket>/blueprint.zip") -> polling glue.get_blueprint(Name=<bp>)["Blueprint"]["Status"] reaches the terminal value "FAILED" (never ACTIVE), with Blueprint.ErrorMessage == "Access denied on object at location: s3://<nonexistent-bucket>/blueprint.zip." (fixed text + the request's BlueprintLocation).
  Actual:   the status polls to "ACTIVE" and no ErrorMessage is present.
  Assertion: AssertionError: expected blueprint to terminate FAILED, got ACTIVE (check_glue_create_blueprint_invalid_location_rejected.py line 70)
  Passing (not part of this divergence, recorded for contrast): StartBlueprintRun on the non-ACTIVE blueprint is refused IllegalBlueprintStateException / "No active definition found for Blueprint: <bp>"; GetBlueprintRuns == []; GetBlueprint after DeleteBlueprint is EntityNotFoundException / "Blueprint with given name not found."

Model evidence: BlueprintStatus enum = [CREATING, ACTIVE, UPDATING, FAILED]; Blueprint$ErrorMessage (ErrorString) exists; CreateBlueprint output carries only Name (asynchronous registration); StartBlueprintRun models IllegalBlueprintStateException (client, HTTP 400).

## Steps to Reproduce
1. Pick a bucket name B that does NOT exist (do not create it).
2. glue.create_blueprint(Name="bp-missing", BlueprintLocation="s3://B/blueprint.zip").
3. Poll glue.get_blueprint(Name="bp-missing")["Blueprint"]["Status"] until it is ACTIVE or FAILED.
   -> AWS: "FAILED", with Blueprint.ErrorMessage == "Access denied on object at location: s3://B/blueprint.zip."
   -> Emulator: "ACTIVE", no ErrorMessage.
4. Control: upload a valid blueprint archive (blueprint.cfg + layout.py) to an existing bucket and register a blueprint at that location — only this one settles ACTIVE on AWS.
