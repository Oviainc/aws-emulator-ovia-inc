Issue-Severity: CRITICAL
Issue-Commit: bffba5cba440022693dbdf16c8135b8d2c175737

# Chime SDK Identity is entirely unreachable: the router resolves requests by sigv4 signing name, so all 5 'chime'-signed services collide and every request is refused with AmbiguousService

> _Note: this issue was identified against the code base at commit `bffba5cba440022693dbdf16c8135b8d2c175737`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAppInstance

## Description
Cause 1 of 1: the emulator's front-door request router dispatches on the sigv4 signing name (`credential scope service` / `Authorization` credential field) instead of the request's endpoint prefix and URI path. Chime is a service family in which several distinct AWS services deliberately share one signing name, so that key is not unique and the router finds 5 candidate services for every Chime-signed request and refuses to serve any of them.

AWS: Smithy (services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/smithy-model/com-amazonaws-chimesdkidentity-chimeidentityservice.json) — service ChimeIdentityService, version 2021-04-20, protocol restJson1, `aws.auth#sigv4` name = `chime`, `aws.api#service` endpointPrefix = `identity-chime`, arnNamespace = `chime`. The sibling Chime SDK services (Chime, Chime SDK Messaging, Chime SDK Meetings, Chime SDK Voice, Chime SDK Media Pipelines) share the SAME sigv4 signing name `chime` while each carrying its OWN endpointPrefix (`identity-chime`, `messaging-chime`, `meetings-chime`, `voice-chime`, `media-pipelines-chime`, `chime`). Real AWS therefore disambiguates by the endpoint host the client was pointed at and by the restJson1 `@http` URI path of the operation — never by signing name alone. A correctly addressed CreateAppInstance (restJson1 POST /app-instances) returns 201 with an AppInstanceArn; it never returns an `AmbiguousService` error, which is not a modeled error of this service (its common errors are BadRequestException, ForbiddenException, ServiceFailureException, ServiceUnavailableException, ThrottledClientException, UnauthorizedClientException).

Emulator: every call made by a correctly configured botocore `chime-sdk-identity` client is rejected with error code `AmbiguousService` and the message (example text, from the replay) "5 emulated services all claim requests for 'chime': chime, chime, chime, chime, chime; the request is rejected rather than served by an arbitrary one". botocore additionally exhausted its retry budget on it ("reached max retries: 3"), i.e. the refusal is delivered in a form the SDK classifies as retryable rather than as a terminal 4xx client error. Hypothesis: the refusal is emitted as a 5xx from the router layer, which is why it is retried; the replay output records only the parsed error code and message. Note the message also shows all 5 candidates rendered with the same display name 'chime' rather than their distinct sdkIds, so the refusal does not even identify which services collided.

Why: a client relying on AWS behavior cannot reach Chime SDK Identity at all. Not one of the 30 modeled operations is exercisable — control-plane CRUD (CreateAppInstance / DescribeAppInstance / UpdateAppInstance / DeleteAppInstance), identity management (CreateAppInstanceUser, CreateAppInstanceAdmin, endpoints, expiration and retention settings) and every modeled error path are all shadowed by a router-level refusal that occurs before any handler runs. Worse, the refusal masquerades as a retryable server fault, so SDKs and application retry loops spin to max attempts and then surface an error code AWS never returns, instead of failing fast or succeeding. Any correctness signal for this service is unobtainable while this holds.

Fix: the front-door service-resolution/routing layer (the component that maps an inbound request to an emulated service), not any chime-sdk-identity handler. Resolution must key on the request's endpoint prefix / Host header — and, where that is absent, on the operation's restJson1 URI path and method — with the sigv4 signing name used only as a weak hint; a shared signing name must not be treated as an identity. Also, a resolution failure must be rendered as a terminal AWS error shape rather than a retryable one, so the SDK does not retry it 3 times. This is the same defect already filed for other signing-name families — see open issues 20261021T234042Z-osis-router-refuses-every-sigv4-iam-request-as-ambiguousservice.md ('iam' vs 'iam-toolbox'), 20261022T031220Z-wisdom-all-wisdom-requests-rejected-with-ambiguousservice-two-emulated.md, 20261022T032542Z-application-autoscaling-lambda-lambda-is-unreachable-four-emulated-services-all-claim.md, 20261022T035032Z-events-every-eventbridge-events-request-is-refused-with-ambiguousservice.md, 20261022T035032Z-rds-every-rds-rds-request-is-refused-with-ambiguousservice.md, 20261022T034453Z-sagemaker-metrics-router-refuses-every-sagemaker-signed-request-as-ambiguousservice.md — none of which carries the 'chime' family or this service's replay keys, so this issue is filed separately and must be re-replayed against chime-sdk-identity.

Traces: all 24 chime-sdk-identity behavioral checks fail identically, each aborting on its very first call (CreateAppInstance) with AmbiguousService: concurrent-create-app-instance-admin-race, concurrent-register-endpoint-same-token-dedup, create-app-instance-admin-promotes-user, create-app-instance-bot-malformed-lex-arn-rejected, create-app-instance-describe-roundtrip, create-app-instance-idempotent-token, create-app-instance-token-reuse-conflict-rejected, create-app-instance-user-duplicate-id-conflict-rejected, create-app-instance-user-with-expiration-settings-at-creation, delete-app-instance-admin-demotes, delete-app-instance-admin-not-admin-succeeds-as-a-noop, delete-app-instance-cascades-users, deregister-endpoint-removes, describe-app-instance-bot-not-found-rejected, expiration-settings-set-and-clear, list-app-instance-users-reflects-created, put-app-instance-retention-settings-range-rejected, register-endpoint-idempotent-token, register-endpoint-invalid-pinpoint-arn-inactive, retention-settings-put-get-roundtrip, tag-resource-lifecycle-roundtrip, update-app-instance-name-metadata-persists, update-app-instance-user-endpoint-allow-messages-persists, update-app-instance-user-persists.

## Resolution (fixer note)

The router-level refusal is gone: the signing name is no longer treated as an identity.
Each of the six services sharing `chime` now claims only a request that ADDRESSES it —
the endpoint the `Host`/`:authority` names, and, for a request naming no endpoint (what a
client pointed at a local `endpoint_url` sends), the URI path space its own model binds,
with each space two of the models share split at the segment they differ at
(`services/common/src/chime_family.rs`). So all 30 of this service's operations are
reachable and `CreateAppInstance` answers 201 with the ARN it minted
(`core/router/tests/all/chime_family_signing_name_routing.rs` mounts all six: it tabulates
the claim decision service by service and host by host, and
`create_app_instance_is_served_by_chime_sdk_identity_and_reads_back` pins that request end
to end, including the read-back through `ListAppInstances` and `DescribeAppInstance`; and
`services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/tests/all/signing_name_route_claim.rs`
pins this service's own claim rule).

ONE of the 24 traces above is NOT yet passable: `tag-resource-lifecycle-roundtrip`.
Five members of the family bind the very same generic tagging routes (`GET /tags`,
`POST /tags?operation=tag-resource|untag-resource`), and for Tag/UntagResource the only
discriminator — `ResourceARN` — travels in the request BODY, which the claim predicate
(`Service::can_handle`, handed `http::request::Parts`) does not receive. That space is
therefore resolved by endpoint prefix only: addressed to `identity-chime` it reaches this
service (which answers those operations' scaffold), and a request naming no endpoint is
claimed by nobody, so the router answers its terminal `404 UnknownService` instead of the
retryable `500 AmbiguousService` with five claimants. That residual is filed as
`20261022T060000Z-chime-family-generic-tagging-routes-still-ambiguous.md`, which also
records that this service's three tagging operations are themselves still scaffolded.
The other 23 traces are no longer blocked by routing.

## Evidence
Suite result: 0/24 passed (0.0%); chime-sdk-identity 0/24 (0%), 24 failed, all with verdict [error].

Expected (AWS, restJson1 POST /app-instances): HTTP 201 with body {"AppInstanceArn": "arn:aws:chime:us-east-1:<account>:app-instance/<id>"}. Modeled errors for this service are BadRequestException, ConflictException, ForbiddenException, ResourceLimitExceededException, ServiceFailureException, ServiceUnavailableException, ThrottledClientException, UnauthorizedClientException. `AmbiguousService` is not among them.

Actual (emulator), identical for all 24 checks:
  botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the CreateAppInstance operation (reached max retries: 3): 5 emulated services all claim requests for 'chime': chime, chime, chime, chime, chime; the request is rejected rather than served by an arbitrary one

Key facts: error code AmbiguousService (unmodeled); the call was retried to the SDK's max attempts, so the response is classified retryable rather than a terminal client error; the collision set is the 5 services sharing sigv4Name `chime` even though each has a distinct endpointPrefix (identity-chime vs messaging-chime / meetings-chime / voice-chime / media-pipelines-chime / chime), which is the key the router should have used. Error message text quoted above is an EXAMPLE, not a requirement; the code and the fact of refusal are the divergence.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Create a botocore client for the service with its normal wire identity: client = session.client("chime-sdk-identity", region_name="us-east-1", endpoint_url=<emulator>) — botocore signs it with sigv4 signing name 'chime' and addresses the 'identity-chime' endpoint prefix.
3. Call any operation, e.g. client.create_app_instance(Name="sim2real-chime-ai-1", Metadata="", ClientRequestToken=uuid4().hex, Tags=[{"Key":"sim2real-env","Value":"behavioral"}]).
4. Observe: instead of a 201 + AppInstanceArn, the SDK retries to max attempts and raises ClientError with code AmbiguousService, reporting that 5 emulated services claim requests for 'chime'.
5. Any other operation (CreateAppInstanceUser, PutAppInstanceRetentionSettings, RegisterAppInstanceUserEndpoint, TagResource, ...) fails the same way, so no handler is reachable.
6. Re-replay the 24 check ids listed in trace_ids to confirm the fix.
