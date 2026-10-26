Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# A ResourceArn that is not an ARN at all answers 404 ResourceNotFoundException instead of 400 ValidationException

_Filed: 2026-10-24T15:30:26Z_
_Operation: ListTagsForResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected (real service): `ListTagsForResource` parses `ResourceArn` before it looks anything up. A value that is not a well-formed ARN of this service — e.g. `banana`, `not:an:arn`, `arn:aws:pca-connector-scep` (too few segments), or a connector ARN with a non-UUID resource id — is an input validation failure and is refused with HTTP 400 `ValidationException`, whose own documentation covers exactly this ("An input validation error occurred"). HTTP 404 `ResourceNotFoundException` is reserved for a syntactically valid resource ARN of this service that holds no record.

Actual (emulator): the handler performs no syntactic check on `ResourceArn` at all. It asks `state::tags_of(resource_arn)`, which decides the resource kind purely by looking for the `/challenge/` substring and then does an exact-key lookup in the `connectors` map; any string that misses answers 404 `ResourceNotFoundException`. Worse, the error body then invents a resource kind for a string that names no resource: `resource_type_of("banana")` returns `Connector`, so the 404 carries `ResourceId: "banana"`, `ResourceType: "Connector"`.

Note this is specific to this operation: `ResourceArn` is modeled as a bare `String` with NO `@pattern`/`@length`, so the framework's boundary constraint validator refuses nothing — unlike `GetConnector`/`GetChallengePassword`/`DeleteChallenge`, whose `ConnectorArn`/`ChallengeArn` labels carry patterns and therefore DO answer 400 for a malformed ARN (asserted by tests/all/challenge_deletion.rs `a_malformed_challenge_arn_is_a_validation_failure_not_a_404`). The emulator is therefore internally inconsistent: the same garbage string is a 400 on a challenge path and a 404 on the tags path.

## Evidence

Spec: smithy_query(operation=ListTagsForResource) → input `{"ResourceArn": {"target": "String", "httpLabel": true, "required": true}}` — target is `String`, with no pattern or length, so nothing at the boundary can refuse a non-ARN and the handler is the only place the check can live. The operation declares both `ValidationException` (HTTP 400) and `ResourceNotFoundException` (HTTP 404); `ValidationException`'s doc is "An input validation error occurred. For example, invalid characters in a name tag, or an invalid pagination token", while `ResourceNotFoundException`'s doc is "The operation tried to access a nonexistent resource" — a string that is not an ARN never named a resource in the first place. Refusing a path label that is not an ARN with a validation error rather than not-found is also established practice in this backlog (see 20261024T034444Z-kafkaconnect-describeconnector-error-response).

Handler: services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/ops/list_tags_for_resource.rs lines 150-192 — `store.tags_of(&input.resource_arn).ok_or_else(|| not_found(&input.resource_arn))?` is the whole lookup, and `not_found` fills `ResourceType` from `crate::state::resource_type_of` (src/state.rs lines 376-414), which falls back to `Connector` for any string without a `/challenge/` segment.

Test coverage: tests/all/resource_tags.rs `an_arn_this_service_holds_no_record_under_is_not_found` (line 504) exercises only WELL-FORMED pca-connector-scep ARNs plus one syntactically valid `arn:aws:s3:...` ARN; no case passes a string that is not an ARN, so nothing asserts the behavior described here either way.

## Steps to Reproduce

1. Create a connector and let it settle to ACTIVE, so the store is non-empty.
2. GET /tags/banana (i.e. ListTagsForResource with ResourceArn = "banana"). Actual: HTTP 404 with __type ResourceNotFoundException, ResourceId "banana" and ResourceType "Connector". Expected: HTTP 400 ValidationException.
3. Repeat with ResourceArn = "arn:aws:pca-connector-scep" (truncated ARN) and with "arn:aws:pca-connector-scep:us-east-1:123456789012:connector/not-a-uuid". Actual: HTTP 404 ResourceNotFoundException in both cases. Expected: HTTP 400 ValidationException.
4. Contrast GET /challengePasswords/<same malformed value> on the same emulator, which answers HTTP 400 ValidationException because ChallengeArn carries a @pattern — showing the inconsistency.
