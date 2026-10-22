Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-data-automation-runtime-tag-untag-partial-removal-on-real-invocation
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-data-automation-runtime-tag-untag-partial-removal-on-real-invocation
Issue-Shape-Id: com.amazonaws.bedrockdataautomationruntime#AmazonBedrockKeystoneRuntimeService
Issue-Key: bedrock-data-automation|CreateDataAutomationProject|2xx->5xx|status
Issue-Commit: 67b7a4ea014b7cfd9cdb5213b26731b0db184c45

# CreateDataAutomationProject returns 500 instead of modeled 201 Created

> _Note: this issue was identified against the code base at commit `67b7a4ea014b7cfd9cdb5213b26731b0db184c45`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataAutomationProject

## Description
Cause 1 of 1: CreateDataAutomationProject on the bedrock-data-automation control plane answers HTTP 500 instead of creating the project and answering its modeled success status.

  AWS: restJson1. CreateDataAutomationProject is bound as PUT /data-automation-projects/ with an explicit @http code of 201, i.e. a successful create answers HTTP 201 Created with CreateDataAutomationProjectResponse in the body (Smithy: services/service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice/smithy-model/com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice.json, service AmazonBedrockKeystoneBuildTimeService, sdkId "Bedrock Data Automation", apiVersion 2023-07-26, protocols [restJson1]; CreateDataAutomationProject http {method: PUT, uri: /data-automation-projects/, code: 201}, idempotent, input CreateDataAutomationProjectRequest with required projectName (@length 1..128, @pattern ^[a-zA-Z0-9-_]+$) and required standardOutputConfiguration, output CreateDataAutomationProjectResponse with required projectArn (@pattern ^arn:aws(|-cn|-us-gov):bedrock:[a-zA-Z0-9-]*:(aws|[0-9]{12}):data-automation-project/[a-zA-Z0-9-]{12,36}$), projectStage and status). The only 500 the operation models is InternalServerException (errorKind server, @httpError 500); every other modeled failure is a 4xx client error (ValidationException 400, ServiceQuotaExceededException 402, AccessDeniedException 403, ConflictException 409, ThrottlingException 429). A well-formed create request therefore must not produce a 5xx. The recorded step is a well-formed create, and its recorded response status is 201.

  Emulator: HTTP 500. The replay reports only the status key as diverging (detail "status: expected 201, got 500"), so no part of the modeled 201 response is produced and the created project never comes into existence.

  Hypothesis: the same front-door routing fault already filed for the sibling runtime service in the open issue "SigV4-name routing makes all bedrock-data-automation-runtime calls 500 AmbiguousService" (20261022T042609Z-bedrock-data-automation-runtime-sigv4-name-routing-makes-all-bedrock-data-automation.md). This service's wire identity collides the same way: service_info gives sigv4Name "bedrock" and awsService.arnNamespace "bedrock" while endpointPrefix is the distinct "bedrock-data-automation", and the emulator's collision diagnostic on the runtime service reports six emulated services all claiming the signing name "bedrock". That open issue is for service bedrock-data-automation-runtime and carries only runtime replay keys, so it does NOT cover this control-plane operation; this divergence is reported with no owning issue and no blocking stub. The hypothesis is not asserted as the cause — only the 500-instead-of-201 behavior is.

  Why: CreateDataAutomationProject is the entry point for the whole Bedrock Data Automation control plane, so a client can never obtain a projectArn and every downstream call that needs one (invoke, status, tag/untag of a real project) is unreachable. Because the failure is a 5xx rather than a modeled 4xx, SDK retry policies classify it as a retryable server fault and retry a request that can never succeed, instead of surfacing a non-retryable client error. In this corpus the failure is a fixture setup step, so it also blocks the dependent steps behind it (the replay reports 7 blocked steps), masking the real tag/untag behavior of the service.

  Fix: the front-door service router / service-registry dispatch key that selects which emulated service serves the request, before CreateDataAutomationProject's handler runs — resolve the target service by its endpointPrefix (and the restJson1 HTTP route) rather than by the shared sigv4 signing name, so bedrock-family services that all sign as "bedrock" remain distinguishable. If dispatch is already correct, the fault is in the CreateDataAutomationProject handler/state for the create path, which must persist the project and serialize the 201 response.

  Traces: bedrock-data-automation-runtime-tag-untag-partial-removal-on-real-invocation#setup#1.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from the checkout (emulator_up true, /__admin/health answered 200 and the data listener accepts; bootstrap_signal false; state_reset_failures 0; 9 fixtures, 27 steps, 5 passed, 15 mismatched, 7 blocked, score 18.5%).

One divergence cluster for service bedrock-data-automation:
  divergence_key: service=bedrock-data-automation|CreateDataAutomationProject|status=201->500|keys=status
  verdict: mismatch
  fixture_id: bedrock-data-automation-runtime-tag-untag-partial-removal-on-real-invocation
  step_key: setup#1
  expected_status: 201 (recorded AWS wire)
  actual_status: 500 (emulator)
  detail: "status: expected 201, got 500"
  affected_count: 1 (bedrock-data-automation-runtime-tag-untag-partial-removal-on-real-invocation#setup#1)
  stub_path: "" (no unimplemented operation blocks it), owned_by: [] (no open issue carries this replay key)

The divergence is on the status alone: the expected 201 Created from the operation's @http binding is replaced by a 500, so the modeled CreateDataAutomationProjectResponse (projectArn/projectStage/status) is never emitted. Smithy grounding: CreateDataAutomationProject http {method PUT, uri /data-automation-projects/, code 201}; modeled errors AccessDeniedException(403), ConflictException(409), InternalServerException(500, server), ServiceQuotaExceededException(402), ThrottlingException(429), ValidationException(400); service common_errors AccessDeniedException, InternalServerException, ThrottlingException, ValidationException; sigv4Name "bedrock", awsService.endpointPrefix "bedrock-data-automation", awsService.arnNamespace "bedrock", protocols [restJson1].

Scope note: the service IS bootstrapped (service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice is reported bootstrapped and has no open bootstrap issue), so this is a fidelity defect, not a missing bootstrap. The 6 other divergence clusters in this replay all belong to the sibling service bedrock-data-automation-runtime and are already owned by 20261022T042609Z-bedrock-data-automation-runtime-sigv4-name-routing-makes-all-bedrock-data-automation.md.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from that checkout (/__admin/health answers 200; the data listener accepts).
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Representative step: fixture bedrock-data-automation-runtime-tag-untag-partial-removal-on-real-invocation, step setup#1 — a restJson1 PUT /data-automation-projects/ signed with credential-scope service "bedrock", carrying a well-formed CreateDataAutomationProjectRequest (required projectName matching ^[a-zA-Z0-9-_]+$ within 1..128 chars, plus required standardOutputConfiguration).
4. Observe HTTP 500 where the recorded AWS response and the operation's @http code are both 201, and no CreateDataAutomationProjectResponse body (no projectArn) is returned.
5. Expected after fix: the step is dispatched to the bedrock-data-automation handlers, the project is created and persisted, and the response is HTTP 201 with projectArn matching the modeled data-automation-project ARN pattern, projectStage and status. No response on this path carries a 5xx status unless InternalServerException is genuinely modeled for it.
6. Re-replay this fixture to confirm: with setup#1 passing, the 7 previously blocked dependent steps behind it must also execute rather than being skipped.
