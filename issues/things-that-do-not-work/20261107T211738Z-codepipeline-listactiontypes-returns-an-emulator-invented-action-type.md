Issue-Source: trace-sweep-divergence
Issue-Trace: codepipeline-catalog-list-action-types
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codepipeline-catalog-list-action-types
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline|ListActionTypes|2xx->2xx|$.actionTypes[*]
Issue-Commit: 2d6e0284801305ae3aa8ad9d3ca49cea550d31a4

# CodePipeline ListActionTypes returns an emulator-invented action-type catalog

> _Note: this issue was identified against the code base at commit `2d6e0284801305ae3aa8ad9d3ca49cea550d31a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListActionTypes

## Description
ListActionTypes answers HTTP 200 with a catalog the emulator made up: its membership, its ordering and the per-entry metadata of each ActionType all differ from the catalog CodePipeline serves, for every owner the operation is asked about. 2 causes, one per owner scope exercised by the corpus.

Cause 1 of 2 — the AWS-owned catalog (fixture codepipeline-catalog-list-action-types, step task#1).
AWS: ListActionTypes returns the service's catalog of available action types for the requested actionOwnerFilter. Smithy: ListActionTypes output.actionTypes is @required (ActionTypeList); ActionType requires id (ActionTypeId: category/owner/provider/version), inputArtifactDetails and outputArtifactDetails (ArtifactDetails.minimumCount/maximumCount, @required, @range 0..5), and carries settings (ActionTypeSettings: entityUrlTemplate/executionUrlTemplate/revisionUrlTemplate/thirdPartyConfigurationUrl) plus actionConfigurationProperties (ActionConfigurationProperty with name/required/key/secret/queryable and a description, @length 1..160). The catalog's *contents* are not in the model, so recorded behavior only: the recording shows 28 AWS-owned entries, led by category=Source owner=AWS provider=S3 version=1, whose four configuration properties (S3Bucket, S3ObjectKey, PollForSourceChanges, AllowOverrideForS3ObjectKey) each carry a description and key=true for the first two, whose settings carry an entityUrlTemplate into the S3 console, and whose outputArtifactDetails is minimumCount=1/maximumCount=1. The recording likewise shows an AWS Lambda Invoke entry with FunctionName/UserParameters descriptions.
Emulator: it returns a shorter, differently ordered list whose first entry is category=Approval owner=AWS provider=Manual version=1, with settings={} and configuration properties (NotificationArn, CustomData, ExternalEntityLink) that carry no description and key=false, and inputArtifactDetails/outputArtifactDetails both 0/0. The replay reports all 28 expected indices $.actionTypes[0]..$.actionTypes[27] as diverging, i.e. no position in the list matches.
Why: this operation IS the discoverability surface for the catalog. A client that enumerates it to decide which provider/category pairs it may put into a pipeline declaration, to render the required/key/secret flags of a provider's configuration keys, or to size input/output artifacts against inputArtifactDetails/outputArtifactDetails, sees providers that do not exist on AWS and does not see the ones that do (S3, Lambda, ...). Its declarations are then built against the wrong set, and the descriptions it surfaces to a user are absent.
Fix: the ListActionTypes handler's built-in action-type catalog (the static table it serves from) — it must carry CodePipeline's AWS-owned entries, in the service's order, each with its settings templates, its configuration-property descriptions and flags, and its real artifact-count bounds.
Traces: codepipeline-catalog-list-action-types#task#1.

Cause 2 of 2 — the ThirdParty-owned catalog (fixture codepipeline-catalog-list-action-types, step task#2).
AWS: same operation and shapes as above, scoped to the ThirdParty owner. Catalog contents are recorded behavior only: the recording shows 5 ThirdParty entries, led by category=Test owner=ThirdParty provider=GhostInspector version=1, with configuration properties ClientId, ClientToken (secret=true), TestId and ExtraParams — each with a description — settings carrying entityUrlTemplate, executionUrlTemplate and thirdPartyConfigurationUrl, inputArtifactDetails 0..1 and outputArtifactDetails 0..0.
Emulator: it returns a different ThirdParty set, led by category=Source owner=ThirdParty provider=GitHub version=1 with its own settings and Owner/Repo/Branch/OAuthToken/PollForSourceChanges properties that carry no description. The replay reports all 5 expected indices $.actionTypes[0]..$.actionTypes[4] as diverging.
Why: a client discovering third-party integrations gets a provider list that does not match the account's, so a declaration naming a provider the emulator advertised is rejected on AWS, and a provider AWS does offer is never offered to the user. Secret-flag and description metadata a console or wizard relies on is missing too.
Fix: same component — the ThirdParty slice of the ListActionTypes handler's catalog table, including the owner filtering that selects it.
Traces: codepipeline-catalog-list-action-types#task#2.

Neither cause is covered by an open issue: the nearest codepipeline response-shape issue, 20261029T202908Z-codepipeline-pipelinedeclaration-responses-omit-modeled-defaults-and-nested.md, is about PipelineDeclaration responses (GetPipeline/CreatePipeline/UpdatePipeline) and not about the action-type catalog.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200).

Cause 1 — step codepipeline-catalog-list-action-types#task#1, operation ListActionTypes, verdict mismatch, expected status 200, actual status 200.
  Diverging keys: $.actionTypes[0] .. $.actionTypes[27] (all 28 entries).
  $.actionTypes[0]: missing — expected {'id': {'__type': 'ActionTypeId', 'category': 'Source', 'owner': 'AWS', 'provider': 'S3', 'version': '1'}, 'actionConfigurationProperties': [S3Bucket (description 'The Amazon S3 Bucket', key=true, required=true), S3ObjectKey (description 'The Amazon S3 object key', key=true, required=true), PollForSourceChanges (description 'The option to poll for source repository changes', key=true, required=false), AllowOverrideForS3ObjectKey (description 'Whether S3 Object Key can be overridden during StartPipelineExecution', key=true, required=false)], 'inputArtifactDetails': {'minimumCount': 0, 'maximumCount': 0}, 'outputArtifactDetails': {'minimumCount': 1, 'maximumCount': 1}, 'settings': {'entityUrlTemplate': 'https://console.aws.amazon.com/s3/home?region=us-west-2#'}}
  got {'id': {'__type': 'ActionTypeId', 'category': 'Approval', 'owner': 'AWS', 'provider': 'Manual', 'version': '1'}, 'settings': {}, 'actionConfigurationProperties': [NotificationArn, CustomData, ExternalEntityLink — all required=false, key=false, secret=false, queryable=false, no description], 'inputArtifactDetails': {'minimumCount': 0, 'maximumCount': 0}, 'outputArtifactDetails': {'minimumCount': 0, 'maximumCount': 0}}
  $.actionTypes[1]: missing — expected the AWS Lambda Invoke entry (FunctionName 'The AWS Lambda function to invoke', UserParameters 'Extra parameters for the AWS Lambda function').

Cause 2 — step codepipeline-catalog-list-action-types#task#2, operation ListActionTypes, verdict mismatch, expected status 200, actual status 200.
  Diverging keys: $.actionTypes[0] .. $.actionTypes[4] (all 5 entries).
  $.actionTypes[0]: missing — expected {'id': {'__type': 'ActionTypeId', 'category': 'Test', 'owner': 'ThirdParty', 'provider': 'GhostInspector', 'version': '1'}, 'actionConfigurationProperties': [ClientId 'The user client id' required=true, ClientToken 'The user client token' required=true secret=true, TestId 'The test id used by the Ghost Inspector tests' required=true, ExtraParams 'A JSON-encoded string of extra parameters to provide to the test or suite at runtime.' required=false], 'inputArtifactDetails': {'minimumCount': 0, 'maximumCount': 1}, 'outputArtifactDetails': {'minimumCount': 0, 'maximumCount': 0}, 'settings': {'entityUrlTemplate': 'https://app.ghostinspector.com/tests/{Config:TestId}', 'executionUrlTemplate': 'https://app.ghostinspector.com/results/{ExternalExecutionId}', 'thirdPartyConfigurationUrl': 'https://app.ghostinspector.com/code-pipeline'}}
  got {'id': {'__type': 'ActionTypeId', 'category': 'Source', 'owner': 'ThirdParty', 'provider': 'GitHub', 'version': '1'}, 'settings': {'thirdPartyConfigurationUrl': 'https://github.com/join', 'entityUrlTemplate': 'https://github.com/{Config:Owner}/{Config:Repo}/tree/{Config:Branch}', 'revisionUrlTemplate': 'https://github.com/{Config:Owner}/{Config:Repo}/commits'}, 'actionConfigurationProperties': [Owner, Repo, Branch, OAuthToken (secret=true), PollForSourceChanges — none with a description]}

Model consulted: services/service-com-amazonaws-codepipeline-codepipeline-20150709/smithy-model — ListActionTypes (input actionOwnerFilter/regionFilter/nextToken; output actionTypes @required, nextToken; errors InvalidNextTokenException, ValidationException), shapes ActionType, ActionTypeId, ActionTypeSettings, ActionConfigurationProperty, ArtifactDetails.

Note: the specific provider names, descriptions, URL templates and artifact counts above are the catalog's own fixed content (not per-run values); only the us-west-2 region inside the S3 entityUrlTemplate is request/region-derived.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture codepipeline-catalog-list-action-types.
2. Step task#1: call ListActionTypes for the AWS-owned catalog. Compare the returned actionTypes list against the recorded response: the emulator's list differs at every index (expected 28 entries led by Source/AWS/S3; emulator leads with Approval/AWS/Manual), and its entries omit actionConfigurationProperties[].description, the settings URL templates, and the non-zero input/outputArtifactDetails counts.
3. Step task#2: call ListActionTypes scoped to ThirdParty. The emulator's list differs at every index (expected 5 entries led by Test/ThirdParty/GhostInspector; emulator leads with Source/ThirdParty/GitHub) and again omits every configuration-property description.
4. Both steps answer 200, so the divergence is purely in the catalog's membership, ordering and per-entry metadata.
