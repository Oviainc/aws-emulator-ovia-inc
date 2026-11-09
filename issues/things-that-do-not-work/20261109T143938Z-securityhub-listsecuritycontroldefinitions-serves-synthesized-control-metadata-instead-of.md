Issue-Source: trace-sweep-divergence
Issue-Trace: securityhub-catalog-list-security-control-definitions
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securityhub-catalog-list-security-control-definitions
Issue-Shape-Id: com.amazonaws.securityhub#SecurityHubAPIService
Issue-Key: securityhub|ListSecurityControlDefinitions|2xx->2xx|$.SecurityControlDefinitions[*].RemediationUrl
Issue-Related: 20261108T003636Z-securityhub-listsecuritycontroldefinitions-catalog-entries-diverge-from-aws-content.md
Issue-Commit: 58511c6cfb9f29f34cba9f5bd79b625dce957a70

# securityhub ListSecurityControlDefinitions serves synthesized control metadata instead of the AWS catalog values

> _Note: this issue was identified against the code base at commit `58511c6cfb9f29f34cba9f5bd79b625dce957a70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSecurityControlDefinitions

## Description
Single cause (Cause 1 of 1): the emulator's built-in Security Hub security-control catalog is a synthesized data table, so GET /securityControls/definitions answers 200 with a correctly framed envelope whose every SecurityControlDefinition element carries generated metadata rather than the catalog content AWS stores for that control. Every returned index of the list ($.SecurityControlDefinitions[0] .. [709]) compares unequal against the recording.

AWS: Smithy: ListSecurityControlDefinitions, GET /securityControls/definitions -> 200, output ListSecurityControlDefinitionsResponse.SecurityControlDefinitions (required list of SecurityControlDefinition) + NextToken; paginated {inputToken: NextToken, outputToken: NextToken, items: SecurityControlDefinitions, pageSize: MaxResults}; modeled errors InternalException(500), InvalidAccessException(401), InvalidInputException(400), LimitExceededException(429). Smithy: SecurityControlDefinition members SecurityControlId/Title/Description/RemediationUrl/SeverityRating/CurrentRegionAvailability are @required, CustomizableProperties and ParameterDefinitions are optional, and ParameterDefinitions is documented as "An object that provides a security control parameter name, description, and the options for customizing it. This object is excluded for a control that doesn't support custom parameters" — i.e. the member MUST be omitted for a control with no customizable parameters, it is not a member every entry carries. Smithy: RemediationUrl is documented as "A link to Security Hub CSPM documentation that explains how to remediate a failed finding for a security control" — a per-control documentation link, not a value derivable from the control id. The model does not fix the catalog's data, so the per-control field VALUES are recorded behavior only: the recording shows AWS returning, per control, that control's full published documentation text in Description and a RemediationUrl pointing at the documentation page for that control's own service/feature.

Emulator: for the same request the emulator answers 200 with a list of the same length and the same SecurityControlId/Title/SeverityRating/CurrentRegionAvailability/CustomizableProperties/Provider values, but three per-element fields are machine-generated, so no element matches:
  1. RemediationUrl is produced from a fixed template — the literal text "https://docs.aws.amazon.com/console/securityhub/", then the entry's SecurityControlId, then the literal text "/remediation" — rather than being read from the control's catalog row. The template is the emulator's invention; AWS's value is a per-control documentation URL that in general points at the evaluated service's own user guide, not at a securityhub console path.
  2. Description is an abbreviated paraphrase of the control's documentation text rather than the full published text, so it is shorter than and textually different from AWS's for every control.
  3. ParameterDefinitions is emitted on entries where the recorded AWS response carries no such member, so the serialized element SHAPE differs too and not merely its values.
Status, error surface and list framing are already correct — expected 200, actual 200; the whole divergence is in the body.

Why: a client that drives remediation, reporting or drift comparison from this catalog is misled in three ways. It renders and follows a RemediationUrl that is not the control's documented remediation page (so remediation runbooks and console deep links point at a URL that does not describe the control), it shows truncated control descriptions in compliance reports, and parameter-aware logic sees a customizable-parameter surface AWS did not report for that control — so code that branches on "does this control expose parameters, and which keys/ranges" takes a path it would never take against AWS, including attempting UpdateSecurityControl-style parameter writes for parameters the control does not have. Because every element diverges, the catalog cannot be used for golden-file comparison against AWS at all.

Fix: the securityhub ListSecurityControlDefinitions handler's static security-control catalog — the data table the handler serializes its SecurityControlDefinition elements from. Description and RemediationUrl must be stored per control as the control's real catalog values instead of being derived from a template or paraphrased, and ParameterDefinitions must be present only on the controls that actually define customizable parameters and omitted otherwise, as the member's own model documentation requires. This is not a serializer-framing or pagination defect: the HTTP binding, status, envelope member name and list length are already right.

Traces: securityhub-catalog-list-security-control-definitions#task#1. The replay cluster key is service=securityhub|ListSecurityControlDefinitions|status=200->200 with mismatching keys $.SecurityControlDefinitions[0] through $.SecurityControlDefinitions[709] — every element — plus the explicit $.SecurityControlDefinitions[N].RemediationUrl sub-key flagged on the leading entries. Elements [0] (ACM.1) and [1] (ACM.2) are the two the replay detail spells out in full.

Related open issue, which does NOT cover this step: 20261108T003636Z-securityhub-listsecuritycontroldefinitions-catalog-entries-diverge-from-aws-content.md describes the same catalog area, but the replay reports this cluster as unowned (owned_by empty), so this fixture step is not gated by it and the divergence still reproduces at the replayed commit. Also distinct from 20261029T180317Z-securityhub-mutations-are-not-reflected-by-later-reads.md (mutations invisible to later reads, DescribeActionTargets/accounts), 20261108T103252Z-securityhub-getsecuritycontroldefinition-stub.md (unimplemented GetSecurityControlDefinition), 20261103T202458Z-securityhub-batchdisablestandards-stub.md, 20261109T123922Z-securityhub-describesecurityhubv2-stub.md and 20261109T132823Z-securityhub-describehub-invalidaccessexception-message-diverges-from-aws-wording.md (DescribeHub error wording).

## Evidence
Fixture securityhub-catalog-list-security-control-definitions, step task#1, operation ListSecurityControlDefinitions, service securityhub. Verdict: mismatch. Expected status 200, actual status 200 — the status line and the error surface are correct; the body diverges.

Cluster key: service=securityhub|ListSecurityControlDefinitions|status=200->200|keys=$.SecurityControlDefinitions[0],$.SecurityControlDefinitions[0].RemediationUrl, ... ,$.SecurityControlDefinitions[709] (every index of the list, 710 elements).

$.SecurityControlDefinitions[0] EXPECTED (recorded AWS):
  {'SecurityControlId': 'ACM.1', 'Title': 'Imported and ACM-issued certificates should be renewed after a specified time period', 'Description': "This control checks whether an AWS Certificate Manager (ACM) certificate is renewed within the specified time period. It checks both imported certificates and certificates provided by ACM. The control fails if the certificate isn't renewed within the specified time period. Unless you provide a custom parameter value for the renewal period, Security Hub uses a default value of 30 days.", 'RemediationUrl': 'https://docs.aws.amazon.com/acm/latest/userguide/email-renewal-validation.html', 'SeverityRating': 'MEDIUM', 'CurrentRegionAvailability': 'AVAILABLE', 'CustomizableProperties': ['Parameters'], 'Provider': 'AWS'}
  — note: NO ParameterDefinitions member.

$.SecurityControlDefinitions[0] ACTUAL (emulator):
  {'SecurityControlId': 'ACM.1', 'Title': 'Imported and ACM-issued certificates should be renewed after a specified time period', 'Description': 'This control checks whether an AWS Certificate Manager (ACM) certificate is renewed within the specified time period. The control fails if the certificate expires within 30 days.', 'RemediationUrl': 'https://docs.aws.amazon.com/console/securityhub/ACM.1/remediation', 'SeverityRating': 'MEDIUM', 'CurrentRegionAvailability': 'AVAILABLE', 'CustomizableProperties': ['Parameters'], 'ParameterDefinitions': {'daysToExpiration': {'Description': 'The number of days before expiration within which an ACM certificate must be renewed', 'ConfigurationOptions': {'Integer': {'DefaultValue': 30, 'Min': 14, 'Max': 365}}}}, 'Provider': 'AWS'}

Classification of each difference — which part is a requirement and which part is a per-control value:
  * RemediationUrl: the FIXED text in the emulator's value is 'https://docs.aws.amazon.com/console/securityhub/' + '/remediation' with the request/catalog value SecurityControlId substituted between them; that template itself is the defect. The requirement is that the field carry the control's own stored documentation link. 'https://docs.aws.amazon.com/acm/latest/userguide/email-renewal-validation.html' is ACM.1's catalog value, an example — it is not a literal to hard-code for other controls.
  * Description: the requirement is the control's full published documentation text (fixed text per control, no variable parts); the emulator's shortened paraphrase is the defect. The two ACM.1 strings above are that control's values, not a template.
  * ParameterDefinitions: the requirement is structural and model-stated — present only for controls that support custom parameters, excluded otherwise. The emulator emits it where the recording does not. The daysToExpiration/14/30/365 numbers are ACM.1-specific catalog values, not part of the requirement.

$.SecurityControlDefinitions[1] (ACM.2) diverges identically: expected Description 'This control checks whether RSA certificates managed by AWS Certificate Manager use a key length of at least 2,048 bits. The control fails if the key length is smaller than 2,048 bits.' with its own per-control RemediationUrl, against the emulator's templated URL and paraphrased text. All 710 indices mismatch the same way, which is why the replay reports a single cluster for the step.

No host, port, timestamp, request id or other per-run value is part of this finding — the diverging fields are static catalog content.

## Steps to Reproduce
1. Clone the repo and boot the emulator from this checkout (the data listener plus /__admin/health must be up).
2. Replay fixture securityhub-catalog-list-security-control-definitions, step task#1: issue ListSecurityControlDefinitions as GET /securityControls/definitions against the securityhub endpoint with no StandardsArn, Providers, MaxResults or NextToken filter.
3. The response is 200 with a SecurityControlDefinitions list of 710 elements — the expected length.
4. Compare the list element-by-element against the fixture's recorded response. Element 0 (SecurityControlId 'ACM.1') shows all three divergences at once: RemediationUrl is 'https://docs.aws.amazon.com/console/securityhub/ACM.1/remediation' (the templated value) instead of the recorded per-control documentation URL, Description is the shortened paraphrase instead of the control's full published text, and an extra ParameterDefinitions map is present where the recording has none.
5. Element 1 ('ACM.2') and every other index 0..709 diverge the same way, so replay_wire_corpus() reports one mismatch cluster with key service=securityhub|ListSecurityControlDefinitions|status=200->200 for this step.
6. Re-replay the same fixture/step after the fix: the whole SecurityControlDefinitions list must match the recording, with no ParameterDefinitions member on entries that the recording omits it for.
