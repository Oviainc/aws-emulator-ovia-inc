Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the bedrock-data-automation-runtime emulator

_Filed: 2026-10-21T21:18:17Z_
_Operation: service-com-amazonaws-bedrockdataautomationruntime-amazonbedrockkeystoneruntimeservice | Class: stub_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 6 operations of AmazonBedrockKeystoneRuntimeService are unmodified scaffolds answering HTTP 501 and holding no state. Implement this minimal, stateful happy path (awsJson1_1, `POST /` with `X-Amz-Target: AmazonBedrockKeystoneRuntimeService.<Op>`; GetDataAutomationStatus carries its `invocationArn` as an @httpLabel per the model):

1. **InvokeDataAutomataionAsync** (`InvokeDataAutomationAsync`) — input `inputConfiguration.s3Uri` (required), `outputConfiguration.s3Uri` (required), `dataAutomationProfileArn` (required, must match the data-automation-profile ARN pattern), optional `dataAutomationConfiguration.dataAutomationProjectArn`/`stage`, `blueprints` (1..40), `encryptionConfiguration`, `notificationConfiguration.eventBridgeConfiguration`, `tags` (0..200), `clientToken` (@idempotencyToken). Must MINT and PERSIST a new invocation record, then return its real `invocationArn` of the form `arn:aws:bedrock:<region>:<account-id>:data-automation-invocation/<id>`. The op is `@idempotent`: the same `clientToken` with the same payload must return the SAME `invocationArn` and not create a second job.
2. **GetDataAutomationStatus** — input `invocationArn`. Must look the invocation up in state and return the stored `status` (`Created` -> `InProgress` -> `Success` progression driven by stored timestamps), `outputConfiguration` (the s3Uri given at submit time), `jobSubmissionTime`, and once complete `jobCompletionTime` + computed `jobDurationInSeconds`. For a well-formed but unknown ARN it must return `ResourceNotFoundException` (HTTP 404), not a hollow 200; for an ARN failing the modeled pattern/length, `ValidationException` (HTTP 400).
3. **TagResource** — input `resourceARN` (data-automation-invocation ARN), `tags` (required). Must resolve the ARN to a stored invocation (404 `ResourceNotFoundException` if absent) and merge the tags into that invocation's tag map (same key overwrites value).
4. **ListTagsForResource** — input `resourceARN`. Must return exactly the tags persisted by InvokeDataAutomationAsync's `tags` member plus any TagResource calls — an empty/absent list when none, never a canned list. 404 for an unknown invocation.
5. **UntagResource** — input `resourceARN`, `tagKeys` (required). Must remove those keys from the stored tag map; a subsequent ListTagsForResource must no longer show them. Removing an absent key is a no-op success. 404 for an unknown invocation.

Optionally round out the slice with **InvokeDataAutomation** (the synchronous variant): input `inputConfiguration` + `dataAutomationProfileArn` required, output must include the required `semanticModality` (AUDIO|DOCUMENT|IMAGE|VIDEO, derived from the input s3Uri extension/asset config rather than hardcoded), `outputConfiguration`, and `outputSegments` (default `[]`).

Every one of these must compute its response from persisted state. No canned responses, no echoing the request back, and no minting an `invocationArn` that is never stored (a random ID without storage makes step 2-5 unimplementable and is a failure). Validation of the modeled ARN patterns and length ranges must produce `ValidationException` (400), and operations on a non-existent invocation must produce `ResourceNotFoundException` (404).

## State Model

In `src/state.rs`, on the service state struct:

- `invocations: HashMap<String /* invocationArn */, Invocation>`
- `client_tokens: HashMap<String /* clientToken */, String /* invocationArn */>` — backs `@idempotent` replay of InvokeDataAutomationAsync
- `struct Invocation {
    invocation_arn: String,
    invocation_id: String,
    input_s3_uri: String,
    output_s3_uri: String,
    data_automation_project_arn: Option<String>,
    stage: Option<DataAutomationStage>,
    data_automation_profile_arn: String,
    blueprints: Vec<Blueprint>,
    encryption_configuration: Option<EncryptionConfiguration>,
    notification_configuration: Option<NotificationConfiguration>,
    status: AutomationJobStatus,   // Created | InProgress | Success | ClientError | ServiceError
    error_type: Option<String>,
    error_message: Option<String>,
    job_submission_time: SystemTime,
    job_completion_time: Option<SystemTime>,
    tags: BTreeMap<String, String>, // key -> value, serialized back as list<Tag>
    client_token: Option<String>,
  }`

`jobDurationInSeconds` is derived (`job_completion_time - job_submission_time`), never stored independently. The status progression may be clock-driven (e.g. `Created` for the first read, then `Success` with a completion time), but the transition must be recorded in `invocations` so repeated reads are consistent.

## Evidence

OPERATIONS.md for this crate lists all 6 operations with their inputs/outputs/errors: InvokeDataAutomationAsync (@idempotent, required `inputConfiguration`, `outputConfiguration`, `dataAutomationProfileArn`; required output `invocationArn`), GetDataAutomationStatus (@readonly, `invocationArn` @httpLabel; outputs `status`, `outputConfiguration`, `jobSubmissionTime`, `jobCompletionTime`, `jobDurationInSeconds`; errors include ResourceNotFoundException 404 and ValidationException 400), TagResource / ListTagsForResource / UntagResource (all keyed by a `data-automation-invocation` `resourceARN`), and InvokeDataAutomation (required output `semanticModality`, `outputSegments` default []). Shape detail from smithy_query: InputConfiguration/OutputConfiguration each require `s3Uri`, DataAutomationConfiguration requires `dataAutomationProjectArn` with optional `stage`, Tag requires `key`+`value`, NotificationConfiguration requires `eventBridgeConfiguration`. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all six handler files (get_data_automation_status.rs, invoke_data_automation.rs, invoke_data_automation_async.rs, list_tags_for_resource.rs, tag_resource.rs, untag_resource.rs), so no operation has behavior yet.

## Steps to Reproduce

1. POST / with X-Amz-Target: AmazonBedrockKeystoneRuntimeService.InvokeDataAutomationAsync and body {"clientToken":"tok-1","inputConfiguration":{"s3Uri":"s3://my-bucket/in/doc.pdf"},"outputConfiguration":{"s3Uri":"s3://my-bucket/out/"},"dataAutomationProfileArn":"arn:aws:bedrock:us-east-1:123456789012:data-automation-profile/us.data-automation-v1","tags":[{"key":"env","value":"dev"}]} — expect 200 with a real invocationArn matching ^arn:aws:bedrock:[a-z0-9-]*:[0-9]{12}:data-automation-invocation/[A-Za-z0-9-_]+$ (today: 501 NotImplemented).
2. Repeat the identical request with the same clientToken "tok-1" — expect the SAME invocationArn and no second job in state.
3. Call GetDataAutomationStatus with that invocationArn — expect 200 whose outputConfiguration.s3Uri is "s3://my-bucket/out/", a status drawn from Created|InProgress|Success, and a jobSubmissionTime equal to the submit time; once status is Success, jobCompletionTime and a jobDurationInSeconds consistent with the two timestamps.
4. Call GetDataAutomationStatus with a well-formed but never-created ARN (arn:aws:bedrock:us-east-1:123456789012:data-automation-invocation/does-not-exist) — expect ResourceNotFoundException (HTTP 404), not 200 and not 501.
5. Call GetDataAutomationStatus with invocationArn "not-an-arn" — expect ValidationException (HTTP 400).
6. Call ListTagsForResource with the invocation ARN — expect exactly [{"key":"env","value":"dev"}] from the submit-time tags.
7. Call TagResource with tags [{"key":"team","value":"search"},{"key":"env","value":"prod"}], then ListTagsForResource — expect env=prod (overwritten) and team=search.
8. Call UntagResource with tagKeys ["team"], then ListTagsForResource — expect only env=prod remaining.
9. Call TagResource/ListTagsForResource/UntagResource against an unknown invocation ARN — expect ResourceNotFoundException (HTTP 404) from each.
