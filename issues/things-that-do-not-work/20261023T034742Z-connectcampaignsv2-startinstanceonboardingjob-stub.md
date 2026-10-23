Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectcampaignsv2.campaign-lifecycle-start-pause-resume-stop-transitions connectcampaignsv2.concurrent-create-campaign-same-name-one-winner connectcampaignsv2.create-campaign-nonexistent-instance-not-found connectcampaignsv2.create-campaign-persists-full-config connectcampaignsv2.delete-campaign-removes-resource connectcampaignsv2.delete-connect-instance-config-blocked-by-active-campaign-rejected connectcampaignsv2.delete-instance-onboarding-job-rejected-while-succeeded connectcampaignsv2.describe-campaign-nonexistent-id-not-found connectcampaignsv2.pause-initialized-campaign-rejected connectcampaignsv2.put-outbound-request-batch-on-initialized-campaign-rejected connectcampaignsv2.resume-initialized-campaign-rejected connectcampaignsv2.stop-initialized-campaign-rejected connectcampaignsv2.tag-resource-and-list-tags-round-trip connectcampaignsv2.update-campaign-channel-subtype-config-round-trip connectcampaignsv2.update-campaign-communication-limits-override-round-trip connectcampaignsv2.update-campaign-entry-limits-persists connectcampaignsv2.update-campaign-name-persists connectcampaignsv2.update-campaign-schedule-persists connectcampaignsv2.update-campaign-source-succeeds-while-initialized
Issue-Shape-Id: com.amazonaws.connectcampaignsv2#AmazonConnectCampaignServiceV2
Issue-Key: connectcampaignsv2|StartInstanceOnboardingJob|NotImplemented
Issue-Commit: d66da8d621774a402898e98aa83287c30e78f3fd

# Implement connectcampaignsv2 StartInstanceOnboardingJob: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d66da8d621774a402898e98aa83287c30e78f3fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartInstanceOnboardingJob

## Description
`StartInstanceOnboardingJob` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/ops/start_instance_onboarding_job.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

19 recorded unit(s) fail on their `StartInstanceOnboardingJob` call, so each one unblocks only when `StartInstanceOnboardingJob` works: `connectcampaignsv2.campaign-lifecycle-start-pause-resume-stop-transitions`, `connectcampaignsv2.concurrent-create-campaign-same-name-one-winner`, `connectcampaignsv2.create-campaign-nonexistent-instance-not-found`, `connectcampaignsv2.create-campaign-persists-full-config`, `connectcampaignsv2.delete-campaign-removes-resource`, `connectcampaignsv2.delete-connect-instance-config-blocked-by-active-campaign-rejected`, `connectcampaignsv2.delete-instance-onboarding-job-rejected-while-succeeded`, `connectcampaignsv2.describe-campaign-nonexistent-id-not-found`, `connectcampaignsv2.pause-initialized-campaign-rejected`, `connectcampaignsv2.put-outbound-request-batch-on-initialized-campaign-rejected`, `connectcampaignsv2.resume-initialized-campaign-rejected`, `connectcampaignsv2.stop-initialized-campaign-rejected`, `connectcampaignsv2.tag-resource-and-list-tags-round-trip`, `connectcampaignsv2.update-campaign-channel-subtype-config-round-trip`, `connectcampaignsv2.update-campaign-communication-limits-override-round-trip`, `connectcampaignsv2.update-campaign-entry-limits-persists`, `connectcampaignsv2.update-campaign-name-persists`, `connectcampaignsv2.update-campaign-schedule-persists`, `connectcampaignsv2.update-campaign-source-succeeds-while-initialized`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/ops/start_instance_onboarding_job.rs`. Replaying the corpus, the emulator refused the `StartInstanceOnboardingJob` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartInstanceOnboardingJob operation: operation StartInstanceOnboardingJob is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the connectcampaignsv2 emulator
Call StartInstanceOnboardingJob and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
