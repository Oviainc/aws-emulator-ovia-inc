Issue-Source: trace-sweep-divergence
Issue-Trace: chatbot-resource-not-found-error-paths
Issue-Step: task#2
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chatbot-resource-not-found-error-paths
Issue-Shape-Id: com.amazonaws.chatbot#WheatleyOrchestration_20171011
Issue-Key: chatbot|DeleteCustomAction|4xx->4xx|$.Message
Issue-Commit: ca72e8d5fc2000c02a01dcf678a8fed8074b0f2e

# Chatbot DeleteCustomAction not-found error emits a Message member AWS does not send

> _Note: this issue was identified against the code base at commit `ca72e8d5fc2000c02a01dcf678a8fed8074b0f2e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCustomAction

## Description
The chatbot emulator's DeleteCustomAction not-found path serializes a populated `Message` member into the ResourceNotFoundException body, where AWS answers the same request with a 404 ResourceNotFoundException that carries no `Message` at all. One cause, in the chatbot service.

Cause 1 of 1: DeleteCustomAction on a custom-action ARN that does not exist returns 404 ResourceNotFoundException with an extra `Message` member.
AWS: Smithy (services/service-com-amazonaws-chatbot-wheatleyorchestration-20171011/smithy-model/com-amazonaws-chatbot-wheatleyorchestration-20171011.json): DeleteCustomAction (POST /delete-custom-action, success http code 204, @idempotent) takes only CustomActionArn (required, @length 1..1011, pattern ^arn:aws:chatbot:[A-Za-z0-9_/.-]{0,63}:[A-Za-z0-9_/.-]{0,63}:custom-action/[a-zA-Z0-9_-]{1,64}$) and models errors InternalServiceError (500), InvalidRequestException (400), ResourceNotFoundException (errorKind client, httpError 404) and UnauthorizedException (403). structure ResourceNotFoundException has exactly one member, `Message` (target ErrorMessage), and it is NOT required - the model permits the error body to omit it. For the delete path AWS exercises that: recorded behavior only - the recorded 404 response for a delete of an absent custom action has x-amzn-errortype ResourceNotFoundException and a body with no `Message` member (the exception is serialized with its optional message absent). Note AWS does send a message for the sibling operations on the same resource and words them differently per operation (UpdateCustomAction: fixed text `Sdk action with id [` ... `] does not exist in account [` ... `]`; GetCustomAction: `sdk action [` ... `] not found for account [` ... `]`, with the action name taken from the request's CustomActionArn and the account id from the caller's account), so the message text is per-operation and for DeleteCustomAction there is none.
Emulator: 404 with errortype ResourceNotFoundException (status and error code correct), but the body additionally carries `Message` set to the UpdateCustomAction-shaped text - fixed text `Sdk action with id [` ... `] does not exist in account [` ... `]`, with the action-name part taken from the request's CustomActionArn and the account-id part from the emulator's account. AWS expects no `Message` key here, so the replayed step reports $.Message as a type mismatch (expected None, got a string).
Why: a client that inspects the error body sees a field AWS never populates for this operation. Round-trip assertions, recorded-response fixtures and error-shape comparisons that expect ResourceNotFoundException from a delete to have no message fail against the emulator, and tooling that branches on message presence (for example treating a message-bearing 404 as a different error class from a bare one, or asserting that delete is message-silent while update is not) takes the wrong branch. It also hides the real requirement that DeleteCustomAction, GetCustomAction and UpdateCustomAction each have their own not-found rendering rather than one shared string.
Fix: the chatbot service crate's DeleteCustomAction handler / its error serializer - the not-found branch must raise ResourceNotFoundException without a message for this operation instead of reusing the shared `Sdk action with id [...] does not exist in account [...]` formatter used by UpdateCustomAction, so the serialized body omits the optional `Message` member.
Traces: chatbot-resource-not-found-error-paths#task#2.

Related open issues (none covers this cause): 20261030T215059Z-chatbot-custom-action-read-and-update-paths-drop.md covers the SAME fixture but step task#3 (UpdateCustomAction answering 403 UnauthorizedException instead of 404 with a message) and the dropped GetCustomAction members - the opposite direction of this defect (missing message there, surplus message here); 20261030T111746Z-chatbot-untagresource-on-a-missing-resource-returns-403.md covers UntagResource; 20261029T220801Z-chatbot-deletecustomaction-stub.md asks for the operation to exist at all and does not specify its not-found body.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 8 fixtures, 67 steps, 54 passed).

Divergence cluster service=chatbot|DeleteCustomAction|status=404->404|keys=$.Message, verdict=mismatch, affected_count=1:
  fixture chatbot-resource-not-found-error-paths, step task#2, operation DeleteCustomAction
  expected_status 404, actual_status 404 (status and x-amzn-errortype agree)
  detail: $.Message: type_mismatch (expected None, got 'Sdk action with id [<ActionName>] does not exist in account [<accountId>]')

The recorded AWS 404 body for this step has no $.Message member; the emulator's 404 body has one. In the recorded text the fixed part is `Sdk action with id [` ... `] does not exist in account [` ... `]`; the bracketed parts are the custom-action name from the request's CustomActionArn and the caller's account id - per-run values, not part of the requirement. The requirement being violated is the member's presence, not its value.

Model grounding: DeleteCustomAction errors include ResourceNotFoundException (errorKind client, httpError 404); structure ResourceNotFoundException { Message: ErrorMessage } with Message optional.

## Steps to Reproduce
1. Clone this checkout, boot the emulator and replay trace set simparity-wire-aws-v1.
2. Fixture chatbot-resource-not-found-error-paths, step task#2: POST /delete-custom-action with a CustomActionArn of the form arn:aws:chatbot:<region>:<account>:custom-action/<name> for a custom action that was never created.
3. Observe the response: 404 with x-amzn-errortype ResourceNotFoundException (correct) but a JSON body containing a `Message` member reading `Sdk action with id [<name>] does not exist in account [<account>]`.
4. Expected: 404 ResourceNotFoundException with no `Message` member in the body, as AWS records for this operation.
