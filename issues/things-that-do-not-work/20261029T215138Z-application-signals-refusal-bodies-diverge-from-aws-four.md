Issue-Source: trace-sweep-divergence
Issue-Trace: application-signals-instrumentation-config-batch-delete
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: application-signals-instrumentation-config-batch-delete application-signals-instrumentation-config-errors application-signals-slo-error-paths
Issue-Shape-Id: com.amazonaws.applicationsignals#ApplicationSignals
Issue-Key: application-signals|GetInstrumentationConfiguration|4xx->4xx|$.Message
Issue-Key: application-signals|GetInstrumentationConfiguration|4xx->4xx|$.ResourceId
Issue-Key: application-signals|GetInstrumentationConfiguration|4xx->4xx|$.ResourceType
Issue-Key: application-signals|CreateInstrumentationConfiguration|4xx->4xx|$.Message
Issue-Key: application-signals|DeleteInstrumentationConfiguration|4xx->4xx|$.Message
Issue-Key: application-signals|DeleteInstrumentationConfiguration|4xx->4xx|$.ResourceId
Issue-Key: application-signals|DeleteInstrumentationConfiguration|4xx->4xx|$.ResourceType
Issue-Key: application-signals|UpdateServiceLevelObjective|4xx->4xx|$.Message
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# application-signals refusal bodies diverge from AWS: four emulator-authored messages and a ResourceNotFoundException carrying members AWS omits

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInstrumentationConfiguration

## Description
Four wire-fixture clusters show application-signals returning the CORRECT HTTP status and error code but a refusal BODY that is not AWS's: the `Message` is prose the emulator authors (an ARN-echoing not-found/already-exists template minted by the crate's generic resource helpers) instead of the sentence AWS words for that error class, and on the instrumentation-configuration 404 paths the body additionally carries `ResourceId`/`ResourceType` members the recorded AWS body does not send. This issue bundles FOUR causes; all four must be closed.

Adjacent OPEN issue, which does NOT cover these: 20261029T192739Z-application-signals-words-seven-refusal-messages-differently-from.md records the same wording defect for Causes 1 and 2 below from the BEHAVIORAL suite. It carries no wire replay key, so the fixture steps named here are not re-replayed by it, and it says nothing about the extra `ResourceId`/`ResourceType` members (Causes 1 and 2) or about Causes 3 and 4 at all.

--- Cause 1 of 4: GetInstrumentationConfiguration after-delete 404 body ---
AWS: Smithy: GetInstrumentationConfiguration POST /get-instrumentation-configuration, errors ResourceNotFoundException (httpError 404), ThrottlingException (429), ValidationException (400). Recorded behavior only for the body: reading a configuration that was deleted is refused 404 ResourceNotFoundException whose body carries ONLY `Message`, with the value `The requested instrumentation configuration has been deleted.` — that whole sentence is FIXED text with NO variable parts (it names no ARN, service, environment, signal type or location hash). The recorded body carries NO `ResourceId` and NO `ResourceType`, even though the model marks both `@required` on ResourceNotFoundException; the recording is the wire truth for what this refusal serializes.
Emulator: 404 ResourceNotFoundException (status and code correct) with `Message` = the crate's generic ARN-echoing template `InstrumentationConfiguration with arn '<config-arn>' does not exist.`, plus `ResourceId` = that same ARN and `ResourceType` = `InstrumentationConfiguration`.
Why: AWS distinguishes the read path's "has been deleted" from the delete path's "does not exist" (Cause 2) with two different fixed sentences; collapsing both into one ARN template destroys that distinction, and a client that logs or matches the message never matches. The two extra members also leak a resource ARN into a body AWS keeps ARN-free, and make the error envelope a shape clients did not record.
Fix: the not-found branch of the GetInstrumentationConfiguration handler (the branch that resolves a tombstoned/deleted record) and the shared instrumentation-configuration not-found constructor it borrows — it must word this operation's own fixed sentence and serialize Message only.
Traces: application-signals-instrumentation-config-batch-delete#task#4 (also task#5 of the same fixture).

--- Cause 2 of 4: DeleteInstrumentationConfiguration unknown-location 404 body ---
AWS: Smithy: DeleteInstrumentationConfiguration POST /delete-instrumentation-configuration, errors ResourceNotFoundException (404), ThrottlingException (429), ValidationException (400). Recorded behavior only for the body: deleting a configuration that does not exist (here, a LocationIdentifier.LocationHash that was never minted) is refused 404 ResourceNotFoundException whose body carries ONLY `Message` = `The requested instrumentation configuration does not exist.` — entirely FIXED text, no variable parts, no identifier of any kind; again no `ResourceId` and no `ResourceType`.
Emulator: 404 ResourceNotFoundException (status and code correct) with the same generic ARN template `InstrumentationConfiguration with arn '<config-arn>' does not exist.` plus `ResourceId`/`ResourceType`.
Why: an idempotent-delete wrapper has nothing to key on when the delete path and the read path return the same sentence, and the sentence it does get is one AWS never sends.
Fix: the not-found branch of the DeleteInstrumentationConfiguration handler, same shared constructor as Cause 1 — the two operations need distinct fixed sentences and a Message-only body.
Traces: application-signals-instrumentation-config-errors#task#4.

--- Cause 3 of 4: CreateInstrumentationConfiguration duplicate-location 409 message ---
AWS: Smithy: CreateInstrumentationConfiguration POST /create-instrumentation-configuration, errors ConflictException (httpError 409), ServiceQuotaExceededException (402), ThrottlingException (429), ValidationException (400); ConflictException models a single `Message` member. Recorded behavior only for the message: a second create at the same Service/Environment/SignalType/Location is refused 409 ConflictException with `Message` = `An instrumentation configuration already exists for this service, environment, signalType and location.` — entirely FIXED text, no variable parts: AWS names the four identity dimensions generically and does NOT echo the ARN, the service name or the location hash.
Emulator: 409 ConflictException (status and code correct) with `Instrumentation configuration with arn '<config-arn>' already exists.` — the crate's generic already-exists template, which interpolates the computed ARN.
Why: the client is told which resource conflicted instead of WHICH identity dimensions make the conflict, which is the actionable part of AWS's sentence; anything comparing or templating this message diverges, and the body discloses an ARN the caller never named.
Fix: the duplicate-location branch of the CreateInstrumentationConfiguration handler / the instrumentation-configuration store's uniqueness check, which must word its own conflict instead of reusing the generic ARN already-exists helper.
Traces: application-signals-instrumentation-config-errors#task#2.

--- Cause 4 of 4: UpdateServiceLevelObjective unknown-SLO 404 message ---
AWS: Smithy: UpdateServiceLevelObjective PATCH /slo/{Id}, errors ResourceNotFoundException (404), ThrottlingException (429), ValidationException (400); `Id` admits either a bare SLO name or a full `slo/<name>` ARN. Recorded behavior only for the message: updating an SLO that does not exist is refused 404 ResourceNotFoundException with a message of the form `Work item with id: <account-id>:<slo-name>:<work-item-discriminator> and namespace: slo does not exist`. The FIXED text is `Work item with id: ` ... ` and namespace: slo does not exist` (no trailing period); the variable parts are the 12-digit ACCOUNT id of the caller, the SLO NAME taken from the request's `Id` (the name half, not the whole ARN), and a server-minted 12-digit work-item discriminator, in that order, colon-separated. Hypothesis: the discriminator is an internal work-item id and cannot be derived from the request — a fixer should treat only the account id and the SLO name as request-derived and must not assert the recording's literal digits.
Emulator: 404 ResourceNotFoundException (status and code correct) with `Slo with arn 'arn:aws:application-signals:<region>:<account>:slo/<slo-name>' does not exist.` — the SLO store's generic ARN not-found template.
Why: this operation's refusal is worded by AWS's work-item layer, not by its SLO-resource layer, so a client comparing the message (or scraping it for the namespace) against AWS's diverges; the emulator also reports an ARN-shaped identity for a request that may have named a bare SLO name.
Fix: the SLO-resolution failure path of the UpdateServiceLevelObjective handler — it must produce this operation's work-item wording instead of delegating to the shared `slo_not_found` helper (the status and code it already returns are correct).
Traces: application-signals-slo-error-paths#task#6.

## Evidence
Wire replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 62 steps, 48 passed, 14 mismatched, score 77.4%, emulator_up=true, bootstrap_signal=false. The four clusters below are mismatches (status correct in Causes 1-4, body wrong); none carries a stub_path or an owning issue.

Cause 1 — fixture application-signals-instrumentation-config-batch-delete, step task#4 (GetInstrumentationConfiguration), expected 404 / actual 404:
  $.Message: value_mismatch (expected 'The requested instrumentation configuration has been deleted.', got "InstrumentationConfiguration with arn 'arn:aws:application-signals:us-west-2:000000000000:instrumentationConfig/<service>/<environment>/SNAPSHOT/<hash>' does not exist.")
  $.ResourceId: type_mismatch (expected None, got the same ARN)
  $.ResourceType: type_mismatch (expected None, got 'InstrumentationConfiguration')
  cluster also covers application-signals-instrumentation-config-batch-delete#task#5. (ARNs shown are one run's values; the defect is the template and the member set, not the values.)

Cause 2 — fixture application-signals-instrumentation-config-errors, step task#4 (DeleteInstrumentationConfiguration), expected 404 / actual 404:
  $.Message: value_mismatch (expected 'The requested instrumentation configuration does not exist.', got "InstrumentationConfiguration with arn '...<never-minted-hash>' does not exist.")
  $.ResourceId / $.ResourceType: type_mismatch (expected None, got the ARN / 'InstrumentationConfiguration')

Cause 3 — fixture application-signals-instrumentation-config-errors, step task#2 (CreateInstrumentationConfiguration), expected 409 / actual 409:
  $.Message: value_mismatch (expected 'An instrumentation configuration already exists for this service, environment, signalType and location.', got "Instrumentation configuration with arn '<config-arn>' already exists.")

Cause 4 — fixture application-signals-slo-error-paths, step task#6 (UpdateServiceLevelObjective), expected 404 / actual 404:
  $.Message: value_mismatch (expected 'Work item with id: <account>:<slo-name>:<12-digit discriminator> and namespace: slo does not exist', got "Slo with arn 'arn:aws:application-signals:us-west-2:<account>:slo/<slo-name>' does not exist.")

Model citations (services/service-com-amazonaws-applicationsignals-applicationsignals/smithy-model/com-amazonaws-applicationsignals-applicationsignals.json):
  ResourceNotFoundException — members ResourceType, ResourceId, Message, all @required; httpError 404. The recorded bodies carry Message only.
  ConflictException — single @required Message; httpError 409.
  GetInstrumentationConfiguration / DeleteInstrumentationConfiguration POST /get-|/delete-instrumentation-configuration — ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400).
  CreateInstrumentationConfiguration POST /create-instrumentation-configuration — ConflictException(409), ServiceQuotaExceededException(402), ThrottlingException(429), ValidationException(400).
  UpdateServiceLevelObjective PATCH /slo/{Id} — ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400); Id pattern admits bare name or slo/<name> ARN.

## Steps to Reproduce
Boot the emulator and point an application-signals client at its data listener.

Cause 1: CreateInstrumentationConfiguration(InstrumentationType=PROBE, SignalType=SNAPSHOT, Service=S, Environment=E, Location.CodeLocation=L, CaptureConfiguration=...), DeleteInstrumentationConfiguration for the same identity, then GetInstrumentationConfiguration with the same LocationIdentifier. The 404 body is the ARN template plus ResourceId/ResourceType instead of Message-only 'The requested instrumentation configuration has been deleted.'

Cause 2: DeleteInstrumentationConfiguration(InstrumentationType=PROBE, SignalType=SNAPSHOT, Service=S, Environment=E, LocationIdentifier.LocationHash=<any 16 hex chars never minted>). The 404 body is the ARN template plus ResourceId/ResourceType instead of Message-only 'The requested instrumentation configuration does not exist.'

Cause 3: CreateInstrumentationConfiguration twice with identical Service/Environment/SignalType/Location. The second call's 409 Message is the ARN already-exists template instead of 'An instrumentation configuration already exists for this service, environment, signalType and location.'

Cause 4: UpdateServiceLevelObjective(Id=<a name or slo/<name> ARN that names no stored SLO>, Description='x'). The 404 Message is "Slo with arn '...' does not exist." instead of the 'Work item with id: <account>:<name>:<discriminator> and namespace: slo does not exist' form.

Or replay the fixture steps named under Traces for each cause.
