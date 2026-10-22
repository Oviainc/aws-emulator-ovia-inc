Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-agreement-describe-agreement-malformed-id-rejected
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-agreement-describe-agreement-malformed-id-rejected marketplace-agreement-validation-error-paths
Issue-Shape-Id: com.amazonaws.marketplaceagreement#AWSMPCommerceService_v20200301
Issue-Key: marketplace-agreement|DescribeAgreement|4xx->4xx|$.fields
Issue-Key: marketplace-agreement|DescribeAgreement|4xx->4xx|$.reason
Issue-Key: marketplace-agreement|SearchAgreements|4xx->4xx|$.fields[*]
Issue-Key: marketplace-agreement|SearchAgreements|4xx->4xx|$.message
Issue-Related: 20261022T041017Z-marketplace-agreement-unknown-agreement-and-agreement-request-lookups-answer-404.md
Issue-Commit: 6d3a954d57304c873e4e6a3ccc977c1ac1664bbd

# ValidationException envelope omits reason and fields and does not name the offending member path

> _Note: this issue was identified against the code base at commit `6d3a954d57304c873e4e6a3ccc977c1ac1664bbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAgreement

## Description
The emulator's input-validation rejections carry the right status (400) but not AWS's ValidationException payload: the modeled 'reason' and 'fields' members are omitted entirely on one path, and on the other the single field entry identifies the top-level container member instead of the offending element's member path. Two causes, one fault in how the validation error is populated/serialized; each must be closed.

Cause 1 of 2: DescribeAgreement with an agreementId that violates the member pattern.
AWS: Smithy: DescribeAgreement.agreementId -> ResourceId, required, length 1..64, pattern ^[A-Za-z0-9_/-]+$; the rejection is ValidationException (errorKind=client, httpError=400) whose structure carries message, reason -> ValidationExceptionReason (enum containing INVALID_AGREEMENT_ID) and fields -> ValidationExceptionFieldList of {name, message}, both members required in each entry. The recording shows status 400 with reason 'INVALID_AGREEMENT_ID' and fields [{name: 'agreementId', message: 'Member is invalid.'}]; the top-level message example is 'Provided agreement id is invalid.' The requirement here is the structural one: a constraint violation on a modeled member must populate reason from the modeled enum and list the offending member in fields (the rule is request-derived - the member that failed is named).
Emulator: returns 400 but with $.reason absent and $.fields absent, carrying only a flat message ("1 validation error detected: Value 'not a valid id!!' at 'agreementId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9_/-]+$").
Why: clients and SDK error handling that read ValidationException.reason to classify a rejection (INVALID_AGREEMENT_ID vs MISSING_AGREEMENT_ID vs INVALID_FILTER_NAME) see None, and anything that surfaces per-field errors to a user from .fields gets an empty list - the structured half of the error contract is missing even though the status matched.
Fix: the service's ValidationException construction/serializer for DescribeAgreement's input validation - emit reason and fields alongside message.
Traces: marketplace-agreement-describe-agreement-malformed-id-rejected#task#1.

Cause 2 of 2: SearchAgreements with an unrecognized filter name inside the filters list.
AWS: Smithy: SearchAgreements.filters -> FilterList (length 1..10) of Filter{name, values}; the rejection is ValidationException (400) with reason from ValidationExceptionReason (INVALID_FILTER_NAME / UNSUPPORTED_FILTERS are modeled values) and fields entries of {name, message}. The recording shows the field entry naming the member PATH of the offending list element - name 'filters.1.member.name' (index of the bad filter plus the member that is bad), message 'Member is invalid.', with top-level message example 'Provided filter name is invalid.' The request-derived rule is that the field entry identifies which filter element and which of its members failed.
Emulator: returns 400 but with a single field entry named just 'filters', whose message is prose listing every accepted filter name, and the same prose prefixed into the top-level message.
Why: a client cannot tell WHICH filter in a multi-filter request was rejected, because the error points at the whole list rather than at index 1's name member; per-element error reporting and any UI that maps field names back to request positions break.
Fix: SearchAgreements' filter validation - build the ValidationExceptionField name as the indexed member path of the failing list element.
Traces: marketplace-agreement-validation-error-paths#task#3.

Not filed as part of this bundle: marketplace-agreement-create-and-accept-agreement-request#task#1 (CreateAgreementRequest, missing agreementProposalIdentifier) diverges ONLY in the wording of the top-level and per-field messages - status, field name and structure already match - and the rule producing AWS's exact wording is not documented in the model, so it is not treated as a defect.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator (emulator_up=true, bootstrap_signal=false, state_reset_failures=0):
- marketplace-agreement-describe-agreement-malformed-id-rejected#task#1 DescribeAgreement, status 400 == 400 but: $.fields missing (expected [{'message': 'Member is invalid.', 'name': 'agreementId'}], got None); $.reason missing (expected 'INVALID_AGREEMENT_ID', got None); $.message expected 'Provided agreement id is invalid.', got "1 validation error detected: Value 'not a valid id!!' at 'agreementId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9_/-]+$".
- marketplace-agreement-validation-error-paths#task#3 SearchAgreements, status 400 == 400 but: $.fields[0] expected {'name': 'filters.1.member.name', 'message': 'Member is invalid.'}, got {'name': 'filters', 'message': 'filter "NotARealFilterName" is not one of ResourceIdentifier, ResourceType, PartyType, ...'}; $.message expected 'Provided filter name is invalid.', got the same prose prefixed with 'filters: '.
Model evidence (services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/smithy-model): ValidationException httpError=400 with members message, reason (ValidationExceptionReason), fields (ValidationExceptionFieldList); ValidationExceptionField requires both name and message; ValidationExceptionReason includes INVALID_AGREEMENT_ID, INVALID_FILTER_NAME, UNSUPPORTED_FILTERS.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Call DescribeAgreement with agreementId='not a valid id!!' (fixture marketplace-agreement-describe-agreement-malformed-id-rejected, step task#1): the response is 400 but has no 'reason' and no 'fields' members.
3. Call SearchAgreements with a filters list whose second element has name='NotARealFilterName' (fixture marketplace-agreement-validation-error-paths, step task#3): the response is 400 but fields[0].name is 'filters' rather than the indexed member path 'filters.1.member.name'.
4. Re-replay both fixture steps after the fix; the 400 bodies must carry reason from ValidationExceptionReason and a fields entry naming the offending member path.
