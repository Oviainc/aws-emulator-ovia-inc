Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: chatbot.associate-wrong-resource-type-rejected
Issue-Shape-Id: com.amazonaws.chatbot#WheatleyOrchestration_20171011
Issue-Key: chatbot.associate-wrong-resource-type-rejected|run#5|AssertionError|assert code == "InvalidRequestException", f"unexpected code={code!r}"
Issue-Commit: 6537d79e9f76a83fe65f7d2499927078c6ec62b1

# Chatbot AssociateToConfiguration answers 403 UnauthorizedException for a Resource ARN that is not a custom-action ARN, instead of 400 InvalidRequestException "Invalid request body"

> _Note: this issue was identified against the code base at commit `6537d79e9f76a83fe65f7d2499927078c6ec62b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateToConfiguration

## Description
One root cause. (Note: the open issue `20261102T105418Z-chatbot-associatetoconfiguration-stub.md` describes AssociateToConfiguration as an unimplemented 501 scaffold; it does NOT cover this. The handler is now implemented and the replay shows it answering a modelled 403, not a 501, so this is a distinct fidelity defect in the implemented handler.)

Cause 1 of 1: a `Resource` value that does not match the modelled custom-action ARN form is refused as an authorization failure instead of as malformed client input. Failing check: chatbot.associate-wrong-resource-type-rejected.

AWS: Smithy model `com.amazonaws.chatbot#AssociateToConfiguration`, input member `Resource` -> `ResourceIdentifier`, @required, @length min 1 max 1011, @pattern `^arn:aws:chatbot:[A-Za-z0-9_/.-]{0,63}:[A-Za-z0-9_/.-]{0,63}:custom-action/[a-zA-Z0-9_-]{1,64}$`. An ARN that does not satisfy that pattern — e.g. a chat-configuration ARN such as a Chime webhook configuration ARN, which has no `custom-action/<name>` resource part — is a constraint violation on a modelled input member, so the request never reaches an authorization decision. The operation models `InvalidRequestException` (HTTP 400, documented "Your request input doesn't meet the constraints required by AWS Chatbot") for exactly that class. Recorded behavior: HTTP 400, error code `InvalidRequestException`, error message `Invalid request body`. That message is entirely FIXED text for this error class — it contains no value taken from the request or from state, so nothing in it varies per run. The operation's `UnauthorizedException` (HTTP 403, "The request was rejected because it doesn't have valid credentials for the target resource") is reserved for a credentials/target-resource authorization refusal, not for a shape violation.

Emulator: the handler deliberately takes a resource-scoped authorization decision BEFORE the `Resource` member's modelled @pattern is judged, and the request shape is marked `skip_validation`, so the @pattern constraint is never enforced on the boundary at all. A `Resource` that is not a mintable custom-action ARN therefore falls into the fail-closed branch and the call is refused with the modelled `UnauthorizedException` (403, the shared UNAUTHORIZED_MESSAGE) rather than `InvalidRequestException` (400, "Invalid request body"). Replay: `AssertionError: unexpected code='UnauthorizedException'` where `InvalidRequestException` was expected.

Why: a client that distinguishes input errors from authorization errors is sent down the wrong path. A 403 `UnauthorizedException` reads as "these credentials may not touch that target", so SDK retry/credential-refresh logic and operator runbooks treat a plain malformed-ARN bug as an IAM or cross-account permissions problem; conversely, code that catches `InvalidRequestException` to surface "you passed the wrong kind of ARN" never fires. Because the pattern is never enforced, any syntactically invalid `Resource` — not just a wrong-kind ARN — is misreported the same way, and genuine authorization failures become indistinguishable from client-input failures.

Fix: belongs in this operation's input validation and the ordering of its handler's pre-checks — enforce the modelled @length/@pattern constraints on `Resource` (and `ChatConfiguration`) on the request boundary so a non-conforming ARN is refused 400 `InvalidRequestException` with message `Invalid request body`, and keep the resource-scoped `UnauthorizedException` fail-closed branch for ARNs that are well-formed under the pattern but name no target this account can hold. The shape currently opts out of boundary validation, so the opt-out has to go together with the re-ordering; otherwise the authorization branch keeps shadowing the client-input class.

Traces: chatbot.associate-wrong-resource-type-rejected (the check creates a Chime webhook configuration and calls AssociateToConfiguration with that configuration's own ARN as `Resource`). Code sites for the fixer: services/service-com-amazonaws-chatbot-wheatleyorchestration-20171011/src/ops/associate_to_configuration.rs lines 15-21 (documented decision to skip the member's @pattern via `skip_validation` on `AssociateToConfigurationRequest`) and lines 168-175 (the `is_mintable_custom_action_arn` fail-closed branch returning `UnauthorizedException`).

## Evidence
Check: chatbot.associate-wrong-resource-type-rejected

Expected (AWS / recorded): AssociateToConfiguration(Resource=<chat configuration ARN>, ChatConfiguration=<same ARN>) -> HTTP 400, Error.Code = "InvalidRequestException", Error.Message = "Invalid request body" (fixed text, no request- or state-derived parts), and no association created.

Actual (emulator): HTTP 403, Error.Code = "UnauthorizedException" (message = the service's shared UNAUTHORIZED_MESSAGE).

Replay output:
  File "check_chatbot_associate_wrong_resource_type_rejected.py", line 38, in run
    assert code == "InvalidRequestException", f"unexpected code={code!r}"
  AssertionError: unexpected code='UnauthorizedException'

Model evidence (smithy_query, query="operation", AssociateToConfiguration):
  Resource: target=ResourceIdentifier, required=true, length 1..1011, pattern ^arn:aws:chatbot:[A-Za-z0-9_/.-]{0,63}:[A-Za-z0-9_/.-]{0,63}:custom-action/[a-zA-Z0-9_-]{1,64}$
  errors: InternalServiceError (client/server, 500), InvalidRequestException (client, 400), UnauthorizedException (client, 403)
A chat-configuration ARN cannot match that pattern (no `custom-action/<name>` segment), so it is a modelled-constraint violation -> 400 InvalidRequestException, not a 403 authorization refusal.

Emulator source (same conclusion from the handler's own comments): associate_to_configuration.rs documents that the resource-scoped authorization check runs "BEFORE that member's ARN shape is judged" and that "the member's modelled @pattern is therefore off the boundary (skip_validation ...)", then returns UnauthorizedException when !is_mintable_custom_action_arn(&input.resource).

## Steps to Reproduce
1. Boot the chatbot emulator (crate service-com-amazonaws-chatbot-wheatleyorchestration-20171011).
2. Create an IAM role trusted by chatbot.amazonaws.com and an SNS topic, then call CreateChimeWebhookConfiguration to obtain a ChatConfigurationArn.
3. Call AssociateToConfiguration with Resource = that ChatConfigurationArn and ChatConfiguration = that same ChatConfigurationArn (i.e. a Resource ARN that does not match the modelled custom-action @pattern).
4. Observe the refusal: the emulator answers HTTP 403 with Error.Code "UnauthorizedException"; AWS answers HTTP 400 with Error.Code "InvalidRequestException" and Error.Message "Invalid request body".
5. Re-replay chatbot.associate-wrong-resource-type-rejected to confirm the fix (it then also asserts the rejected ARN does not appear in ListAssociations, which is covered separately by the open ListAssociations stub issue).
