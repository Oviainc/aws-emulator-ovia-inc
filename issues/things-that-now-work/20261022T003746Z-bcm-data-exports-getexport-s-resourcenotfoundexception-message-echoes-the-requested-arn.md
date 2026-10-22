Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bcm-data-exports.get-export-nonexistent-arn-not-found bcm-data-exports.get-export-cross-account-arn-isolated
Issue-Shape-Id: com.amazonaws.bcmdataexports#AWSBillingAndCostManagementDataExports
Issue-Key: bcm-data-exports.get-export-nonexistent-arn-not-found|run#6|AssertionError|assert message == NOT_FOUND_MSG, f"expected message {NOT_FOUND_MSG!r}, got {message!r}"
Issue-Key: bcm-data-exports.get-export-cross-account-arn-isolated|run#6|AssertionError|assert get_message == NOT_FOUND_MSG, (
Issue-Commit: 7d2cd57c0bb94f32b894957a728316a26e29b449

# GetExport's ResourceNotFoundException message echoes the requested ARN instead of AWS's fixed generic not-found message

> _Note: this issue was identified against the code base at commit `7d2cd57c0bb94f32b894957a728316a26e29b449`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExport

## Description
One root cause, grounded in two failing behavioral checks.

Cause 1 of 1: the ResourceNotFoundException body GetExport returns for an export ARN that resolves to nothing carries an emulator-authored, request-derived message instead of the fixed, request-independent message real BCM Data Exports returns. Failing checks: bcm-data-exports.get-export-nonexistent-arn-not-found, bcm-data-exports.get-export-cross-account-arn-isolated.

AWS: recorded behavior only for the message text. Smithy (services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/smithy-model/com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports.json): GetExport.ExportArn is a required Arn (@length 20..2048, pattern ^arn:aws[-a-z0-9]*:(bcm-data-exports):[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$); modeled errors are InternalServerException (500), ResourceNotFoundException (404, "The specified Amazon Resource Name (ARN) in the request doesn't exist."), ThrottlingException (429), ValidationException (400). The model does not state a message string. The two recordings do, and they agree: for a well-formed ExportArn naming an export that does not exist in the caller's own account, AND for a well-formed ExportArn naming a DIFFERENT account, AWS answers HTTP 404 / ResourceNotFoundException with exactly the same constant body message 'No matching definition found'. The rule the two recordings establish is therefore: the not-found message is a single fixed string that does not vary with — and does not restate — the ARN, the account or the resource name in the request. That constancy is the behavior under test (the cross-account check is tagged 'overly-permissive'/no-cross-tenant-leak): AWS's answer for "exists in another account" is byte-identical to its answer for "does not exist", so nothing in the reply confirms anything about the ARN the caller supplied.

Emulator: the status (404) and the error code (ResourceNotFoundException) are correct — both checks got code ResourceNotFoundException — but the message is built from the request. Replay output: got 'No export found for the Amazon Resource Name (ARN) arn:aws:bcm-data-exports:us-east-1:000000000000:export/does-not-exist/00000000-0000-0000-0000-000000000000.' for the own-account bogus ARN, and 'No export found for the Amazon Resource Name (ARN) arn:aws:bcm-data-exports:us-east-1:111122223333:export/isolated-test/00000000-0000-0000-0000-000000000000.' for the cross-account ARN, where AWS returns the constant 'No matching definition found' in both cases. The message is interpolated from the input ARN, so it differs per request instead of being constant.

Why: a client relying on AWS's behavior gets two things wrong. (1) Any consumer that keys off the not-found body — log fingerprinting, test assertions, retry/branch logic that distinguishes AWS not-found texts — sees a message AWS never emits, and sees it vary per call where AWS's is invariant. (2) The emulator's reply is a function of the caller-supplied ARN, so unlike AWS it reflects request content (the other account's id and the export name) back into the error body; code written against AWS's deliberately uninformative, identical answers for "absent" and "belongs to another account" is exercised against an emulator whose answers are distinguishable in shape and content, and the isolation behavior the suite checks cannot be validated against it.

Fix: the shared not-found constructor for this service's export ARNs, i.e. the error-construction helper in the service's state module that every reader hands its ARN to — not GetExport's lookup logic, which is correct (it does return a 404 for an unknown ARN). The same helper backs the other ARN readers/mutators of this service, so the fix should make the message a fixed constant there rather than formatting the ARN into it; the sibling members it sets alongside the message were not exercised by these checks and are not part of this report.

Traces: bcm-data-exports.get-export-nonexistent-arn-not-found (GetExport on arn:aws:bcm-data-exports:us-east-1:<caller-account>:export/does-not-exist/0000...0000 — asserted message 'No matching definition found'); bcm-data-exports.get-export-cross-account-arn-isolated (GetExport on the same ARN shape with account 111122223333 — same asserted message; the check failed at this assertion before reaching its ListTagsForResource assertions, so nothing is claimed about those here). Code sites: services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/src/ops/get_export.rs:155-159 (lookup miss -> ResourceNotFoundException via the helper) and services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/src/state.rs:133-139 (export_not_found formats the ARN into `message`). No existing open issue covers this service's not-found message rule; the nearest analogues filed for other services are 20261021T235115Z-outposts-error-message-bodies-do-not-follow-the.md and 20261022T000159Z-compute-optimizer-automation-error-payloads-diverge-from-aws.md ("not-found leaks the ARN"), neither of which is this service.

## Evidence
Check bcm-data-exports.get-export-nonexistent-arn-not-found [mismatch]:
  AssertionError: expected message 'No matching definition found', got 'No export found for the Amazon Resource Name (ARN) arn:aws:bcm-data-exports:us-east-1:000000000000:export/does-not-exist/00000000-0000-0000-0000-000000000000.'

Check bcm-data-exports.get-export-cross-account-arn-isolated [mismatch]:
  AssertionError: expected message 'No matching definition found' for a cross-account GetExport, got 'No export found for the Amazon Resource Name (ARN) arn:aws:bcm-data-exports:us-east-1:111122223333:export/isolated-test/00000000-0000-0000-0000-000000000000.'

Expected (recorded AWS, both cases identical):
  HTTP 404, Error.Code = ResourceNotFoundException, Error.Message = 'No matching definition found'
Actual (emulator):
  HTTP 404, Error.Code = ResourceNotFoundException  <- correct
  Error.Message = 'No export found for the Amazon Resource Name (ARN) <the ExportArn from the request>.'  <- request-derived, varies per call

Source of the diverging message:
  services/.../bcmdataexports.../src/state.rs:133-139
    pub fn export_not_found(arn: &str) -> ResourceNotFoundException {
        ResourceNotFoundException {
            message: format!("No export found for the Amazon Resource Name (ARN) {arn}."),
            ...
  reached from services/.../bcmdataexports.../src/ops/get_export.rs:155-159 on a store miss.

## Steps to Reproduce
1. Boot the emulator and point a botocore client at the bcm-data-exports endpoint.
2. Own-account absent export: call GetExport with a well-formed ExportArn that was never created, e.g. arn:aws:bcm-data-exports:us-east-1:<caller-account>:export/does-not-exist/00000000-0000-0000-0000-000000000000 (the ARN must satisfy the modeled Arn pattern so it is not refused as ValidationException).
3. Observe HTTP 404 with Error.Code = ResourceNotFoundException (correct) and Error.Message = 'No export found for the Amazon Resource Name (ARN) <that ARN>.' AWS returns Error.Message = 'No matching definition found'.
4. Cross-account: call GetExport with the same ARN shape but a foreign account id, e.g. arn:aws:bcm-data-exports:us-east-1:111122223333:export/isolated-test/00000000-0000-0000-0000-000000000000.
5. Observe the message again restates the supplied ARN, so the two replies differ from each other; against AWS both replies are byte-identical ('No matching definition found').
6. Re-replay to confirm a fix: bcm-data-exports.get-export-nonexistent-arn-not-found and bcm-data-exports.get-export-cross-account-arn-isolated. Note the second check continues past this assertion to ListTagsForResource expectations that have never been reached, so it may surface further, separate divergences once the message rule is fixed.
