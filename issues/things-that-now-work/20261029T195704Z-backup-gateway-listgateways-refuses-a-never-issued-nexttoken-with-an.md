Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup-gateway.list-gateways-returns-list-shape
Issue-Shape-Id: com.amazonaws.backupgateway#BackupOnPremises_v20210101
Issue-Key: backup-gateway.list-gateways-returns-list-shape|run#8|AssertionError|assert got == expected, f"expected {expected!r} for a fabricated NextToken, got {got!r}"
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# ListGateways refuses a never-issued NextToken with an emulator-authored validation sentence instead of AWS's pagination-token message

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListGateways

## Description
REOPENS 20261028T042745Z-backup-gateway-listgateways-pagination.md: its fix left the call refused.

ONE root cause. That resolved issue removed the silent acceptance of a NextToken this service never issued, and the call is now refused — but the refusal is worded by this crate rather than as AWS words an invalid pagination token, so the behavioral check that exercises the same input still fails.

AWS: a NextToken that is syntactically acceptable (Smithy: ListGateways.NextToken @length 1..1000, @pattern ^.+$, so botocore forwards it unchanged) but was never minted by a previous ListGateways response is refused by the service with HTTP 400 ValidationException (Smithy: ListGateways errors = InternalServerException(500), ThrottlingException(429), ValidationException(@httpError 400)) and Message exactly 'The pagination token is invalid and cannot be processed.' — the whole sentence is FIXED text for this error class: it has no variable part and in particular never echoes the submitted token, the member name or a regex. The model publishes no wording for ValidationException$Message, so the sentence is recorded behavior (the behavioral check pins it); the status and error class are modeled.

Emulator: HTTP 400 ValidationException (class and status correct) with Message "1 validation error detected: Value at 'nextToken' failed to satisfy constraint: the token must be one a previous ListGateways response returned" — i.e. the crate's generic member-constraint envelope ('<n> validation error(s) detected: Value at '<member-path>' failed to satisfy constraint: <clause>') applied to a condition that is not a modeled member constraint. No part of that string is AWS's text for this class.

Why: the error message is a requirement of the error class. A client (or an SDK retry/pagination wrapper, a log assertion, or a human diagnosing a stale cursor) that matches AWS's pagination-token sentence never matches, and the emulator's sentence misreports the cause as a constraint violation on the nextToken member — which would also lead a caller to 'fix' the token's format when no format is wrong, only its provenance.

Fix: the unissued-token refusal in the ListGateways handler (services/service-com-amazonaws-backupgateway-backuponpremises-v20210101/src/ops/list_gateways.rs, the resume_after(...).ok_or_else arm) must state AWS's fixed pagination-token sentence rather than routing through crate::service::handler_constraint_violation/member_path. The sentence belongs in one place beside the crate's other refusal constants in crate::state so the identical refusal in the sibling listings (ListHypervisors, ListVirtualMachines), which share decode_page_token/resume_after, is worded the same.

Traces: backup-gateway.list-gateways-returns-list-shape.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, 15/19 passed; this check [mismatch]:

backup-gateway.list-gateways-returns-list-shape — ListGateways(NextToken='sim2real-not-a-real-continuation-token'):
  expected ('ValidationException', 'The pagination token is invalid and cannot be processed.')
  got      ('ValidationException', "1 validation error detected: Value at 'nextToken' failed to satisfy constraint: the token must be one a previous ListGateways response returned")
The error code and (per botocore surfacing the 400) the status class match; only the Message diverges. The positive leg of the check (ListGateways returning a well-formed Gateways list) passed.

Message anatomy: AWS's 'The pagination token is invalid and cannot be processed.' is entirely fixed text — nothing in it comes from the request or from state. The emulator's string carries the crate's own fixed envelope plus a hand-written clause; it echoes the member path 'nextToken' and an explanatory clause AWS does not send.

Source of the behavior in the checkout: src/ops/list_gateways.rs:164-189 builds the refusal with crate::service::handler_constraint_violation(None, &crate::service::member_path("NextToken"), "the token must be one a previous ListGateways response returned"); the comment at :174-180 says the wording is deliberately the member-constraint envelope. Smithy (services/service-com-amazonaws-backupgateway-backuponpremises-v20210101/smithy-model/com-amazonaws-backupgateway-backuponpremises-v20210101.json): ListGateways input NextToken length 1..1000 pattern ^.+$, errors ValidationException(400)/ThrottlingException(429)/InternalServerException(500).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Call ListGateways with no NextToken; observe HTTP 200 and a Gateways list (this leg is correct).
3. Call ListGateways with NextToken='sim2real-not-a-real-continuation-token' (any 1..1000-character string this service never issued; it satisfies the modeled pattern so the SDK sends it).
4. Expect HTTP 400 ValidationException with Message exactly 'The pagination token is invalid and cannot be processed.'; observe the same code with Message "1 validation error detected: Value at 'nextToken' failed to satisfy constraint: the token must be one a previous ListGateways response returned".
5. Re-replay behavioral check backup-gateway.list-gateways-returns-list-shape to confirm.
6. Repeat step 3 against ListHypervisors and ListVirtualMachines, which share the same token-resolution helper, to confirm the wording is fixed in one place.
