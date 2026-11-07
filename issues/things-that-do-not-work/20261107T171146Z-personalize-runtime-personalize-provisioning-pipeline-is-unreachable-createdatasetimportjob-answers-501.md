Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: personalize-runtime.get-personalized-ranking-round-trip personalize-runtime.recommendations-recipe-type-mismatch-rejected
Issue-Shape-Id: com.amazonaws.personalizeruntime#AmazonPersonalizeRuntime
Issue-Key: personalize-runtime|CreateDatasetImportJob|NotImplemented
Issue-Commit: c98f49a3ab045fe250f88b9f8f7e4420d8205746

# Personalize provisioning pipeline is unreachable: CreateDatasetImportJob answers 501 NotImplemented, so no campaign can exist for personalize-runtime inference

> _Note: this issue was identified against the code base at commit `c98f49a3ab045fe250f88b9f8f7e4420d8205746`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDatasetImportJob

## Description
Cause 1 of 1: the Amazon Personalize control-plane pipeline cannot be driven over the wire, because CreateDatasetImportJob is still an unimplemented scaffold, so every personalize-runtime data-plane success path is unreachable. Failing checks: personalize-runtime.get-personalized-ranking-round-trip, personalize-runtime.recommendations-recipe-type-mismatch-rejected.

AWS: a client provisions a recommender by calling CreateSchema, CreateDatasetGroup, CreateDataset, CreateDatasetImportJob, CreateSolution, CreateSolutionVersion and CreateCampaign, polling the matching Describe* until each resource reports ACTIVE, and then calls the runtime plane. Smithy (com.amazonaws.personalize): CreateDatasetImportJob takes required jobName (@length 1-63, pattern ^[a-zA-Z0-9][a-zA-Z0-9\-_]*$), required datasetArn (pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$), required dataSource and optional roleArn/tags/importMode/publishAttributionMetricsToS3; it returns 200 with datasetImportJobArn and models ONLY client errors InvalidInputException (400), LimitExceededException (409), ResourceAlreadyExistsException (403), ResourceInUseException (409), ResourceNotFoundException (404) and TooManyTagsException (400). DescribeDatasetImportJob then reports the job's status progressing to ACTIVE. 501 NotImplemented is not a modeled outcome of this operation at all. With the pipeline complete, GetPersonalizedRanking returns 200 with a personalizedRanking that is a same-set permutation of inputList carrying float scores in descending order plus a non-empty recommendationId, and a campaign backed by a recipe of the wrong type is refused by the operations needing another recipe type with InvalidInputException (400).

Emulator: CreateDatasetImportJob answers HTTP 501 with error code NotImplemented and message "operation CreateDatasetImportJob is scaffolded but not implemented in this emulator" — fixed text "operation " + <the operation name taken from the request> + " is scaffolded but not implemented in this emulator", which is the emulator's own scaffold wording and not AWS wording for any error class. The handler is still the generated stub carrying the SCAFFOLDED, NOT IMPLEMENTED marker and returning Scaffolded::not_implemented. Because the import job can never be created, CreateSolution/CreateSolutionVersion/CreateCampaign are never reached, no campaign ever exists, and both end-to-end runtime checks abort at this call. Related open issue 20261023T071630Z-personalize-runtime-no-wire-path-registers-a-personalize-campaign-so describes the same unreachable provisioning path but is keyed to the earlier symptom (the harness retry loop swallowing the 501 until the 60s check timeout); the 501 now surfaces directly as the verdict, so that issue's replay keys no longer cover these checks.

Why: a client of this emulator has no sequence of SDK calls that brings a campaign into existence, so GetRecommendations, GetPersonalizedRanking and GetActionRecommendations can only ever be refused. Any application or test suite that provisions a Personalize pipeline and then performs inference fails at step 4 of provisioning with a 501 that real AWS never returns, and the recipe-type mismatch refusal (which requires a real ACTIVE campaign) can never be observed either. Code written against AWS sees a transport-level "not implemented" instead of a modeled response, so even error handling cannot recover.

Fix: implement the operation in its own handler, services/service-com-amazonaws-personalize-amazonpersonalize/src/ops/create_dataset_import_job.rs — validate the modeled input constraints, record the import job in that crate's state with an ARN and a status that progresses to ACTIVE for DescribeDatasetImportJob, and delete the SCAFFOLDED marker. The remaining pipeline operations (CreateSolution, CreateSolutionVersion, CreateCampaign and their Describe* status progression) belong in their own handler files in the same directory, and CreateCampaign must seed the personalize-runtime campaign registry from the shared engine (the registration seam in services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/state.rs), carrying the solution version's recipe type so the existing recipe-type refusal becomes reachable.

Traces: personalize-runtime.get-personalized-ranking-round-trip (ranking round trip never reaches inference), personalize-runtime.recommendations-recipe-type-mismatch-rejected (recipe-type refusal never reachable).

## Evidence
Both failing checks raise, from botocore, at the control-plane call inside the campaign builder:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDatasetImportJob operation: operation CreateDatasetImportJob is scaffolded but not implemented in this emulator

Expected (AWS, per the Smithy model): HTTP 200 with {"datasetImportJobArn": "arn:aws:personalize:<region>:<account>:dataset-import-job/<jobName>"}; on bad input one of the modeled client errors (InvalidInputException 400, ResourceNotFoundException 404, ...). DescribeDatasetImportJob then reports status CREATE PENDING -> ACTIVE.
Actual: HTTP 501, error code NotImplemented, message "operation CreateDatasetImportJob is scaffolded but not implemented in this emulator" (fixed scaffold text with the operation name from the request substituted in) — an unmodeled error class for this operation.

Downstream expectations that are therefore unreachable:
- get-personalized-ranking-round-trip expects 200 from GetPersonalizedRanking with personalizedRanking a permutation of the 10-item inputList, float scores descending, non-empty recommendationId.
- recommendations-recipe-type-mismatch-rejected expects 200 itemList from a USER_PERSONALIZATION campaign and InvalidInputException (400) from GetPersonalizedRanking / GetActionRecommendations against that same campaign.

Handler still a stub: services/service-com-amazonaws-personalize-amazonpersonalize/src/ops/create_dataset_import_job.rs lines 3-12 carry the "SCAFFOLDED, NOT IMPLEMENTED" marker and the handler returns the scaffold error (line ~197).

## Steps to Reproduce
1. Boot the emulator.
2. personalize.CreateSchema(name=<name>, schema=<interactions schema JSON>) -> succeeds; personalize.CreateDatasetGroup + poll DescribeDatasetGroup to ACTIVE -> succeeds; personalize.CreateDataset(datasetType="Interactions") -> succeeds.
3. personalize.CreateDatasetImportJob(jobName=<name>, datasetArn=<the dataset ARN>, dataSource={"dataLocation": "s3://<bucket>/<key>"}, roleArn=<role ARN>) -> HTTP 501 NotImplemented "operation CreateDatasetImportJob is scaffolded but not implemented in this emulator". Expected: HTTP 200 with datasetImportJobArn, and DescribeDatasetImportJob progressing to ACTIVE.
4. Because step 3 cannot complete, CreateSolution / CreateSolutionVersion / CreateCampaign are never reachable, so personalize-runtime.GetPersonalizedRanking(campaignArn=<any>, userId=<any>, inputList=[...]) can only answer 400 InvalidInputException "<arn> does not exist or is not active yet." Expected after the fix: the pipeline reaches an ACTIVE campaign and GetPersonalizedRanking returns 200 with a descending-scored permutation of inputList and a non-empty recommendationId.
