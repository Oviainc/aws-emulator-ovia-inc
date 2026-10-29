Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pca-connector-ad.create-connector-empty-security-group-list-rejected pca-connector-ad.create-connector-invalid-directory-id-pattern-rejected pca-connector-ad.create-template-group-access-control-entry-invalid-sid-pattern-rejected pca-connector-ad.create-template-group-access-control-entry-invalid-access-rights-enum-rejected pca-connector-ad.create-service-principal-name-arn-type-mismatch-rejected pca-connector-ad.list-connectors-malformed-next-token-rejected pca-connector-ad.create-connector-invalid-certificate-authority-arn-pattern-rejected pca-connector-ad.create-connector-missing-iam-permission-rejected pca-connector-ad.create-template-invalid-name-characters-rejected pca-connector-ad.get-connector-not-found pca-connector-ad.delete-connector-not-found pca-connector-ad.create-template-nonexistent-connector-not-found pca-connector-ad.untag-resource-malformed-resource-arn-rejected
Issue-Shape-Id: com.amazonaws.pcaconnectorad#PcaConnectorAd
Issue-Key: pca-connector-ad.create-connector-empty-security-group-list-rejected|run#8|AssertionError|assert message == (
Issue-Key: pca-connector-ad.create-connector-invalid-directory-id-pattern-rejected|run#7|AssertionError|assert message == (
Issue-Key: pca-connector-ad.create-template-group-access-control-entry-invalid-sid-pattern-rejected|run#7|AssertionError|assert message == (
Issue-Key: pca-connector-ad.create-template-group-access-control-entry-invalid-access-rights-enum-rejected|run#7|AssertionError|assert message == (
Issue-Key: pca-connector-ad.create-service-principal-name-arn-type-mismatch-rejected|run#7|AssertionError|assert message == (
Issue-Key: pca-connector-ad.list-connectors-malformed-next-token-rejected|run#4|AssertionError|assert message == (
Issue-Key: pca-connector-ad.create-connector-invalid-certificate-authority-arn-pattern-rejected|_error#7|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded: {result!r}")
Issue-Key: pca-connector-ad.create-connector-missing-iam-permission-rejected|_error#20|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded: {result!r}")
Issue-Key: pca-connector-ad.create-template-invalid-name-characters-rejected|run#6|AssertionError|assert code == "ValidationException", (
Issue-Key: pca-connector-ad.get-connector-not-found|run#6|AssertionError|assert message == f"Connector was not found: {connector_arn}.", message
Issue-Key: pca-connector-ad.delete-connector-not-found|run#12|AssertionError|assert message == f"Connector was not found: {connector_arn}.", message
Issue-Key: pca-connector-ad.create-template-nonexistent-connector-not-found|run#7|AssertionError|assert message == f"Connector was not found: {connector_arn}.", message
Issue-Key: pca-connector-ad.untag-resource-malformed-resource-arn-rejected|run#4|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Commit: 69bb2c4e0c47afe7ac06c67651974f0cf752730a

# pca-connector-ad diverges on seven counts: ValidationException message rendering (two formats), CreateConnector accepts unregistered directories and unauthorized callers, CreateTemplate Name pattern unenforced, not-found message wording, UntagResource is a 501 stub

> _Note: this issue was identified against the code base at commit `69bb2c4e0c47afe7ac06c67651974f0cf752730a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnector

## Description
Seven distinct root causes in the pca-connector-ad emulator, all grounded in failing behavioral checks. Service model: services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/smithy-model/com-amazonaws-pcaconnectorad-pcaconnectorad.json. (No open pca-connector-ad issue exists; the only sibling issues are pca-connector-scep ones, which do not cover any of these.) The separate ALREADY-FILED ListDirectoryRegistrations stub issue is NOT part of this bundle.

=== Cause 1 of 7: ValidationException messages for CreateConnector and CreateTemplateGroupAccessControlEntry use the generic "1 validation error detected" template instead of this service's bracketed JSON-schema wording ===
AWS: these two operations report modeled-constraint violations with a bracketed JSON-schema-validator message, HTTP 400, error code ValidationException (Smithy: CreateConnector and CreateTemplateGroupAccessControlEntry both declare ValidationException @httpError 400, documented as "An input validation error occurred. For example, invalid characters in a template name, or if a pagination token is invalid"). The message templates, with their fixed text and their request-derived variable parts, are:
  * @pattern violation -> `[ECMA 262 regex "<the pattern as the model declares it, anchors included>" does not match input string "<the rejected value from the request>"]`. Fixed text: `[ECMA 262 regex `, ` does not match input string `, `]` and the quoting. Variables: the pattern (from the model) and the input string (from the request).
  * @length min violation on a list -> `[array is too short: must have at least <min> elements but instance has <actual> elements]`. Fixed text: `[array is too short: must have at least `, ` elements but instance has `, ` elements]`. Variables: the modeled minimum and the actual element count from the request.
  * enum violation -> `[instance value ("<rejected value>") not found in enum (possible values: ["<v1>","<v2>",...])]`. Fixed text: `[instance value (`, `) not found in enum (possible values: [`, `])]`. Variables: the rejected value from the request and the enum's modeled values in model order.
Note the wording carries NO member path and NO "1 validation error detected" prefix.
Emulator: the central validate step emits the standard cross-service template instead, e.g. `1 validation error detected: Value '0 items' at 'vpcInformation.securityGroupIds' failed to satisfy constraint: Member must have length greater than or equal to 1`; `1 validation error detected: Value 'not-a-valid-directory-id' at 'directoryId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^d-[0-9a-f]{10}$`; `1 validation error detected: Value 'not-a-valid-sid' at 'groupSecurityIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: ^S-[0-9]-([0-9]+-){1,14}[0-9]+$`; `1 validation error detected: Value 'MAYBE' at 'accessRights.enroll' failed to satisfy constraint: Member must satisfy enum value set: [ALLOW, DENY]`. The code (ValidationException) and status are correct; only the message body diverges.
Why: a client that parses or matches on the rejection text (error-classification middleware, user-facing diagnostics, regression assertions) sees a message real AWS never sends for these operations, and loses the regex/enum/bound detail AWS puts in the bracketed form.
Fix: the per-service ValidationException message renderer for pca-connector-ad's constraint violations (the service's validation layer that formats modeled @pattern/@length/enum failures for CreateConnector and CreateTemplateGroupAccessControlEntry), not the generic cross-service template.
Traces: pca-connector-ad.create-connector-empty-security-group-list-rejected, pca-connector-ad.create-connector-invalid-directory-id-pattern-rejected, pca-connector-ad.create-template-group-access-control-entry-invalid-sid-pattern-rejected, pca-connector-ad.create-template-group-access-control-entry-invalid-access-rights-enum-rejected.

=== Cause 2 of 7: where the "1 validation error detected" wording IS correct, the quoted regex keeps the model's ^ and $ anchors, which AWS strips ===
AWS: CreateServicePrincipalName and ListConnectors reject a @pattern violation with HTTP 400 ValidationException and the message `1 validation error detected: Value '<rejected value>' at '<member path, lowerCamel>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern WITHOUT the leading ^ and trailing $>`. Fixed text: `1 validation error detected: Value '`, `' at '`, `' failed to satisfy constraint: Member must satisfy regular expression pattern: `. Variables: the rejected value and member path from the request, and the modeled pattern with its anchors removed. Smithy: CreateServicePrincipalName.DirectoryRegistrationArn @pattern `^arn:[\w-]+:pca-connector-ad:[\w-]+:[0-9]+:directory-registration\/d-[0-9a-f]{10}$` -> reported as `arn:[\w-]+:pca-connector-ad:[\w-]+:[0-9]+:directory-registration\/d-[0-9a-f]{10}`; ListConnectors.NextToken @pattern `^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-]{2}==|[A-Za-z0-9_-]{3}=)?$` -> reported as `(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-]{2}==|[A-Za-z0-9_-]{3}=)?`.
Emulator: emits the pattern verbatim from the model, anchors included — `... pattern: ^arn:[\w-]+:pca-connector-ad:[\w-]+:[0-9]+:directory-registration\/d-[0-9a-f]{10}$` and `... pattern: ^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-]{2}==|[A-Za-z0-9_-]{3}=)?$`. Code and status are right; the message differs by exactly the two anchor characters.
Why: any client comparing the rejection message (or extracting the advertised pattern to show a user) gets a string AWS never emits.
Fix: the anchor-stripping step of the pattern-constraint message formatter in the service's validation layer.
Traces: pca-connector-ad.create-service-principal-name-arn-type-mismatch-rejected, pca-connector-ad.list-connectors-malformed-next-token-rejected.

=== Cause 3 of 7: CreateConnector creates a connector for a DirectoryId that has no DirectoryRegistration, instead of refusing it ===
AWS: CreateConnector requires the named Active Directory to be registered; with a pattern-valid but unregistered DirectoryId it refuses with HTTP 404 ResourceNotFoundException and the message `DirectoryRegistration was not found: <DirectoryId as supplied in the request>.` (fixed text `DirectoryRegistration was not found: ` plus the request's DirectoryId plus a trailing period), and NO connector is created. Smithy: CreateConnector models ResourceNotFoundException @httpError 404, documented "The operation tried to access a nonexistent resource."; CreateConnector.DirectoryId is @required. The specific message text is recorded behavior only.
Emulator: accepts the call and returns HTTP 202 with a freshly minted ConnectorArn, e.g. `{'ConnectorArn': 'arn:aws:pca-connector-ad:<region>:<account>:connector/<uuid>'}`, and the connector is then visible in ListConnectors. services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/src/ops/create_connector.rs:166-243 — the handler goes straight from idempotency/quota checks to inserting a ConnectorRecord; nothing consults the directory-registration store at all.
Why: callers that depend on CreateConnector validating its directory see a phantom connector for a directory that does not exist, and never receive the 404 their error handling is written against; worse, the over-permissive accept silently mutates state (a connector is persisted) where AWS mutates nothing.
Fix: CreateConnector's handler (services/.../src/ops/create_connector.rs) must resolve the DirectoryId against the directory-registration state before minting a connector and return the modeled ResourceNotFoundException when it is absent.
Traces: pca-connector-ad.create-connector-invalid-certificate-authority-arn-pattern-rejected (its positive control, a pattern-valid CA ARN with unregistered DirectoryId `d-1234567890`, succeeded with 202 instead of 404). The same unregistered-directory refusal is the control expectation of pca-connector-ad.create-connector-invalid-directory-id-pattern-rejected and pca-connector-ad.create-connector-missing-iam-permission-rejected.

=== Cause 4 of 7: CreateConnector performs no authorization check, so a principal whose policy denies pca-connector-ad:CreateConnector still creates a connector ===
AWS: a caller assuming a role whose policy does not allow pca-connector-ad:CreateConnector is refused with HTTP 403 AccessDeniedException and the message `User: <caller principal ARN> is not authorized to perform: pca-connector-ad:CreateConnector on resource: <resource ARN>` (fixed text `User: `, ` is not authorized to perform: `, ` on resource: `; the principal ARN comes from the assumed-role session and the action/resource from the request), and no connector is created. Smithy: CreateConnector models AccessDeniedException @httpError 403, documented "You can receive this error if you attempt to create a resource share when you don't have the required permissions...". The exact denial sentence is recorded behavior only.
Emulator: the restricted client's CreateConnector returns HTTP 202 with a ConnectorArn (`arn:aws:pca-connector-ad:<region>:<account>:connector/<uuid>`); no AccessDeniedException is ever produced. create_connector.rs never inspects the request's principal or any policy.
Why: a client or test that verifies least-privilege behavior (that a scoped-down role cannot create connectors) passes against the emulator while the equivalent call on AWS is denied, so permission regressions go undetected and the emulator persists state AWS would refuse to create.
Fix: an authorization gate for CreateConnector ahead of its state mutation (the service's authorization middleware / the handler's pre-mutation checks in src/ops/create_connector.rs), returning the modeled AccessDeniedException.
Traces: pca-connector-ad.create-connector-missing-iam-permission-rejected.

=== Cause 5 of 7: CreateTemplate does not enforce the Name @pattern, so a malformed Name is never rejected ===
AWS: CreateTemplate rejects a Name containing a disallowed character (comma, and likewise backslash, apostrophe, plus, semicolon, <, =, >, #, double-quote, or an all-whitespace name) with HTTP 400 ValidationException BEFORE resolving the connector, with the bracketed message of Cause 1: `[ECMA 262 regex "^(?!^\s+$)((?![\x5c'\x2b,;<=>#\x22])([\x20-\x7E]))+$" does not match input string "<the rejected Name>"]`. Smithy: CreateTemplate.Name -> TemplateName, @required, @length 1..64, @pattern `^(?!^\s+$)((?![\x5c'\x2b,;<=>#\x22])([\x20-\x7E]))+$`; CreateTemplate models ValidationException @httpError 400. Input validation precedes the parent lookup: AWS answers ValidationException even when the ConnectorArn names a nonexistent connector.
Emulator: the malformed Name passes validation entirely; the request reaches the handler, which resolves the parent first and answers ResourceNotFoundException (`Connector <arn> not found`) for the nonexistent connector — so the Name violation is never reported. Observed: expected code ValidationException, got 'ResourceNotFoundException'. See services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/src/ops/create_template.rs:190-201 (parent resolved first) and the scaffold's own contract that modeled constraints are refused before `handle` runs — for this member they are not. Hypothesis: this @pattern uses negative lookahead `(?!...)`, which the Rust `regex` crate does not support, so the compiled-constraint path drops the pattern silently instead of enforcing it.
Why: a client gets a 404 about an unrelated resource (or, with a real connector, a template created under a name AWS forbids) instead of the 400 that tells it which input was bad; template names that AWS rejects become persistable, and the rejection-ordering contract (validation before resource lookup) is inverted.
Fix: CreateTemplate's input validation for the Name member (the service's constraint-compilation/validation layer, with a lookahead-capable check or an equivalent explicit character test), evaluated before the connector lookup in src/ops/create_template.rs.
Traces: pca-connector-ad.create-template-invalid-name-characters-rejected (and the control arm of pca-connector-ad.create-template-nonexistent-connector-not-found asserts the same ValidationException for `bad,name`).

=== Cause 6 of 7: ResourceNotFoundException message wording is inverted — `Connector <arn> not found` instead of `Connector was not found: <arn>.` ===
AWS: a not-found refusal from this service words its message `<ResourceType> was not found: <identifier as supplied in the request>.` — fixed text `<Type> was not found: ` and a trailing period, with the identifier taken from the request. Observed resource types in this suite: `Connector was not found: <ConnectorArn>.`, `Template was not found: <TemplateArn>.`, `DirectoryRegistration was not found: <DirectoryId or DirectoryRegistrationArn>.` Status 404, code ResourceNotFoundException (Smithy: ResourceNotFoundException @httpError 404 on GetConnector, DeleteConnector, CreateTemplate and the rest). The message text is recorded behavior only.
Emulator: emits `Connector <arn> not found` — the subject/predicate order is reversed, there is no colon, and the trailing period is missing. See services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/src/ops/get_connector.rs:169 and services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/src/ops/create_template.rs:195 (`format!("Connector {} not found", ...)`). Status and error code are correct in all three traces.
Why: every client that surfaces or matches the not-found message (log assertions, user-facing text, error-mapping tables) sees text AWS never emits, for the single most common failure of these operations.
Fix: the ResourceNotFoundException message construction in each affected operation handler (get_connector.rs, create_template.rs, and the other handlers that build this error), normalized to the service's `<Type> was not found: <id>.` wording.
Traces: pca-connector-ad.get-connector-not-found, pca-connector-ad.delete-connector-not-found (the tolerant double-delete and the subsequent 404 code are correct; only the GetConnector message diverges), pca-connector-ad.create-template-nonexistent-connector-not-found.

=== Cause 7 of 7: UntagResource is an unimplemented scaffold stub, so tag removal and its ARN validation do not exist ===
AWS: UntagResource removes the named tag keys and answers HTTP 204. A syntactically malformed ResourceArn is refused with error code BadRequestException and the message `Invalid input resource arn: bad syntax` (entirely fixed text), and a well-formed ARN naming a nonexistent resource is refused with NotFoundException and the message `Resource was not found` (also fixed text). Smithy: UntagResource DELETE /tags/{ResourceArn}, success 204, ResourceArn @required @httpLabel, TagKeys @required @httpQuery `tagKeys`; the model declares AccessDenied/InternalServer/ResourceNotFound/Throttling/ValidationException only — the BadRequestException/NotFoundException codes and both messages are recorded behavior only.
Emulator: every UntagResource call answers HTTP 501 with error code `NotImplemented` and the message `operation UntagResource is scaffolded but not implemented in this emulator`. services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/src/ops/untag_resource.rs:190 returns `crate::scaffolded::Scaffolded::not_implemented("UntagResource")`, and the file still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker.
Why: no client can remove a tag from any pca-connector-ad resource, and a 501 `NotImplemented` is not a modeled outcome — it is retried or treated as a server fault by SDK error handling rather than as the client-side refusal AWS returns.
Fix: implement the handler in services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/src/ops/untag_resource.rs (remove the keys from the addressed resource's tag map in src/state.rs, answer 204, refuse a malformed ARN and an unknown resource as above) and delete the SCAFFOLDED marker line.
Traces: pca-connector-ad.untag-resource-malformed-resource-arn-rejected.

## Evidence
Suite: 1/15 passed; pca-connector-ad 1/15 (7%), 14 failed (13 filed here, 1 skipped as ALREADY FILED for the ListDirectoryRegistrations stub).

Cause 1 — generic validation template instead of bracketed wording (code/status correct, message wrong):
* create-connector-empty-security-group-list-rejected (VpcInformation.SecurityGroupIds=[], client-side validation disabled)
  expected: `[array is too short: must have at least 1 elements but instance has 0 elements]`
  actual:   `1 validation error detected: Value '0 items' at 'vpcInformation.securityGroupIds' failed to satisfy constraint: Member must have length greater than or equal to 1`
* create-connector-invalid-directory-id-pattern-rejected (DirectoryId="not-a-valid-directory-id")
  expected: `[ECMA 262 regex "^d-[0-9a-f]{10}$" does not match input string "not-a-valid-directory-id"]`
  actual:   `1 validation error detected: Value 'not-a-valid-directory-id' at 'directoryId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^d-[0-9a-f]{10}$`
* create-template-group-access-control-entry-invalid-sid-pattern-rejected (GroupSecurityIdentifier="not-a-valid-sid")
  expected: `[ECMA 262 regex "^S-[0-9]-([0-9]+-){1,14}[0-9]+$" does not match input string "not-a-valid-sid"]`
  actual:   `1 validation error detected: Value 'not-a-valid-sid' at 'groupSecurityIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: ^S-[0-9]-([0-9]+-){1,14}[0-9]+$`
* create-template-group-access-control-entry-invalid-access-rights-enum-rejected (AccessRights.Enroll="MAYBE")
  expected: `[instance value ("MAYBE") not found in enum (possible values: ["ALLOW","DENY"])]`
  actual:   `1 validation error detected: Value 'MAYBE' at 'accessRights.enroll' failed to satisfy constraint: Member must satisfy enum value set: [ALLOW, DENY]`

Cause 2 — anchors not stripped from the advertised pattern:
* create-service-principal-name-arn-type-mismatch-rejected (DirectoryRegistrationArn given a connector ARN)
  expected tail: `... regular expression pattern: arn:[\w-]+:pca-connector-ad:[\w-]+:[0-9]+:directory-registration\/d-[0-9a-f]{10}`
  actual tail:   `... regular expression pattern: ^arn:[\w-]+:pca-connector-ad:[\w-]+:[0-9]+:directory-registration\/d-[0-9a-f]{10}$`
* list-connectors-malformed-next-token-rejected (NextToken="this-is-not-a-real-pagination-token")
  expected tail: `... regular expression pattern: (?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-]{2}==|[A-Za-z0-9_-]{3}=)?`
  actual tail:   `... regular expression pattern: ^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-]{2}==|[A-Za-z0-9_-]{3}=)?$`

Cause 3 — unregistered directory accepted:
* create-connector-invalid-certificate-authority-arn-pattern-rejected
  expected: HTTP 404, ResourceNotFoundException, `DirectoryRegistration was not found: d-1234567890.`, no connector created
  actual:   HTTP 202, body `{'ConnectorArn': 'arn:aws:pca-connector-ad:us-west-2:<account>:connector/<uuid>'}` (AssertionError: "expected a rejection, but the call succeeded"); the connector is persisted and counted by ListConnectors
  code: src/ops/create_connector.rs:166-243 inserts a ConnectorRecord with no directory-registration lookup

Cause 4 — no authorization gate:
* create-connector-missing-iam-permission-rejected (role assumed with a policy denying pca-connector-ad:CreateConnector; retried for up to the check's propagation window)
  expected: HTTP 403, AccessDeniedException, `User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: pca-connector-ad:CreateConnector on resource: <resource arn>`
  actual:   HTTP 202, body `{'ConnectorArn': 'arn:aws:pca-connector-ad:us-west-2:<account>:connector/<uuid>'}`

Cause 5 — Name pattern unenforced:
* create-template-invalid-name-characters-rejected (Name="bad,name", ConnectorArn a well-formed nonexistent connector)
  expected: ValidationException with `[ECMA 262 regex "^(?!^\s+$)((?![\x5c'\x2b,;<=>#\x22])([\x20-\x7E]))+$" does not match input string "bad,name"]`
  actual:   `ResourceNotFoundException` (AssertionError: "expected ValidationException for a Name containing a disallowed comma, got 'ResourceNotFoundException'")

Cause 6 — not-found message wording:
* get-connector-not-found / delete-connector-not-found / create-template-nonexistent-connector-not-found
  expected: `Connector was not found: arn:aws:pca-connector-ad:us-west-2:<account>:connector/<uuid>.`
  actual:   `Connector arn:aws:pca-connector-ad:us-west-2:<account>:connector/<uuid> not found`
  (same code ResourceNotFoundException and status 404 in all three; delete-connector-not-found's tolerant 202 double-delete assertions passed, the failure is only the message on the following GetConnector)
  code: src/ops/get_connector.rs:169, src/ops/create_template.rs:195

Cause 7 — UntagResource stub:
* untag-resource-malformed-resource-arn-rejected (ResourceArn="definitely-not-an-arn", TagKeys=["k"])
  expected: BadRequestException, `Invalid input resource arn: bad syntax` (and NotFoundException / `Resource was not found` for a well-formed unknown ARN)
  actual:   `NotImplemented` — "operation UntagResource is scaffolded but not implemented in this emulator" (HTTP 501)
  code: src/ops/untag_resource.rs:190, file still marked `SCAFFOLDED, NOT IMPLEMENTED`

## Steps to Reproduce
Boot the emulator and point a botocore client at it (region us-west-2). All 13 checks below are in the behavioral suite and can be replayed directly; the trace_ids list is exactly the set a fix must re-run.

Cause 1:
1. With `Config(parameter_validation=False)`, call CreateConnector(DirectoryId="d-1234567890", CertificateAuthorityArn=<pattern-valid acm-pca CA ARN>, VpcInformation={"SecurityGroupIds": []}) -> ValidationException, but the message is the generic "1 validation error detected..." instead of `[array is too short: must have at least 1 elements but instance has 0 elements]`.
2. Call CreateConnector(DirectoryId="not-a-valid-directory-id", ...) -> message is the generic template instead of `[ECMA 262 regex "^d-[0-9a-f]{10}$" does not match input string "not-a-valid-directory-id"]`.
3. Call CreateTemplateGroupAccessControlEntry(TemplateArn=<well-formed template ARN>, GroupSecurityIdentifier="not-a-valid-sid", GroupDisplayName=<any>, AccessRights={"Enroll":"ALLOW","AutoEnroll":"ALLOW"}) -> generic template instead of the bracketed ECMA-262 message.
4. Same call with AccessRights.Enroll="MAYBE" (client-side validation disabled) -> generic template instead of `[instance value ("MAYBE") not found in enum (possible values: ["ALLOW","DENY"])]`.

Cause 2:
5. Call CreateServicePrincipalName(DirectoryRegistrationArn=<a connector ARN>, ConnectorArn=<same connector ARN>) -> message quotes the pattern with ^ and $; AWS strips both.
6. Call ListConnectors(NextToken="this-is-not-a-real-pagination-token") -> same anchor divergence.

Cause 3:
7. Note len(ListConnectors()["Connectors"]), then call CreateConnector(DirectoryId="d-1234567890" (never registered), CertificateAuthorityArn=<pattern-valid acm-pca CA ARN>, VpcInformation={"SecurityGroupIds":["sg-0123456789abcdef0"]}). It returns HTTP 202 with a ConnectorArn and ListConnectors grows by one; AWS returns 404 ResourceNotFoundException `DirectoryRegistration was not found: d-1234567890.` and creates nothing.

Cause 4:
8. Create an IAM role with a policy that does NOT allow pca-connector-ad:CreateConnector, assume it via STS, and repeat the step-7 CreateConnector with those credentials. It returns HTTP 202 with a ConnectorArn; AWS returns 403 AccessDeniedException.

Cause 5:
9. Call CreateTemplate(ConnectorArn=<well-formed but nonexistent connector ARN>, Name="bad,name", Definition={"TemplateV2": {...}}) -> ResourceNotFoundException; AWS returns ValidationException for the Name before looking the connector up.

Cause 6:
10. Call GetConnector(ConnectorArn=<well-formed nonexistent connector ARN>) -> 404 ResourceNotFoundException with message `Connector <arn> not found` instead of `Connector was not found: <arn>.`
11. Call DeleteConnector twice on that ARN (both tolerantly return 202, correct), then GetConnector again -> same message divergence.
12. Call CreateTemplate against that nonexistent ConnectorArn with a clean Name -> same message divergence.

Cause 7:
13. Call UntagResource(ResourceArn="definitely-not-an-arn", TagKeys=["k"]) -> HTTP 501 `NotImplemented`; AWS returns BadRequestException `Invalid input resource arn: bad syntax`. Repeat with a well-formed ARN for a nonexistent resource -> AWS returns NotFoundException `Resource was not found`.
