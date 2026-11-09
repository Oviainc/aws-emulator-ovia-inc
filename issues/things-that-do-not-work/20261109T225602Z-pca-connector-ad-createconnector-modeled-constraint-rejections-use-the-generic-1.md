Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pca-connector-ad.create-connector-empty-security-group-list-rejected pca-connector-ad.create-connector-invalid-directory-id-pattern-rejected pca-connector-ad.create-connector-invalid-certificate-authority-arn-pattern-rejected
Issue-Shape-Id: com.amazonaws.pcaconnectorad#PcaConnectorAd
Issue-Key: pca-connector-ad.create-connector-empty-security-group-list-rejected|run#7|AssertionError|assert message == (
Issue-Key: pca-connector-ad.create-connector-invalid-directory-id-pattern-rejected|run#6|AssertionError|assert message == (
Issue-Key: pca-connector-ad.create-connector-invalid-certificate-authority-arn-pattern-rejected|_error#6|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded: {result!r}")
Issue-Commit: 6bb51c191bdb333bc331bcde4b456674734a5a8d

# CreateConnector: modeled-constraint rejections use the generic "1 validation error detected" template, and an unregistered DirectoryId is accepted (202 + persisted connector) instead of refused

> _Note: this issue was identified against the code base at commit `6bb51c191bdb333bc331bcde4b456674734a5a8d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnector

## Description
Two net-new root causes in the pca-connector-ad emulator's CreateConnector path, both grounded in failing behavioral checks of this replay. Service model: services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/smithy-model/com-amazonaws-pcaconnectorad-pcaconnectorad.json.

Related open issues (named here because they look similar but the suite did NOT mark these three checks as owned by them, so they are re-filed): 20261029T190840Z-pca-connector-ad-diverges-on-seven-counts-validationexception.md (its Causes 1 and 3 describe the same two behaviors), 20261029T224504Z-pca-connector-ad-constraint-rejections-use-the-generic.md, and 20261029T224504Z-pca-connector-ad-createconnector-skips-the-directoryregistration-prerequisite-and-persists-a.md. A fixer closing this issue should re-replay exactly the checks in trace_ids.

=== Cause 1 of 2: CreateConnector renders @length and @pattern violations with the cross-service "1 validation error detected" template instead of this service's bracketed JSON-schema wording ===
AWS: CreateConnector refuses a modeled-constraint violation with HTTP 400 and error code ValidationException (Smithy: CreateConnector declares ValidationException @httpError 400; CreateConnector.DirectoryId -> DirectoryId @required @pattern "^d-[0-9a-f]{10}$"; VpcInformation.SecurityGroupIds -> SecurityGroupIdList @required @length min 1 max 4 @uniqueItems). The message is the bracketed JSON-schema-validator form, with NO member path and NO "1 validation error detected" prefix (message text: recorded behavior only):
  * list @length min violation -> `[array is too short: must have at least <min> elements but instance has <actual> elements]`. Fixed text: `[array is too short: must have at least `, ` elements but instance has `, ` elements]`; variables: the modeled minimum (1 here) and the element count actually sent.
  * @pattern violation -> `[ECMA 262 regex "<the pattern exactly as the model declares it, anchors included>" does not match input string "<the rejected value from the request>"]`. Fixed text: `[ECMA 262 regex `, ` does not match input string `, `]` plus the quoting; variables: the modeled pattern and the request's value.
Emulator: the status (400) and code (ValidationException) are correct, only the message body diverges — it emits the generic cross-service template: for an empty SecurityGroupIds list `1 validation error detected: Value '0 items' at 'vpcInformation.securityGroupIds' failed to satisfy constraint: Member must have length greater than or equal to 1`, and for a malformed DirectoryId `1 validation error detected: Value 'not-a-valid-directory-id' at 'directoryId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^d-[0-9a-f]{10}$`.
Why: a client that matches on or surfaces the rejection text (error-classification middleware, user diagnostics, regression assertions) sees a message real AWS never sends for this operation, and loses the bracketed regex/bound detail AWS puts there.
Fix: the per-service ValidationException message renderer for pca-connector-ad's modeled-constraint failures (the service's input-validation layer that formats @pattern/@length violations for CreateConnector), rather than the generic cross-service template.
Traces: pca-connector-ad.create-connector-empty-security-group-list-rejected, pca-connector-ad.create-connector-invalid-directory-id-pattern-rejected.

=== Cause 2 of 2: CreateConnector mints and persists a connector for a DirectoryId that has no DirectoryRegistration, instead of refusing with ResourceNotFoundException ===
AWS: CreateConnector requires the named Active Directory to be registered first. With a pattern-valid CertificateAuthorityArn and a pattern-valid but unregistered DirectoryId, AWS refuses with HTTP 404, error code ResourceNotFoundException, and the message `DirectoryRegistration was not found: <DirectoryId exactly as supplied in the request>.` (fixed text `DirectoryRegistration was not found: ` plus the request's DirectoryId plus a trailing period), and creates nothing. Smithy: CreateConnector models ResourceNotFoundException @httpError 404 and DirectoryId is @required; the message text is recorded behavior only.
Emulator: the call succeeds — HTTP 202 with a freshly minted `ConnectorArn` of the form `arn:aws:pca-connector-ad:<region>:<account>:connector/<uuid>` — and the connector is then returned by ListConnectors, so state AWS never mutates is persisted. No ResourceNotFoundException is produced for the unregistered directory at all.
Why: clients relying on CreateConnector to validate its directory prerequisite never receive the modeled 404 their error handling is written against, and end up with a phantom connector attached to a directory that was never registered; a negative test that asserts the refusal passes against the emulator and fails against AWS.
Fix: CreateConnector's handler must resolve DirectoryId against the directory-registration state before minting/inserting the connector record, and return the modeled ResourceNotFoundException with the service's `<Type> was not found: <id>.` wording when it is absent.
Traces: pca-connector-ad.create-connector-invalid-certificate-authority-arn-pattern-rejected (its positive-control arm: a pattern-valid acm-pca CA ARN with unregistered DirectoryId succeeded with 202 instead of 404). The same unregistered-directory refusal is the control expectation of pca-connector-ad.create-connector-invalid-directory-id-pattern-rejected.

## Evidence
Suite: 1/15 passed; pca-connector-ad 1/15 (7%), 14 failed — 11 skipped as OWNED by already-open issues, the 3 below clustered here.

Cause 1 (status 400 and code ValidationException correct; message wrong):
* create-connector-empty-security-group-list-rejected — CreateConnector(DirectoryId="d-1234567890", CertificateAuthorityArn=<pattern-valid acm-pca CA ARN>, VpcInformation={"SecurityGroupIds": []}) with botocore Config(parameter_validation=False)
  expected message: `[array is too short: must have at least 1 elements but instance has 0 elements]`
  actual message:   `1 validation error detected: Value '0 items' at 'vpcInformation.securityGroupIds' failed to satisfy constraint: Member must have length greater than or equal to 1`
* create-connector-invalid-directory-id-pattern-rejected — CreateConnector(DirectoryId="not-a-valid-directory-id", ...)
  expected message: `[ECMA 262 regex "^d-[0-9a-f]{10}$" does not match input string "not-a-valid-directory-id"]`
  actual message:   `1 validation error detected: Value 'not-a-valid-directory-id' at 'directoryId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^d-[0-9a-f]{10}$`

Cause 2 (an over-permissive accept, with state mutation):
* create-connector-invalid-certificate-authority-arn-pattern-rejected — control arm CreateConnector(DirectoryId="d-1234567890" (never registered), CertificateAuthorityArn=<pattern-valid acm-pca CA ARN>, VpcInformation={"SecurityGroupIds":["sg-0123456789abcdef0"]}, Tags={...})
  expected: HTTP 404, ResourceNotFoundException, message `DirectoryRegistration was not found: d-1234567890.`, and no connector created
  actual:   HTTP 202, body `{'ConnectorArn': 'arn:aws:pca-connector-ad:<region>:<account>:connector/<uuid>'}` — harness reported `AssertionError: expected a rejection, but the call succeeded`; the connector is subsequently listed by ListConnectors (the generated ARN/uuid/account values differ per run and are not part of the requirement).

## Steps to Reproduce
Boot the emulator and point a botocore client at it (region us-west-2).

Cause 1:
1. With `Config(parameter_validation=False)`, call CreateConnector(DirectoryId="d-1234567890", CertificateAuthorityArn=<pattern-valid acm-pca CA ARN>, VpcInformation={"SecurityGroupIds": []}). The call is refused 400/ValidationException (correct) but the message is the generic "1 validation error detected: Value '0 items' at 'vpcInformation.securityGroupIds' ..." instead of `[array is too short: must have at least 1 elements but instance has 0 elements]`.
2. Call CreateConnector(DirectoryId="not-a-valid-directory-id", CertificateAuthorityArn=<pattern-valid acm-pca CA ARN>, VpcInformation={"SecurityGroupIds": ["sg-0123456789abcdef0"]}). Message is the generic template instead of `[ECMA 262 regex "^d-[0-9a-f]{10}$" does not match input string "not-a-valid-directory-id"]`.

Cause 2:
3. Record len(ListConnectors()["Connectors"]). Call CreateConnector(DirectoryId="d-1234567890" (no DirectoryRegistration ever created for it), CertificateAuthorityArn=<pattern-valid acm-pca CA ARN>, VpcInformation={"SecurityGroupIds":["sg-0123456789abcdef0"]}). It returns HTTP 202 with a ConnectorArn and ListConnectors grows by one; AWS returns HTTP 404 ResourceNotFoundException `DirectoryRegistration was not found: d-1234567890.` and creates nothing.
4. Re-run the two checks named in trace_ids for Cause 1 and the one for Cause 2 to confirm the fix.
