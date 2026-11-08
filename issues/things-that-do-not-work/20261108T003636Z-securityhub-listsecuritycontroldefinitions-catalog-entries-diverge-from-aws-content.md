Issue-Source: trace-sweep-divergence
Issue-Trace: securityhub-catalog-list-security-control-definitions
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securityhub-catalog-list-security-control-definitions
Issue-Shape-Id: com.amazonaws.securityhub#SecurityHubAPIService
Issue-Key: securityhub|ListSecurityControlDefinitions|2xx->2xx|$.SecurityControlDefinitions[*]
Issue-Commit: 4cf020d2c1273a702afbbf64c24c2520a1d80374

# securityhub ListSecurityControlDefinitions catalog entries diverge from AWS content

> _Note: this issue was identified against the code base at commit `4cf020d2c1273a702afbbf64c24c2520a1d80374`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSecurityControlDefinitions

## Description
Single cause (Cause 1 of 1): the emulator's built-in Security Hub security-control catalog serves control definitions whose per-control fields do not match what AWS returns, for EVERY entry of the returned list.

AWS: Smithy: ListSecurityControlDefinitions GET /securityControls/definitions -> ListSecurityControlDefinitionsResponse.SecurityControlDefinitions (required list of SecurityControlDefinition), paginated {inputToken: NextToken, outputToken: NextToken, items: SecurityControlDefinitions, pageSize: MaxResults}; errors InternalException(500), InvalidAccessException(401), InvalidInputException(400), LimitExceededException(429). The model does not fix the catalog's CONTENT, so for the per-control field values this is recorded behavior only. The recording shows that for a control, AWS returns the control's real documentation text in `Description` and a `RemediationUrl` that points at the control's own AWS documentation page for that service/feature; for the recorded ACM.1 entry the response carries SecurityControlId, Title, Description, RemediationUrl, SeverityRating, CurrentRegionAvailability, CustomizableProperties and Provider and carries NO `ParameterDefinitions` member.

Emulator: for the same request the emulator answers 200 with a list of the same length and the same SecurityControlIds/Titles, but every element's body is synthesized rather than carrying AWS's catalog content, so all 710 indices ($.SecurityControlDefinitions[0] .. [709]) compare unequal. Three systematic differences show in the recorded element:
  - `Description` is an abbreviated/paraphrased sentence instead of AWS's documentation text. For ACM.1 the emulator returns "This control checks whether an AWS Certificate Manager (ACM) certificate is renewed within the specified time period. The control fails if the certificate expires within 30 days." where AWS returns "This control checks whether an AWS Certificate Manager (ACM) certificate is renewed within the specified time period. It checks both imported certificates and certificates provided by ACM. The control fails if the certificate isn't renewed within the specified time period. Unless you provide a custom parameter value for the renewal period, Security Hub uses a default value of 30 days."
  - `RemediationUrl` is generated from a fixed template `https://docs.aws.amazon.com/console/securityhub/<SecurityControlId>/remediation` (fixed text, with the control id substituted) instead of the per-control documentation URL AWS stores for that control (recorded for ACM.1: https://docs.aws.amazon.com/acm/latest/userguide/email-renewal-validation.html). The template is the emulator's invention, not a per-control value.
  - the emulator emits an extra `ParameterDefinitions` map (for ACM.1: daysToExpiration with ConfigurationOptions.Integer DefaultValue/Min/Max) on an entry where the recorded AWS response has no such member, so the serialized shape of the element differs too.

Why: a client that drives remediation or reporting from this catalog is misled. It renders or follows a RemediationUrl that is not the control's documented remediation page, shows truncated control descriptions, and sees a parameter surface (ParameterDefinitions) that AWS did not report for that control — so parameter-aware logic (e.g. deciding a control is customizable and which parameter keys/ranges exist) takes a branch it would never take against AWS. Because the divergence covers every element, nothing in the catalog can be trusted for comparison or golden-file testing against AWS.

Fix: the securityhub ListSecurityControlDefinitions handler's static security-control catalog (the data table the handler serializes from) — the per-control Description and RemediationUrl must be the control's real catalog values rather than a derived template, and ParameterDefinitions must only be emitted for controls that actually carry one. Not a serializer-framing bug: the envelope, status and list length are already correct.

Traces: securityhub-catalog-list-security-control-definitions#task#1 (expected vs actual for $.SecurityControlDefinitions[0] = ACM.1 and [1] = ACM.2; all indices 0-709 mismatch in the same way).

## Evidence
Fixture securityhub-catalog-list-security-control-definitions, step task#1, operation ListSecurityControlDefinitions. Expected status 200, actual status 200 — the divergence is in the body, cluster key service=securityhub|ListSecurityControlDefinitions|status=200->200 with mismatching keys $.SecurityControlDefinitions[0] through $.SecurityControlDefinitions[709] (every element).

$.SecurityControlDefinitions[0] expected:
  {'SecurityControlId': 'ACM.1', 'Title': 'Imported and ACM-issued certificates should be renewed after a specified time period', 'Description': "This control checks whether an AWS Certificate Manager (ACM) certificate is renewed within the specified time period. It checks both imported certificates and certificates provided by ACM. The control fails if the certificate isn't renewed within the specified time period. Unless you provide a custom parameter value for the renewal period, Security Hub uses a default value of 30 days.", 'RemediationUrl': 'https://docs.aws.amazon.com/acm/latest/userguide/email-renewal-validation.html', 'SeverityRating': 'MEDIUM', 'CurrentRegionAvailability': 'AVAILABLE', 'CustomizableProperties': ['Parameters'], 'Provider': 'AWS'}

$.SecurityControlDefinitions[0] actual:
  {'SecurityControlId': 'ACM.1', 'Title': 'Imported and ACM-issued certificates should be renewed after a specified time period', 'Description': 'This control checks whether an AWS Certificate Manager (ACM) certificate is renewed within the specified time period. The control fails if the certificate expires within 30 days.', 'RemediationUrl': 'https://docs.aws.amazon.com/console/securityhub/ACM.1/remediation', 'SeverityRating': 'MEDIUM', 'CurrentRegionAvailability': 'AVAILABLE', 'CustomizableProperties': ['Parameters'], 'ParameterDefinitions': {'daysToExpiration': {'Description': 'The number of days before expiration within which an ACM certificate must be renewed', 'ConfigurationOptions': {'Integer': {'DefaultValue': 30, 'Min': 14, 'Max': 365}}}}, 'Provider': 'AWS'}

Differences, classified: RemediationUrl — emulator value is the fixed template 'https://docs.aws.amazon.com/console/securityhub/' + <SecurityControlId> + '/remediation'; AWS value is a per-control documentation URL (the ACM.1 URL above is that control's value, not a literal requirement for other controls). Description — fixed per-control documentation text; the emulator's is a shortened paraphrase. ParameterDefinitions — present in the emulator response, absent from the recorded AWS response for this control.

$.SecurityControlDefinitions[1] (ACM.2) diverges the same way (expected Description 'This control checks whether RSA certificates managed by AWS Certificate Manager use a key length of at least 2,048 bits. The control fails if the key length is smaller than 2,048 bits.' with its own RemediationUrl).

No existing open securityhub issue covers this: 20261029T180317Z-securityhub-mutations-are-not-reflected-by-later-reads.md is about mutations not being visible to later reads (DescribeActionTargets/accounts), and 20261103T202458Z-securityhub-batchdisablestandards-stub.md is the BatchDisableStandards stub.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture securityhub-catalog-list-security-control-definitions, step task#1: GET /securityControls/definitions against the securityhub endpoint (ListSecurityControlDefinitions, no StandardsArn filter).
3. Compare the returned SecurityControlDefinitions list element-by-element with the fixture's recorded response. Inspect element 0 (SecurityControlId ACM.1): the emulator's Description is the shortened paraphrase, RemediationUrl is https://docs.aws.amazon.com/console/securityhub/ACM.1/remediation instead of the recorded per-control doc URL, and an extra ParameterDefinitions map is present.
4. Every index 0..709 mismatches in the same way, so `replay_wire_corpus` reports one cluster for the step.
