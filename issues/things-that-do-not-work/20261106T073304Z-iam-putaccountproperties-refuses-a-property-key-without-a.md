Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.put-account-properties
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.put-account-properties|run#11|AssertionError|assert code == "InvalidInput", (
Issue-Commit: c83662acd782fe5bfa18e86152cd5905ede91863

# iam: PutAccountProperties refuses a property key without a Namespace/Name separator with ValidationError instead of the modeled InvalidInput and AWS's 'Property key must be in format Namespace/PropertyName: <key>'

> _Note: this issue was identified against the code base at commit `c83662acd782fe5bfa18e86152cd5905ede91863`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAccountProperties

## Description
REOPENS 20261029T192356Z-iam-10-net-new-refusal-message-and-response.md: its fix left the call refused. That resolved issue's cause 7 required GetAccountProperties/PutAccountProperties to stop answering InvalidInput and answer ValidationError for the invalid property it exercised (check ids iam.op-contract-getAccountProperties / iam.op-contract-put-account-properties). The handler now classifies EVERY property refusal as ValidationError, which is the wrong class for the business-rule refusal below, so this call is still refused with a class the caller cannot catch. Number of causes to close: 1.

Cause 1 of 1: a property key that satisfies the modeled key pattern but carries no '/' separator is a business-rule refusal (InvalidInput), not a constraint violation (ValidationError).
AWS: Smithy (com.amazonaws.iam#PutAccountProperties) models ConcurrentModificationException (409), InvalidInputException (awsQueryError InvalidInput, HTTP 400) and ServiceFailureException (500); ValidationError is NOT a modeled error for this operation. The Properties map key targets accountPropertyKeyType with length 1..50 and pattern ^[A-Za-z][A-Za-z0-9/_-]*$ — a key such as 'NoSlashHere' SATISFIES that pattern, so no constraint is violated and the refusal is IAM's own business rule: HTTP 400, Code InvalidInput, Message 'Property key must be in format Namespace/PropertyName: <key>' (fixed text 'Property key must be in format Namespace/PropertyName: ' + the offending key taken from the request). The rejected write must leave account properties untouched: the bad key must not appear in GetAccountProperties and a previously written property (e.g. RoleManager/Enabled) must keep its value.
Emulator: the same call is refused with Code 'ValidationError'. The write itself is correctly rejected, but the error class is one the IAM SDK error surface does not expose for this operation, and the message could not be compared because the code assertion fails first.
Why: a client catching InvalidInputException — the only client-side refusal PutAccountProperties models — does not catch ValidationError, so a perfectly ordinary bad-key write surfaces as an unknown error and any retry/repair branch keyed on the modeled error never runs. It also contradicts the modeled error set, so generated clients have no shape to deserialize into.
Fix: the refusal classification in the put_account_properties handler — the Namespace/PropertyName separator rule must raise IAM's InvalidInput (modeled InvalidInputException, HTTP 400) with the message templated above, distinct from the ValidationError the handler now raises for every property defect; the earlier fix's ValidationError must stay only on the path that resolved issue graded.
Traces: iam.put-account-properties.

## Evidence
Suite simparity-behavioral-aws-v1 replayed against this checkout; service heading `### iam` 331/456, 124 failed. The check below is marked neither ALREADY FILED nor OWNED.

[mismatch] iam.put-account-properties, at check line 121:
  assert code == "InvalidInput", (...)
  AssertionError: expected InvalidInput for a malformed property key, got ValidationError

Legs that already passed before that assertion, so the operation itself works: GetAccountProperties read the original RoleManager/Enabled value; PutAccountProperties flipped it; GetAccountProperties showed the flipped value.

The refused request: put_account_properties(Properties={'NoSlashHere': 'true'}).
Expected (AWS): Error.Code 'InvalidInput' (HTTP 400) and Error.Message 'Property key must be in format Namespace/PropertyName: NoSlashHere' — 'NoSlashHere' is the key taken from the request, the rest is fixed text; afterwards GetAccountProperties must not contain 'NoSlashHere' and RoleManager/Enabled must still hold the flipped value.
Actual (emulator): Error.Code 'ValidationError'; the message was not compared because the code assertion fails first.
Also unreached because of this failure (same check, must hold after the fix): a zero-permission IAM user calling PutAccountProperties must be refused Code 'AccessDenied' with 'User: <user-arn> is not authorized to perform: iam:PutAccountProperties on resource: * because no identity-based policy allows the iam:PutAccountProperties action' plus IAM's authorization-details trailer, and the property must be restored to its original value.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an IAM client at it.
2. Call get_account_properties() and note Properties['RoleManager/Enabled'] (default 'false').
3. Call put_account_properties(Properties={'RoleManager/Enabled': <the other value>}) and confirm get_account_properties() reflects it.
4. Call put_account_properties(Properties={'NoSlashHere': 'true'}).
   Observed: ClientError with Error.Code 'ValidationError'.
   Expected: Error.Code 'InvalidInput', HTTP 400, Error.Message 'Property key must be in format Namespace/PropertyName: NoSlashHere'.
5. Confirm get_account_properties() still has no 'NoSlashHere' key and RoleManager/Enabled unchanged by the rejected write.
6. Create a user with no policy and a fresh access key; with it call put_account_properties(Properties={'RoleManager/Enabled':'true'}) and confirm AccessDenied with IAM's implicit-deny message for iam:PutAccountProperties on resource '*'.
7. Restore the original RoleManager/Enabled value and re-replay iam.put-account-properties.
