Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# DeleteChallenge answers 202 for a challenge ARN that holds no record instead of ResourceNotFoundException

_Filed: 2026-10-24T02:08:31Z_
_Operation: DeleteChallenge | Class: error-response_

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: DeleteChallenge against a well-formed challenge ARN that names no stored challenge (already deleted, never created, or whose owning connector is gone) fails with ResourceNotFoundException (HTTP 404) carrying ResourceId=<challenge ARN>, ResourceType=Challenge. The operation's only input is the challenge ARN, so that modeled error is reachable only for a missing challenge.

Actual: the handler calls `store.remove_challenge(&input.challenge_arn)`, discards the boolean it returns, and always answers 202 with an empty body. ResourceNotFoundException is therefore unreachable for DeleteChallenge, and deleting a challenge under a connector that does not exist at all also reports success.

## Evidence

Smithy model (smithy_query operation=DeleteChallenge): errors include ResourceNotFoundException (client, httpError=404); input is only `ChallengeArn` (@httpLabel, @required, pattern ...connector/<uuid>/challenge/<uuid>). Handler: src/ops/delete_challenge.rs lines 182-187 — comment "a challenge ARN that holds no record is a 202 rather than a 404", return value of `remove_challenge` ignored. Test coverage asserts the opposite of the spec: tests/all/connector_lifecycle.rs `deletes_are_observable_and_idempotent` lines 1196-1202 assert 202 on a repeat DeleteChallenge; no test asserts 404.

## Steps to Reproduce

1. CreateConnector, wait for ACTIVE, CreateChallenge — note the returned Challenge.Arn.
2. DeleteChallenge with that ARN (202).
3. DeleteChallenge again with the same ARN: observe 202; expected 404 ResourceNotFoundException (ResourceType=Challenge).
4. Also call DeleteChallenge with a pattern-valid challenge ARN under a connector that was never created: observe 202 instead of 404.
