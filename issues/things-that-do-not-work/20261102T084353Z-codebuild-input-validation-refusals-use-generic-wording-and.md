Issue-Source: trace-sweep-divergence
Issue-Trace: codebuild-cbnv-batch-get-malformed-requests
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codebuild-cbnv-batch-get-malformed-requests codebuild-cbnv-build-history-route codebuild-cbnv-list-projects-enum-casing-then-describe
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild|BatchGetProjects|4xx->4xx|$.message
Issue-Key: codebuild|BatchGetProjects|4xx->4xx|$.Message
Issue-Key: codebuild|BatchGetProjects|4xx->4xx|$.__type
Issue-Key: codebuild|BatchGetBuilds|4xx->4xx|$.message
Issue-Key: codebuild|ListProjects|4xx->4xx|$.message
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# CodeBuild input-validation refusals use generic wording and the wrong error code (4 causes)

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetProjects

## Description
FOUR net-new root causes in the codebuild crate (services/service-com-amazonaws-codebuild-codebuild-20161006): the emulator refuses malformed input with a generic constraint-violation template ("N validation error detected: Value ... at '<member>' failed to satisfy constraint: ...") where CodeBuild emits its own short service messages, and in one case with the wrong error code entirely. The open issues 20261029T175929Z-codebuild-absent-resource-and-invalid-input-error-messages.md and 20261029T204615Z-codebuild-error-message-wording-diverges-from-aws-on.md describe the same FAMILY of wording defects but carry no replay key for any of the four steps below, so these four must be closed and these fixtures replayed. ALL FOUR must be closed before this issue is resolved.

Cause 1 of 4: an empty or absent BatchGetProjects `names` list is refused with the generic constraint message.
AWS: Smithy BatchGetProjects(names @required ProjectNames @length 1-100) -> error InvalidInputException (client, HTTP 400). A request whose names list is empty or omitted is refused with HTTP 400, __type 'InvalidInputException' and the message 'The input project name list cannot be empty.' - fixed text in full, with no member name, count or value interpolated.
Emulator: HTTP 400 InvalidInputException whose message is "1 validation error detected: Value null at 'names' failed to satisfy constraint: Member must not be null" - a generic smithy-constraint rendering naming the member and the offending value.
Why: the message is the only distinguishing payload on this refusal (status and __type already match), so a client or test that matches CodeBuild's documented text never recognizes the condition, and operators reading logs see a message AWS never emits.
Fix: the BatchGetProjects handler's input validation - emit the service's own empty-list message instead of the generic constraint renderer for the names member.
Traces: codebuild-cbnv-batch-get-malformed-requests#task#3 (same cluster also covers task#5 and task#7 of that fixture, the omitted-member and empty-list variants).

Cause 2 of 4: a `names` member of the wrong JSON type is reported as a ValidationException instead of a SerializationException.
AWS: a request whose `names` member is not a JSON list fails DESERIALIZATION, before input validation: HTTP 400 with __type 'SerializationException' and a capital-M 'Message' member reading 'Expected list or null' (fixed text).
Emulator: HTTP 400 with __type 'ValidationException' and no 'Message' member at all - the wrong error code, and the message carried under a different member name.
Why: SerializationException signals a malformed wire payload and ValidationException a semantically invalid one; a client that branches on the code (and SDK error classes do) mishandles a corrupt request body, and the 'Message' member clients read for serialization errors is absent.
Fix: the JSON protocol deserializer for codebuild inputs - a type mismatch on a list member must surface as SerializationException with the 'Message' member, not be funnelled into the validation layer.
Traces: codebuild-cbnv-batch-get-malformed-requests#task#4.

Cause 3 of 4: an empty BatchGetBuilds `ids` list is refused with the generic constraint message.
AWS: Smithy BatchGetBuilds(ids @required BuildIds @length 1-100) -> error InvalidInputException (client, HTTP 400). An empty ids list yields HTTP 400, __type 'InvalidInputException', message 'The input build ID list cannot be empty.' (fixed text, nothing interpolated).
Emulator: HTTP 400 InvalidInputException whose message is "1 validation error detected: Value '0 items' at 'ids' failed to satisfy constraint: Member must have length greater than or equal to 1".
Why: same as Cause 1 - the refusal is only identifiable by its text, and the emulator's text matches no AWS response.
Fix: the BatchGetBuilds handler's input validation - emit the service's empty-list message for the ids member.
Traces: codebuild-cbnv-build-history-route#task#8.

Cause 4 of 4: an unknown ListProjects `sortBy` value is refused with the generic enum-constraint message.
AWS: Smithy ListProjects(sortBy ProjectSortByType enum [NAME, CREATED_TIME, LAST_MODIFIED_TIME]) -> error InvalidInputException (client, HTTP 400). A value outside the enum (including a wrong-case one such as a lowercase name) is refused with HTTP 400, __type 'InvalidInputException' and the message 'Invalid sortBy type specified: <the submitted value>' - the prefix 'Invalid sortBy type specified: ' is fixed and the trailing token is the value taken verbatim from the request.
Emulator: HTTP 400 InvalidInputException whose message is "1 validation error detected: Value '<value>' at 'sortBy' failed to satisfy constraint: Member must satisfy enum value set: [NAME, CREATED_TIME, LAST_MODIFIED_TIME]".
Why: enum casing is a very common client bug; the emulator's message trains callers to expect a smithy constraint report and defeats any assertion on CodeBuild's wording.
Fix: the ListProjects handler's input validation / enum coercion - emit 'Invalid sortBy type specified: <value>'.
Traces: codebuild-cbnv-list-projects-enum-casing-then-describe#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (165 steps, 110 passed, 55 mismatched, score 66.7%). None of these four clusters is owned by an open issue or blocked by a stub.

Cause 1 - codebuild-cbnv-batch-get-malformed-requests#task#3 BatchGetProjects, 400/400: $.message expected 'The input project name list cannot be empty.', got "1 validation error detected: Value null at 'names' failed to satisfy constraint: Member must not be null". Cluster also covers task#5 and task#7 of the same fixture.
Cause 2 - codebuild-cbnv-batch-get-malformed-requests#task#4 BatchGetProjects, 400/400: $.__type expected 'SerializationException', got 'ValidationException'; $.Message missing (expected 'Expected list or null').
Cause 3 - codebuild-cbnv-build-history-route#task#8 BatchGetBuilds, 400/400: $.message expected 'The input build ID list cannot be empty.', got "1 validation error detected: Value '0 items' at 'ids' failed to satisfy constraint: Member must have length greater than or equal to 1".
Cause 4 - codebuild-cbnv-list-projects-enum-casing-then-describe#task#3 ListProjects, 400/400: $.message expected 'Invalid sortBy type specified: name' (for the submitted value 'name'), got "1 validation error detected: Value 'name' at 'sortBy' failed to satisfy constraint: Member must satisfy enum value set: [NAME, CREATED_TIME, LAST_MODIFIED_TIME]".

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json - BatchGetProjects(names @required ProjectNames @length 1-100; error InvalidInputException client 400), BatchGetBuilds(ids @required BuildIds @length 1-100; error InvalidInputException client 400), ListProjects(sortBy ProjectSortByType values [NAME, CREATED_TIME, LAST_MODIFIED_TIME]; error InvalidInputException client 400), InvalidInputException {message}. SerializationException is not modeled - it is the protocol-level deserialization error shown by the recording, hence 'recorded behavior only' for its 'Message' member.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the wire corpus (replay_wire_corpus) and read the codebuild divergences, or reproduce each cause directly against the emulator's data listener:
3. Cause 1: POST CodeBuild_20161006.BatchGetProjects with body {} (and with {"names": []}) - expect HTTP 400 InvalidInputException 'The input project name list cannot be empty.'; observe the generic '1 validation error detected: ...' message.
4. Cause 2: POST CodeBuild_20161006.BatchGetProjects with {"names": "not-a-list"} - expect HTTP 400 __type SerializationException with Message 'Expected list or null'; observe __type ValidationException and no Message.
5. Cause 3: POST CodeBuild_20161006.BatchGetBuilds with {"ids": []} - expect HTTP 400 InvalidInputException 'The input build ID list cannot be empty.'; observe the generic constraint message.
6. Cause 4: POST CodeBuild_20161006.ListProjects with {"sortBy": "name"} - expect HTTP 400 InvalidInputException 'Invalid sortBy type specified: name'; observe the generic enum-constraint message.
