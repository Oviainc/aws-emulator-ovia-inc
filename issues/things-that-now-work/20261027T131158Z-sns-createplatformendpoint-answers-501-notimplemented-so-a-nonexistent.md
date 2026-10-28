Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.create-platform-endpoint-nonexistent-app-fails
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.create-platform-endpoint-nonexistent-app-fails|run#10|AssertionError|assert ghost_code == "NotFound", f"CreatePlatformEndpoint: expected NotFound, got {ghost_code!r}"
Issue-Commit: 16c20ea7d22d9314dc8611d90389344009516a2f

# sns CreatePlatformEndpoint answers 501 NotImplemented, so a nonexistent PlatformApplicationArn is never refused with 404 NotFound 'PlatformApplication does not exist' (1 cause)

> _Note: this issue was identified against the code base at commit `16c20ea7d22d9314dc8611d90389344009516a2f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePlatformEndpoint

## Description
Cause 1 of 1: CreatePlatformEndpoint is an unimplemented handler that refuses every call with NotImplemented, so neither the success path nor the not-found refusal taxonomy of the mobile-push endpoint plane exists.

AWS: Smithy (com-amazonaws-sns-amazonsimplenotificationservice): CreatePlatformEndpoint takes required PlatformApplicationArn and Token (plus CustomUserData, Attributes) and returns EndpointArn; its modeled errors are NotFoundException (awsQueryError code 'NotFound', HTTP 404), InvalidParameterException ('InvalidParameter', 400), AuthorizationErrorException ('AuthorizationError', 403) and InternalErrorException ('InternalError', 500). For a well-formed PlatformApplicationArn in the caller's own account/region that names no existing application (arn:aws:sns:<region>:<account-id>:app/GCM/<name>), AWS refuses with HTTP 404, error code NotFound and the message 'PlatformApplication does not exist' — that whole string is fixed text for this error class, it interpolates nothing from the request. Against an application that DOES exist (e.g. a BAIDU application created with an arbitrary PlatformPrincipal/PlatformCredential pair), the same call with a fabricated device token fails for a platform-specific reason or succeeds with an EndpointArn, but is never NotFound: existence of the application and validity of the token are distinct refusal classes.

Emulator: CreatePlatformApplication succeeds, but every CreatePlatformEndpoint — for the real application and for the ghost ARN alike — is refused with error code 'NotImplemented' (the scaffold stub at services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/create_platform_endpoint.rs, HTTP 501). The replay asserts 'CreatePlatformEndpoint: expected NotFound, got \'NotImplemented\''. The auto-filed stub issue for this operation (raised from sns.set-endpoint-attributes-not-found) asks for the operation to exist; this issue records the refusal semantics the implementation must carry, so both can be closed together.

Why: a client cannot register mobile-push endpoints at all, and error-handling code cannot distinguish a deleted/mistyped platform application (retriable-by-reconfiguration 404 NotFound) from a bad device token (400 InvalidParameter) — it sees an unmodelled 501 NotImplemented, which no SDK retry/branching logic for SNS expects and which cannot be mapped to any modeled exception.

Fix: the CreatePlatformEndpoint handler and its platform-application state lookup — resolve PlatformApplicationArn in the caller's account/region, raise NotFoundException (404, code NotFound, message 'PlatformApplication does not exist') when it does not resolve, validate Token/Attributes as InvalidParameterException, and otherwise mint and store the endpoint and return its EndpointArn.

Traces: sns.create-platform-endpoint-nonexistent-app-fails.

## Evidence
Expected (AWS): sns.create_platform_endpoint(PlatformApplicationArn='arn:aws:sns:<region>:<account>:app/GCM/<nonexistent>', Token='token-<uuid>') -> ClientError with HTTP 404, Error.Code 'NotFound', Error.Message 'PlatformApplication does not exist'; the same call against a real (BAIDU) application returns an EndpointArn or fails with a non-NotFound code.

Actual (emulator): both calls return Error.Code 'NotImplemented' (HTTP 501). Replay detail: AssertionError: CreatePlatformEndpoint: expected NotFound, got 'NotImplemented' (check_sns_create_platform_endpoint_nonexistent_app_fails.py line 82).

## Steps to Reproduce
1. CreatePlatformApplication(Name=<unique>, Platform='BAIDU', Attributes={PlatformPrincipal: <any api key>, PlatformCredential: <any secret key>}) and keep the returned PlatformApplicationArn.
2. CreatePlatformEndpoint(PlatformApplicationArn=<that ARN>, Token='token-<uuid>') — observe error code NotImplemented instead of a success or a platform-specific refusal.
3. CreatePlatformEndpoint(PlatformApplicationArn='arn:aws:sns:<region>:<caller account>:app/GCM/<random-unused-name>', Token='token-<uuid>') — observe error code NotImplemented instead of HTTP 404 NotFound with message 'PlatformApplication does not exist'.
