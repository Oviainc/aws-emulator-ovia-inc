Issue-Source: trace-sweep-divergence
Issue-Trace: pca-connector-scep-error-not-found-resources
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pca-connector-scep-error-not-found-resources
Issue-Shape-Id: com.amazonaws.pcaconnectorscep#PcaConnectorScep
Issue-Key: pca-connector-scep|GetConnector|4xx->4xx|$.Message
Issue-Key: pca-connector-scep|ListChallengeMetadata|4xx->4xx|$.Message
Issue-Key: pca-connector-scep|CreateChallenge|4xx->4xx|$.Message
Issue-Key: pca-connector-scep|CreateChallenge|4xx->4xx|$.ResourceType
Issue-Key: pca-connector-scep|GetChallengeMetadata|4xx->4xx|$.Message
Issue-Key: pca-connector-scep|GetChallengePassword|4xx->4xx|$.Message
Issue-Commit: ef775f68482e6d25f63f0fc1f052163f266c1a25

# ResourceNotFoundException message text and ResourceType diverge from AWS on every pca-connector-scep read path

> _Note: this issue was identified against the code base at commit `ef775f68482e6d25f63f0fc1f052163f266c1a25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConnector

## Description
Shared fault: the emulator renders ResourceNotFoundException with its own sentence template instead of AWS's, so the `Message` field is wrong on every read operation that can raise it, and on CreateChallenge the `ResourceType` field is wrong too. HTTP status (404) and `x-amzn-errortype` (ResourceNotFoundException) are already correct in all five cases — only the body text diverges. 5 causes, all pca-connector-scep.

AWS's message template for this error class is `"<ResourceType> was not found: <ResourceId>."` — fixed text `" was not found: "` plus a trailing period, with the resource-type word and the ARN taken from the request/state. The emulator's template is `"<ResourceType> <ResourceId> could not be found"` — different fixed text, no trailing period. That one template substitution is the root cause of causes 1-5.

Smithy: pca-connector-scep (restJson1). ResourceNotFoundException is errorKind=client, httpError=404, with required members Message, ResourceId and ResourceType; ResourceType is documented as "The resource type, which can be either Connector or Challenge." It is a modeled error of GetConnector, ListChallengeMetadata, CreateChallenge, GetChallengeMetadata and GetChallengePassword.

Cause 1 of 5: GetConnector on an unknown connector ARN.
  AWS: 404 ResourceNotFoundException, `Message` = "Connector was not found: <connectorArn>." (fixed text "Connector was not found: " + the ARN from the request + "."); ResourceId = the requested ARN, ResourceType = "Connector". Recorded response confirms this wording.
  Emulator: 404 ResourceNotFoundException but `Message` = "Connector <connectorArn> could not be found" — the ARN is interpolated in the middle, the fixed words differ and the period is absent.
  Why: clients and test suites that assert on the not-found message (log scrapers, retry/branch logic keyed on message text, snapshot tests of error bodies) see text that never occurs on AWS, so code validated against the emulator fails against the real service and vice versa.
  Fix: the shared ResourceNotFoundException constructor/serializer for this service (the error-rendering helper used by the read handlers), not the individual handlers.
  Traces: pca-connector-scep-error-not-found-resources#task#1.

Cause 2 of 5: ListChallengeMetadata for an unknown connector ARN.
  AWS: 404 ResourceNotFoundException, `Message` = "Connector was not found: <connectorArn>." with ResourceType "Connector".
  Emulator: same status and error code, but `Message` = "Connector <connectorArn> could not be found".
  Why: as cause 1 — the paginated list path reports a not-found connector with non-AWS text.
  Fix: same shared error renderer, reached from the ListChallengeMetadata handler.
  Traces: pca-connector-scep-error-not-found-resources#task#2.

Cause 3 of 5: CreateChallenge against an unknown connector ARN — message AND ResourceType wrong.
  AWS: 404 ResourceNotFoundException with `Message` = "ConnectorArn was not found: <connectorArn>." and `ResourceType` = "ConnectorArn" — on the create path AWS names the offending *input member* (ConnectorArn), not the resource kind.
  Emulator: `Message` = "Connector <connectorArn> could not be found" and `ResourceType` = "Connector".
  Why: a client that branches on `ResourceType` to tell "the connector you named does not exist" from "some other resource is missing" gets the wrong discriminator, in addition to the wrong message.
  Fix: the CreateChallenge handler's not-found branch must pass the input-member name as ResourceType, plus the shared message template.
  Traces: pca-connector-scep-error-not-found-resources#task#3.

Cause 4 of 5: GetChallengeMetadata on an unknown challenge ARN.
  AWS: 404 ResourceNotFoundException, `Message` = "Challenge was not found: <challengeArn>.", ResourceType "Challenge".
  Emulator: `Message` = "Challenge <challengeArn> could not be found".
  Why: as cause 1, for the challenge-metadata read path.
  Fix: shared error renderer.
  Traces: pca-connector-scep-error-not-found-resources#task#4.

Cause 5 of 5: GetChallengePassword on an unknown challenge ARN.
  AWS: 404 ResourceNotFoundException, `Message` = "Challenge was not found: <challengeArn>.", ResourceType "Challenge".
  Emulator: `Message` = "Challenge <challengeArn> could not be found".
  Why: as cause 1, for the challenge-password read path.
  Fix: shared error renderer.
  Traces: pca-connector-scep-error-not-found-resources#task#5.

Related but NOT covering these: open issue 20261028T160034Z-pca-connector-scep-getchallengepassword-wrong-behavior.md concerns GetChallengePassword serving a password for a challenge under a deleted connector — a state-lifecycle defect, not this message template; it carries no replay key for these steps.

## Evidence
Replay of fixture pca-connector-scep-error-not-found-resources (trace set simparity-wire-aws-v1), 5 mismatch clusters, status 404 == 404 in all five:

- task#1 GetConnector: $.Message expected 'Connector was not found: arn:aws:pca-connector-scep:us-west-2:000000000000:connector/fa9ce23c-aedb-47ef-94d6-505cf953adba.', got 'Connector arn:aws:pca-connector-scep:us-west-2:000000000000:connector/fa9ce23c-aedb-47ef-94d6-505cf953adba could not be found'
- task#2 ListChallengeMetadata: same expected/actual pair as task#1
- task#3 CreateChallenge: $.Message expected 'ConnectorArn was not found: <arn>.', got 'Connector <arn> could not be found'; $.ResourceType expected 'ConnectorArn', got 'Connector'
- task#4 GetChallengeMetadata: $.Message expected 'Challenge was not found: arn:...:connector/fa9ce23c-.../challenge/1b7f2991-f611-452a-9050-38557f08ec1d.', got 'Challenge <arn> could not be found'
- task#5 GetChallengePassword: same expected/actual pair as task#4

Fixed text of the AWS template: '<type> was not found: ' and the terminating '.'. Variable parts filled from the request/state: the type word (Connector | Challenge | the input member name ConnectorArn on CreateChallenge) and the ARN. The specific ARNs and UUIDs above are this recording's values, not requirements.

## Steps to Reproduce
1. Boot the emulator and replay fixture pca-connector-scep-error-not-found-resources from trace set simparity-wire-aws-v1.
2. Steps task#1..task#5 call GetConnector, ListChallengeMetadata, CreateChallenge, GetChallengeMetadata and GetChallengePassword with a connector/challenge ARN that was never created.
3. Each returns 404 with x-amzn-errortype: ResourceNotFoundException (correct) but a body whose $.Message reads '<type> <arn> could not be found' instead of '<type> was not found: <arn>.'; on CreateChallenge $.ResourceType is additionally 'Connector' instead of 'ConnectorArn'.
