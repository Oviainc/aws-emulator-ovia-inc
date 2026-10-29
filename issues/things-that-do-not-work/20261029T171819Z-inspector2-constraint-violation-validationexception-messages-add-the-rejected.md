Issue-Source: trace-sweep-divergence
Issue-Trace: inspector2-code-security-scan-configuration-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: inspector2-code-security-scan-configuration-lifecycle inspector2-filter-lifecycle-and-validation inspector2-get-clusters-for-image-and-connector-validation
Issue-Shape-Id: com.amazonaws.inspector2#Inspector2
Issue-Key: inspector2|CreateCodeSecurityScanConfiguration|4xx->4xx|$.message
Issue-Key: inspector2|CreateFilter|4xx->4xx|$.message
Issue-Key: inspector2|CreateConnector|4xx->4xx|$.message
Issue-Commit: 4fccfcb9e83e9e12ca223d9fc117bac8a0390f59

# inspector2 constraint-violation ValidationException messages add the rejected value and restate the constraint wrongly

> _Note: this issue was identified against the code base at commit `4fccfcb9e83e9e12ca223d9fc117bac8a0390f59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFilter

## Description
THREE causes, all one shared defect: the emulator's constraint-violation message builder for inspector2 ValidationException (HTTP 400) renders a different sentence from AWS. In every case the status (400) and the error type (ValidationException) are correct and only the `message` string diverges, in three consistent ways: (a) the emulator interpolates the REJECTED VALUE into the sentence ("Value 'X' at '<path>' ...") where AWS writes "Value at '<path>' ..." for these constraint classes; (b) for an enum member inside a list the emulator names the indexed member path and a bare enum clause, where AWS names the LIST member and nests the member clause inside "Member must satisfy constraint: [ ... ]"; (c) the enum value set and the regular-expression pattern are reproduced in a form AWS does not use — AWS's value-set ordering differs from the model's declaration order, and AWS strips the anchors from the pattern it quotes. The fixed text of each sentence ("N validation error detected: ", "Value at '", "' failed to satisfy constraint: Member must satisfy enum value set: [", "Member must satisfy regular expression pattern: ") is the requirement; the variable parts are the member path, the enum value list and the pattern, which come from the request's shape and the model — never from one recording's rejected value.

Cause 1 of 3: CreateCodeSecurityScanConfiguration, an unmodelled string in configuration.ruleSetCategories.
AWS: Smithy: CreateCodeSecurityScanConfiguration.configuration -> CodeSecurityScanConfiguration.ruleSetCategories (required, list of RuleSetCategory, @length min 1 max 3); RuleSetCategory is an enum with values SAST, IAC, SCA; modeled error ValidationException (httpError 400). Recorded response: 400 with message "1 validation error detected: Value at 'configuration.ruleSetCategories' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [IAC, SAST, SCA]]" — AWS names the LIST member (no element index, no `.member` suffix), omits the rejected value, and wraps the member constraint in "Member must satisfy constraint: [ ... ]". The value set is enumerated IAC, SAST, SCA.
Emulator: answers 400 but with "1 validation error detected: Value 'NOT_A_REAL_RULE_SET_CATEGORY' at 'configuration.ruleSetCategories.1.member' failed to satisfy constraint: Member must satisfy enum value set: [SAST, IAC, SCA]" — rejected value interpolated, indexed `.member` path instead of the list member, no nested "Member must satisfy constraint: [...]" wrapper, and the value set in model-declaration order.
Why: a client (or a test/assertion harness) that matches on AWS's message for a bad rule-set category never matches, and the leaked request value changes the message for every distinct bad input, so the error is not stable across requests the way AWS's is.
Fix: the shared constraint-violation message builder used by CreateCodeSecurityScanConfiguration's request validation (list-of-enum branch: suppress the value, name the list member, emit the nested member-constraint wrapper, emit AWS's value-set ordering).
Traces: inspector2-code-security-scan-configuration-lifecycle#task#3.

Cause 2 of 3: CreateFilter, an unmodelled comparison in filterCriteria.severity[].comparison.
AWS: Smithy: CreateFilter.filterCriteria (required, FilterCriteria) -> severity is a list of StringFilter; StringFilter.comparison (required) targets StringComparison with values EQUALS, PREFIX, NOT_EQUALS; modeled error ValidationException (httpError 400). Recorded response: 400 with message "1 validation error detected: Value at 'filterCriteria.severity.1.member.comparison' failed to satisfy constraint: Member must satisfy enum value set: [NOT_EQUALS, PREFIX, EQUALS]" — the rejected value is absent and the value set is enumerated NOT_EQUALS, PREFIX, EQUALS.
Emulator: answers 400 with "1 validation error detected: Value 'NOT_A_REAL_COMPARISON' at 'filterCriteria.severity.1.member.comparison' failed to satisfy constraint: Member must satisfy enum value set: [EQUALS, PREFIX, NOT_EQUALS]" — the path is right, but the rejected value is interpolated and the value set is in model-declaration order.
Why: same as above — the message a client sees for an invalid filter comparison is not the AWS message, and it varies with the submitted value.
Fix: the same constraint-violation message builder, reached from CreateFilter's filterCriteria validation (scalar-enum branch: omit the value, emit AWS's value-set ordering).
Traces: inspector2-filter-lifecycle-and-validation#task#5.

Cause 3 of 3: CreateConnector, a providerDetail.azure.awsConfigConnectorArn that breaks the member's @pattern.
AWS: Smithy: CreateConnector.providerDetail (required, union ProviderDetailCreate) -> azure -> AzureProviderDetailCreate.awsConfigConnectorArn (required, @length 1..512, @pattern "^arn:([^:]+):config:([^:]+):([^:]+):connector/([^/]+)/([^/]+)/([^/:\\s]+)$"); modeled error ValidationException (httpError 400). Recorded response: 400 with message "1 validation error detected: Value at 'providerDetail.azure.awsConfigConnectorArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:([^:]+):config:([^:]+):([^:]+):connector/([^/]+)/([^/]+)/([^/:\\s]+)" — the rejected value is absent and the quoted pattern carries NO leading ^ and NO trailing $.
Emulator: answers 400 with "1 validation error detected: Value 'not-a-real-arn' at 'providerDetail.azure.awsConfigConnectorArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:([^:]+):config:([^:]+):([^:]+):connector/([^/]+)/([^/]+)/([^/:\\s]+)$" — the rejected value is interpolated and the pattern is quoted with its anchors.
Why: a client matching AWS's pattern-violation wording (including the anchor-stripped pattern it quotes) does not match, and the echoed value makes the message request-dependent.
Fix: the same constraint-violation message builder, pattern branch (omit the value; quote the model pattern with its ^ / $ anchors stripped), reached from CreateConnector's providerDetail validation.
Traces: inspector2-get-clusters-for-image-and-connector-validation#task#2.

## Evidence
Cause 1 — fixture inspector2-code-security-scan-configuration-lifecycle step task#3, CreateCodeSecurityScanConfiguration: expected status 400, actual 400; $.message expected "1 validation error detected: Value at 'configuration.ruleSetCategories' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [IAC, SAST, SCA]]", actual "1 validation error detected: Value 'NOT_A_REAL_RULE_SET_CATEGORY' at 'configuration.ruleSetCategories.1.member' failed to satisfy constraint: Member must satisfy enum value set: [SAST, IAC, SCA]".

Cause 2 — fixture inspector2-filter-lifecycle-and-validation step task#5, CreateFilter: expected 400, actual 400; $.message expected "1 validation error detected: Value at 'filterCriteria.severity.1.member.comparison' failed to satisfy constraint: Member must satisfy enum value set: [NOT_EQUALS, PREFIX, EQUALS]", actual "1 validation error detected: Value 'NOT_A_REAL_COMPARISON' at 'filterCriteria.severity.1.member.comparison' failed to satisfy constraint: Member must satisfy enum value set: [EQUALS, PREFIX, NOT_EQUALS]".

Cause 3 — fixture inspector2-get-clusters-for-image-and-connector-validation step task#2, CreateConnector: expected 400, actual 400; $.message expected "1 validation error detected: Value at 'providerDetail.azure.awsConfigConnectorArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:([^:]+):config:([^:]+):([^:]+):connector/([^/]+)/([^/]+)/([^/:\\s]+)", actual "1 validation error detected: Value 'not-a-real-arn' at 'providerDetail.azure.awsConfigConnectorArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:([^:]+):config:([^:]+):([^:]+):connector/([^/]+)/([^/]+)/([^/:\\s]+)$".

In all three the HTTP status and the x-amzn-errortype (ValidationException) agree; only the message text diverges. Shared, value-independent pattern of divergence: rejected value interpolated where AWS omits it; indexed `.member` path where AWS names the list member and nests "Member must satisfy constraint: [...]"; enum value set in model order rather than AWS's order; @pattern quoted with anchors AWS strips.

## Steps to Reproduce
1. Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator.
2. Cause 1: POST /codesecurity/scan-configuration/create (CreateCodeSecurityScanConfiguration) with a valid name/level and configuration.ruleSetCategories containing a second element that is not SAST/IAC/SCA — fixture inspector2-code-security-scan-configuration-lifecycle, step task#3. Compare $.message to the recorded one.
3. Cause 2: POST /filters/create (CreateFilter) with filterCriteria.severity[1].comparison set to a string outside EQUALS/PREFIX/NOT_EQUALS — fixture inspector2-filter-lifecycle-and-validation, step task#5.
4. Cause 3: POST /connector/create (CreateConnector) with providerDetail.azure.awsConfigConnectorArn set to a string that does not match the member's @pattern — fixture inspector2-get-clusters-for-image-and-connector-validation, step task#2.
5. Each returns 400 ValidationException with a message differing from AWS's as described.
