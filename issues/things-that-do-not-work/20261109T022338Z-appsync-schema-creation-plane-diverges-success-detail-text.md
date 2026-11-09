Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-eval-schema-creation-introspection
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-eval-schema-creation-introspection appsync-schema-creation-introspection
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|GetSchemaCreationStatus|2xx->2xx|$.details
Issue-Key: appsync|StartSchemaCreation|4xx->4xx|$.message
Issue-Key: appsync|StartSchemaCreation|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|StartSchemaCreation|4xx->4xx|status
Issue-Commit: 638db06e9ece87c67045cb61620cbfbd131c727c

# AppSync schema-creation plane diverges: SUCCESS detail text is a constant and the unknown-API refusal uses the wrong class (2 causes)

> _Note: this issue was identified against the code base at commit `638db06e9ece87c67045cb61620cbfbd131c727c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSchemaCreationStatus

## Description
The AppSync schema-creation plane (StartSchemaCreation / GetSchemaCreationStatus, both bound to /v1/apis/{apiId}/schemacreation) answers its own strings rather than AWS's: the SUCCESS status detail is a hard-coded sentence that carries none of the information AWS reports, and an unknown/invalid apiId is refused with the wrong HTTP status and the wrong modeled error. 2 net-new causes, both in this service's schema-creation handlers.

Cause 1 of 2: GetSchemaCreationStatus returns a constant SUCCESS detail instead of AWS's type-count report.
AWS: 200 with GetSchemaCreationStatusResponse.details (Smithy: GetSchemaCreationStatus output { status: SchemaStatus, details: String }; details documented as "Detailed information about the status of the schema creation operation."). For a schema creation that succeeded, AWS words the detail with the template "Successfully created schema with <N> types." — the fixed text is "Successfully created schema with " and " types.", and the single variable part N is the number of types the submitted SDL defines, taken from the definition the caller passed to StartSchemaCreation (recorded behavior only for the exact sentence; the model documents the member but not its wording).
Emulator: 200 with details = 'The schema was created and is ready for use.' — a constant sentence, independent of the schema that was submitted, that reports no type count at all.
Why: a client that polls GetSchemaCreationStatus and parses/asserts the detail to confirm how much of its SDL was accepted gets a string that matches no AWS wording and carries no type count, so a build step that reports or gates on "created schema with N types" silently reports nothing. The message is a requirement, so the divergence is a defect even for clients that only log it.
Fix: the GetSchemaCreationStatus handler (its response projection for the SUCCESS/ACTIVE schema state) must derive `details` from the stored schema — counting the types in the parsed SDL — instead of returning a literal.
Traces: appsync-eval-schema-creation-introspection#task#3, appsync-schema-creation-introspection#task#3.

Cause 2 of 2: StartSchemaCreation against an API id that does not exist answers 404 NotFoundException instead of AWS's 400 BadRequestException.
AWS: 400 with x-amzn-errortype BadRequestException and the message template "Invalid API ID <apiId>" — the fixed text is "Invalid API ID " and the single variable part is the apiId taken verbatim from the request's URI label (Smithy: StartSchemaCreation input apiId is a required @httpLabel String on POST /v1/apis/{apiId}/schemacreation, with no pattern constraint; its modeled errors include BadRequestException httpError 400 and NotFoundException httpError 404, so AWS deliberately classifies an unresolvable apiId on this operation as a bad request rather than a missing resource).
Emulator: 404 with x-amzn-errortype NotFoundException and the message 'API not found.' — the wrong status, the wrong error code, and a fixed sentence that does not echo the rejected apiId.
Why: a client's typed-error dispatch is driven by the error code, so code that catches BadRequestException on StartSchemaCreation (the error AWS models for this case) does not catch the emulator's NotFoundException and the failure escapes as an unhandled 404; retry/backoff policies that treat 404 as a resource-eventually-appears condition will also loop where AWS fails fast. The echoed apiId is also lost, so the caller cannot tell which id was rejected.
Fix: StartSchemaCreation's apiId resolution step — raise the modeled BadRequestException (400) with AWS's "Invalid API ID <apiId>" template rather than routing through the shared not-found path used by the other api-scoped handlers.
Traces: appsync-schema-creation-introspection#task#4.

Related open issues that do NOT cover these: "AppSync validation refusals are emulator-authored: wrong message text, wrong error class, and one check missing" (20261029T200818Z-appsync-validation-refusals-are-emulator-authored-wrong-message.md) carries GetIntrospectionSchema's format refusal from the same fixture but neither of these replay keys; the replay report lists both clusters below with an empty owned_by.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up: true, bootstrap_signal: false).

Cause 1 - appsync-eval-schema-creation-introspection#task#3, GetSchemaCreationStatus, expected status 200 / actual status 200:
  $.details: value_mismatch (expected 'Successfully created schema with 3 types.', got 'The schema was created and is ready for use.')
  cluster (2 steps): appsync-eval-schema-creation-introspection#task#3, appsync-schema-creation-introspection#task#3
  divergence_key: service=appsync|GetSchemaCreationStatus|status=200->200|keys=$.details

Cause 2 - appsync-schema-creation-introspection#task#4, StartSchemaCreation, expected status 400 / actual status 404:
  status: expected 400, got 404
  $.message: value_mismatch (expected 'Invalid API ID nonexistent0000000000', got 'API not found.')
  $headers.x-amzn-errortype: value_mismatch (expected 'BadRequestException', got 'NotFoundException')
  cluster (1 step): appsync-schema-creation-introspection#task#4
  divergence_key: service=appsync|StartSchemaCreation|status=400->404|keys=$.message,$headers.x-amzn-errortype,status

(The literal values 'nonexistent0000000000' and '3 types' are this recording's request/schema inputs filling the templates described above, not requirements in themselves.)

## Steps to Reproduce
1. Clone the repository and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus; fixtures appsync-eval-schema-creation-introspection and appsync-schema-creation-introspection mismatch at the steps listed under Traces.
3. By hand, cause 1: CreateGraphqlApi, then StartSchemaCreation on that apiId with an SDL defining 3 types, then poll GET /v1/apis/{apiId}/schemacreation (GetSchemaCreationStatus) until status is SUCCESS and read `details`. Expect 'Successfully created schema with 3 types.'; the emulator answers 'The schema was created and is ready for use.'
4. By hand, cause 2: POST /v1/apis/nonexistent0000000000/schemacreation (StartSchemaCreation) with any definition blob. Expect HTTP 400, x-amzn-errortype BadRequestException, message 'Invalid API ID nonexistent0000000000'; the emulator answers HTTP 404, x-amzn-errortype NotFoundException, message 'API not found.'
