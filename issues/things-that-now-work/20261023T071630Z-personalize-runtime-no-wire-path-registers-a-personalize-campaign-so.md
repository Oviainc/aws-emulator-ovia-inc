Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: personalize-runtime.get-personalized-ranking-round-trip personalize-runtime.recommendations-recipe-type-mismatch-rejected
Issue-Shape-Id: com.amazonaws.personalizeruntime#AmazonPersonalizeRuntime
Issue-Key: personalize-runtime.get-personalized-ranking-round-trip|_create_import_job_with_retry#2|Timeout|time.sleep(10)
Issue-Key: personalize-runtime.recommendations-recipe-type-mismatch-rejected|_create_import_job_with_retry#2|Timeout|time.sleep(10)
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# No wire path registers a Personalize campaign, so every personalize-runtime inference call can only be refused

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPersonalizedRanking

## Description
Cause 1 of 1 — a campaign cannot be brought into existence over the wire, so the personalize-runtime data plane has no reachable success path.
AWS: a caller provisions a pipeline through the Amazon Personalize control plane — CreateSchema, CreateDatasetGroup, CreateDataset, CreateDatasetImportJob, CreateSolution, CreateSolutionVersion, CreateCampaign — polling Describe* until each resource reports ACTIVE, and then GetRecommendations / GetPersonalizedRanking / GetActionRecommendations serve inference from the deployed campaign: GetPersonalizedRanking returns a `personalizedRanking` that is a same-set permutation of `inputList` with float `score`s in descending order plus a non-empty `recommendationId`; a campaign backed by the wrong recipe type is refused with InvalidInputException (400) by the two operations that need a different recipe type. Smithy: GetPersonalizedRanking/GetRecommendations/GetActionRecommendations (restJson1, POST /personalize-ranking, /recommendations, /action-recommendations) model success 200 and errors InvalidInputException (400) / ResourceNotFoundException (404).
Emulator: the runtime registry holds campaigns, recommenders and filters that are registrable ONLY from Rust — `register_campaign` / `register_recommender` / `register_filter` on `ComAmazonawsPersonalizeruntimeAmazonpersonalizeruntimeState`, whose own module doc says a resource "exists only after its seam registered it" and that there is deliberately no auto-registration. The sibling control plane cannot drive those seams either: its pipeline operations are still 501 scaffolds (`services/service-com-amazonaws-personalize-amazonpersonalize/src/ops/create_dataset_import_job.rs` carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and returns `Scaffolded::not_implemented`), and that crate's state holds only schemas, dataset groups and datasets. So an SDK client has no way to make any campaign exist, and every inference call it makes can only be refused. Both end-to-end checks never get past provisioning and the harness kills them at the 60s timeout (CreateDatasetImportJob is retried 8x10s behind a swallow-all retry loop, so the underlying 501 never surfaces as the verdict).
Why: the entire happy path of this service is unreachable for any client of the emulator. Nothing an SDK user does can produce a 200 from GetRecommendations, GetPersonalizedRanking or GetActionRecommendations, and the recipe-type refusal (an InvalidInputException that requires a real ACTIVE campaign of a known recipe type) can never be observed, so every client exercise of this data plane sees only rejections.
Fix: give the registration seams a wire path — implement the Personalize control-plane pipeline operations (CreateDatasetImportJob, CreateSolution, CreateSolutionVersion, CreateCampaign and their Describe* status progression, each its own handler file under `services/service-com-amazonaws-personalize-amazonpersonalize/src/ops/`) and have CreateCampaign/CreateRecommender/CreateFilter seed the runtime registry from the SAME shared engine, exactly the pattern the open issue "Wire the Personalize control plane to the Personalize Events registry" (20261022T000000Z-personalize-control-plane-does-not-register-event-trackers-or-datasets-for-ingestion) asks for on the ingestion crate — that issue covers the personalize-events registry only and does NOT cover the runtime registry addressed here. The campaign record must carry its solution version's recipe type so the existing recipe-type refusal in `resolve()` becomes reachable.
Traces: personalize-runtime.get-personalized-ranking-round-trip, personalize-runtime.recommendations-recipe-type-mismatch-rejected.

## Evidence
Both checks fail with "Check timed out after 60.0s" while still inside `_build_campaign` → `_create_interactions_pipeline_stage1`, which calls `personalize.create_dataset_import_job` through an 8-attempt / 10s-sleep retry loop (80s > the 60s check budget) and then polls `describe_dataset_import_job` for ACTIVE.
Expected (get-personalized-ranking-round-trip): HTTP 200 with `personalizedRanking` a permutation of the 10-item `inputList`, float scores sorted descending, non-empty `recommendationId`.
Expected (recommendations-recipe-type-mismatch-rejected): HTTP 200 itemList from a USER_PERSONALIZATION campaign, then InvalidInputException (400) from GetPersonalizedRanking and GetActionRecommendations against that same campaign.
Actual: no campaign can be created at all — `services/service-com-amazonaws-personalize-amazonpersonalize/src/ops/create_dataset_import_job.rs` lines 3-6 and 197 return 501 NotImplemented; `services/service-com-amazonaws-personalize-amazonpersonalize/src/state.rs` lines 80-87 store only schemas/dataset groups/datasets; `services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/state.rs` lines 27-41 document that the only registration path is the in-process Rust seam.

## Steps to Reproduce
1. Boot the emulator.
2. Call personalize.CreateSchema, CreateDatasetGroup (poll to ACTIVE) and CreateDataset — these succeed.
3. Call personalize.CreateDatasetImportJob -> HTTP 501 NotImplemented; the pipeline cannot continue, so CreateSolution/CreateSolutionVersion/CreateCampaign are never reachable.
4. Call personalize-runtime.GetPersonalizedRanking (or GetRecommendations / GetActionRecommendations) with ANY campaignArn -> HTTP 400 InvalidInputException "<arn> does not exist or is not active yet.", with no sequence of SDK calls that could have made it exist. Expected after the fix: step 3 onward succeed, the campaign reaches ACTIVE, and step 4 returns 200 with a scored, descending-ordered ranking.
