Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-signals.batch-delete-instrumentation-configurations-partial-failure-rejected application-signals.delete-instrumentation-config-not-found-rejected application-signals.instrumentation-config-create-get-delete-lifecycle application-signals.slo-duplicate-name-conflict-rejected application-signals.slo-incompatible-sli-type-switch-rejected application-signals.slo-missing-sli-config-validation-rejected application-signals.tag-resource-nonexistent-arn-rejected
Issue-Shape-Id: com.amazonaws.applicationsignals#ApplicationSignals
Issue-Key: application-signals.batch-delete-instrumentation-configurations-partial-failure-rejected|run#17|AssertionError|assert error_entries.get(bogus_arn) == (
Issue-Key: application-signals.delete-instrumentation-config-not-found-rejected|run#11|AssertionError|assert message == "The requested instrumentation configuration does not exist.", (
Issue-Key: application-signals.instrumentation-config-create-get-delete-lifecycle|run#20|AssertionError|assert message == "The requested instrumentation configuration has been deleted.", (
Issue-Key: application-signals.slo-duplicate-name-conflict-rejected|run#11|AssertionError|assert message == "A resource with the same identifier already exists", (
Issue-Key: application-signals.slo-incompatible-sli-type-switch-rejected|run#11|AssertionError|assert message == "Updating from request-based SLO to period-based SLO is not allowed", (
Issue-Key: application-signals.slo-missing-sli-config-validation-rejected|run#5|AssertionError|assert message == "Either SliConfig or RequestBasedSliConfig must be populated", (
Issue-Key: application-signals.tag-resource-nonexistent-arn-rejected|run#11|AssertionError|assert message == f"Resource with arn '{bogus_arn}' does not exist.", (
Issue-Commit: 2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf

# application-signals words seven refusal messages differently from AWS: SLO validation/conflict and instrumentation-configuration not-found all answer emulator-invented prose

> _Note: this issue was identified against the code base at commit `2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateServiceLevelObjective

## Description
Seven behavioral checks fail against application-signals. In every one the HTTP status and the error CODE the emulator returns are already correct; what diverges is the `message` field of the error, which the emulator words with its own prose (usually an ARN-echoing template minted by the crate's generic resource helpers) instead of the wording AWS uses for that error class. The error message is part of the contract: clients log it, surface it to operators, and some match on it. Below, each cause gives AWS's wording with its fixed text and its variable parts called out, the emulator's wording, and the component the wording belongs in.

Note on an adjacent open issue: 20261028T202011Z-application-signals-createservicelevelobjective-error-response.md already observes that the SLI-config refusals (Causes 5 and 6 here) answer "a bare prose sentence" and proposes rewording them into the crate's `constraint_clause`/`validation_envelope` builder. That issue does NOT cover these causes and its proposed target wording is itself wrong against AWS: AWS does not wrap these two rules in an `N validation error(s) detected: Value at '<member>' failed to satisfy constraint: ...` envelope at all — it answers the plain sentences quoted in Causes 5 and 6. Whoever fixes that issue and this one must land the wording below, not the envelope.

--- Cause 1 of 7: BatchDeleteInstrumentationConfigurations per-item not-found message ---
AWS: Smithy: BatchDeleteInstrumentationConfigurations POST /batch-delete-instrumentation-configurations; output member `Errors` -> BatchDeleteErrorList, per-item entries carrying `ResourceArn`/`Code`/`Message`; the operation itself models only ThrottlingException and ValidationException, so a missing ARN is reported per item and never as a top-level failure. Recorded behavior only for the message: for an ARN in the batch that names no live configuration, the per-item entry is Code `ResourceNotFoundException` with Message `The instrumentation config is already deleted or expired.` — that whole sentence is FIXED text with NO variable parts: it names no ARN, no service, no environment and no hash. (AWS deliberately does not distinguish deleted from expired here, which is why the sentence is a disjunction.)
Emulator: the per-item Message is the crate's generic ARN-echoing not-found template, `InstrumentationConfiguration with arn '<arn>' does not exist.` — i.e. it interpolates the request's ARN into a sentence AWS never sends for this item.
Why: a client reading the batch result to decide whether to retry or to treat the item as already-reaped reads a sentence AWS never produces; log scrapers and runbooks keyed on the "already deleted or expired" phrasing never match, and the entry leaks an ARN into a message AWS keeps ARN-free.
Fix: the per-item error construction in the BatchDeleteInstrumentationConfigurations handler (and whatever shared not-found message helper it borrows) must emit this operation's own fixed sentence rather than reusing the generic ARN template.
Traces: application-signals.batch-delete-instrumentation-configurations-partial-failure-rejected

--- Cause 2 of 7: DeleteInstrumentationConfiguration second-delete not-found message ---
AWS: Smithy: DeleteInstrumentationConfiguration POST /delete-instrumentation-configuration, errors ResourceNotFoundException (404), ThrottlingException, ValidationException. Recorded behavior only for the message: deleting a configuration that is already gone is refused ResourceNotFoundException with Message `The requested instrumentation configuration does not exist.` — entirely FIXED text, no variable parts, no ARN and no identifier.
Emulator: 404 ResourceNotFoundException (correct code) with `InstrumentationConfiguration with arn '<arn>' does not exist.`, the generic ARN template again.
Why: AWS distinguishes the delete path's "does not exist" from the read path's "has been deleted" (Cause 3) with two different fixed sentences; collapsing both into one ARN template destroys that distinction and gives idempotent-delete wrappers nothing to key on.
Fix: the DeleteInstrumentationConfiguration handler's not-found branch.
Traces: application-signals.delete-instrumentation-config-not-found-rejected

--- Cause 3 of 7: GetInstrumentationConfiguration after-delete not-found message ---
AWS: Smithy: GetInstrumentationConfiguration POST /get-instrumentation-configuration, errors ResourceNotFoundException (404), ThrottlingException, ValidationException. Recorded behavior only for the message: reading a configuration that was deleted is refused ResourceNotFoundException with Message `The requested instrumentation configuration has been deleted.` — entirely FIXED text, no variable parts. Note this is a DIFFERENT sentence from Cause 2's, for the same tombstoned record, selected by which operation was called.
Emulator: 404 ResourceNotFoundException (correct code) with `InstrumentationConfiguration with arn '<arn>' does not exist.`.
Why: a client that reads back after a delete cannot tell "deleted" from "never existed"; it also sees the same text the delete path returns, so the two tombstone states AWS exposes collapse into one.
Fix: the GetInstrumentationConfiguration handler's not-found branch — specifically the branch that resolves a tombstoned/deleted record, which must carry its own wording distinct from the delete handler's.
Traces: application-signals.instrumentation-config-create-get-delete-lifecycle

--- Cause 4 of 7: CreateServiceLevelObjective duplicate-Name conflict message ---
AWS: Smithy: CreateServiceLevelObjective POST /slo, errors include ConflictException (httpError 409). Recorded behavior only for the message: creating a second SLO whose `Name` already exists in the account is refused 409 ConflictException with Message `A resource with the same identifier already exists` — entirely FIXED text, with NO trailing period, and notably NO variable parts: it does not name the SLO, its ARN or its Name.
Emulator: 409 ConflictException (correct code) with `Slo with arn '<arn>' already exists.` — the crate's generic already-exists template, which interpolates the computed ARN and adds a period.
Why: the message tells a client "same identifier" generically; a client comparing the message to AWS's (or matching it) diverges, and the emulator additionally discloses the ARN of a resource the caller did not name in a field AWS keeps generic.
Fix: the duplicate-name branch of the CreateServiceLevelObjective handler / the SLO store's uniqueness check, which must word its own conflict rather than reuse the generic ARN already-exists helper.
Traces: application-signals.slo-duplicate-name-conflict-rejected

--- Cause 5 of 7: UpdateServiceLevelObjective evaluation-kind-switch message ---
AWS: Smithy: UpdateServiceLevelObjective PATCH /slo/{Id}, errors ResourceNotFoundException, ThrottlingException, ValidationException (400); input carries both `SliConfig` and `RequestBasedSliConfig`, and the model cannot express that the stored SLO's evaluation kind may not change. Recorded behavior only for the message: supplying `SliConfig` (period-based) for an SLO stored as request-based is refused 400 ValidationException with Message `Updating from request-based SLO to period-based SLO is not allowed` — FIXED text, no trailing period; the only part that varies is the DIRECTION, which is taken from the stored record's evaluation type and the supplied config's kind (the mirror case reads "Updating from period-based SLO to request-based SLO is not allowed"). AWS names the actual direction attempted; it does not emit a direction-agnostic sentence.
Emulator: 400 ValidationException (correct code) with the direction-agnostic sentence `You cannot change from a period-based SLO to a request-based SLO, or change from a request-based SLO to a period-based SLO.`
Why: the client is told the rule but not which direction it tripped, so it cannot report which half of its request was wrong; the wording also differs from AWS's for an error class clients compare on.
Fix: the evaluation-kind guard in the UpdateServiceLevelObjective handler (per the adjacent issue, src/ops/update_service_level_objective.rs's third bare-sentence refusal) — it must word the attempted direction from the stored SLO's evaluation type and the request's config kind.
Traces: application-signals.slo-incompatible-sli-type-switch-rejected

--- Cause 6 of 7: CreateServiceLevelObjective missing-SLI-config message ---
AWS: Smithy: CreateServiceLevelObjective input declares `SliConfig` and `RequestBasedSliConfig` both OPTIONAL, so the "exactly one of" rule is handler-enforced; ValidationException httpError 400. Recorded behavior only for the message: omitting both is refused 400 ValidationException with Message `Either SliConfig or RequestBasedSliConfig must be populated` — entirely FIXED text, no trailing period, no variable parts, and crucially NOT wrapped in the `N validation error(s) detected: Value at '<member>' failed to satisfy constraint: ...` envelope AWS uses for MODELLED constraints of this same operation.
Emulator: 400 ValidationException (correct code) with `You must specify either SliConfig or RequestBasedSliConfig.` (crate::state::absent_sli_config_refusal).
Why: a client that keys on this message to prompt for the missing config never matches. This also contradicts the open issue named above, which would move this message INTO the constraint envelope — a second, larger divergence from AWS; the required wording is the plain sentence quoted here.
Fix: `absent_sli_config_refusal` (and the CreateServiceLevelObjective handler branch that returns it), not the constraint-envelope builder.
Traces: application-signals.slo-missing-sli-config-validation-rejected

--- Cause 7 of 7: TagResource not-found message is resource-type-specific ---
AWS: Smithy: TagResource POST /tag-resource, input `ResourceArn` (AmazonResourceName, required, length 1-1024), errors ResourceNotFoundException (404), ServiceQuotaExceededException, ThrottlingException. Recorded behavior only for the message: tagging an ARN that resolves to no existing resource is refused 404 ResourceNotFoundException with Message `Resource with arn '<ResourceArn>' does not exist.` — the FIXED text is `Resource with arn '` ... `' does not exist.` and the single variable part is the `ResourceArn` echoed VERBATIM from the request. The tagging plane is resource-type-agnostic: the word is `Resource`, never the concrete type the ARN's resource segment happens to spell.
Emulator: 404 ResourceNotFoundException (correct code) with `Slo with arn '<ResourceArn>' does not exist.` — it substitutes the concrete type `Slo` for AWS's generic `Resource`, apparently by routing through the SLO store's own not-found helper.
Why: a client tagging across resource types gets a type word AWS does not send, and the message varies by type where AWS's does not; anything comparing or templating this message diverges.
Fix: the TagResource handler's ARN-resolution failure path must produce the tagging plane's own generic `Resource with arn '...' does not exist.` wording rather than delegating to the per-type (SLO) not-found message.
Traces: application-signals.tag-resource-nonexistent-arn-rejected

## Evidence
Suite: 17/24 passed; all 7 failures are application-signals mismatches. Every one asserts on the error `message` only — the error Code asserted immediately before it passed in each case.

Cause 1 — check batch-delete-instrumentation-configurations-partial-failure-rejected, line 68:
  expected Errors[<bogus-arn>] == ("ResourceNotFoundException", "The instrumentation config is already deleted or expired.")
  got      ("ResourceNotFoundException", "InstrumentationConfiguration with arn 'arn:aws:application-signals:us-west-2:000000000000:instrumentationConfig/<service>/<environment>/SNAPSHOT/<hash>' does not exist.")
  (the ARN shown is one run's value; the defect is the template, not the value)

Cause 2 — check delete-instrumentation-config-not-found-rejected, line 53:
  expected message == "The requested instrumentation configuration does not exist."
  got      "InstrumentationConfiguration with arn 'arn:aws:application-signals:us-west-2:000000000000:instrumentationConfig/<service>/<environment>/SNAPSHOT/<hash>' does not exist."
  (code assertion `code == "ResourceNotFoundException"` on line 52 passed)

Cause 3 — check instrumentation-config-create-get-delete-lifecycle, line 83:
  expected message == "The requested instrumentation configuration has been deleted."
  got      "InstrumentationConfiguration with arn 'arn:aws:application-signals:us-west-2:000000000000:instrumentationConfig/<service>/<environment>/SNAPSHOT/<hash>' does not exist."
  (the create/get/delete happy path and DeletionStatus=='DELETED' all passed; only the post-delete read's message diverged)

Cause 4 — check slo-duplicate-name-conflict-rejected, line 43:
  expected message == "A resource with the same identifier already exists"
  got      "Slo with arn 'arn:aws:application-signals:us-west-2:000000000000:slo/<name>' already exists."
  (code assertion `code == "ConflictException"` on line 42 passed)

Cause 5 — check slo-incompatible-sli-type-switch-rejected, line 45:
  expected message == "Updating from request-based SLO to period-based SLO is not allowed"
  got      "You cannot change from a period-based SLO to a request-based SLO, or change from a request-based SLO to a period-based SLO."
  (code assertion `code == "ValidationException"` passed; the post-refusal state assertions — EvaluationType still 'RequestBased', RequestBasedSli retained — were never reached)

Cause 6 — check slo-missing-sli-config-validation-rejected, line 23:
  expected message == "Either SliConfig or RequestBasedSliConfig must be populated"
  got      "You must specify either SliConfig or RequestBasedSliConfig."
  (code assertion `code == "ValidationException"` on line 21 passed)

Cause 7 — check tag-resource-nonexistent-arn-rejected, line 38:
  expected message == "Resource with arn 'arn:aws:application-signals:us-west-2:000000000000:slo/sim2real-does-not-exist-<name>' does not exist."
  got      "Slo with arn 'arn:aws:application-signals:us-west-2:000000000000:slo/sim2real-does-not-exist-<name>' does not exist."
  (identical but for the leading type word: AWS says `Resource`, the emulator says `Slo`; code assertion `ResourceNotFoundException` passed)

Model citations used above (services/service-com-amazonaws-applicationsignals-applicationsignals/smithy-model/com-amazonaws-applicationsignals-applicationsignals.json):
  CreateServiceLevelObjective POST /slo — errors AccessDeniedException(403), ConflictException(409), ServiceQuotaExceededException(402), ThrottlingException(429), ValidationException(400); SliConfig and RequestBasedSliConfig both optional.
  UpdateServiceLevelObjective PATCH /slo/{Id} — errors ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400).
  TagResource POST /tag-resource — ResourceArn: AmazonResourceName required length 1..1024; errors ResourceNotFoundException(404), ServiceQuotaExceededException(402), ThrottlingException(429).
  GetInstrumentationConfiguration POST /get-instrumentation-configuration — errors ResourceNotFoundException(404), ThrottlingException, ValidationException.
  DeleteInstrumentationConfiguration POST /delete-instrumentation-configuration — errors ResourceNotFoundException(404), ThrottlingException, ValidationException.
  BatchDeleteInstrumentationConfigurations POST /batch-delete-instrumentation-configurations — output Errors: BatchDeleteErrorList (per-item Code/Message/ResourceArn); operation models only ThrottlingException(429) and ValidationException(400), so a missing ARN is per-item.

## Steps to Reproduce
Boot the emulator and point an `application-signals` client (botocore) at it.

Cause 1: create two instrumentation configurations (InstrumentationType=PROBE, SignalType=SNAPSHOT, same Service/Environment, different Location.CodeLocation.MethodName). Call BatchDeleteInstrumentationConfigurations with DeletionTarget.ResourceArns.ResourceArns = [the first config's ARN, that ARN with its trailing location-hash segment replaced by a hash that was never minted], InstrumentationType=PROBE. DeletedCount==1 and SuccessfulDeletions are correct; inspect Errors[<bogus ARN>].Message — it is the ARN-echoing template instead of "The instrumentation config is already deleted or expired."

Cause 2: create one instrumentation configuration, DeleteInstrumentationConfiguration it (DeletionStatus=='DELETED'), then call DeleteInstrumentationConfiguration again with the same LocationIdentifier.LocationHash. The 404 ResourceNotFoundException's Message is the ARN template instead of "The requested instrumentation configuration does not exist."

Cause 3: create one instrumentation configuration, delete it, then GetInstrumentationConfiguration with the same LocationIdentifier.LocationHash. The 404 ResourceNotFoundException's Message is the ARN template instead of "The requested instrumentation configuration has been deleted."

Cause 4: CreateServiceLevelObjective(Name=N, RequestBasedSliConfig=<any valid request-based config>), then CreateServiceLevelObjective again with the same Name=N and any config. The 409 ConflictException's Message is "Slo with arn '<arn>' already exists." instead of "A resource with the same identifier already exists".

Cause 5: CreateServiceLevelObjective(Name=N, RequestBasedSliConfig=...), then UpdateServiceLevelObjective(Id=<its Arn>, SliConfig=<a period-based config>). The 400 ValidationException's Message is the direction-agnostic "You cannot change from a period-based SLO to a request-based SLO, or change from a request-based SLO to a period-based SLO." instead of "Updating from request-based SLO to period-based SLO is not allowed". Repeat in the mirror direction (period-based SLO updated with RequestBasedSliConfig) — AWS words that one "Updating from period-based SLO to request-based SLO is not allowed"; the emulator returns the same single sentence.

Cause 6: CreateServiceLevelObjective(Name=N) with neither SliConfig nor RequestBasedSliConfig. The 400 ValidationException's Message is "You must specify either SliConfig or RequestBasedSliConfig." instead of "Either SliConfig or RequestBasedSliConfig must be populated".

Cause 7: CreateServiceLevelObjective(Name=N, RequestBasedSliConfig=...) to get a real ARN, then TagResource(ResourceArn=<that ARN with its resource name replaced by one that does not exist>, Tags=[{Key,Value}]). The 404 ResourceNotFoundException's Message begins "Slo with arn '..." instead of "Resource with arn '..."; the ARN echoed after it is correct.
