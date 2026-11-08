Issue-Source: trace-sweep-divergence
Issue-Trace: securityagent-batch-security-requirements-errors
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securityagent-batch-security-requirements-errors
Issue-Shape-Id: com.amazonaws.securityagent#SecurityAgent
Issue-Key: securityagent|BatchGetSecurityRequirements|2xx->2xx|$.errors[*].code
Issue-Key: securityagent|BatchGetSecurityRequirements|2xx->2xx|$.errors[*].message
Issue-Key: securityagent|BatchCreateSecurityRequirements|2xx->2xx|$.errors[*].message
Issue-Commit: 77fc38481d0944218df9b5c2964437ecad54bbf8

# Batch security requirement item errors carry wrong code and wording

> _Note: this issue was identified against the code base at commit `77fc38481d0944218df9b5c2964437ecad54bbf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetSecurityRequirements

## Description
Two net-new causes, both in the per-item `errors` list the batch security-requirement operations return. The HTTP status and the response shape are right; the item error's `code` and `message` are not AWS's.

Cause 1 of 2 (fixture securityagent-batch-security-requirements-errors, step task#4, BatchGetSecurityRequirements): the per-item not-found error uses the exception shape name as its code and the emulator's own sentence as its message.
AWS: Smithy: BatchGetSecurityRequirements returns errors (BatchSecurityRequirementErrors, @required) of BatchSecurityRequirementError{securityRequirementName, code, message}, all three @required, with code documented as "The error code" and message as "The error message" — both free-form Strings, so the model does not fix the vocabulary and the recording is the authority. The recording shows code `ResourceNotFound` (the batch item error code, which is NOT the modeled exception shape name ResourceNotFoundException — that name belongs to the call-level 404 error) and message `Security requirement with name: <name> was not found`: fixed text `Security requirement with name: ` + the requested name verbatim from the request + ` was not found`.
Emulator: code `ResourceNotFoundException` and message `No security requirement found for name: <name>` — a different code token and different fixed text (the name itself is taken from the request correctly).
Why: an item error code is the field a client branches on to tell "this one name is absent" from a throttle or a validation failure, and it is the one part of a partial-success response that is machine-readable. A client matching `ResourceNotFound` sees no match, falls through to its unknown-error path, and treats a normal missing-name outcome as an unexpected failure; log and metric pipelines keyed on the code mis-bucket it.
Fix: the batch item error vocabulary in crate::state — the BATCH_REQUIREMENT_ERROR_NOT_FOUND constant and the security_requirement_not_found_message() helper used by the BatchGetSecurityRequirements handler (and by every other batch handler that reports a missing name).

Cause 2 of 2 (same fixture, step task#3, BatchCreateSecurityRequirements): the per-item duplicate-name error message is not AWS's sentence.
AWS: Smithy: BatchCreateSecurityRequirements (POST /BatchCreateSecurityRequirements, HTTP 201) returns securityRequirements (@required) and errors (@required) of the same BatchSecurityRequirementError; a name that already exists in the pack is reported per item rather than failing the call (the call-level ConflictException, HTTP 409, is a different situation). Recorded behavior for the wording: message `Security requirement name must be unique within a pack. Security requirement with name: <name> already exists` — fixed text `Security requirement name must be unique within a pack. Security requirement with name: ` + the requested name verbatim + ` already exists`. The error's code matched and the status (201) matched.
Emulator: message `Security requirement already exists for name: <name>` — the uniqueness rule sentence is dropped and the remaining text is reworded.
Why: the message is the only place AWS states the rule the caller broke (names are unique WITHIN a pack, so the same name is legal in another pack). A client or operator reading the emulator's sentence cannot tell the scope of the constraint, and assertions on AWS's documented wording fail.
Fix: the duplicate-name message helper in crate::state used by the BatchCreateSecurityRequirements handler when it reports an existing name in `errors`.

In both causes the only variable part of the message is the security requirement name, taken verbatim from the request's entry; everything else is fixed text and must match AWS's wording exactly.

Traces: securityagent-batch-security-requirements-errors#task#4 (BatchGetSecurityRequirements item code + message), #task#3 (BatchCreateSecurityRequirements item message).

## Evidence
Fixture securityagent-batch-security-requirements-errors, verdict mismatch.

task#4 BatchGetSecurityRequirements — expected status 200, actual 200:
  $.errors[0].code: value_mismatch — expected 'ResourceNotFound', got 'ResourceNotFoundException'
  $.errors[0].message: value_mismatch — expected 'Security requirement with name: missing-req-xyz was not found', got 'No security requirement found for name: missing-req-xyz'
  ('missing-req-xyz' is the name the request asked for — the variable part; the surrounding text is the fixed part that diverges.)

task#3 BatchCreateSecurityRequirements — expected status 201, actual 201:
  $.errors[0].message: value_mismatch — expected 'Security requirement name must be unique within a pack. Security requirement with name: <requested name> already exists', got 'Security requirement already exists for name: <requested name>'
  (The recorded name carries a per-run probe suffix and is not itself a requirement; it is echoed from the request in both responses.)

Smithy: BatchSecurityRequirementError{securityRequirementName, code, message} all @required, code doc "The error code", message doc "The error message"; BatchGetSecurityRequirements/BatchCreateSecurityRequirements both return errors (BatchSecurityRequirementErrors) @required; BatchCreateSecurityRequirements http code 201; the modeled call-level errors are ResourceNotFoundException (404) and ConflictException (409), which are distinct from the batch item code.

Citations: services/service-com-amazonaws-securityagent-securityagent/src/ops/batch_get_security_requirements.rs:200-204 (code = crate::state::BATCH_REQUIREMENT_ERROR_NOT_FOUND, message = crate::state::security_requirement_not_found_message(name)); services/service-com-amazonaws-securityagent-securityagent/src/state.rs (that constant and the duplicate-name message helper used by batch_create_security_requirements.rs).

## Steps to Reproduce
1. Boot the emulator (fresh state).
2. sa = boto3.client("securityagent", endpoint_url=<emulator>)
3. pack = sa.create_security_requirement_pack(...) -> packId
4. sa.batch_create_security_requirements(packId=packId, securityRequirements=[{name: 'req-existing', description: '...'}])
5. Repeat step 4 with the same name: observe HTTP 201 with errors[0].message == 'Security requirement already exists for name: req-existing' instead of AWS's 'Security requirement name must be unique within a pack. Security requirement with name: req-existing already exists' (cause 2).
6. sa.batch_get_security_requirements(packId=packId, securityRequirementNames=['missing-req-xyz']): observe HTTP 200 with errors[0].code == 'ResourceNotFoundException' and errors[0].message == 'No security requirement found for name: missing-req-xyz' instead of code 'ResourceNotFound' and message 'Security requirement with name: missing-req-xyz was not found' (cause 1).
7. Replay fixture securityagent-batch-security-requirements-errors steps task#4 and task#3 to confirm.
