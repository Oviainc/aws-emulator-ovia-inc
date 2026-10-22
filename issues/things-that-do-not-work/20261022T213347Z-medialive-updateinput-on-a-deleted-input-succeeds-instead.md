Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: medialive.input-lifecycle-url-pull
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive.input-lifecycle-url-pull|_error#21|AssertionError|raise AssertionError("call succeeded; expected a ClientError")
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# medialive UpdateInput on a deleted input succeeds instead of returning NotFoundException

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateInput

## Description
One root cause (Cause 1 of 1).

Cause 1 of 1: UpdateInput accepts a mutation against an input id that has already been deleted.

AWS: UpdateInput (Smithy: medialive UpdateInput, PUT /prod/inputs/{InputId}, modeled errors include NotFoundException with httpError 404, plus BadRequestException 400 / ConflictException 409) refuses any mutation of an input that no longer exists. Once DeleteInput has been called and DescribeInput reports State=DELETED, the input id is dead: a further UpdateInput on it is answered with HTTP 404 and error code NotFoundException (recorded behavior; the recorded message, e.g. "inputId=<id> already deleted", is an example only). The Smithy model binds NotFoundException to UpdateInput precisely for this missing-resource class.

Emulator: after the input has been deleted and has settled into State=DELETED, the identical UpdateInput call (changing Name) returns a successful 200 response instead of any error. The check's _error() helper raises "call succeeded; expected a ClientError", i.e. no ClientError of any kind was produced — the emulator treats the deleted id as still mutable.

Why: a client relying on AWS's behavior uses the 404/NotFoundException from a mutation as the authoritative signal that a resource is gone (delete-confirmation loops, idempotent cleanup, reconciliation of desired vs. actual state, retry logic after a failed delete). Against the emulator those code paths never see the terminal error: a deleted input silently accepts writes, so a test that asserts a resource is really gone passes against the emulator and fails against AWS, and a reconciler can resurrect or keep updating state for a resource AWS would consider destroyed. It also hides double-delete/stale-id bugs during development.

Fix: the UpdateInput handler and the input state store for this service — UpdateInput must look the id up in the live-input state (treating DELETED/absent ids as not found) and emit the modeled NotFoundException (HTTP 404) before applying any mutation, the same existence precondition the read path already applies. DeleteInput's state transition must leave the id unusable for mutation rather than merely marked.

Traces: medialive.input-lifecycle-url-pull — the check creates a URL_PULL input, round-trips create/describe/update/delete successfully (those assertions pass), then calls UpdateInput on the deleted id and fails at line 51 with "call succeeded; expected a ClientError".

## Evidence
Check medialive.input-lifecycle-url-pull, verdict mismatch.

Expected (AWS, recorded): UpdateInput(InputId=<deleted id>, Name=...) -> HTTP 404, Error.Code = "NotFoundException" (recorded message example: "inputId=<id> already deleted").

Actual (emulator): the call returns a successful response. Replay output:
  File "check_medialive_input_lifecycle_url_pull.py", line 51, in run
    mutation_code, mutation_message = _error(lambda: medialive.update_input(InputId=input_id, Name=new_name + "-dead"))
  File "check_medialive_input_lifecycle_url_pull.py", line 69, in _error
    raise AssertionError("call succeeded; expected a ClientError")
  AssertionError: call succeeded; expected a ClientError

Preceding steps in the same check succeeded, so the divergence is isolated to the post-delete mutation: create returned State=DETACHED, DescribeInput round-tripped the source URL, UpdateInput before deletion persisted Name and Sources, DeleteInput was accepted and DescribeInput settled on State=DELETED.

Smithy model (services/service-com-amazonaws-medialive-medialive/smithy-model/com-amazonaws-medialive-medialive.json): UpdateInput errors = [BadGatewayException 502, BadRequestException 400, ConflictException 409, ForbiddenException 403, GatewayTimeoutException 504, InternalServerErrorException 500, NotFoundException 404].

## Steps to Reproduce
1. CreateInput(Name=<unique>, Type="URL_PULL", Sources=[{"Url": "https://example.invalid/hls/a.m3u8"}]) and note the returned Input.Id.
2. DeleteInput(InputId=<id>).
3. Poll DescribeInput(InputId=<id>) until State == "DELETED".
4. Call UpdateInput(InputId=<id>, Name="<anything>").
   Expected: HTTP 404 with Error.Code "NotFoundException".
   Actual: a 200 success response with no error.

Or replay the behavioral check directly: medialive.input-lifecycle-url-pull.
