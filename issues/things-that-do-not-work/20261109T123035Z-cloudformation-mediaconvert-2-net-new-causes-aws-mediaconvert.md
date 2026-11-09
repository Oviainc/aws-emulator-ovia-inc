Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__mediaconvert.deploy-mediaconvert-queue-jobtemplate-preset-round-trip-and-delete cloudformation__mediaconvert.deploy-mediaconvert-queue-reserved-pricing-plan-rejected
Issue-Key: cloudformation__mediaconvert.deploy-mediaconvert-queue-jobtemplate-preset-round-trip-and-delete|run#10|AssertionError|assert listed == [(out[f"{prefix}Ref"], out[f"{prefix}Arn"])], (
Issue-Key: cloudformation__mediaconvert.deploy-mediaconvert-queue-reserved-pricing-plan-rejected|run#8|AssertionError|assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}"
Issue-Commit: 5386ff6290ae18fab3ce660917360d5fb6557b5e

# cloudformation__mediaconvert: 2 net-new causes — AWS::MediaConvert::Queue/JobTemplate/Preset reach CREATE_COMPLETE without being provisioned in MediaConvert, and a RESERVED queue with no reservation plan is accepted instead of failing the resource

> _Note: this issue was identified against the code base at commit `5386ff6290ae18fab3ce660917360d5fb6557b5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two causes.

Cause 1 of 2: a stack declaring AWS::MediaConvert::Queue, AWS::MediaConvert::JobTemplate and AWS::MediaConvert::Preset reaches CREATE_COMPLETE and its Outputs carry a Ref and a GetAtt Arn for each resource, but none of the three resources exists in MediaConvert. Failing check: cloudformation__mediaconvert.deploy-mediaconvert-queue-jobtemplate-preset-round-trip-and-delete.
AWS: each type is backed by the MediaConvert control plane — Smithy (com-amazonaws-mediaconvert-mediaconvert): CreateQueue POST /2017-08-29/queues (members Name required, PricingPlan, Status, ReservationPlanSettings, Tags), and the parallel CreateJobTemplate / CreatePreset. After CREATE_COMPLETE the created objects are first-class MediaConvert resources: ListQueues (GET /2017-08-29/queues, paginated over output member Queues) lists exactly one entry whose Name equals the template's Name and whose Arn equals the stack's Fn::GetAtt ['Queue','Arn'] (and Ref is the queue Name); likewise ListJobTemplates/ListPresets. GetQueue then reports PricingPlan ON_DEMAND, Status ACTIVE and the template's own tags, GetJobTemplate reports the declared SettingsJson and its Queue as the queue's ARN, GetPreset the declared SettingsJson; an UpdateStack that only flips Status to PAUSED updates the same queue in place (CreatedAt unchanged); and after DeleteStack all three are gone — GetQueue/GetJobTemplate/GetPreset answer NotFoundException 404 with message 'The resource you specified in name was not found.'
Emulator: CloudFormation reports CREATE_COMPLETE and serves a Ref and an Arn for each resource, but MediaConvert knows nothing about them: ListQueues returns no entry for the declared name (the check saw 'list_queues lists []' while Ref/GetAtt were the queue name and 'arn:aws:mediaconvert:<region>:<account>:queue/<name>'). The ARN and Ref are therefore synthesized by the CloudFormation side rather than returned by a real CreateQueue, and every subsequent read (GetQueue, GetJobTemplate, GetPreset, ListTagsForResource, the in-place PAUSED update, the post-delete NotFoundException) is unreachable.
Why: a client that deploys its MediaConvert estate from a template gets a green stack and usable-looking ARNs for resources that do not exist; the first CreateJob against the stack's queue, or any GetQueue/ListQueues reconciliation, fails against a queue CloudFormation claims to have created. Drift detection and delete semantics are meaningless because nothing was created.
Fix: the CloudFormation resource providers for AWS::MediaConvert::Queue / ::JobTemplate / ::Preset must call the mediaconvert handlers (CreateQueue / CreateJobTemplate / CreatePreset, and Update/Delete) and take Ref and the Arn attribute from those responses and from mediaconvert state, instead of recording a synthesized physical id in cloudformation's own stack state.

Cause 2 of 2: AWS::MediaConvert::Queue with PricingPlan RESERVED and no ReservationPlanSettings is accepted — the stack reaches CREATE_COMPLETE instead of failing the resource and rolling back. Failing check: cloudformation__mediaconvert.deploy-mediaconvert-queue-reserved-pricing-plan-rejected.
AWS: CreateQueue refuses a reserved queue submitted without reservation plan settings. Smithy models ReservationPlanSettings as an optional member of CreateQueueRequest and BadRequestException (httpError 400) as a modeled error, so the rule is enforced in the handler, not by the shape; AWS answers HTTP 400 with the message 'You must provide reservation plan settings when creating a reserved queue.' Through CloudFormation that refusal fails the resource: the Queue resource records a single CREATE_FAILED event whose ResourceStatusReason has the registry-handler form 'Resource handler returned message: "You must provide reservation plan settings when creating a reserved queue. (Service: MediaConvert, Status Code: 400, Request ID: <request id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)' — the fixed text is that sentence plus the Service/Status Code wrapper; the request id, attempt count and request token are per-call values — and the stack ends ROLLBACK_COMPLETE, after which GetQueue for that name answers NotFoundException 'The resource you specified in name was not found.'
Emulator: the stack settles CREATE_COMPLETE (the check asserted ROLLBACK_COMPLETE and got 'CREATE_COMPLETE'), so there is no CREATE_FAILED row, no handler reason and no rollback; the invalid reserved queue is reported as successfully provisioned.
Why: a template bug that AWS catches at deploy time is silently accepted, so a client believes it owns a reserved queue with a reservation plan it never supplied; the negative path code (read the CREATE_FAILED reason, expect ROLLBACK_COMPLETE, expect the queue absent) never triggers.
Fix: mediaconvert's CreateQueue validation must reject PricingPlan=RESERVED without ReservationPlanSettings with BadRequestException/400 and that exact message, and the AWS::MediaConvert::Queue provider must surface the refusal as a CREATE_FAILED resource with a HandlerErrorCode InvalidRequest reason so the stack rolls back. (Cause 2 is likely downstream of Cause 1 — a provider that never calls CreateQueue can never see its refusal — but both behaviors must be closed.)

Traces: see trace_ids.

## Evidence
Cause 1 — expected: list_queues/list_job_templates/list_presets each list exactly [(Name, Arn)] matching the stack Outputs' Ref/GetAtt pair.
Actual: AssertionError: list_queues lists []; Ref/GetAtt are 'sim2real-mcq-a641e15e0849'/'arn:aws:mediaconvert:us-west-2:000000000000:queue/sim2real-mcq-a641e15e0849' (the name/account/region are per-run values; the defect is the empty list against a non-empty Ref/Arn).

Cause 2 — expected: the RESERVED-queue stack ends 'ROLLBACK_COMPLETE' with one Queue CREATE_FAILED event whose reason is 'Resource handler returned message: "You must provide reservation plan settings when creating a reserved queue. (Service: MediaConvert, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)', and GetQueue for that name answers NotFoundException 'The resource you specified in name was not found.'
Actual: AssertionError: rejected stack ended 'CREATE_COMPLETE'.

## Steps to Reproduce
Cause 1:
1. CreateStack with a template declaring AWS::MediaConvert::Queue (Name=<q>, PricingPlan=ON_DEMAND, Status=ACTIVE, Tags), AWS::MediaConvert::JobTemplate (Name=<jt>, SettingsJson, Queue=Fn::GetAtt [Queue, Arn]) and AWS::MediaConvert::Preset (Name=<p>, SettingsJson), with Outputs exporting each Ref and Fn::GetAtt ...Arn.
2. Wait for CREATE_COMPLETE and read DescribeStacks Outputs — a Ref and an Arn are returned for all three.
3. Call mediaconvert ListQueues (paginated) and filter for <q>: the list is empty. Same for ListJobTemplates/<jt> and ListPresets/<p>. GetQueue(Name=<q>) likewise has nothing to read.

Cause 2:
1. CreateStack with a template whose only resource is AWS::MediaConvert::Queue {Name: <q>, PricingPlan: RESERVED, Status: ACTIVE} and no ReservationPlanSettings.
2. Poll DescribeStacks: the stack reaches CREATE_COMPLETE instead of ROLLBACK_COMPLETE.
3. DescribeStackEvents has no CREATE_FAILED row for the Queue resource, so no handler reason/HandlerErrorCode InvalidRequest is available.
