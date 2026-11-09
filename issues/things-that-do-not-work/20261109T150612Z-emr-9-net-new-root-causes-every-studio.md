Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.delete-studio-session-mapping-then-get-fails emr.op-session-mapping-unknown-studio-fails emr.studio-session-mapping-lifecycle-crud emr.update-studio-session-mapping-nonexistent-identity-rejected emr.studio-session-mapping-duplicate-create-rejected emr.studio-session-mapping-missing-identity-rejected emr.update-studio-session-mapping-conflicting-identity-type-rejected emr.studio-session-mapping-invalid-identity-rejected emr.studio-session-mapping-invalid-session-policy-arn-rejected emr.op-auto-termination-policy-ops-reject-unknown-cluster emr.auto-termination-policy-really-terminates-the-idle-cluster emr.spark-bigint-aggregate-beyond-float64-precision-is-exact
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.delete-studio-session-mapping-then-get-fails|run#15|AssertionError|assert message == expected, f"expected the message {expected!r}, got {message!r}"
Issue-Key: emr.op-session-mapping-unknown-studio-fails|run#47|AssertionError|assert message == expected, f"expected the message {expected!r}, got {message!r}"
Issue-Key: emr.studio-session-mapping-lifecycle-crud|run#28|AssertionError|assert delete_message == expected, f"expected the message {expected!r}, got {delete_message!r}"
Issue-Key: emr.update-studio-session-mapping-nonexistent-identity-rejected|run#11|AssertionError|assert update_message == expected, f"expected the message {expected!r}, got {update_message!r}"
Issue-Key: emr.studio-session-mapping-duplicate-create-rejected|run#11|AssertionError|assert message == expected, f"expected the message {expected!r}, got {message!r}"
Issue-Key: emr.studio-session-mapping-missing-identity-rejected|run#18|AssertionError|assert message == expected, f"expected the message {expected!r}, got {message!r}"
Issue-Key: emr.update-studio-session-mapping-conflicting-identity-type-rejected|run#13|AssertionError|assert message == expected, f"expected the message {expected!r}, got {message!r}"
Issue-Key: emr.studio-session-mapping-invalid-identity-rejected|_error#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: emr.studio-session-mapping-invalid-session-policy-arn-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: emr.op-auto-termination-policy-ops-reject-unknown-cluster|_assert_rejections#11|AssertionError|assert not wrong, f"expected {expected!r} for every op, got mismatches: {wrong}"
Issue-Key: emr.auto-termination-policy-really-terminates-the-idle-cluster|_poll#13|Timeout|time.sleep(interval_s)
Issue-Key: emr.spark-bigint-aggregate-beyond-float64-precision-is-exact|run#15|AssertionError|assert step["Status"]["State"] == "COMPLETED", (
Issue-Commit: 6bab9504f4457a639e729a9dc2b08c46f536304f

# EMR: 9 net-new root causes — every Studio session-mapping refusal is emulator prose without AWS's sentence or the AWSEditors envelope, CreateStudioSessionMapping validates neither the identity nor the policy ARN, auto-termination never fires, and a step settles FAILED without running

> _Note: this issue was identified against the code base at commit `6bab9504f4457a639e729a9dc2b08c46f536304f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStudioSessionMapping

## Description
Nine distinct net-new root causes clustered from this run's failures listed under the `emr` service heading. Every cause is numbered and carries its own failing check ids; a fixer must close ALL NINE before submitting.

For every message below, the fixed text is the requirement and the variable parts are named. The EMR Studio plane (the AWSEditors backend) words every 400 InvalidRequestException as `<AWS sentence> (Service: AWSEditors; Status Code: 400; Error Code: InvalidRequestException; Request ID: <request id>; Proxy: null)`. The per-call request id is NOT part of the requirement — the checks lift the observed request id into the expected string before comparing. The studio ids, GUIDs and names in the recordings are per-run values that only fill the template.

=== Cause 1 of 9: the absent-session-mapping refusal (Get/Update/Delete) carries emulator prose instead of AWS's sentence and the AWSEditors envelope ===
AWS: Smithy (com.amazonaws.emr#ElasticMapReduce): GetStudioSessionMapping, UpdateStudioSessionMapping and DeleteStudioSessionMapping each model exactly InvalidRequestException (client, HTTP 400) and InternalServerError (500) — there is no not-found error shape, so an absent mapping is reported as InvalidRequestException. Recorded behavior for the text: the Message is the fixed sentence `Studio session mapping does not exist.` followed by the AWSEditors envelope above. Fixed text: the sentence and every label of the envelope; the only variable part is the request id.
Emulator: status 400 and code InvalidRequestException are correct, but the Message is the emulator-authored `Studio '<studio-id>' has no session mapping for <IdentityType> '<identity>'.` — AWS's sentence is absent, the whole envelope is absent, and the studio id/identity AWS does not echo are leaked.
Why: every client, CLI user and runbook that classifies this refusal by message (the only way to tell it from any other InvalidRequestException on these operations, since no not-found shape is modelled) fails to recognise it, and the caller cannot recover the backend service name or request id AWS puts in the message for support.
Fix: the EMR crate's shared Studio session-mapping refusal construction (the not-found path of get_studio_session_mapping.rs / update_studio_session_mapping.rs / delete_studio_session_mapping.rs) — emit AWS's sentence plus the AWSEditors envelope.
Traces: emr.delete-studio-session-mapping-then-get-fails, emr.op-session-mapping-unknown-studio-fails, emr.studio-session-mapping-lifecycle-crud, emr.update-studio-session-mapping-nonexistent-identity-rejected.

=== Cause 2 of 9: the duplicate-create refusal is emulator prose instead of AWS's 'already exists' sentence and envelope ===
AWS: Smithy CreateStudioSessionMapping models InvalidRequestException (client, 400) and InternalServerError. Recorded behavior for the text: creating a second mapping for the same (StudioId, IdentityType, identity) is refused 400 InvalidRequestException with `Studio session mapping already exists.` plus the AWSEditors envelope. Fixed text: that sentence and the envelope labels; variable part: the request id only — AWS echoes neither the studio id nor the identity and gives no remediation clause.
Emulator: `Studio '<studio-id>' is already mapped to <IdentityType> '<identity>'. Use UpdateStudioSessionMapping to change its session policy.` — a different sentence that echoes the studio id and identity and appends advice AWS does not emit.
Why: a client that treats 'already exists' as benign (idempotent re-apply, as IaC and provisioning tools do) cannot match the sentence, so a harmless duplicate is handled as an unknown failure.
Fix: the duplicate branch of create_studio_session_mapping.rs in the EMR crate (shared Editors-plane refusal helper).
Traces: emr.studio-session-mapping-duplicate-create-rejected.

=== Cause 3 of 9: the 'exactly one of IdentityName/IdentityId' refusal uses a shortened sentence and no envelope ===
AWS: Smithy CreateStudioSessionMapping takes StudioId, IdentityType and SessionPolicyArn as required and IdentityId / IdentityName as optional; exactly one of the two identity members must be supplied. Recorded behavior for the text: a request supplying neither (or both) is refused 400 InvalidRequestException with `Either IdentityName or IdentityId must be specified and only one of them must be specified.` plus the AWSEditors envelope. Fixed text: that whole sentence; variable part: the request id only.
Emulator: `Either IdentityName or IdentityId must be specified, but not both` — a reworded, shortened sentence with no terminating period and no envelope.
Why: the refusal is unrecognisable to a client matching AWS's sentence, and the truncated wording loses the 'only one of them' requirement that tells the caller which of the two input errors it made.
Fix: the identity-member validation in create_studio_session_mapping.rs (and the equivalent validation on update/get/delete) in the EMR crate.
Traces: emr.studio-session-mapping-missing-identity-rejected.

=== Cause 4 of 9: an IdentityType that conflicts with the mapping's real type is refused as 'no session mapping' instead of AWS's 'IdentityId is invalid.' ===
AWS: a session mapping's key is (StudioId, IdentityType, identity). Recorded behavior: UpdateStudioSessionMapping (and GetStudioSessionMapping) naming an IdentityId that exists in the Studio's Identity Center as a GROUP but passing IdentityType='USER' is refused 400 InvalidRequestException with `IdentityId is invalid.` plus the AWSEditors envelope — AWS classifies the mismatch as a bad identity id, not as a missing mapping. Fixed text: that sentence and the envelope; variable part: the request id. The correctly-typed mapping must be left untouched.
Emulator: `Studio '<studio-id>' has no session mapping for USER '<group guid>'.` — the wrong AWS sentence (a not-found statement rather than the invalid-identity statement) and no envelope.
Why: a caller cannot distinguish 'you used the wrong IdentityType for this GUID' from 'this mapping was deleted', so it retries or re-creates instead of correcting the type.
Fix: the identity resolution in update_studio_session_mapping.rs / get_studio_session_mapping.rs — resolve the id against the Identity Store, detect the type mismatch and refuse with AWS's `IdentityId is invalid.` sentence.
Traces: emr.update-studio-session-mapping-conflicting-identity-type-rejected.

=== Cause 5 of 9: CreateStudioSessionMapping accepts an identity that resolves to nobody in IAM Identity Center ===
AWS: Smithy CreateStudioSessionMapping models only InvalidRequestException (client, 400) and InternalServerError; EMR resolves the supplied IdentityId/IdentityName in the Studio's Identity Center instance. Recorded behavior: a random GUID that resolves to no user is refused 400 InvalidRequestException with `IdentityId is invalid.` plus the AWSEditors envelope, and no mapping is left behind (ListStudioSessionMappings must not report it).
Emulator: the call SUCCEEDS — HTTP 200 with an empty body — and a mapping for the nonexistent identity is stored.
Why: a client gets a Studio mapping that grants nothing and can never be used, with no signal the identity was wrong; the negative control of identity validation can never fail.
Fix: validation in the EMR crate's create_studio_session_mapping.rs — look the identity up in the Studio's Identity Store before storing anything.
Traces: emr.studio-session-mapping-invalid-identity-rejected.

=== Cause 6 of 9: CreateStudioSessionMapping accepts a SessionPolicyArn that is not an ARN at all ===
AWS: Smithy CreateStudioSessionMapping.SessionPolicyArn is required (XmlStringMaxLen256) and must be an IAM policy ARN. Recorded behavior: SessionPolicyArn='not-an-arn' is refused 400 InvalidRequestException with `SessionPolicyArn is malformed.` plus the AWSEditors envelope, and creates nothing — a following GetStudioSessionMapping for that identity is refused (Cause 1's sentence).
Emulator: HTTP 200 with an empty JSON body ({}), and the mapping is stored carrying the malformed policy ARN.
Why: a typo in the session policy ARN silently produces a mapping whose policy cannot exist, so the Studio user's permissions are not what the client asked for and the mistake surfaces much later, if ever.
Fix: SessionPolicyArn validation (ARN form, then resolvability) in create_studio_session_mapping.rs / update_studio_session_mapping.rs in the EMR crate.
Traces: emr.studio-session-mapping-invalid-session-policy-arn-rejected.

=== Cause 7 of 9: the auto-termination-policy writes refuse an unknown cluster with the job-flow message instead of EMR's cluster-id sentence ===
AWS: Smithy PutAutoTerminationPolicy and RemoveAutoTerminationPolicy take ClusterId (required, ClusterId) and model no operation-specific error, so the refusal is the service's cluster-id ValidationException. Recorded behavior for the text: an unknown ClusterId is refused with code ValidationException and message `Cluster id '<ClusterId>' is not valid.` — fixed text `Cluster id '` / `' is not valid.`; the variable part is the ClusterId taken from the request.
Emulator: the code ValidationException is right, but both operations answer the constant `Specified job flow ID not valid.` — the message template of the legacy job-flow operations, which neither names the member nor echoes the id.
Why: a client that matches EMR's cluster-id sentence (or parses the offending id out of it) does not recognise the refusal, and the same refusal becomes indistinguishable from a job-flow-style rejection raised by a different operation family.
Fix: the unknown-cluster branch of put_auto_termination_policy.rs / remove_auto_termination_policy.rs in the EMR crate — use the cluster-id refusal helper, not the job-flow one.
Related open issues, neither of which carries this check's replay key: 20261108T042656Z-emr-auto-termination-policy-writes-skip-the-idletimeout.md and 20261108T090729Z-elasticmapreduce-emr-auto-termination-policy-operations-refuse-unknown-clusterid.md.
Traces: emr.op-auto-termination-policy-ops-reject-unknown-cluster.

=== Cause 8 of 9: an AutoTerminationPolicy is never a timer — an idle cluster carrying IdleTimeout is never terminated ===
AWS: Smithy AutoTerminationPolicy.IdleTimeout documents the rule: 'Specifies the amount of idle time in seconds after which the cluster automatically terminates. You can specify a minimum of 60 seconds and a maximum of 604800 seconds (seven days).' Recorded behavior: a step-less cluster launched with AutoTerminationPolicy.IdleTimeout is still alive well before the timeout elapses, then shuts itself down with NOBODY calling TerminateJobFlows — DescribeCluster reaches state TERMINATED (an orderly shutdown, not TERMINATED_WITH_ERRORS and not a failure StateChangeReason code), and the cluster's own EndDateTime - ReadyDateTime is at least the IdleTimeout.
Emulator: the policy is stored and read back correctly, but it never fires: the cluster stays in its running state and no terminal state is ever observed — the check is cut off by the harness at its 900 s budget with the minimum 60 s IdleTimeout configured.
Why: a client that relies on auto-termination as its cost guardrail keeps a cluster running forever and is billed for it; any workflow that waits for the policy's shutdown (and every assertion on the idle span) never completes.
Fix: the EMR crate's cluster-lifecycle state machine — arm a real idle timer from the stored AutoTerminationPolicy (set at RunJobFlow or by PutAutoTerminationPolicy, cleared by RemoveAutoTerminationPolicy) and drive the cluster to TERMINATED, with EndDateTime at least IdleTimeout after ReadyDateTime, when it has been idle that long.
Traces: emr.auto-termination-policy-really-terminates-the-idle-cluster.

=== Cause 9 of 9: a job-flow step settles FAILED without running its application, so a submitted Spark job's result is never produced ===
AWS: recorded behavior only (the step shapes carry no execution semantics). A step added to a WAITING cluster is picked up, its application really runs, and ListSteps/DescribeStep report Status.State == COMPLETED with the step's own side effects observable (here: an exact bigint aggregate written to S3). A FAILED step carries a StateChangeReason/FailureDetails describing the real failure.
Emulator: the step reaches terminal FAILED with the emulator-authored reason 'This emulator runs the step lifecycle but executes no Hadoop jar, so the step ended without running its application.' and FailureDetails.Message 'No application was started for this step: this emulator holds the cluster's step state and runs no Hadoop, Spark or MapReduce runtime, so the step's own side effects did not happen.' — so COMPLETED is unreachable and no step effect ever happens.
Why: every client that submits a step and waits for COMPLETED is told its job failed, with a reason that describes the emulator rather than the job; any assertion about the step's output (exact numeric results, written S3 objects) can never hold.
Fix: the EMR crate's step-execution path (the AddJobFlowSteps / step-state machine and the runtime it should drive) so a well-formed step runs and settles COMPLETED with its side effects.
Related open issue, which does NOT carry this check's replay key: 20261108T151103Z-emr-4-net-new-root-causes-every-job.md (its Cause 'every job-flow step settles FAILED without running its application'); 20261104T102001Z-emr-8-net-new-root-causes-createvpcendpoint-refuses.md lists this check id under a cause whose symptom was a timeout, not this terminal-FAILED reason, and neither carries the current replay key.
Traces: emr.spark-bigint-aggregate-beyond-float64-precision-is-exact.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout (service heading `emr`: 102/219 passing). In every message comparison below the check substitutes the OBSERVED request id into the expected string before comparing, so the request id is not part of the divergence; ids, GUIDs and names are per-run values.

Cause 1 — emr.delete-studio-session-mapping-then-get-fails [mismatch]: after DeleteStudioSessionMapping, GetStudioSessionMapping expected message "Studio session mapping does not exist. (Service: AWSEditors; Status Code: 400; Error Code: InvalidRequestException; Request ID: <id>; Proxy: null)"; got "Studio 'es-0000000000001' has no session mapping for USER 'sim2real-emr-studio-user-<suffix>'.". Identical divergence in emr.op-session-mapping-unknown-studio-fails, emr.studio-session-mapping-lifecycle-crud (on the post-delete read) and emr.update-studio-session-mapping-nonexistent-identity-rejected (on UpdateStudioSessionMapping).

Cause 2 — emr.studio-session-mapping-duplicate-create-rejected [mismatch]: expected "Studio session mapping already exists. (Service: AWSEditors; Status Code: 400; Error Code: InvalidRequestException; Request ID: <id>; Proxy: null)"; got "Studio 'es-0000000000001' is already mapped to GROUP '<guid>'. Use UpdateStudioSessionMapping to change its session policy.".

Cause 3 — emr.studio-session-mapping-missing-identity-rejected [mismatch]: expected 'Either IdentityName or IdentityId must be specified and only one of them must be specified. (Service: AWSEditors; Status Code: 400; Error Code: InvalidRequestException; Request ID: <id>; Proxy: null)'; got 'Either IdentityName or IdentityId must be specified, but not both'.

Cause 4 — emr.update-studio-session-mapping-conflicting-identity-type-rejected [mismatch]: a mapping created as IdentityType=GROUP + IdentityId=<group guid>, then UpdateStudioSessionMapping(IdentityType='USER', IdentityId=<same guid>): expected "IdentityId is invalid. (Service: AWSEditors; ... Proxy: null)"; got "Studio 'es-0000000000001' has no session mapping for USER '<guid>'." (code InvalidRequestException is correct). The correctly-typed update in the same check is the positive control and succeeds.

Cause 5 — emr.studio-session-mapping-invalid-identity-rejected [mismatch]: CreateStudioSessionMapping(StudioId, IdentityType='USER', IdentityId=<random GUID>, SessionPolicyArn=<valid>) expected 400 InvalidRequestException; got success — AssertionError: expected the call to be rejected, but it succeeded and returned None.

Cause 6 — emr.studio-session-mapping-invalid-session-policy-arn-rejected [mismatch]: CreateStudioSessionMapping(..., SessionPolicyArn='not-an-arn') expected 400 InvalidRequestException 'SessionPolicyArn is malformed. (Service: AWSEditors; ...)'; got HTTP 200 with an empty body ({'ResponseMetadata': {'HTTPStatusCode': 200, ... 'content-length': '2'}}).

Cause 7 — emr.op-auto-termination-policy-ops-reject-unknown-cluster [mismatch]: expected ('ValidationException', "Cluster id 'j-ZZZZZZZZZZZZZ' is not valid.") for every op; got {'PutAutoTerminationPolicy': ('ValidationException', 'Specified job flow ID not valid.'), 'RemoveAutoTerminationPolicy': ('ValidationException', 'Specified job flow ID not valid.')}.

Cause 8 — emr.auto-termination-policy-really-terminates-the-idle-cluster [mismatch]: RunJobFlow with AutoTerminationPolicy={'IdleTimeout': 60} (the documented minimum); GetAutoTerminationPolicy reports the policy, then the poll for a terminal cluster state never satisfies — Error: Check timed out after 900.0s. Expected: DescribeCluster reaches TERMINATED on its own, with EndDateTime - ReadyDateTime >= 60 s.

Cause 9 — emr.spark-bigint-aggregate-beyond-float64-precision-is-exact [mismatch]: AssertionError: the bigint-sum step must COMPLETE; got 'FAILED' (reason {'Message': 'This emulator runs the step lifecycle but executes no Hadoop jar, so the step ended without running its application.'}, details {'Reason': ..., 'Message': "No application was started for this step: this emulator holds the cluster's step state and runs no Hadoop, Spark or MapReduce runtime, so the step's own side effects did not happen."}).

## Steps to Reproduce
Boot the emulator and replay the check ids in trace_ids from suite simparity-behavioral-aws-v1 (service heading `emr`).
1. Causes 1-6: adopt/create an IAM Identity Center instance and a real user/group in it, then CreateStudio(AuthMode='SSO', IdcInstanceArn=<idc arn>). Then:
   (1) CreateStudioSessionMapping for a real identity, DeleteStudioSessionMapping, then GetStudioSessionMapping and UpdateStudioSessionMapping for the same identity — assert code InvalidRequestException and message 'Studio session mapping does not exist.' + the '(Service: AWSEditors; Status Code: 400; Error Code: InvalidRequestException; Request ID: <id>; Proxy: null)' envelope.
   (2) CreateStudioSessionMapping twice for the same (StudioId, IdentityType, identity) — assert 'Studio session mapping already exists.' + envelope.
   (3) CreateStudioSessionMapping supplying BOTH IdentityName and IdentityId (or neither) — assert 'Either IdentityName or IdentityId must be specified and only one of them must be specified.' + envelope.
   (4) Create a mapping as IdentityType='GROUP' with the group's GUID, then UpdateStudioSessionMapping/GetStudioSessionMapping with IdentityType='USER' and the same GUID — assert 'IdentityId is invalid.' + envelope, and that the GROUP mapping is untouched.
   (5) CreateStudioSessionMapping with IdentityId=<random GUID> — assert 400 InvalidRequestException and that ListStudioSessionMappings never reports it.
   (6) CreateStudioSessionMapping with SessionPolicyArn='not-an-arn' — assert 400 InvalidRequestException 'SessionPolicyArn is malformed.' + envelope and that no mapping was created.
2. Cause 7: PutAutoTerminationPolicy and RemoveAutoTerminationPolicy with ClusterId='j-ZZZZZZZZZZZZZ' — assert ValidationException with "Cluster id 'j-ZZZZZZZZZZZZZ' is not valid.".
3. Cause 8: RunJobFlow a step-less KeepJobFlowAliveWhenNoSteps cluster with AutoTerminationPolicy={'IdleTimeout': 60}; confirm GetAutoTerminationPolicy returns IdleTimeout=60; poll DescribeCluster — expect it to stay alive for at least half the timeout and then reach TERMINATED by itself, with EndDateTime - ReadyDateTime >= 60 s; observe that no terminal state is ever reached.
4. Cause 9: RunJobFlow a cluster, wait for WAITING, AddJobFlowSteps with a spark-submit step that sums a bigint sequence to S3, poll ListSteps — expect COMPLETED and the exact sum in the output; observe FAILED with the 'executes no Hadoop jar' reason.
