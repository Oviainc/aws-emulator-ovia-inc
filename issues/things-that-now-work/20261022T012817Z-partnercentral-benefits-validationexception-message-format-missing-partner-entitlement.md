Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: partnercentral-benefits.associate-benefit-application-resource-malformed-arn-rejected partnercentral-benefits.cancel-benefit-application-transitions-to-canceled partnercentral-benefits.create-benefit-application-invalid-catalog-enum-rejected partnercentral-benefits.create-benefit-application-requires-active-partner-benefit-rejected partnercentral-benefits.create-benefit-application-starts-pending-submission-blocked-by-entitlement-gate partnercentral-benefits.disassociate-benefit-application-resource-malformed-arn-rejected partnercentral-benefits.get-benefit-allocation-malformed-identifier-rejected partnercentral-benefits.get-benefit-application-malformed-identifier-rejected partnercentral-benefits.list-benefit-applications-invalid-next-token-rejected partnercentral-benefits.list-benefits-max-results-out-of-range-rejected partnercentral-benefits.list-benefits-requires-active-partner-benefit-rejected partnercentral-benefits.recall-benefit-application-requires-active-partner-benefit-rejected partnercentral-benefits.submit-benefit-application-malformed-identifier-rejected partnercentral-benefits.submit-benefit-application-twice-rejected partnercentral-benefits.update-benefit-application-after-submission-rejected partnercentral-benefits.update-benefit-application-malformed-identifier-rejected
Issue-Shape-Id: com.amazonaws.partnercentralbenefits#PartnerCentralBenefitsService
Issue-Key: partnercentral-benefits.associate-benefit-application-resource-malformed-arn-rejected|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: partnercentral-benefits.cancel-benefit-application-transitions-to-canceled|run#4|AssertionError|assert malformed == ("ValidationException", expected), (
Issue-Key: partnercentral-benefits.create-benefit-application-invalid-catalog-enum-rejected|run#4|AssertionError|assert control == ("AccessDeniedException", _DENIED), (
Issue-Key: partnercentral-benefits.create-benefit-application-requires-active-partner-benefit-rejected|run#4|AssertionError|assert control == ("ResourceNotFoundException", f"Catalog `{catalog}` not found"), (
Issue-Key: partnercentral-benefits.create-benefit-application-starts-pending-submission-blocked-by-entitlement-gate|run#4|AssertionError|assert control == ("ValidationException", expected), (
Issue-Key: partnercentral-benefits.disassociate-benefit-application-resource-malformed-arn-rejected|run#6|AssertionError|assert refusal == ("ValidationException", expected), (
Issue-Key: partnercentral-benefits.get-benefit-allocation-malformed-identifier-rejected|run#4|AssertionError|assert refusal == ("ValidationException", expected), (
Issue-Key: partnercentral-benefits.get-benefit-application-malformed-identifier-rejected|run#4|AssertionError|assert refusal == ("ValidationException", expected), (
Issue-Key: partnercentral-benefits.list-benefit-applications-invalid-next-token-rejected|run#4|AssertionError|assert control == ("ValidationException", expected), (
Issue-Key: partnercentral-benefits.list-benefits-max-results-out-of-range-rejected|_error#2|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded: {result!r}")
Issue-Key: partnercentral-benefits.list-benefits-requires-active-partner-benefit-rejected|run#4|AssertionError|assert control == ("ValidationException", expected), (
Issue-Key: partnercentral-benefits.recall-benefit-application-requires-active-partner-benefit-rejected|run#4|AssertionError|assert control == ("ValidationException", expected), (
Issue-Key: partnercentral-benefits.submit-benefit-application-malformed-identifier-rejected|run#6|AssertionError|assert refusal == ("ValidationException", expected), (
Issue-Key: partnercentral-benefits.submit-benefit-application-twice-rejected|run#4|AssertionError|assert (shape_code, shape_message) == ("ValidationException", expected), (
Issue-Key: partnercentral-benefits.update-benefit-application-after-submission-rejected|run#8|AssertionError|assert create_code == "AccessDeniedException", (
Issue-Key: partnercentral-benefits.update-benefit-application-malformed-identifier-rejected|run#7|AssertionError|assert refusal == ("ValidationException", expected), (
Issue-Commit: 5b247f22dea7573e9fb6c4376c776b3a345a666f

# PartnerCentral Benefits: ValidationException message format, missing partner-entitlement gate, and ResourceNotFoundException message (4 root causes)

> _Note: this issue was identified against the code base at commit `5b247f22dea7573e9fb6c4376c776b3a345a666f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBenefitApplication, ListBenefits, ListBenefitApplications, GetBenefitApplication, GetBenefitAllocation, SubmitBenefitApplication, UpdateBenefitApplication, CancelBenefitApplication, RecallBenefitApplication, AssociateBenefitApplicationResource, DisassociateBenefitApplicationResource

## Description
This issue bundles FOUR distinct root causes in the PartnerCentral Benefits emulator (sdkId "PartnerCentral Benefits", crate services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice, awsJson1_0). All four must be closed before submitting. No existing open issue covers any of them; the only related filings are the (fixed) bootstrap issue 20261021T234754Z-bootstrap-service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice.md and the four auto-filed scaffold-stub issues for AmendBenefitApplication / RecallBenefitApplication / TagResource / UntagResource, which are explicitly NOT part of this issue.

=== Cause 1 of 4: ValidationException message echoes the offending member value ===
AWS: for a request member that violates a modeled constraint, this service returns HTTP 400, error code ValidationException, with a message built deterministically from the request in the form
  "1 validation error detected: Value at '<wireMemberName>' failed to satisfy constraint: <constraint clause>"
The offending VALUE is not echoed. Because the message is constructed from the request (it names the member and its constraint), its construction rule is a requirement, not an incidental string. Recorded behavior shows this for both constraint families:
  - @pattern: "... Value at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: (arn:.+|benappl-[0-9a-z]{14})"
  - @range:   "... Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"
Smithy (services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/smithy-model/com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice.json): GetBenefitApplicationInput.Identifier -> BenefitApplicationIdentifier @pattern; ListBenefitsInput.MaxResults @range max 100; ValidationException is a service-common error, errorKind client, httpError 400.
Emulator: emits the value inline, e.g. "1 validation error detected: Value '!!! not a valid identifier !!!' at 'identifier' failed to satisfy constraint: ..." and "Value '1000' at 'maxResults' failed to satisfy constraint: ...". Code (ValidationException) and status are correct; only the message construction diverges.
Why: a client (or a test/assertion layer) that parses the documented AWS validation-message form to recover which member failed and why mis-parses the emulator's message, and any caller that logs these messages leaks the rejected request value into logs where AWS deliberately does not.
Fix: the shared constraint-validation stage of the centralized request walk (deserialize -> validate -> handle) that renders constraint failures into ValidationException.Message — i.e. the smithy server runtime's constraint-violation message formatter, not any individual handler in src/ops/.
Traces: partnercentral-benefits.get-benefit-application-malformed-identifier-rejected, partnercentral-benefits.get-benefit-allocation-malformed-identifier-rejected, partnercentral-benefits.submit-benefit-application-malformed-identifier-rejected, partnercentral-benefits.update-benefit-application-malformed-identifier-rejected, partnercentral-benefits.associate-benefit-application-resource-malformed-arn-rejected, partnercentral-benefits.disassociate-benefit-application-resource-malformed-arn-rejected, partnercentral-benefits.cancel-benefit-application-transitions-to-canceled, partnercentral-benefits.submit-benefit-application-twice-rejected, partnercentral-benefits.recall-benefit-application-requires-active-partner-benefit-rejected, partnercentral-benefits.create-benefit-application-starts-pending-submission-blocked-by-entitlement-gate, partnercentral-benefits.list-benefits-requires-active-partner-benefit-rejected, partnercentral-benefits.list-benefit-applications-invalid-next-token-rejected.

=== Cause 2 of 4: @pattern ValidationException message does not strip the regex anchors ===
AWS: the pattern quoted in the ValidationException message is the modeled regex with its leading '^' and trailing '$' removed. Recorded messages show "Member must satisfy regular expression pattern: (arn:.+|benappl-[0-9a-z]{14})", "... pattern: (arn:.+|ben-[0-9a-z]{14})" and "... pattern: arn:aws:([a-zA-Z0-9\\-])+:([a-z]{2}(-gov)?-[a-z]+-\\d{1})?:(\\d{12})?:(.+)" — all unanchored — while the model declares them anchored.
Smithy: BenefitApplicationIdentifier @pattern "^(arn:.+|benappl-[0-9a-z]{14})$"; BenefitId @pattern "^(arn:.+|ben-[0-9a-z]{14})$"; BenefitAllocationIdentifier @pattern "^(arn:.+|benalloc-[0-9a-z]{14})$"; Arn @pattern "^arn:aws:([a-zA-Z0-9\\-])+:([a-z]{2}(-gov)?-[a-z]+-\\d{1})?:(\\d{12})?:(.+)$".
Emulator: quotes the modeled pattern verbatim, anchors included: "... pattern: ^(arn:.+|benappl-[0-9a-z]{14})$", "... pattern: ^arn:aws:([a-zA-Z0-9\\-])+:...:(.+)$". The matching itself is correct (the right requests are rejected); only the pattern as reported diverges.
Why: a client that feeds the reported pattern back into its own regex engine for client-side pre-validation, or that compares it to the published API model, gets a different expression than AWS reports; message-parsing clients keyed on the documented form fail to match.
Fix: same shared constraint-validation/message-formatting stage as Cause 1 — strip the anchoring '^' prefix and '$' suffix when rendering a @pattern violation. Must not change which values are accepted.
Traces: partnercentral-benefits.get-benefit-application-malformed-identifier-rejected, partnercentral-benefits.get-benefit-allocation-malformed-identifier-rejected, partnercentral-benefits.submit-benefit-application-malformed-identifier-rejected, partnercentral-benefits.update-benefit-application-malformed-identifier-rejected, partnercentral-benefits.associate-benefit-application-resource-malformed-arn-rejected, partnercentral-benefits.disassociate-benefit-application-resource-malformed-arn-rejected, partnercentral-benefits.cancel-benefit-application-transitions-to-canceled, partnercentral-benefits.submit-benefit-application-twice-rejected, partnercentral-benefits.recall-benefit-application-requires-active-partner-benefit-rejected, partnercentral-benefits.create-benefit-application-starts-pending-submission-blocked-by-entitlement-gate.

=== Cause 3 of 4: the partner-entitlement gate is absent, so un-entitled callers are served ===
AWS: the Benefits API is fronted by an entitlement gate that applies uniformly to reads and writes. For a caller whose account has no active AWS partner benefit, a fully well-formed request is refused with HTTP 403, error code AccessDeniedException, message (recorded) "This action requires an active AWS partner benefit. Register as an AWS partner if you haven't already, or contact AWS partner central support." The recorded ordering is: request-shape validation first (ValidationException wins over the gate), then the entitlement gate, then resource-existence lookups (so the gate wins over ResourceNotFoundException). Smithy: AccessDeniedException is a service-common error on every operation, errorKind client, httpError 403, with a required Message member; recorded behavior only for the gate's placement and message text.
Emulator: there is no entitlement gate at all. ListBenefits(Catalog="Sandbox", MaxResults=20) returns HTTP 200 with a seeded BenefitSummaries list (ben-3v5et29cht5uzy "Promotional Credits", ben-4yg8ijdk2mh6l4 "Marketing Development Funds", ...). CreateBenefitApplication with a well-formed request falls straight through to the existence check and answers ResourceNotFoundException instead of AccessDeniedException. Inspecting src/ops/get_benefit_application.rs confirms the handler goes directly from state lookup to ResourceNotFoundException with no authorization stage ahead of it.
Why: this is the overly-permissive class. Code written against the emulator sees reads and mutations succeed (or 404) where real AWS returns 403, so error handling for the un-entitled case is never exercised and ships untested; a client cannot distinguish "not entitled" from "does not exist". It also makes every downstream behavioral expectation on this service unreachable from the emulator.
Fix: an authorization/entitlement stage for the whole service, ahead of per-operation handlers and behind request-shape validation (service-level middleware or a shared guard invoked at the top of each src/ops/*.rs handle), returning the modeled AccessDeniedException; and the state layer must not pre-seed an entitled principal that bypasses it.
Traces: partnercentral-benefits.list-benefits-max-results-out-of-range-rejected (well-formed ListBenefits returned 200 instead of erroring), partnercentral-benefits.create-benefit-application-invalid-catalog-enum-rejected (expected AccessDeniedException, got ResourceNotFoundException "benefit ben-00000000000000 does not exist in catalog Sandbox"), partnercentral-benefits.update-benefit-application-after-submission-rejected (expected AccessDeniedException for CreateBenefitApplication, got ResourceNotFoundException).

=== Cause 4 of 4: ResourceNotFoundException message for a nonexistent Catalog ===
AWS: CreateBenefitApplication with a Catalog that satisfies the modeled pattern but names no real catalog is rejected with HTTP 404, error code ResourceNotFoundException, and a message built from the request naming the catalog: recorded form "Catalog `<catalogName>` not found" (backtick-quoted member value). Smithy: CatalogName @pattern "^[A-Za-z0-9_-]+$" only — catalog existence is a business-logic check, not a shape constraint; ResourceNotFoundException is service-common, errorKind client, httpError 404.
Emulator: returns the correct code and status but a different message-construction rule: "catalog <catalogName> does not exist" (lowercase, unquoted, "does not exist"). The sibling resource lookups use the same non-AWS form, e.g. "benefit <id> does not exist in catalog <catalog>" and "benefit application <id> does not exist in catalog <catalog>".
Why: the message is produced by a known rule from request content, so a client that surfaces or matches on the AWS form for "which resource was not found" reads the emulator's message as a different condition; it is the only distinguishing detail when the code alone (404/ResourceNotFoundException) is shared by catalog, benefit and application lookups.
Fix: the existence-check branches of the operation handlers in services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/ops/ (e.g. create_benefit_application.rs, get_benefit_application.rs) — render ResourceNotFoundException.Message with AWS's "<Resource> `<value>` not found" construction. Lower severity than Causes 1-3: status and code already match.
Traces: partnercentral-benefits.create-benefit-application-requires-active-partner-benefit-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1, 0/20 passed for partnercentral-benefits (20 mismatched). 4 of the 20 are marked ALREADY FILED (AmendBenefitApplication, RecallBenefitApplication, TagResource, UntagResource scaffold stubs) and are excluded here; the remaining 16 cluster into the 4 causes above.

Cause 1 (value echoed in ValidationException message)
  expected: "1 validation error detected: Value at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws:([a-zA-Z0-9\\-])+:([a-z]{2}(-gov)?-[a-z]+-\\d{1})?:(\\d{12})?:(.+)"
  actual:   "1 validation error detected: Value 'not-an-arn' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws:([a-zA-Z0-9\\-])+:([a-z]{2}(-gov)?-[a-z]+-\\d{1})?:(\\d{12})?:(.+)$"
  expected: "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"
  actual:   "1 validation error detected: Value '1000' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"

Cause 2 (anchors not stripped)
  expected: "... Member must satisfy regular expression pattern: (arn:.+|benappl-[0-9a-z]{14})"
  actual:   "... Member must satisfy regular expression pattern: ^(arn:.+|benappl-[0-9a-z]{14})$"
  expected: "... Member must satisfy regular expression pattern: (arn:.+|ben-[0-9a-z]{14})"   (benefitIdentifier)
  actual:   "... Member must satisfy regular expression pattern: ^(arn:.+|ben-[0-9a-z]{14})$"
  expected: "... Member must satisfy regular expression pattern: (arn:.+|benalloc-[0-9a-z]{14})"  (GetBenefitAllocation identifier)
  actual:   "... Member must satisfy regular expression pattern: ^(arn:.+|benalloc-[0-9a-z]{14})$"

Cause 3 (no entitlement gate)
  ListBenefits(Catalog="Sandbox", MaxResults=20)
    expected: a ClientError — HTTP 403 AccessDeniedException, "This action requires an active AWS partner benefit. Register as an AWS partner if you haven't already, or contact AWS partner central support."
    actual:   HTTP 200 {'BenefitSummaries': [{'Id': 'ben-3v5et29cht5uzy', 'Catalog': 'Sandbox', 'Arn': 'arn:aws:partnercentral:us-east-1:000000000000:catalog/Sandbox/benefit/ben-3v5et29cht5uzy', 'Name': 'Promotional Credits', ... 'Status': 'ACTIVE'}, ...]}
  CreateBenefitApplication(Catalog="Sandbox", BenefitIdentifier="ben-00000000000000", Name=...)
    expected: ('AccessDeniedException', <entitlement message above>)
    actual:   ('ResourceNotFoundException', 'benefit ben-00000000000000 does not exist in catalog Sandbox')
  CreateBenefitApplication (same, from update-benefit-application-after-submission-rejected)
    expected code: 'AccessDeniedException'   actual code: 'ResourceNotFoundException'

Cause 4 (catalog-not-found message)
  CreateBenefitApplication(Catalog="not-a-real-catalog-34556872", ...)
    expected: ('ResourceNotFoundException', 'Catalog `not-a-real-catalog-34556872` not found')
    actual:   ('ResourceNotFoundException', 'catalog not-a-real-catalog-34556872 does not exist')

Bootstrap gate: this service's bootstrap issue (20261021T234754Z-bootstrap-service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice.md) is resolved and its crate 'service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice' is in get_bootstrap_status().bootstrapped; the emulator answered every call above, so these are fidelity findings, not a bootstrap gap.

## Steps to Reproduce
Client for all steps (us-east-1 only, api.aws host):
  client = boto3.client('partnercentral-benefits', region_name='us-east-1', endpoint_url='https://partnercentral-benefits.us-east-1.api.aws')

Cause 1 + Cause 2 — constraint-violation message construction:
  1. client.get_benefit_application(Catalog='Sandbox', Identifier='not-a-valid-identifier!!')
     -> AWS: 400 ValidationException, "1 validation error detected: Value at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: (arn:.+|benappl-[0-9a-z]{14})"
     -> Emulator: value echoed and pattern anchored.
  2. client.associate_benefit_application_resource(Catalog='Sandbox', Identifier='benappl-00000000000001', ResourceArn='not-an-arn', ...)
     -> same divergence on 'resourceArn'.
  3. client.list_benefits(Catalog='Sandbox', MaxResults=1000)
     -> AWS: 400 ValidationException, "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"
     -> Emulator: "Value '1000' at 'maxResults' ...".

Cause 3 — entitlement gate (account with no active AWS partner benefit; the emulator's default principal):
  4. client.list_benefits(Catalog='Sandbox', MaxResults=20)
     -> AWS: 403 AccessDeniedException with the entitlement message.
     -> Emulator: 200 with a seeded BenefitSummaries list.
  5. client.create_benefit_application(Catalog='Sandbox', ClientToken=uuid, BenefitIdentifier='ben-00000000000000', Name='x')
     -> AWS: 403 AccessDeniedException.
     -> Emulator: 404 ResourceNotFoundException 'benefit ... does not exist in catalog Sandbox'.
  6. client.list_benefit_applications(Catalog='Sandbox', MaxResults=50)
     -> AWS: 403 AccessDeniedException (the gate fronts reads too).
  Ordering to preserve: step 3 (malformed input) must still return ValidationException, i.e. shape validation runs BEFORE the gate, and the gate runs BEFORE any resource-existence lookup.

Cause 4 — nonexistent catalog message:
  7. client.create_benefit_application(Catalog='not-a-real-catalog-34556872', ClientToken=uuid, BenefitIdentifier='ben-00000000000000', Name='x')
     -> AWS: 404 ResourceNotFoundException, 'Catalog `not-a-real-catalog-34556872` not found'.
     -> Emulator: 'catalog not-a-real-catalog-34556872 does not exist'.
