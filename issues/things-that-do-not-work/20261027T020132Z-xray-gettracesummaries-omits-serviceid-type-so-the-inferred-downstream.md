Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: xray.remote-namespace-marks-the-quartz-root-cause-remote-and-infers-its-service
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray.remote-namespace-marks-the-quartz-root-cause-remote-and-infers-its-service|run#15|AssertionError|assert ("QuartzAPI", "remote") not in p_ids and ("QuartzAPI", "remote") in r_ids, (
Issue-Commit: f6760c7a32941ed55445a6cfba7b22a816e41414

# GetTraceSummaries omits ServiceId.Type, so the inferred downstream service a 'namespace: remote' subsegment creates is not reported as a remote service in TraceSummary.ServiceIds

> _Note: this issue was identified against the code base at commit `f6760c7a32941ed55445a6cfba7b22a816e41414`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTraceSummaries

## Description
One root cause in the xray service, which is bootstrapped and answering (103/133 checks pass overall, 81% of xray checks), so this is a fidelity defect and not a bootstrap gap.

RELATED, NOT COVERING: open issue 20261025T043615Z-xray-3-net-new-behavioral-root-causes-getservicegraph.md lists this check id under its Cause 2 ('GetTraceSummaries never derives ResponseTimeRootCauses'), but that cause is no longer what the check fails on and the replay report does NOT mark this failure OWNED: the check now gets PAST the whole root-cause derivation — the summary is produced, ResponseTimeRootCauses is non-empty, the EntityPath runs <caller service> -> QuartzAPI, the terminal Coverage equals 19/21, and the terminal entity's Remote flag is correctly false for the plain frame and true for the remote twin (all asserted before the failing line). The remaining divergence is one member of a different structure, TraceSummary.ServiceIds[].Type.

Cause 1 of 1: the ServiceIds entries of a trace summary carry no Type, so the inferred remote service cannot be distinguished from the caller's own service.
AWS: Smithy (services/service-com-amazonaws-xray-awsxray/smithy-model/com-amazonaws-xray-awsxray.json) models TraceSummary$ServiceIds -> ServiceId{Name, Names, AccountId, Type} (Type is a String; the model's docs for ServiceId are empty, so the value set is recorded behavior only). Recorded behavior (the check is carved from a real-AWS replay): for a root segment carrying a downstream subsegment with 'namespace': 'remote', X-Ray synthesises an inferred service node for that downstream call and lists it in the summary's ServiceIds as an entry whose Name is the subsegment's name and whose Type is the fixed string 'remote'; the caller's own (non-inferred) service is listed without that Type. For the otherwise identical frame WITHOUT 'namespace', no such remote entry appears at all. Fixed text: the Type value 'remote'. Variable part: the Name, which is the subsegment's own name from the submitted document (the check's generated service names and the 'QuartzAPI' subsegment name are request values, not part of the requirement).
Emulator: the summary is produced and its root causes are derived, but every ServiceIds entry comes back with Type absent/empty. For the remote twin the inferred downstream node IS listed by name, yet without Type ('remote'), so a reader cannot tell it from the caller's own service; for the plain frame only the caller's service is listed, which is correct.
Why: Type='remote' is how a client learns that one of the services in a trace is an external dependency X-Ray inferred rather than a producer that reported segments. An agent or console reading 'the time went to an external dependency' off the summary's ServiceIds sees a list of indistinguishable services, so the one key a producer sets ('namespace': 'remote') is unobservable on this read path even though the emulator already honours it when setting the root-cause entity's Remote flag — the two derivations disagree.
Fix: populate ServiceId.Type when projecting a trace's services into TraceSummary.ServiceIds, from the same inferred/namespace derivation that already produces the Remote flag — services/service-com-amazonaws-xray-awsxray/src/ops/get_trace_summaries.rs (the ServiceIds projection) and the inferred-segment derivation it reads in services/service-com-amazonaws-xray-awsxray/src/state.rs (inferred_segments, documented at state.rs:48-51).
Traces: xray.remote-namespace-marks-the-quartz-root-cause-remote-and-infers-its-service.

## Evidence
xray.remote-namespace-marks-the-quartz-root-cause-remote-and-infers-its-service [mismatch]:

check_xray_remote_namespace_marks_the_quartz_root_cause_remote_and_infers_its_service.py, line 65:
  assert ("QuartzAPI", "remote") not in p_ids and ("QuartzAPI", "remote") in r_ids
  AssertionError: only the remote twin should list a remote QuartzAPI service: plain [('sim2real-flint-motion-855deea944de', '')], remote [('QuartzAPI', ''), ('sim2real-flint-remote-c28f93909b1f', '')]

where each tuple is (ServiceId.Name, ServiceId.Type or ''). Observed: every Type is '' (the member is absent). Expected: the remote twin's ServiceIds contains an entry with Type 'remote' for the inferred downstream service, and the plain frame's does not.

The earlier assertions in the same check all passed, which locates the defect precisely:
- PutTraceSegments accepted both frames (UnprocessedTraceSegments == []);
- GetTraceSummaries returned exactly one summary per trace id WITH a non-empty ResponseTimeRootCauses;
- ResponseTimeRootCauses[0].Services[0].EntityPath == [<caller service>, 'QuartzAPI'] for both frames;
- the terminal entity's Coverage == 19/21 (within 0.001) for both frames;
- (p_last['Remote'], r_last['Remote']) == (False, True), i.e. the 'namespace': 'remote' key IS honoured in the root-cause entity.
Only the ServiceIds[].Type projection diverges. The two frames are identical 21 s traces (a 2 s InternalPipeline subsegment plus a 19 s downstream subsegment) except that the twin's downstream subsegment adds "namespace": "remote".

## Steps to Reproduce
1. git_clone, then run_behavioral_suite (it builds and boots the emulator); use a us-west-2 xray client. Confirm bootstrap_signal is false and xray answers.
2. PutTraceSegments with two segment documents, each a root segment of 21 s under its own unique service name, containing two subsegments: 'InternalPipeline' (2 s) and a downstream subsegment named 'QuartzAPI' (19 s, with http.request/response set). In the SECOND document only, add "namespace": "remote" to the QuartzAPI subsegment. Observe UnprocessedTraceSegments == [].
3. For each frame call xray.get_trace_summaries(StartTime=now-15m, EndTime=now+1m, FilterExpression='service("<that frame's service name>")') and locate the summary for its trace id.
4. Read summary['ResponseTimeRootCauses'][0]['Services'][0]['EntityPath'][-1]['Remote'] — observe False for the plain frame and True for the remote twin (correct today).
5. Read [(i['Name'], i.get('Type')) for i in summary['ServiceIds']] — observe Type absent/empty on every entry, including the inferred 'QuartzAPI' entry of the remote twin. Expected: the remote twin lists ('QuartzAPI', 'remote') and the plain frame lists no remote entry.
6. Re-replay xray.remote-namespace-marks-the-quartz-root-cause-remote-and-infers-its-service to confirm the fix.
