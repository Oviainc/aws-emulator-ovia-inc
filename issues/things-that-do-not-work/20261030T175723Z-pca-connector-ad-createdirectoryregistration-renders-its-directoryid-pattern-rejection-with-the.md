Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pca-connector-ad.create-directory-registration-invalid-directory-id-pattern-rejected
Issue-Shape-Id: com.amazonaws.pcaconnectorad#PcaConnectorAd
Issue-Key: pca-connector-ad.create-directory-registration-invalid-directory-id-pattern-rejected|run#5|AssertionError|assert message == (
Issue-Commit: 96efc7e677bf514eee6c6e14a4fa1b11294b220d

# CreateDirectoryRegistration renders its DirectoryId @pattern rejection with the generic "1 validation error detected" template instead of the service's bracketed ECMA-262 wording

> _Note: this issue was identified against the code base at commit `96efc7e677bf514eee6c6e14a4fa1b11294b220d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDirectoryRegistration

## Description
One net-new root cause in the pca-connector-ad emulator, grounded in one failing behavioral check. Service model: services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/smithy-model/com-amazonaws-pcaconnectorad-pcaconnectorad.json.

Cause 1 of 1: CreateDirectoryRegistration reports a modeled @pattern violation on DirectoryId with the cross-service "1 validation error detected" template instead of this service's bracketed JSON-schema-validator wording. Failing check: pca-connector-ad.create-directory-registration-invalid-directory-id-pattern-rejected.

AWS: a DirectoryId that fails the modeled pattern is refused with HTTP 400 and error code ValidationException, and the message is the bracketed ECMA-262 form this service uses for body-member constraint failures:
`[ECMA 262 regex "<the pattern exactly as the model declares it, anchors included>" does not match input string "<the rejected value taken from the request>"]`.
Fixed text: `[ECMA 262 regex `, ` does not match input string `, the trailing `]`, and the double-quoting around both variables. Variable parts: the @pattern string (from the model) and the rejected DirectoryId (from the request). This family carries NO member path and NO error-count prefix. Nothing must be created: the directory-registration collection is unchanged and ListDirectoryRegistrations still answers normally.
Smithy: CreateDirectoryRegistration POST /directoryRegistrations, success 202; input member DirectoryId is @required with @pattern `^d-[0-9a-f]{10}$`; the operation models ValidationException with @httpError 400. The choice of message family (bracketed ECMA-262 rather than the generic template) is recorded behavior only.

Emulator: the status (400) and the error code (ValidationException) are correct, and no registration is created, but the message body is rendered by the generic cross-service validation formatter: `1 validation error detected: Value '<value>' at 'directoryId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^d-[0-9a-f]{10}$`. So the refusal text loses the ECMA-262 framing and gains a member path and an error-count prefix AWS never sends for this operation.

Why: a client that matches on, logs, or surfaces the rejection text (error-classification middleware, user-facing diagnostics, regression assertions) receives a message real AWS never emits for CreateDirectoryRegistration, and loses the bracketed regex framing AWS uses to report body-member constraint failures across this service.

Fix: the pca-connector-ad validation layer's ValidationException message renderer must cover CreateDirectoryRegistration's body members as well, emitting the bracketed ECMA-262 form for @pattern failures rather than falling through to the generic cross-service template. This is the same renderer, reached from a different operation's input validation; the operation handler and its state path behave correctly otherwise.

Related open issues (neither covers this operation, and the replay report did not mark this check as owned by either): 20261029T190840Z-pca-connector-ad-diverges-on-seven-counts-validationexception.md (its Cause 1 names only CreateConnector and CreateTemplateGroupAccessControlEntry) and 20261029T224504Z-pca-connector-ad-constraint-rejections-use-the-generic.md (a wire issue naming only CreateConnector.DirectoryId and CreateTemplate.Name). CreateDirectoryRegistration appears in neither, so this check is ungated by both.

Traces: pca-connector-ad.create-directory-registration-invalid-directory-id-pattern-rejected.

## Evidence
Suite: 1/15 passed; pca-connector-ad 1/15 (7%), 14 failed — 13 of them marked OWNED by 20261029T190840Z-pca-connector-ad-diverges-on-seven-counts-validationexception.md and skipped; this is the one surviving failure.

Check pca-connector-ad.create-directory-registration-invalid-directory-id-pattern-rejected:
  call: CreateDirectoryRegistration(DirectoryId="totally-not-a-directory-id")
  expected code: ValidationException  -> actual code: ValidationException (correct)
  expected message: [ECMA 262 regex "^d-[0-9a-f]{10}$" does not match input string "totally-not-a-directory-id"]
  actual message:   1 validation error detected: Value 'totally-not-a-directory-id' at 'directoryId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^d-[0-9a-f]{10}$
  AssertionError raised at the `assert message == (...)` comparison in the check.
  The surrounding assertions passed: the pre/post ListDirectoryRegistrations counts were equal (no registration created) and ListDirectoryRegistrations still returned its "DirectoryRegistrations" collection key, so only the message body diverges.

Message anatomy, for the fix: fixed text is `[ECMA 262 regex `, ` does not match input string `, `]` plus the double-quotes; the variables are the modeled pattern `^d-[0-9a-f]{10}$` (from CreateDirectoryRegistration.DirectoryId's @pattern) and the request's DirectoryId value. The recorded value "totally-not-a-directory-id" is one run's input filling the template, not part of it.

## Steps to Reproduce
Boot the emulator and point a botocore client at it (region us-west-2), or replay the behavioral suite check pca-connector-ad.create-directory-registration-invalid-directory-id-pattern-rejected.

1. Record `len(client.list_directory_registrations()["DirectoryRegistrations"])`.
2. Call `client.create_directory_registration(DirectoryId="totally-not-a-directory-id")` (any value failing `^d-[0-9a-f]{10}$`).
3. Observe HTTP 400 with code ValidationException (correct) but message `1 validation error detected: Value 'totally-not-a-directory-id' at 'directoryId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^d-[0-9a-f]{10}$`. AWS answers `[ECMA 262 regex "^d-[0-9a-f]{10}$" does not match input string "totally-not-a-directory-id"]`.
4. Re-read ListDirectoryRegistrations: the count is unchanged and the collection key is present, which is correct — the only divergence is the message text.
