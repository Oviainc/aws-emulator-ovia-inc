Issue-Source: trace-sweep-divergence
Issue-Trace: datazone-create-validation-account-env
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datazone-create-validation-account-env datazone-create-validation-project-family datazone-create-validation-subscription-group datazone-domain-scoped-denied-sweep-4
Issue-Shape-Id: com.amazonaws.datazone#DataZone
Issue-Key: datazone|CreateAccountPool|4xx->4xx|$.message
Issue-Key: datazone|CreateEnvironmentProfile|4xx->4xx|$.message
Issue-Key: datazone|CreateAssetFilter|4xx->4xx|$.message
Issue-Key: datazone|CreateProjectProfile|4xx->4xx|$.message
Issue-Key: datazone|CreateFormType|4xx->4xx|$.message
Issue-Key: datazone|CreateSubscriptionRequest|4xx->4xx|$.message
Issue-Key: datazone|CreateGroupProfile|4xx->4xx|$.message
Issue-Key: datazone|CreateProject|4xx->4xx|$.message
Issue-Key: datazone|CreateGlossary|4xx->4xx|$.message
Issue-Commit: 7b78f885090352093daa8ac758bd09ec0b97c628

# datazone error messages diverge from AWS wording in validation and not-found/denied responses

> _Note: this issue was identified against the code base at commit `7b78f885090352093daa8ac758bd09ec0b97c628`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAccountPool

## Description
NINE causes. The emulator returns the right HTTP status and the right x-amzn-errortype for these datazone failures, but the `message` member of the error body does not match the message AWS produces. Two systematic rules are broken by the constraint-violation messages: (a) AWS REDACTS the offending value for members marked @sensitive, emitting `Value at '<member>' failed to satisfy constraint: ...`, while emitting `Value '<value>' at '<member>' ...` only for non-sensitive members; (b) AWS renders the constraint's regular expression with its outer anchors normalised (outermost `^`/`$` stripped; a modelled pattern that is not anchored is wrapped as `.*(<pattern>).*`), not as the raw modelled pattern string. Two further causes are plain message-text divergences on not-found and access-denied paths. Fix all nine before submitting.

Cause 1 of 9: CreateAccountPool, name violating its pattern.
AWS: Smithy CreateAccountPoolInput.name -> AccountPoolName, @required, @length(min 1,max 64), @pattern "^[\\w -]+$", @sensitive; error ValidationException (httpError 400, message @required). Because the member is @sensitive the value is redacted and the pattern is reported de-anchored: 400 ValidationException with message `1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w -]+`. Fixed text: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: "; variable parts: the member name from the request's offending member and the pattern from the model.
Emulator: 400 ValidationException, but message `1 validation error detected: Value 'bad name with spaces !!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\w -]+$` — it interpolates the sensitive value and keeps the anchors.
Why: a client (or a log scrubber) relying on AWS not echoing @sensitive input sees the sensitive value leak into the error body, and assertions/parsers keyed on AWS's message text do not match.
Fix: the shared request-validation/error-message formatter for datazone (sensitivity-aware value redaction + pattern de-anchoring), not this one handler.
Traces: datazone-create-validation-account-env#task#1.

Cause 2 of 9: CreateEnvironmentProfile, same sensitive-name pattern violation.
AWS: 400 ValidationException, message `1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w -]+` (name is @sensitive with pattern "^[\\w -]+$").
Emulator: same message but with the sensitive value interpolated and the anchors retained.
Why/Fix: as cause 1.
Traces: datazone-create-validation-account-env#task#2.

Cause 3 of 9: CreateAssetFilter, same sensitive-name pattern violation.
AWS: 400 ValidationException, message `1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w -]+`.
Emulator: value interpolated, anchors retained.
Why/Fix: as cause 1.
Traces: datazone-create-validation-account-env#task#3.

Cause 4 of 9: CreateProjectProfile, same sensitive-name pattern violation.
AWS: 400 ValidationException, message `1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w -]+`.
Emulator: value interpolated, anchors retained.
Why/Fix: as cause 1.
Traces: datazone-create-validation-project-family#task#2.

Cause 5 of 9: CreateFormType uses a completely different message template.
AWS: Smithy CreateFormTypeInput.name -> FormTypeName, @required, @length(1,128), @pattern "^(amazon.datazone.)?(?![0-9_])\\w+$|^_\\w*[a-zA-Z0-9]\\w*$", @sensitive; error ValidationException (400). Message uses the standard envelope with the value redacted and outer anchors stripped: `1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: (amazon.datazone.)?(?![0-9_])\w+$|^_\w*[a-zA-Z0-9]\w*`.
Emulator: 400 but an entirely non-AWS template — `name <value> does not satisfy the constraint: Member must satisfy regular expression pattern: ^(amazon.datazone.)?(?![0-9_])\w+$|^_\w*[a-zA-Z0-9]\w*$` — dropping the "1 validation error detected: Value at '<member>' failed to satisfy constraint:" envelope entirely and echoing the sensitive value.
Why: clients and SDK-level test assertions that match AWS's "N validation error(s) detected" envelope cannot recognise this error at all.
Fix: route this operation's pattern check through the shared validation-message formatter instead of a hand-written string.
Traces: datazone-create-validation-project-family#task#3.

Cause 6 of 9: CreateSubscriptionRequest length violation echoes the sensitive value.
AWS: Smithy CreateSubscriptionRequestInput.requestReason -> RequestReason, @required, @length(min 1, max 4096), @sensitive; error ValidationException (400). Message: `1 validation error detected: Value at 'requestReason' failed to satisfy constraint: Member must have length less than or equal to 4096` — fixed text with the member name and the modelled bound as the only variable parts, and NO value, because the member is @sensitive.
Emulator: 400, but the message embeds the oversized sensitive payload (truncated with a "... [N bytes]" marker) between "Value " and " at 'requestReason'".
Why: the sensitive request body is echoed back (and into client logs), and the message no longer matches AWS's.
Fix: sensitivity-aware value redaction in the length-constraint branch of the shared validation formatter.
Traces: datazone-create-validation-subscription-group#task#1.

Cause 7 of 9: CreateGroupProfile reports an un-normalised pattern.
AWS: Smithy CreateGroupProfileInput.groupIdentifier -> GroupIdentifier, @pattern "(^([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-...-[A-Fa-f0-9]{12}$|[\\p{L}\\p{M}\\p{S}\\p{N}\\p{P}\\t\\n\\r  ]+)" and NOT @sensitive, so the value IS shown: `1 validation error detected: Value '' at 'groupIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: .*(^([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12}$|[\p{L}\p{M}\p{S}\p{N}\p{P}\t\n\r  ]+).*`. The modelled pattern is unanchored, so AWS reports it wrapped in `.*(` ... `).*`; the empty-string value here is simply whatever the request sent.
Emulator: 400 with the same envelope and value but the raw modelled pattern, missing the `.*(`/`).*` wrapping.
Why: the reported constraint differs from the one AWS reports, so a client cannot derive the accepted input shape from the error.
Fix: pattern-normalisation step of the shared validation formatter (strip outer anchors when present, wrap when absent).
Traces: datazone-create-validation-subscription-group#task#2.

Cause 8 of 9: CreateProject not-found message omits the domain identifier.
AWS: Smithy CreateProject POST /v2/domains/{domainIdentifier}/projects, domainIdentifier @required @pattern "^dzd[-_][a-zA-Z0-9_-]{1,36}$"; error ResourceNotFoundException (httpError 404, doc "The specified resource cannot be found.", message @required). For a syntactically valid but nonexistent domain the recorded response is 404 with message `Domain <domainIdentifier> not found` — fixed text "Domain " ... " not found", with the identifier taken verbatim from the request path (the recording's dzd_bogus00000000001 is just this run's value).
Emulator: 404 ResourceNotFoundException with the constant message `Domain not found` — the request-derived identifier is dropped.
Why: the client cannot tell which domain of a multi-domain request was missing; message-matching clients diverge.
Fix: CreateProject's domain lookup error construction (format the identifier into the ResourceNotFoundException message).
Traces: datazone-create-validation-project-family#task#4.

Cause 9 of 9: CreateGlossary access-denied message is not AWS's.
AWS: Smithy CreateGlossary error AccessDeniedException (httpError 403, doc "You do not have sufficient access to perform this action."). For a caller not permitted on the operation the recorded message is `User is not permitted to perform operation: CreateGlossary` — fixed text "User is not permitted to perform operation: " plus the operation name.
Emulator: 403 AccessDeniedException (status and errortype correct) but message `You do not have access to domain <domainIdentifier>`, i.e. a domain-scoped sentence instead of AWS's operation-scoped one.
Why: clients that branch on AWS's denial wording (or surface it to users) get different text for every denied datazone call.
Fix: the datazone authorization layer's AccessDeniedException message construction, shared across operations.
Traces: datazone-domain-scoped-denied-sweep-4#task#6.

## Evidence
Expected vs actual per cause, from the replay report:
1. CreateAccountPool 400/400 — $.message expected "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w -]+", got "1 validation error detected: Value 'bad name with spaces !!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\w -]+$".
2. CreateEnvironmentProfile 400/400 — identical expected/actual message divergence.
3. CreateAssetFilter 400/400 — identical expected/actual message divergence.
4. CreateProjectProfile 400/400 — identical expected/actual message divergence.
5. CreateFormType 400/400 — expected "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: (amazon.datazone.)?(?![0-9_])\\w+$|^_\\w*[a-zA-Z0-9]\\w*", got "name bad name with spaces !! does not satisfy the constraint: Member must satisfy regular expression pattern: ^(amazon.datazone.)?(?![0-9_])\\w+$|^_\\w*[a-zA-Z0-9]\\w*$".
6. CreateSubscriptionRequest 400/400 — expected "1 validation error detected: Value at 'requestReason' failed to satisfy constraint: Member must have length less than or equal to 4096", got the same sentence with the 5000-byte sensitive value interpolated after "Value ".
7. CreateGroupProfile 400/400 — expected pattern text ".*(^([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-...$|[\\p{L}...]+).*", got the same pattern without the ".*(" / ").*" wrapping.
8. CreateProject 404/404 — $.message expected 'Domain dzd_bogus00000000001 not found', got 'Domain not found'.
9. CreateGlossary 403/403 — $.message expected 'User is not permitted to perform operation: CreateGlossary', got 'You do not have access to domain dzd_bogus00000000001'.
Status and x-amzn-errortype matched in all nine; only the message member diverged.

## Steps to Reproduce
Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator and inspect the error body of these steps:
1. datazone-create-validation-account-env, task#1 (CreateAccountPool with a name violating ^[\w -]+$).
2. datazone-create-validation-account-env, task#2 (CreateEnvironmentProfile, same).
3. datazone-create-validation-account-env, task#3 (CreateAssetFilter, same).
4. datazone-create-validation-project-family, task#2 (CreateProjectProfile, same).
5. datazone-create-validation-project-family, task#3 (CreateFormType with an invalid name).
6. datazone-create-validation-subscription-group, task#1 (CreateSubscriptionRequest with requestReason longer than 4096).
7. datazone-create-validation-subscription-group, task#2 (CreateGroupProfile with an empty groupIdentifier).
8. datazone-create-validation-project-family, task#4 (CreateProject against a nonexistent domain id).
9. datazone-domain-scoped-denied-sweep-4, task#6 (CreateGlossary as a caller that is not permitted).
Compare the `message` member against the recorded response in each case.
