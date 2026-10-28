Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm-contacts.describe-engagement-without-replication-set-rejected ssm-contacts.get-rotation-without-replication-set-rejected
Issue-Shape-Id: com.amazonaws.ssmcontacts#SSMContacts
Issue-Key: ssm-contacts.describe-engagement-without-replication-set-rejected|run#6|AssertionError|assert message == f"Invalid value provided - Invalid resource Arn {fake_engagement_arn}", (
Issue-Key: ssm-contacts|GetRotation|ValidationException
Issue-Commit: 13cd51356c86db6d039912819137db7f55e61de7

# Incident Manager account gate refuses DescribeEngagement/GetRotation with the wrong ValidationException message ("Account not found for the request" instead of "Invalid resource Arn <arn>")

> _Note: this issue was identified against the code base at commit `13cd51356c86db6d039912819137db7f55e61de7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEngagement

## Description
Cause 1 of 1: the shared replication-set (account) precondition refusal in ssm-contacts words its ValidationException message about the ACCOUNT rather than about the resource ARN the request named, so every ARN-addressed read gated by it answers with a message real AWS never emits. Failing checks: ssm-contacts.describe-engagement-without-replication-set-rejected, ssm-contacts.get-rotation-without-replication-set-rejected.

AWS: Smithy (services/service-com-amazonaws-ssmcontacts-ssmcontacts/smithy-model/com-amazonaws-ssmcontacts-ssmcontacts.json) models DescribeEngagement.EngagementId and GetRotation.RotationId as SsmContactsArn (@length 1..2048, pattern ^arn:(aws|aws-cn|aws-us-gov):ssm-contacts:[-\w+=/,.@]*:[0-9]+:([\w+=/,.@:-]+)*$), and both operations declare ValidationException (errorKind client, httpError 400) alongside ResourceNotFoundException (404). The model does not state the message text, so the message requirement is recorded behavior only: on an account with no active Incident Manager replication set, real AWS refuses BOTH operations for a well-formed but never-created ARN with HTTP 400, Error.Code "ValidationException" and Error.Message "Invalid value provided - Invalid resource Arn <arn>". Fixed text: "Invalid value provided - Invalid resource Arn "; variable part: the ARN string taken verbatim from the request member (EngagementId for DescribeEngagement, RotationId for GetRotation). The recorded check notes this wording holds for both hyphenated and pure-hex resource-id segments, i.e. it is not an ARN-syntax diagnosis — AWS cannot resolve ANY replication-set-scoped ARN on a never-onboarded account and words the refusal against the ARN.

Emulator: the status (400) and code (ValidationException) are right, but the message is the account-worded constant "Invalid value provided - Account not found for the request" for both operations. It contains no ARN at all, and its fixed text differs from AWS's. Both gated handlers delegate to one shared state helper, which is why both diverge identically.

Why: a client that distinguishes Incident Manager failure classes by the ValidationException message — the normal way to tell this precondition/ARN-resolution refusal from a member-level validation failure, since the modelled reason/fields carry no resource identity — matches nothing, and a client that echoes or logs the offending ARN from the message loses it entirely. Any parity assertion on the message fails even though code and status agree.

Fix: the shared account precondition in the service state — ComAmazonawsSsmcontactsSsmcontactsState::require_active_replication_set — must produce the ARN-worded message, which means the refusal has to be given the request's ARN (or the per-operation account_precondition_failed helpers must word it) instead of returning a request-independent constant. Both call sites pass through the same helper, so one change fixes both; peers of these two operations that share the gate must be kept consistent with the same wording.

Traces: ssm-contacts.describe-engagement-without-replication-set-rejected (DescribeEngagement with a well-formed engagement/<id>/<id> ARN); ssm-contacts.get-rotation-without-replication-set-rejected (GetRotation with a well-formed rotation/<id> ARN, after a passing ListContacts positive control). Emulator call sites: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/describe_engagement.rs lines 203-205 and 233-239; services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/get_rotation.rs lines 190-192 and 232-238; shared helper documented at services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/state.rs lines 26-33.

## Evidence
Expected (real AWS, both operations, account with no active replication set):
  HTTP 400, Error.Code = "ValidationException"
  Error.Message = "Invalid value provided - Invalid resource Arn <the ARN supplied in EngagementId/RotationId>"

Actual (emulator, both operations):
  HTTP 400, Error.Code = "ValidationException" (correct)
  Error.Message = "Invalid value provided - Account not found for the request"

Assertion from the suite:
  check_ssm_contacts_describe_engagement_without_replication_set_rejected.py:35
    assert message == f"Invalid value provided - Invalid resource Arn {fake_engagement_arn}"
    AssertionError: Invalid value provided - Account not found for the request
  check_ssm_contacts_get_rotation_without_replication_set_rejected.py:55
    assert message == f"Invalid value provided - Invalid resource Arn {fake_rotation_arn}"
    AssertionError: Invalid value provided - Account not found for the request

Message anatomy: fixed text "Invalid value provided - Invalid resource Arn " (required verbatim); variable suffix = the ARN value from the request member. The account/region/resource-id values in any single recording are per-run fill-ins, not part of the requirement.

## Steps to Reproduce
1. Boot the emulator with a fresh ssm-contacts state (no replication set has been created, which is the default).
2. Confirm the service is otherwise healthy: ssm-contacts ListContacts with AliasPrefix=<unique> returns Contacts == [].
3. Call DescribeEngagement with EngagementId = arn:aws:ssm-contacts:<region>:<account>:engagement/<name>/<id> (never created, matches the SsmContactsArn pattern).
   -> emulator: 400 ValidationException, message "Invalid value provided - Account not found for the request"; AWS: 400 ValidationException, message "Invalid value provided - Invalid resource Arn <that same ARN>".
4. Call GetRotation with RotationId = arn:aws:ssm-contacts:<region>:<account>:rotation/<id> (never created).
   -> same divergence.
5. Re-replay ssm-contacts.describe-engagement-without-replication-set-rejected and ssm-contacts.get-rotation-without-replication-set-rejected to confirm a fix.
