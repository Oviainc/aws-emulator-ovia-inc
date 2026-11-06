Issue-Severity: MEDIUM
Issue-Commit: ed195bdda9ffdbdf051f799fc5087e4de9084b05

# iam: PutAccountProperties refuses a property key without the Namespace/Name separator as ValidationError instead of the modeled InvalidInput

> _Note: this issue was identified against the code base at commit `ed195bdda9ffdbdf051f799fc5087e4de9084b05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAccountProperties

## Description
Cause 1 of 1: PutAccountProperties classifies a well-formed-but-unqualified property key as a request-shape violation rather than as the business-rule refusal IAM models for it, so the caller gets the wrong error code (and the wrong message class) for an input that satisfies every modeled constraint.

AWS: the on-disk Smithy model (services/service-com-amazonaws-iam-awsidentitymanagementv20100508) declares PutAccountProperties with required input member Properties -> accountPropertiesMapType, whose KEY shape accountPropertyKeyType carries @length(min 1, max 50) and @pattern "^[A-Za-z][A-Za-z0-9/_-]*$", and declares the operation's errors as ConcurrentModificationException, InvalidInputException and ServiceFailureException. InvalidInputException is a client error with httpError 400 and awsQueryError code "InvalidInput" ("The request was rejected because an invalid or out-of-range value was supplied for an input parameter"). A key such as "NoSlashHere" SATISFIES the modeled pattern and length — the slash is not required by the pattern — so the requirement that a key be qualified as Namespace/PropertyName is business logic, and IAM refuses it with HTTP 400, error code InvalidInput and the message, as a template: "Property key must be in format Namespace/PropertyName: <offending-key>". Fixed text, exactly as AWS words it: "Property key must be in format Namespace/PropertyName: "; the single variable part is the offending key taken verbatim from the request. The rejected write has no effect: the bad key does not appear in GetAccountProperties and no other property changes.
Emulator: the same call is refused with error code ValidationError — the class reserved for a violation of the modeled shape constraints — so a caller is told its request was malformed on the wire rather than that the key is unqualified, and the error code does not appear among the operation's modeled errors at all.
Why: the code is the branch a client switches on. ValidationError is not in PutAccountProperties' modeled error set, so a generated client or a retry/classification layer built from the model has no case for it and falls through to a generic failure; a caller that handles InvalidInputException — the only input-rejection error IAM models here — never sees its handler run. It also misattributes the defect: the key is shape-valid, and a client that trusts ValidationError will try to 'fix' a pattern that was already satisfied.
Fix: PutAccountProperties' validation in the iam service crate — after the modeled shape constraints pass, the Namespace/PropertyName check must raise InvalidInputException (awsQuery code InvalidInput, HTTP 400) with AWS's "Property key must be in format Namespace/PropertyName: <key>" message, leaving account properties untouched.
Traces: iam.put-account-properties.

## Evidence
Suite simparity-behavioral-aws-v1 replayed against this checkout: 451/793 passed; service heading `### iam` 329/456. The check is marked neither ALREADY FILED nor OWNED in the report.

[mismatch] iam.put-account-properties, at the malformed-key leg:
  AssertionError: expected InvalidInput for a malformed property key, got ValidationError

The legs that passed first, so the operation itself works: GetAccountProperties read the current RoleManager/Enabled; PutAccountProperties(Properties={'RoleManager/Enabled': <flipped value>}) was accepted and GetAccountProperties reflected it.
The failing request: PutAccountProperties(Properties={'NoSlashHere': 'true'}).
  Expected: error code InvalidInput (InvalidInputException, HTTP 400) with message "Property key must be in format Namespace/PropertyName: NoSlashHere", the key absent from GetAccountProperties afterwards and RoleManager/Enabled unchanged.
  Actual: error code ValidationError. The key 'NoSlashHere' matches the modeled key pattern ^[A-Za-z][A-Za-z0-9/_-]*$ and its 1..50 length, so no shape constraint was violated.
The later legs of the check (the zero-permission caller's AccessDenied message and the restore) are downstream of this assertion and were not reached.

## Steps to Reproduce
1. Boot the emulator from this checkout; point an iam client at it.
2. Call GetAccountProperties and note RoleManager/Enabled.
3. Call PutAccountProperties(Properties={'RoleManager/Enabled': <the other boolean>}) and confirm GetAccountProperties reflects it.
4. Call PutAccountProperties(Properties={'NoSlashHere': 'true'}).
5. Observed: a 400 with Error.Code 'ValidationError'. Expected: Error.Code 'InvalidInput' with Error.Message 'Property key must be in format Namespace/PropertyName: NoSlashHere'.
6. Call GetAccountProperties again and confirm 'NoSlashHere' is absent and RoleManager/Enabled is unchanged; then restore the original value.
