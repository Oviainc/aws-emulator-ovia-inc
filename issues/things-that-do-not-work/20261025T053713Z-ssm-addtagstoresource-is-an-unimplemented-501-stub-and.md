Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__ssm__sts.assumed-role-sees-the-same-account-state ssm__sts.parameter-last-modified-user-is-the-live-sts-session-identity
Issue-Key: iam__ssm__sts|AddTagsToResource|NotImplemented
Issue-Key: ssm__sts.parameter-last-modified-user-is-the-live-sts-session-identity|run#16|AssertionError|assert recorded, (
Issue-Commit: 9736c84da1ff98c2027d891c67e5be6441f006f6

# SSM: AddTagsToResource is an unimplemented 501 stub, and DescribeParameters never reports ParameterMetadata.LastModifiedUser (2 causes)

> _Note: this issue was identified against the code base at commit `9736c84da1ff98c2027d891c67e5be6441f006f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddTagsToResource, DescribeParameters

## Description
TWO distinct root causes in the SSM (Systems Manager / Parameter Store) emulator, bundled because they share one service. Both must be closed before submitting.

Note on attribution: cause 1 is grounded in the check id `iam__ssm__sts.assumed-role-sees-the-same-account-state`, whose leading token is a composite of the three services the check spans; the diverging call is SSM's AddTagsToResource, so the issue is filed under service `ssm`. The SSM crate (services/service-com-amazonaws-ssm-amazonssm) is serving — PutParameter/GetParameter/DescribeParameters all answered with real per-region parameter state in this same replay.

Cause 1 of 2: AddTagsToResource answers 501 NotImplemented for every request, including the tagging of a parameter that does not exist.
AWS: AddTagsToResource is a served SSM operation (Smithy: AddTagsToResource, input ResourceType (ResourceTypeForTagging enum, incl. `Parameter`) required, ResourceId required, Tags (TagList, @length 0..1000) required; output AddTagsToResourceResult, success HTTP 200). Its modeled client errors are InvalidResourceId (HTTP 400, "The resource ID isn't valid. Verify that you entered the correct ID and try again."), InvalidResourceType (400), TooManyTagsError (400), TooManyUpdates (400). Tagging a ResourceType=Parameter whose name no longer exists is refused with error code `InvalidResourceId` at HTTP 400 — that is the recorded expectation the check asserts for a deleted parameter.
Emulator: every AddTagsToResource call is refused with error code `NotImplemented` at HTTP 501, message (example) 'operation AddTagsToResource is scaffolded but not implemented in this emulator'. The handler is still the generated scaffold: services/service-com-amazonaws-ssm-amazonssm/src/ops/add_tags_to_resource.rs returns `Err(crate::scaffolded::Scaffolded::not_implemented("AddTagsToResource"))` at the end of `handle` (line 188), and carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker.
Why: a client that tags parameters, documents or patch baselines gets an unmodeled 501-class code for an operation AWS serves, so neither the success path (200, tags readable through ListTagsForResource) nor the modeled 400 refusal class is reachable. Error-handling code that branches on InvalidResourceId / InvalidResourceType never takes its branch, and tag-driven workflows cannot run at all. 501 NotImplemented is not in this operation's modeled error set, so an SDK surfaces it as an unknown/retryable server fault rather than the client error AWS returns.
Fix: implement the AddTagsToResource handler in the SSM service crate — it is the only file for this operation — resolving ResourceId per ResourceType against the crate's state (for `Parameter`, the per-region parameter store the Put path writes), returning 200 with the tags recorded, InvalidResourceId when the resource does not exist, and InvalidResourceType for a type the crate holds no store for. No open issue names SSM AddTagsToResource (searched: none), and the replay report did not mark this failure as already filed.
Traces: iam__ssm__sts.assumed-role-sees-the-same-account-state — 'AddTagsToResource /sim2real-behav-identity/<id>: expected InvalidResourceId HTTP 400, got NotImplemented HTTP 501'.

Cause 2 of 2: DescribeParameters now answers 200 but omits LastModifiedUser from every ParameterMetadata.
AWS: Smithy: DescribeParameters output Parameters (ParameterMetadataList) of ParameterMetadata; ParameterMetadata.LastModifiedUser is documented as "Amazon Resource Name (ARN) of the Amazon Web Services user who last changed the parameter". So after a PutParameter the metadata of that parameter reports the ARN of the identity that performed the write, and it is reported consistently for every parameter written by that identity (the model does not state whether the ARN is the assumed-role session ARN or the underlying role ARN — recorded behavior only for which of the two forms; the requirement here is that the member is PRESENT and carries the writing principal's ARN, not a particular spelling).
Emulator: the member is absent entirely. The replayed DescribeParameters answered HTTP 200 with metadata {'Name': ..., 'ARN': ..., 'Type': 'String', 'LastModifiedDate': ..., 'Version': 3, 'Tier': 'Standard', 'DataType': 'text'} and no 'LastModifiedUser' key, for a parameter the check had just written three versions of under a known STS session. The ParameterMetadata is built by `to_parameter_metadata` in the crate's state, called at services/service-com-amazonaws-ssm-amazonssm/src/ops/describe_parameters.rs:391; it leaves LastModifiedUser unset, and the PutParameter path records no writer identity to populate it from.
Why: a client auditing or reconciling "who last changed this parameter" — a common change-control and drift-detection use — reads None where AWS always returns an ARN, and code that parses that ARN (for account id or role name) raises on a missing field instead. GetParameterHistory's per-version LastModifiedUser is unreachable for the same reason.
Fix: record the calling principal's ARN on each parameter version in the SSM crate's state when PutParameter writes it, and emit it as LastModifiedUser from the ParameterMetadata builder (and the ParameterHistory builder) in that state module. Related but NOT the same defect: open issue 20261024T065715Z-ssm-sts-ssm-describeparameters-is-an-unimplemented-scaffold-stub-so describes DescribeParameters refusing with NotImplemented; the operation now serves 200 (so that refusal is gone) while the LastModifiedUser metadata is still never recorded, and the replay report did not mark this failure as owned by it.
Traces: ssm__sts.parameter-last-modified-user-is-the-live-sts-session-identity — 'DescribeParameters must record who last changed the parameter; LastModifiedUser was None on {...}'.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1 against the emulator built from this checkout (34/169 checks passed; these two failures carried neither an ALREADY FILED nor an OWNED by marker).

Cause 1 — check iam__ssm__sts.assumed-role-sees-the-same-account-state:
  AssertionError: AddTagsToResource /sim2real-behav-identity/bcc927eca4f2: expected InvalidResourceId HTTP 400, got 'NotImplemented' HTTP 501: 'operation AddTagsToResource is scaffolded but not implemented in this emulator'
  Expected: HTTP 400 with error code InvalidResourceId (modeled client error of AddTagsToResource).
  Actual: HTTP 501 with error code NotImplemented (not a modeled error of this operation).
  Code: services/service-com-amazonaws-ssm-amazonssm/src/ops/add_tags_to_resource.rs:4 ('SCAFFOLDED, NOT IMPLEMENTED') and :188 (Scaffolded::not_implemented("AddTagsToResource")).

Cause 2 — check ssm__sts.parameter-last-modified-user-is-the-live-sts-session-identity:
  AssertionError: DescribeParameters must record who last changed the parameter; LastModifiedUser was None on {'Name': '/sim2real-behav-lastwriter-a-e61a04d62183', 'ARN': 'arn:aws:ssm:us-west-2:000000000000:parameter/sim2real-behav-lastwriter-a-e61a04d62183', 'Type': 'String', 'LastModifiedDate': datetime(2026, 10, 25, 5, 17, 48), 'Version': 3, 'Tier': 'Standard', 'DataType': 'text'}
  Expected: the same ParameterMetadata plus LastModifiedUser carrying the ARN of the identity that issued the PutParameter calls (per Smithy doc for ParameterMetadata.LastModifiedUser).
  Actual: HTTP 200, member absent.
  Code: services/service-com-amazonaws-ssm-amazonssm/src/ops/describe_parameters.rs:391 (`version.to_parameter_metadata(record)`).

Smithy consulted: services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json — AddTagsToResource (errors InternalServerError 500, InvalidResourceId 400, InvalidResourceType 400, TooManyTagsError 400, TooManyUpdates 400); ParameterMetadata.LastModifiedUser doc.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator (run_behavioral_suite does both).
2. Re-replay the two checks named in trace_ids:
   - iam__ssm__sts.assumed-role-sees-the-same-account-state
   - ssm__sts.parameter-last-modified-user-is-the-live-sts-session-identity

Manual repro, cause 1 (AddTagsToResource 501):
  ssm.put_parameter(Name='/repro/p', Value='v', Type='String')
  ssm.delete_parameter(Name='/repro/p')   # or simply use a name never created
  ssm.add_tags_to_resource(ResourceType='Parameter', ResourceId='/repro/p', Tags=[{'Key':'k','Value':'v'}])
  -> emulator: HTTP 501, code NotImplemented. AWS: HTTP 400, code InvalidResourceId.
  Also: for an EXISTING parameter the same call must answer HTTP 200 and the tag must be visible via ssm.list_tags_for_resource(ResourceType='Parameter', ResourceId='/repro/p').

Manual repro, cause 2 (LastModifiedUser):
  who = sts.get_caller_identity()['Arn']
  ssm.put_parameter(Name='/repro/writer', Value='v1', Type='String')
  ssm.put_parameter(Name='/repro/writer', Value='v2', Type='String', Overwrite=True)
  md = ssm.describe_parameters(ParameterFilters=[{'Key':'Name','Values':['/repro/writer']}])['Parameters'][0]
  -> emulator: 'LastModifiedUser' not in md. AWS: md['LastModifiedUser'] is the ARN of the identity in `who`.
  Same for ssm.get_parameter_history(Name='/repro/writer'): every version carries LastModifiedUser.
