Issue-Source: trace-sweep-divergence
Issue-Trace: iam-agentcore-traj-team-role-deny-is-read-only
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-agentcore-traj-team-role-deny-is-read-only iam-bap-simulate-with-bucket-policy iam-ebecs-events-role-run-task-pass-role-simulation iam-ecsm-simulate-instance-role-secret-read-chain iam-execute-api-post-scoped-grant-simulate-and-fix iam-execute-api-simulation-request-mistakes
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member[*].EvalDecisionDetails
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member[*].MatchedStatements.member.SourcePolicyId
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member[*].ResourceSpecificResults.member.MatchedStatements.member.EndPosition
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member[*].ResourceSpecificResults.member.MatchedStatements.member.SourcePolicyId
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member[*].ResourceSpecificResults.member.MatchedStatements.member.StartPosition
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member.EvalDecisionDetails
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member.MatchedStatements.member.EndPosition
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member.MatchedStatements.member.StartPosition
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member.ResourceSpecificResults.member.MatchedStatements.member.EndPosition
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member.ResourceSpecificResults.member.MatchedStatements.member.StartPosition
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member[*].EvalDecision
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member[*].EvalResourceName
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member[*].ResourceSpecificResults.member[*].EvalResourceDecision
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member[*].ResourceSpecificResults.member[*].MatchedStatements.member.EndPosition
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member[*].ResourceSpecificResults.member[*].MatchedStatements.member.SourcePolicyId
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member[*].ResourceSpecificResults.member[*].MatchedStatements.member.StartPosition
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member.MissingContextValues.member
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member.ResourceSpecificResults.member.MissingContextValues.member
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member.MatchedStatements.member.SourcePolicyId
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member.ResourceSpecificResults.member.MatchedStatements.member.SourcePolicyId
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member.ResourceSpecificResults.member.MissingContextValues
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member[*].MatchedStatements
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member[*].ResourceSpecificResults.member.EvalResourceDecision
Issue-Related: 20261029T184359Z-iam-modeled-empty-members-are-omitted-from-iam-success.md
Issue-Commit: f9571cbe65aa0d276fa5ca35992cbed3efbd51b6

# IAM SimulatePrincipalPolicy evaluation results omit modeled detail members and misreport the aggregate decision

> _Note: this issue was identified against the code base at commit `f9571cbe65aa0d276fa5ca35992cbed3efbd51b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SimulatePrincipalPolicy

## Description
SimulatePrincipalPolicy returns an EvaluationResult that is missing most of the modeled detail members, mislabels the matched statement, and in one case reports the wrong decision. 9 distinct causes, all in the SimulatePrincipalPolicy result projection.

Cause 1 of 9: EvaluationResult omits OrganizationsDecisionDetail, EvalDecisionDetails and MissingContextValues, and MatchedStatements entries omit StartPosition/EndPosition and carry an unqualified SourcePolicyId.
AWS: Smithy shape EvaluationResult declares OrganizationsDecisionDetail (target OrganizationsDecisionDetail), EvalDecisionDetails (map), MissingContextValues (ContextKeyNamesResultListType) and MatchedStatements (StatementListType); Smithy shape Statement declares SourcePolicyId, SourcePolicyType, StartPosition and EndPosition (each a Position of Line+Column). The recording shows OrganizationsDecisionDetail present with AllowedByOrganizations=true, EvalDecisionDetails and MissingContextValues present as empty members, Statement.StartPosition/EndPosition populated with the character span of the statement inside the policy document, and SourcePolicyId for an inline role policy qualified as role_<roleName>_<policyName>.
Emulator: all of OrganizationsDecisionDetail, EvalDecisionDetails, MissingContextValues, StartPosition and EndPosition are absent from the response, and SourcePolicyId is the bare inline policy name.
Why: a client that reads AllowedByOrganizations, enumerates MissingContextValues, or locates the deciding statement by Line/Column gets None and cannot tell an allow-by-no-SCP from an unevaluated organization; a client that disambiguates two identically named inline policies by the role_<role>_<policy> qualifier sees a name that does not identify its source.
Fix: the SimulatePrincipalPolicy response serializer/projection of EvaluationResult and Statement.
Traces: iam-agentcore-traj-team-role-deny-is-read-only#task#9.

Cause 2 of 9: MissingContextValues is populated with a context key the simulation did not actually need.
AWS: recorded behavior only — for a simulation whose resource policy condition is satisfied, MissingContextValues is returned as an empty member.
Emulator: returns MissingContextValues containing aws:SecureTransport, and also omits OrganizationsDecisionDetail/EvalDecisionDetails and the statement positions.
Why: a client treats a non-empty MissingContextValues as "the simulation is incomplete, supply more context" and re-runs or reports an indeterminate verdict where AWS gave a complete one.
Fix: the context-key collection step of policy simulation.
Traces: iam-bap-simulate-with-bucket-policy#task#3.

Cause 3 of 9: the same spurious aws:SecureTransport entry plus the missing OrganizationsDecisionDetail/EvalDecisionDetails members across a further 8 recorded simulate steps.
AWS: as Cause 1/2 — empty MissingContextValues and present OrganizationsDecisionDetail.
Emulator: MissingContextValues = {member: aws:SecureTransport}, OrganizationsDecisionDetail and EvalDecisionDetails absent.
Why: same as Cause 2, on every simulate call that carries a resource policy with a condition.
Fix: same component.
Traces: iam-bap-simulate-with-bucket-policy#task#4.

Cause 4 of 9: EvalResourceName returns the concrete request ARN instead of AWS's ARN template for the action's resource type.
AWS: Smithy EvaluationResult.EvalResourceName is documented as "The ARN template for the simulated resource type (for example, arn:${Partition}:s3:::${BucketName}/${KeyName}), or * if no ARN format is defined for the action. This is not a specific customer-provided resource ARN". The recording shows arn:aws:ecs:${Region}:${Account}:${ResourceType}/${ResourcePath} — fixed text is the template form with ${...} placeholders; only the service segment varies with the simulated action.
Emulator: echoes the caller's ResourceArns entry verbatim as EvalResourceName (a concrete account/region/resource ARN).
Why: a client that keys per-resource decisions off ResourceSpecificResults and uses the top-level EvalResourceName as the action's resource-type template reads a specific resource where AWS gives a template, so it misattributes the aggregate decision to one resource.
Fix: the SimulatePrincipalPolicy result projection (EvalResourceName derivation).
Traces: iam-ebecs-events-role-run-task-pass-role-simulation#task#4.

Cause 5 of 9: MissingContextValues reports only one of the context keys the policy requires.
AWS: the recording shows the deduplicated set of all keys referenced by the evaluated policies but not supplied — here both ecs:cluster and iam:PassedToService, in the top-level result and mirrored in ResourceSpecificResults.
Emulator: returns only iam:PassedToService, dropping the condition key contributed by the resource-scoped statement.
Why: a caller using the simulator to discover which ContextEntries it must supply gets an incomplete list, supplies them, and still receives an indeterminate result.
Fix: the context-key collection step of policy simulation (union across all evaluated statements).
Traces: iam-ebecs-events-role-run-task-pass-role-simulation#task#9.

Cause 6 of 9: unqualified SourcePolicyId and missing statement positions on a further 7 recorded simulate steps.
AWS: SourcePolicyId for an inline role policy is role_<roleName>_<policyName>; StartPosition/EndPosition carry Line and Column.
Emulator: SourcePolicyId is the bare inline policy name and both positions are absent.
Why: as Cause 1.
Fix: same component.
Traces: iam-ebecs-events-role-run-task-pass-role-simulation#task#10.

Cause 7 of 9: MissingContextValues absent entirely where AWS reports one missing key.
AWS: the recording shows MissingContextValues = {member: ecs:cluster} in the top-level result and in ResourceSpecificResults.
Emulator: the member is not serialized at all.
Why: a client cannot distinguish "no context missing" from "context missing" and treats a conditional allow as unconditional.
Fix: same component.
Traces: iam-ebecs-events-role-run-task-pass-role-simulation#task#11.

Cause 8 of 9: per-action results in a multi-action simulation lose their detail members and statement qualification.
AWS: each EvaluationResults member carries OrganizationsDecisionDetail, empty MissingContextValues and positioned MatchedStatements whose SourcePolicyId is role_<roleName>_<policyName>.
Emulator: those members are absent and SourcePolicyId is the bare inline policy name on both the first and second evaluation result.
Why: as Cause 1, and the defect is per-member so a client cannot work around it by reading only the first result.
Fix: same component.
Traces: iam-ecsm-simulate-instance-role-secret-read-chain#task#7.

Cause 9 of 9: the simulator reports allowed where AWS reports implicitDeny, and attributes a matched statement to a decision that matched nothing.
AWS: Smithy EvaluationResult.EvalDecision is required and documented as the aggregate decision; the recording shows EvalDecision=implicitDeny with MatchedStatements empty, and the matching ResourceSpecificResults entry EvalResourceDecision=implicitDeny.
Emulator: returns EvalDecision=allowed and EvalResourceDecision=allowed, with MatchedStatements naming SourcePolicyId=secrets-read / SourcePolicyType=IAM Policy — i.e. a statement whose action or resource does not in fact match the simulated request is treated as a match.
Why: this inverts the authorization verdict. A client using the simulator as a pre-flight permission check is told the call is permitted when AWS says no policy allows it, so it proceeds and the real call fails.
Fix: the policy-evaluation matcher behind SimulatePrincipalPolicy (action/resource matching before a statement is recorded as matched).
Traces: iam-ecsm-simulate-instance-role-secret-read-chain#task#8.

The open issue 20261102T091623Z-iam-simulateprincipalpolicy-mislabels-inline-policy-sources-and-wildcard.md describes a related SourcePolicyId labelling fault but does not carry the replay keys for any of the 9 causes above.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — iam-agentcore-traj-team-role-deny-is-read-only#task#9, SimulatePrincipalPolicy, 200 vs 200: $...EvaluationResults.member[0].OrganizationsDecisionDetail expected {'AllowedByOrganizations': 'true'}, got None; .EvalDecisionDetails expected '', got None; .MatchedStatements.member.StartPosition expected {'Line': '1', 'Column': '212'}, got None; .EndPosition expected {'Line': '1', 'Column': '479'}, got None; .MatchedStatements.member.SourcePolicyId expected 'role_sim2real-probe-agmcp-quartz-ed5cafbc_CloudWatchLogsReadPolicy', got 'CloudWatchLogsReadPolicy'; .MissingContextValues expected '', got None (same deltas repeated under ResourceSpecificResults and for member[1]).

Cause 2 — iam-bap-simulate-with-bucket-policy#task#3: .MissingContextValues expected '', got {'member': 'aws:SecureTransport'}; OrganizationsDecisionDetail/EvalDecisionDetails/StartPosition/EndPosition all expected present, got None.

Cause 3 — iam-bap-simulate-with-bucket-policy#task#4 (cluster of 8 steps, also iam-bap-simulate-with-bucket-policy#task#5/#task#6, iam-ebecs-events-role-run-task-pass-role-simulation#task#2, iam-ecsm-simulate-instance-role-secret-read-chain#task#9, iam-execute-api-post-scoped-grant-simulate-and-fix#task#6/#task#11, iam-execute-api-simulation-request-mistakes#task#6): OrganizationsDecisionDetail expected {'AllowedByOrganizations': 'true'}, got None; EvalDecisionDetails expected '', got None; MissingContextValues expected '', got {'member': 'aws:SecureTransport'}.

Cause 4 — iam-ebecs-events-role-run-task-pass-role-simulation#task#4: .EvalResourceName expected 'arn:aws:ecs:${Region}:${Account}:${ResourceType}/${ResourcePath}', got 'arn:aws:ecs:us-west-2:000000000000:task-definition/sim2real-probe-ebecs-sim-e5d192fa-td:1'; SourcePolicyId expected 'role_sim2real-probe-ebecs-ev-e5d192fa_events-run-task', got 'events-run-task'.

Cause 5 — iam-ebecs-events-role-run-task-pass-role-simulation#task#9: .MissingContextValues.member expected ['ecs:cluster', 'iam:PassedToService'], got 'iam:PassedToService' (same under ResourceSpecificResults).

Cause 6 — iam-ebecs-events-role-run-task-pass-role-simulation#task#10 (cluster of 7, also #task#12, iam-ecsm-simulate-instance-role-secret-read-chain#task#11, iam-execute-api-post-scoped-grant-simulate-and-fix#task#7/#task#10, iam-execute-api-simulation-request-mistakes#task#7/#task#8): SourcePolicyId expected 'role_sim2real-probe-ebecs-ev-e5d192fa_events-run-task', got 'events-run-task'; StartPosition expected {'Line': '1', 'Column': '323'}, EndPosition expected {'Line': '1', 'Column': '543'}, got None.

Cause 7 — iam-ebecs-events-role-run-task-pass-role-simulation#task#11 (also #task#13): MissingContextValues expected {'member': 'ecs:cluster'}, got None, top level and under ResourceSpecificResults.

Cause 8 — iam-ecsm-simulate-instance-role-secret-read-chain#task#7 (also #task#12): member[0].SourcePolicyId expected 'role_sim2real-probe-ecsm-sim-97b77475_secrets-read', got 'secrets-read'; positions and OrganizationsDecisionDetail/MissingContextValues missing on member[0] and member[1].

Cause 9 — iam-ecsm-simulate-instance-role-secret-read-chain#task#8: member[0].EvalDecision expected 'implicitDeny', got 'allowed'; member[0].ResourceSpecificResults.member.EvalResourceDecision expected 'implicitDeny', got 'allowed'; member[0].MatchedStatements expected '', got {'member': {'SourcePolicyId': 'secrets-read', 'SourcePolicyType': 'IAM Policy'}}.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay fixture iam-agentcore-traj-team-role-deny-is-read-only and compare step task#9 (SimulatePrincipalPolicy) against the recording.
3. Replay the steps named in trace_ids: iam-bap-simulate-with-bucket-policy#task#3 and #task#4, iam-ebecs-events-role-run-task-pass-role-simulation#task#4/#task#9/#task#10/#task#11, iam-ecsm-simulate-instance-role-secret-read-chain#task#7/#task#8.
4. For each, diff the SimulatePrincipalPolicyResponse EvaluationResults member tree: OrganizationsDecisionDetail, EvalDecisionDetails, MissingContextValues, MatchedStatements[].SourcePolicyId/StartPosition/EndPosition, EvalResourceName, EvalDecision and the mirrored ResourceSpecificResults entries.
