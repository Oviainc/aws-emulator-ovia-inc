Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.list-connection-functions-reflects-created-function
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|DescribeConnectionFunction|EntityNotFound
Issue-Commit: a71d1dc7ff666cd55a918c22c2ad4069b0e12fff

# cloudfront DescribeConnectionFunction's EntityNotFound message is emulator-authored ("The entity was not found: no connection function <id> in the <stage> stage") instead of AWS's fixed "The specified connection function does not exist."

> _Note: this issue was identified against the code base at commit `a71d1dc7ff666cd55a918c22c2ad4069b0e12fff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConnectionFunction

## Description
This issue carries ONE net-new root cause in the cloudfront service. A fixer must close it before submitting. The failing check is marked neither ALREADY FILED nor OWNED in the replay report.

Related issues that do NOT cover this cause: 20261026T092638Z-cloudfront-three-error-contract-divergences-entitynotfound-messages-append.md (FIXED; same message-fidelity family, but its cause 1 covers only GetAnycastIpList and GetConnectionGroupByRoutingEndpoint — not the connection-function handlers, which still author their own sentence) and 20261027T155218Z-cloudfront-listconnectionfunctions-enum-validation-message-lists-the-functionstage.md (OPEN; ListConnectionFunctions InvalidArgument enum message, a different operation and a different error code).

Cause 1 of 1: the EntityNotFound refusal of DescribeConnectionFunction for an identifier that does not exist returns an emulator-authored sentence that embeds the looked-up identifier and the requested stage, instead of CloudFront's fixed per-code sentence.

AWS: Smithy: DescribeConnectionFunction (GET /2020-05-31/connection-function/{Identifier}/describe, success 200) models exactly AccessDenied (403), EntityNotFound (404), InvalidArgument (400), UnsupportedOperation (400); its input is Identifier (httpLabel, required) plus Stage (httpQuery, enum FunctionStage). When the identifier names no connection function in the requested stage — including an id whose function was just deleted — the call is refused 404 EntityNotFound (shape com.amazonaws.cloudfront#EntityNotFound, doc "The entity was not found.", single Message member). For this operation the Message is a CONSTANT sentence that carries nothing from the request (recorded behavior): "The specified connection function does not exist." — the entire sentence is fixed text; there is no variable part, in particular no function id and no stage name. The same constant governs the EntityNotFound refusal of a repeat DeleteConnectionFunction (DELETE /2020-05-31/connection-function/{Id}, 204 on success, EntityNotFound 404 modeled) for the same dead id — the check asserts that leg next but never reached it.

Emulator: the status (404) and the code (EntityNotFound) match, but the message is a different, emulator-authored sentence built from the request: "The entity was not found: no connection function <Identifier> in the <Stage> stage" (replay, verbatim: 'The entity was not found: no connection function cf_7IYCGPKTANSF3 in the DEVELOPMENT stage'). The id token and the stage token are request-derived values real AWS never sends here; the surrounding text ("The entity was not found: no connection function ... in the ... stage") also differs from AWS's fixed sentence, and the trailing period is dropped. Everything before this point in the check passed: CreateConnectionFunction, ListConnectionFunctions reflecting the new id, DeleteConnectionFunction with the create ETag, and the id disappearing from the list all behaved, so only the refusal text diverges.

Why: a client or golden-file test comparing the documented per-code sentence sees a mismatch on every connection-function not-found path, and the appended identifier/stage leaks caller input into an error contract that carries none — code that parses the message to recover a resource name or a stage gets a value AWS never emits, while code matching the documented sentence to distinguish "function absent" from other 404s never matches at all.

Fix: the EntityNotFound message construction used by the connection-function handlers of the cloudfront service (DescribeConnectionFunction's handler and the shared not-found/error constructor in the service's state/error module that DeleteConnectionFunction also uses) — serve the fixed per-code sentence with no request-derived identifier or stage suffix, and keep the terminating period.

Traces: cloudfront.list-connection-functions-reflects-created-function.

## Evidence
Replay output, verbatim tail (cloudfront.list-connection-functions-reflects-created-function, verdict mismatch):

  File "check_cloudfront_list_connection_functions_reflects_created_function.py", line 112, in run
    assert message == "The specified connection function does not exist.", f"unexpected message {message!r}"
  AssertionError: unexpected message 'The entity was not found: no connection function cf_7IYCGPKTANSF3 in the DEVELOPMENT stage'

The assertion is reached only after the error CODE already matched: the check first asserts `code == "EntityNotFound"` (passed), then compares `exc.response["Error"]["Message"]`. The refused call is `cf.describe_connection_function(Identifier=fn_id, Stage="DEVELOPMENT")` issued after `cf.delete_connection_function(Id=fn_id, IfMatch=fn_etag)` succeeded and after polling confirmed the id no longer appears in ListConnectionFunctions.

Fixed text vs variable parts of the emulator's message: 'cf_7IYCGPKTANSF3' is the Id returned by this run's CreateConnectionFunction (a per-run generated value) and 'DEVELOPMENT' is the request's Stage query parameter — both are request/state-derived and are not part of AWS's message template at all. AWS's message for this error class has NO variable part: the required text is exactly "The specified connection function does not exist."

Model facts used (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json): DescribeConnectionFunction errors = AccessDenied(403), EntityNotFound(404), InvalidArgument(400), UnsupportedOperation(400); http GET /2020-05-31/connection-function/{Identifier}/describe, success 200; input Identifier (httpLabel, required), Stage (httpQuery, enum FunctionStage). DeleteConnectionFunction errors = AccessDenied(403), CannotDeleteEntityWhileInUse(409), EntityNotFound(404), InvalidArgument(400), InvalidIfMatchVersion(400), PreconditionFailed(412), UnsupportedOperation(400); http DELETE /2020-05-31/connection-function/{Id}, success 204. EntityNotFound = client error, httpError 404, single Message member.

## Steps to Reproduce
1. Create a connection function: `aws cloudfront create-connection-function --name sim2real-cf-connfn-1 --connection-function-code fileb://fn.js --connection-function-config Comment=probe,Runtime=cloudfront-js-2.0` and keep the returned ConnectionFunctionSummary.Id and ETag.
2. Confirm it is listed: `aws cloudfront list-connection-functions --max-items 100` contains that Id.
3. Delete it: `aws cloudfront delete-connection-function --id <id> --if-match <etag>` (204), and confirm it is gone from list-connection-functions.
4. Read the dead id back: `aws cloudfront describe-connection-function --identifier <id> --stage DEVELOPMENT`. Observe 404 EntityNotFound (correct) with Message 'The entity was not found: no connection function <id> in the DEVELOPMENT stage' instead of the fixed sentence 'The specified connection function does not exist.'
5. Repeat the delete on the dead id: `aws cloudfront delete-connection-function --id <id> --if-match <etag>` and compare its EntityNotFound Message to the same fixed sentence.

Or replay the check id in trace_ids.
