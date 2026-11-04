Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.feature-group-create-active-delete sagemaker.feature-group-create-to-deleted-state
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.feature-group-create-active-delete|run#14|AssertionError|assert update_code == "ResourceNotFound", (
Issue-Key: sagemaker.feature-group-create-to-deleted-state|run#17|AssertionError|assert mutation_code == "ResourceNotFound", (
Issue-Commit: c1b34f592c646ed434adfb831b97b313d8f8d1bc

# SageMaker UpdateFeatureGroup is an unimplemented scaffold stub: every call answers 501 NotImplemented instead of the modeled ResourceNotFound/success (1 cause)

> _Note: this issue was identified against the code base at commit `c1b34f592c646ed434adfb831b97b313d8f8d1bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFeatureGroup

## Description
One net-new root cause. This issue bundles ONE cause and that one cause must be closed before submitting.

Cause 1 of 1: UpdateFeatureGroup is still the generated scaffold stub, so the whole mutation half of the Feature Store feature-group lifecycle is unreachable and the operation never produces either of its two real outcomes (a 200 echoing FeatureGroupArn for a live group, or a 400 ResourceNotFound for a name that no longer exists).

AWS: Smithy (services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json): UpdateFeatureGroup takes FeatureGroupName (FeatureGroupNameOrArn, @required, @length 1..256, @pattern ^(arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:feature-group\/)?([a-zA-Z0-9]([_-]*[a-zA-Z0-9]){0,63})$), plus optional FeatureAdditions (@length 1..100), OnlineStoreConfig and ThroughputConfig; its output FeatureGroupArn is @required; its ONLY modeled errors are ResourceLimitExceeded (client, HTTP 400) and ResourceNotFound (client, HTTP 400) — NotImplemented is not modeled on this operation at all. Recorded behavior: after DeleteFeatureGroup has taken effect, a mutation addressed to that FeatureGroupName is refused HTTP 400, error code ResourceNotFound, with the same text the service already uses for the read path — "Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name <FeatureGroupName>". Fixed text: "Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name "; the only variable part is the FeatureGroupName taken from the request. (The emulator already renders exactly that message for DescribeFeatureGroup on the same deleted name — both checks pass that assertion immediately before failing — so the required wording is not in doubt.)

Emulator: every UpdateFeatureGroup call, for any input and regardless of whether the feature group exists, returns the scaffold error — error code NotImplemented (HTTP 501), "operation UpdateFeatureGroup is scaffolded but not implemented in this emulator". The handler binds its input and immediately returns the stub error; nothing is read from or written to feature-group state.

Why: a client relying on AWS's behavior gets two things wrong. (1) It can never add a feature, change OnlineStoreConfig/TtlDuration or switch ThroughputConfig on a feature group the emulator itself created and reports as Created — the group is effectively immutable, so any schema-evolution or TTL-configuration path dead-ends. (2) Its not-found branch never runs: instead of the modeled 400 ResourceNotFound that tells it the group is gone, it receives an unmodeled code on a 5xx status, which SDK retry logic classifies as a transient server fault and retries, turning a deterministic "this resource no longer exists" into a retry storm ending in an error the caller cannot key on. The same 501 also makes a deleted group indistinguishable from a live one on the mutation path.

Fix: the operation's handler — services/service-com-amazonaws-sagemaker-sagemaker/src/ops/update_feature_group.rs. Resolve FeatureGroupName (name or ARN form, per the member's pattern) against feature-group state; refuse an absent/deleted name with the modeled ResourceNotFound carrying the message template above; otherwise apply FeatureAdditions / OnlineStoreConfig / ThroughputConfig to the stored record and return the group's FeatureGroupArn. The SCAFFOLDED marker line in the module header must be deleted as part of the fix.

Traces: sagemaker.feature-group-create-active-delete, sagemaker.feature-group-create-to-deleted-state. Neither check was marked ALREADY FILED or OWNED by the replay report. Closest open issues, none of which covers this operation: 20261029T222441Z-sagemaker-lineage-update-projection-and-scaling-recommendation-paths.md names the UpdateTrial, UpdateCodeRepository and UpdateAction stubs but not UpdateFeatureGroup; 20261029T192819Z-sagemaker-update-operations-are-accepted-but-never-mutate.md is about Update* calls that are ACCEPTED and silently no-op (this one is refused 501, never accepted); 20261024T160943Z-sagemaker-control-plane-gaps-createuserprofile-and-deleteaibenchmarkjob-are.md covers CreateFeatureGroup's missing offline-store access probe; 20261023T183034Z-sagemaker-feature-store-data-plane-is-not-event.md and 20261029T072224Z-s3-sagemaker-a-feature-store-feature-group-with.md cover the Feature Store data plane and offline-store propagation after a successful create.

## Evidence
Cause 1 — replayed against the emulator:

sagemaker.feature-group-create-active-delete (check_sagemaker_feature_group_create_active_delete.py line 64):
  "AssertionError: expected a mutation on the deleted FeatureGroupName to fail with ResourceNotFound, got 'NotImplemented'"
The check had already PASSED the preceding assertions: CreateFeatureGroup succeeded, DescribeFeatureGroup reached FeatureGroupStatus='Created' with the echoed FeatureGroupName/RecordIdentifierFeatureName, DeleteFeatureGroup succeeded, and the post-delete DescribeFeatureGroup was refused with code ResourceNotFound and message "Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name <name>". Only the follow-on UpdateFeatureGroup(FeatureGroupName=<deleted name>, FeatureAdditions=[{'FeatureName':'late_addition','FeatureType':'String'}]) diverged.

sagemaker.feature-group-create-to-deleted-state (check_sagemaker_feature_group_create_to_deleted_state.py line 62):
  "AssertionError: expected UpdateFeatureGroup on a deleted group to fail with ResourceNotFound, got NotImplemented"
Same shape: create -> Created -> describe (FeatureGroupArn, RecordIdentifierFeatureName, OnlineStoreConfig.EnableOnlineStore=True all correct) -> delete -> DescribeFeatureGroup refused ResourceNotFound with the AWS message; then UpdateFeatureGroup(FeatureGroupName=<deleted name>, FeatureAdditions=[{'FeatureName':'extra_feature','FeatureType':'String'}]) answered NotImplemented.

Source of the 501 (services/service-com-amazonaws-sagemaker-sagemaker/src/ops/update_feature_group.rs):
  line 4: "//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented` and asserts NOTHING about `UpdateFeatureGroup`'s real behaviour."
  line 167: `let _ = (&input, ctx, &self.engine);`
  line 177: `Err(crate::scaffolded::Scaffolded::not_implemented("UpdateFeatureGroup"))`
The modeled error channel for the operation already exists in that file (lines 107-112: ResourceLimitExceeded, ResourceNotFound), so the correct refusal is constructible without model changes.

Expected vs actual, as a client sees it:
  expected: HTTP 400, code ResourceNotFound, Message "Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name <FeatureGroupName>"
  actual:   HTTP 501, code NotImplemented, Message "operation UpdateFeatureGroup is scaffolded but not implemented in this emulator"
For a LIVE feature group:
  expected: HTTP 200 with the @required FeatureGroupArn and the update applied to stored state
  actual:   HTTP 501 NotImplemented

## Steps to Reproduce
Negative path (what the two checks exercise):
1. CreateFeatureGroup(FeatureGroupName=<name>, RecordIdentifierFeatureName='record_id', EventTimeFeatureName='event_time', FeatureDefinitions=[{'FeatureName':'record_id','FeatureType':'String'},{'FeatureName':'event_time','FeatureType':'String'}], OnlineStoreConfig={'EnableOnlineStore': True}).
2. Poll DescribeFeatureGroup(FeatureGroupName=<name>) until FeatureGroupStatus == 'Created'.
3. DeleteFeatureGroup(FeatureGroupName=<name>).
4. Poll DescribeFeatureGroup(FeatureGroupName=<name>) until it is refused -> observe the expected 400 ResourceNotFound "Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name <name>" (this part already matches).
5. UpdateFeatureGroup(FeatureGroupName=<name>, FeatureAdditions=[{'FeatureName':'late_addition','FeatureType':'String'}]) -> expect HTTP 400, code ResourceNotFound, Message "Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name <name>"; observe HTTP 501, code NotImplemented.

Positive control (also broken, same stub):
6. Repeat steps 1-2, then UpdateFeatureGroup(FeatureGroupName=<name>, FeatureAdditions=[{'FeatureName':'extra_feature','FeatureType':'String'}]) on the LIVE group -> expect HTTP 200 with FeatureGroupArn and the new feature visible in a subsequent DescribeFeatureGroup's FeatureDefinitions; observe HTTP 501 NotImplemented and no state change.

Re-replay sagemaker.feature-group-create-active-delete and sagemaker.feature-group-create-to-deleted-state to confirm the fix.
