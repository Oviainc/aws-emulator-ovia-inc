Issue-Source: trace-sweep-divergence
Issue-Trace: pca-connector-ad-template-group-access-control-entry-lifecycle
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pca-connector-ad-template-group-access-control-entry-lifecycle pca-connector-ad-template-lifecycle
Issue-Shape-Id: com.amazonaws.pcaconnectorad#PcaConnectorAd
Issue-Key: pca-connector-ad|CreateTemplateGroupAccessControlEntry|4xx->4xx|$.message
Issue-Key: pca-connector-ad|GetTemplateGroupAccessControlEntry|4xx->4xx|$.message
Issue-Key: pca-connector-ad|ListTemplateGroupAccessControlEntries|4xx->4xx|$.message
Issue-Key: pca-connector-ad|UpdateTemplateGroupAccessControlEntry|4xx->4xx|$.message
Issue-Key: pca-connector-ad|DeleteTemplateGroupAccessControlEntry|4xx->4xx|$.message
Issue-Key: pca-connector-ad|GetTemplate|4xx->4xx|$.message
Issue-Key: pca-connector-ad|UpdateTemplate|4xx->4xx|$.message
Issue-Key: pca-connector-ad|DeleteTemplate|4xx->4xx|$.message
Issue-Commit: c2fc8676d63317349fbaa2ea39a789b7f43b5131

# templateArn ValidationException quotes the pattern with its ^ and $ anchors

> _Note: this issue was identified against the code base at commit `c2fc8676d63317349fbaa2ea39a789b7f43b5131`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTemplate

## Description
Eight net-new wire causes, one shared fault, on the template-ARN member: every operation that rejects a malformed templateArn with the "1 validation error detected" ValidationException echoes the member's @pattern exactly as the Smithy model declares it, anchors included, where AWS advertises the pattern with the leading `^` and trailing `$` removed. Status (400), code (ValidationException), count prefix, member path and rejected value are correct in all eight; the message differs by exactly those two characters. (Filed separately from the connector/directory-registration ARN bundle only because a bundle carries at most ten causes; the underlying renderer is the same one.)

Shared AWS requirement: HTTP 400, code ValidationException (Smithy: ValidationException @httpError 400 on each of these operations, Message @required), message `1 validation error detected: Value '<rejected value>' at 'templateArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[\w-]+:pca-connector-ad:[\w-]+:[0-9]+:connector\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}\/template\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}`. Fixed text: `1 validation error detected: Value '`, `' at '`, `' failed to satisfy constraint: Member must satisfy regular expression pattern: `. Variables: the rejected value and member path from the request, and the modeled pattern minus its anchors (Smithy: TemplateArn @httpLabel @required @length 5..200 @pattern `^arn:[\w-]+:pca-connector-ad:[\w-]+:[0-9]+:connector\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}\/template\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$`). The anchor-stripping is recorded behavior only.

Shared emulator behavior: the message renderer interpolates the compiled schema's pattern verbatim, producing `... pattern: ^arn:...\/template\/...$`.

Shared Why: clients that compare the refusal text or surface the advertised pattern get a string AWS never emits for a templateArn @pattern violation.

Shared Fix: the anchor-stripping step of the @pattern-constraint message renderer in the pca-connector-ad validation layer, evaluated before `handle`.

Cause 1 of 8: CreateTemplateGroupAccessControlEntry(TemplateArn='not-a-valid-arn'). Traces: fixture pca-connector-ad-template-group-access-control-entry-lifecycle, step task#2.
Cause 2 of 8: GetTemplateGroupAccessControlEntry, same member. Traces: same fixture, step task#3.
Cause 3 of 8: ListTemplateGroupAccessControlEntries, same member. Traces: same fixture, step task#4.
Cause 4 of 8: UpdateTemplateGroupAccessControlEntry, same member. Traces: same fixture, step task#5.
Cause 5 of 8: DeleteTemplateGroupAccessControlEntry, same member. Traces: same fixture, step teardown#6.
Cause 6 of 8: GetTemplate, same member. Traces: fixture pca-connector-ad-template-lifecycle, step task#3.
Cause 7 of 8: UpdateTemplate, same member. Traces: fixture pca-connector-ad-template-lifecycle, step task#4.
Cause 8 of 8: DeleteTemplate, same member. Traces: fixture pca-connector-ad-template-lifecycle, step teardown#5.

Related open issue (does NOT cover these wire steps): 20261029T190840Z-pca-connector-ad-diverges-on-seven-counts-validationexception.md reports the anchor fault behaviorally for CreateServicePrincipalName and ListConnectors.NextToken; it names none of the eight operations above and carries no wire replay key.

## Evidence
All eight steps: expected 400 == actual 400, code ValidationException in both, only $.message diverges.

Representative (pca-connector-ad-template-group-access-control-entry-lifecycle#task#2, CreateTemplateGroupAccessControlEntry):
  expected $.message: "1 validation error detected: Value 'not-a-valid-arn' at 'templateArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[\\w-]+:pca-connector-ad:[\\w-]+:[0-9]+:connector\\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}\\/template\\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}"
  actual   $.message: "... pattern: ^arn:[\\w-]+:pca-connector-ad:[\\w-]+:[0-9]+:connector\\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}\\/template\\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$"

Byte-identical expected/actual pairs for the other seven:
  …#task#3 GetTemplateGroupAccessControlEntry
  …#task#4 ListTemplateGroupAccessControlEntries
  …#task#5 UpdateTemplateGroupAccessControlEntry
  …#teardown#6 DeleteTemplateGroupAccessControlEntry
  pca-connector-ad-template-lifecycle#task#3 GetTemplate
  pca-connector-ad-template-lifecycle#task#4 UpdateTemplate
  pca-connector-ad-template-lifecycle#teardown#5 DeleteTemplate

## Steps to Reproduce
Boot the emulator and replay simparity-wire-aws-v1, or call it directly (region us-west-2, client-side validation disabled). For each of CreateTemplateGroupAccessControlEntry, GetTemplateGroupAccessControlEntry, ListTemplateGroupAccessControlEntries, UpdateTemplateGroupAccessControlEntry, DeleteTemplateGroupAccessControlEntry, GetTemplate, UpdateTemplate and DeleteTemplate, pass TemplateArn='not-a-valid-arn' (plus whatever other members the call requires). Each answers 400 ValidationException with the quoted pattern wrapped in ^ and $; AWS emits the same message with both anchors removed. Fixtures/steps: pca-connector-ad-template-group-access-control-entry-lifecycle#task#2..task#5 and #teardown#6; pca-connector-ad-template-lifecycle#task#3, #task#4, #teardown#5.
