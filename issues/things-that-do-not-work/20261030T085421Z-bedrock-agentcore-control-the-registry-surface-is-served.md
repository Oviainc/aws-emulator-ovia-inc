Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.create-registry-duplicate-name-conflict bedrock-agentcore-control.registry-record-lifecycle-with-approval bedrock-agentcore-control.submit-registry-record-for-approval-concurrent-submission-conflict
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control.create-registry-duplicate-name-conflict|_assert_denied_naming_op#6|AssertionError|raise AssertionError(
Issue-Key: bedrock-agentcore-control.registry-record-lifecycle-with-approval|run#6|AssertionError|raise AssertionError(
Issue-Key: bedrock-agentcore-control.submit-registry-record-for-approval-concurrent-submission-conflict|_error#5|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Commit: ba1310c994fc47a47280538753d01db9a79290df

# bedrock-agentcore-control: the Registry surface is served instead of being refused — CreateRegistry/ListRegistries/GetRegistry/DeleteRegistry must answer 403 AccessDeniedException naming the action

> _Note: this issue was identified against the code base at commit `ba1310c994fc47a47280538753d01db9a79290df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRegistry

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: the AgentCore Registry action namespace carries no entitlement gate in the emulator, so every Registry operation is fully served out of local state where AWS refuses it outright.

AWS: recorded behavior only for the gate itself — the Smithy model declares the surface normally (CreateRegistry: POST /registries, success 202, required output member registryArn matching ^arn:aws(-[^:]+)?:bedrock-agentcore:[a-z0-9-]+:[0-9]{12}:registry/[a-zA-Z0-9]{12,16}$; ListRegistries: GET /registries, 200; GetRegistry: GET /registries/{registryId}, 200; DeleteRegistry: DELETE /registries/{registryId}, 202), and every one of those four operations models AccessDeniedException at HTTP 403 ("This exception is thrown when a request is denied per access permissions"). The recordings show the AWS Agent Registry feature is gated beneath ordinary IAM authorization for the recorded account: each of the four operations is refused immediately and deterministically with HTTP 403, error code AccessDeniedException, and a message that names the specific action attempted. The message template is: fixed text `User: `, then the caller's ARN (a value taken from the credentials that signed the request — the sts:GetCallerIdentity Arn of the signing principal, so an assumed-role caller yields its assumed-role ARN), then fixed text ` is not authorized to perform: bedrock-agentcore:`, then the action name of the operation invoked (`CreateRegistry`, `ListRegistries`, `GetRegistry`, `DeleteRegistry`). Only the ARN and the action name are variable; the rest of the text is fixed. The recordings also establish that the denial is specific to this namespace, not a blanket refusal: ListAgentRuntimes succeeds with HTTP 200 under the same credentials; and that the gate sits beneath IAM: a brand-new IAM role granted an explicit Allow on `bedrock-agentcore:*` is still denied CreateRegistry with the same code and the same message, with its own assumed-role ARN substituted.

Emulator: the Registry surface is implemented against local state and answers success. CreateRegistry returns HTTP 202 with a minted registry, e.g. {'registryArn': 'arn:aws:bedrock-agentcore:<region>:<account>:registry/<registryId>'} (the handler mints an id, inserts a RegistryRecord, enforces name uniqueness, a CREATING->READY clock and a MAX_REGISTRIES quota). ListRegistries likewise returns a list rather than refusing. No AccessDeniedException is ever produced for these actions, under the default credentials or under a freshly created role. Because the very first Registry call succeeds, all three checks fail at that call and never reach their later assertions on GetRegistry/DeleteRegistry/ListRegistries denial and on the explicit-grant probe.

Why: a client written against AWS gets the opposite verdict from the emulator. Code that expects to catch AccessDeniedException (feature-detecting whether the Agent Registry capability is entitled for the account, or asserting that a tenant cannot create registries) sees a 202 and a live registryArn instead, so it takes the entitled path and then builds registry records, approvals and submissions on an account where none of that exists on AWS. Negative-path and authorization tests pass locally and fail in production; a least-privilege audit that relies on the deny being enforced below IAM is silently inverted.

Fix: the four Registry operation handlers in the bedrock-agentcore-control service crate — src/ops/create_registry.rs, src/ops/list_registries.rs, src/ops/get_registry.rs, src/ops/delete_registry.rs — must apply the namespace entitlement gate as their first act, before any state lookup, mutation, validation or quota check, returning the modeled AccessDeniedException (403) with the message template above, with the action name of the invoked operation and the caller ARN resolved from the request's signing credentials. The gate belongs in one shared helper used by all four (the service's authorization/entitlement layer, alongside the other refusal helpers in src/state.rs) rather than being re-derived per handler, and it must not be bypassable by an identity policy: a principal holding `bedrock-agentcore:*` is still denied. The dependent record operations the checks name as gated by it (SubmitRegistryRecordForApproval, UpdateRegistryRecord, DeleteRegistryRecord) become unreachable once no registry can be created; they should not be made to answer success through some other path.

Traces: bedrock-agentcore-control.create-registry-duplicate-name-conflict (mismatch — CreateRegistry returned 202 with a registryArn; assertion "CreateRegistry succeeded ... Registry is no longer withdrawn for this account"); bedrock-agentcore-control.registry-record-lifecycle-with-approval (mismatch — "create_registry succeeded"); bedrock-agentcore-control.submit-registry-record-for-approval-concurrent-submission-conflict (mismatch — "expected a ClientError, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 202, ...}, 'registryArn': ...}"). Emulator code: services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/create_registry.rs lines 173-282 (handler mints and persists a registry; no denial path). No open issue covers this: the registry-named open issues (e.g. 20261029T185843Z-agent-registry-is-entirely-unreachable-router-resolves-services.md, 20261029T222852Z-agent-registry-control-agentregistrycontrol-wire-replay-every-step-refused-pre-dispatch.md) concern the separate agent-registry / agent-registry-control services and their router ambiguity, not the bedrock-agentcore-control Registry actions.

## Evidence
Expected (AWS, from the recordings): each of the four Registry operations refused immediately.
  HTTP 403, Error.Code = "AccessDeniedException"
  Error.Message = "User: <caller-arn> is not authorized to perform: bedrock-agentcore:<ActionName>"
    fixed text: "User: " / " is not authorized to perform: bedrock-agentcore:"
    variable:   <caller-arn> = ARN of the principal that signed the request (assumed-role ARN for an assumed role); <ActionName> = CreateRegistry | ListRegistries | GetRegistry | DeleteRegistry
  Still denied, with the same code and message, for a fresh IAM role holding an explicit Allow on "bedrock-agentcore:*" (the gate is beneath IAM).
  Positive control: ListAgentRuntimes under the same credentials returns HTTP 200 with an "agentRuntimes" member — the denial is namespace-specific, not blanket.

Actual (emulator):
  CreateRegistry -> HTTP 202, body {"registryArn": "arn:aws:bedrock-agentcore:<region>:<account>:registry/<registryId>"} (registry persisted in service state; no error raised)
  ListRegistries -> succeeds (never reached in the checks, since CreateRegistry already fails the assertion)
  No AccessDeniedException is reachable for these actions under any credentials.

Smithy (services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json):
  CreateRegistry  POST /registries   success 202, errors: AccessDeniedException 403, ConflictException 409, InternalServerException 500, ServiceQuotaExceededException 402, ThrottlingException 429, ValidationException 400
  ListRegistries  GET /registries    success 200, errors include AccessDeniedException 403
  GetRegistry     GET /registries/{registryId}    success 200, errors include AccessDeniedException 403
  DeleteRegistry  DELETE /registries/{registryId} success 202, errors include AccessDeniedException 403

Emulator source: src/ops/create_registry.rs handle() reaches state, enforces clientToken replay, CUSTOM_JWT validation, name uniqueness, MAX_REGISTRIES quota, then mints and inserts a RegistryRecord and returns the ARN — there is no AccessDeniedException construction anywhere in the handler, although CreateRegistryErrorModeled::AccessDeniedException is declared.

## Steps to Reproduce
1. Boot the emulator and point a botocore client at it: bac = client("bedrock-agentcore-control").
2. Positive control: bac.list_agent_runtimes() -> expect HTTP 200 with "agentRuntimes" (this does hold today).
3. bac.create_registry(name="<unique alnum/dash name>")
   Expected: ClientError, HTTP 403, Code "AccessDeniedException", Message "User: <caller arn from sts.get_caller_identity()[\"Arn\"]> is not authorized to perform: bedrock-agentcore:CreateRegistry".
   Actual: HTTP 202 with {"registryArn": "arn:aws:bedrock-agentcore:...:registry/<id>"}.
4. bac.list_registries(maxResults=1) / bac.get_registry(registryId="aaaaaaaaaaaa") / bac.delete_registry(registryId="aaaaaaaaaaaa")
   Expected: the same AccessDeniedException, each naming bedrock-agentcore:ListRegistries / GetRegistry / DeleteRegistry respectively.
   Actual: served from local state (success, or a not-found/validation refusal) — never AccessDeniedException.
5. Create an IAM role with an inline policy {"Effect":"Allow","Action":"bedrock-agentcore:*","Resource":"*"}, sts:AssumeRole into it, and retry create_registry with those credentials.
   Expected: still AccessDeniedException, message naming the assumed-role ARN and bedrock-agentcore:CreateRegistry.
   Actual: succeeds.
6. Re-replay: bedrock-agentcore-control.create-registry-duplicate-name-conflict, bedrock-agentcore-control.registry-record-lifecycle-with-approval, bedrock-agentcore-control.submit-registry-record-for-approval-concurrent-submission-conflict.
