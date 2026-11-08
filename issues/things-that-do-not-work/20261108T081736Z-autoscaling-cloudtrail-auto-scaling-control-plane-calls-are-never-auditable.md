Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudtrail.obs-autoscaling-3-rejected-asg-calls-are-audited-with-their-sdk-error-and-request-parameters
Issue-Key: autoscaling__cloudtrail.obs-autoscaling-3-rejected-asg-calls-are-audited-with-their-sdk-error-and-request-parameters|run#47|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Commit: 2d22b922898c4268bec96b0923119d5f2b2856eb

# Auto Scaling control-plane calls are never auditable through CloudTrail LookupEvents, so an audit poll for their records never completes

> _Note: this issue was identified against the code base at commit `2d22b922898c4268bec96b0923119d5f2b2856eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
ONE net-new root cause: nothing the Auto Scaling control plane does reaches CloudTrail's LookupEvents index, so a caller polling for the audit record of a CreateAutoScalingGroup / UpdateAutoScalingGroup call waits forever.

Cause 1 of 1: CloudTrail LookupEvents never returns a management event for eventSource 'autoscaling.amazonaws.com'.

AWS: successful AND rejected Auto Scaling control-plane calls are recorded by CloudTrail as management events (documented at https://docs.aws.amazon.com/autoscaling/ec2/userguide/logging-using-cloudtrail.html). LookupEvents with LookupAttributes [{AttributeKey: 'EventName', AttributeValue: 'CreateAutoScalingGroup'}] (and likewise 'UpdateAutoScalingGroup') over a window covering the call returns an Events entry whose EventSource is 'autoscaling.amazonaws.com' and whose CloudTrailEvent JSON carries: eventSource 'autoscaling.amazonaws.com', eventName equal to the API action, requestID equal to the RequestId the SDK saw on that very call, readOnly false, and requestParameters equal to the parameters actually sent (camelCased wire names, e.g. autoScalingGroupName, launchTemplate {launchTemplateName, version}, minSize, maxSize, desiredCapacity, availabilityZones, vPCZoneIdentifier). A SUCCESSFUL call carries no errorCode and no errorMessage. A REJECTED call carries errorCode = the exception shape name (AlreadyExistsFault for the duplicate CreateAutoScalingGroup whose wire code is AlreadyExists; ValidationException for the MinSize>MaxSize UpdateAutoScalingGroup whose wire code is ValidationError), errorMessage byte-identical to the message the SDK raised, and no responseElements. All three records are indexed within about a minute of the calls. Recorded behavior only: the CloudTrail Smithy model does not state which sources are indexed or what a record contains.

Emulator: the poll never finds any of the three requestIDs and the check is killed by the harness — 'Check timed out after 180.0s' — with no assertion having fired. Hypothesis for the stall location: the CloudTrail poll loop is the only unbounded wait in the check body (ceiling 1200 s, 2 s interval, exits as soon as all three records are seen), and every assertion ahead of it (the duplicate-create code/message, the MinSize>MaxSize code/message, the unchanged sizing, and a successful single LookupEvents round trip, which the check performs before the loop) would have surfaced as an AssertionError verdict instead of a timeout. So the emulator answers LookupEvents, but never with a record whose eventSource is autoscaling.amazonaws.com and whose requestID matches a call just made.

Why: a client cannot audit Auto Scaling at all against this emulator. Compliance and incident tooling that reads LookupEvents to confirm who changed a group — and in particular to distinguish accepted from REFUSED changes via errorCode/errorMessage — sees an empty index and either reports 'no activity' or blocks indefinitely waiting for an event that is never indexed; here it consumed the whole check budget rather than failing.

Fix: the CloudTrail event state and its LookupEvents read path, plus the recording hook on the Auto Scaling operation handlers — Auto Scaling control-plane invocations (accepted and rejected alike) must be appended as management events carrying eventSource, eventName, the response's requestID, readOnly=false, the request's parameters, and, for rejections only, errorCode (the exception shape name) and errorMessage (the SDK's message) with responseElements absent; LookupEvents must index them by EventName and return them inside the requested time window.

Traces: autoscaling__cloudtrail.obs-autoscaling-3-rejected-asg-calls-are-audited-with-their-sdk-error-and-request-parameters [mismatch].

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (62/414 checks passed; the emulator is up and answering).

autoscaling__cloudtrail.obs-autoscaling-3-rejected-asg-calls-are-audited-with-their-sdk-error-and-request-parameters [mismatch]:
  Error: Check timed out after 180.0s

The check (GROUP error-semantics, SERVICES ['autoscaling','cloudtrail']) does, in order: CreateAutoScalingGroup (MinSize=0, MaxSize=1, DesiredCapacity=0) and keeps its RequestId; a duplicate CreateAutoScalingGroup expecting code 'AlreadyExists' and message 'AutoScalingGroup by this name already exists - A group with the name <name> already exists'; UpdateAutoScalingGroup(MinSize=3, MaxSize=1) expecting code 'ValidationError' and message 'Max bound, 1, must be greater than or equal to min bound, 3'; DescribeAutoScalingGroups to prove the refusals changed nothing; one timed LookupEvents round trip; then

  while time.monotonic() - t0 < 1200.0:   # _INDEX_CEILING_S, 2s interval
      for event_name in ('CreateAutoScalingGroup', 'UpdateAutoScalingGroup'):
          events = _lookup_all(ct, StartTime=..., EndTime=..., MaxResults=50,
                               LookupAttributes=[{'AttributeKey': 'EventName',
                                                  'AttributeValue': event_name}])
          ... match rec['requestParameters']['autoScalingGroupName'] == name and rec['requestID']
      if len(found) == len(wanted): break

Expected (AWS): all three requestIDs found, each with eventSource 'autoscaling.amazonaws.com'; the success record WITHOUT errorCode/errorMessage and with requestParameters == {autoScalingGroupName, launchTemplate{launchTemplateName, version}, minSize, maxSize, desiredCapacity, availabilityZones, vPCZoneIdentifier} as sent; the duplicate record with errorCode 'AlreadyExistsFault' and errorMessage identical to the SDK's; the bounds record with errorCode 'ValidationException', errorMessage identical to the SDK's, requestParameters {autoScalingGroupName, minSize, maxSize}, responseElements absent and readOnly false. Group names, request ids and timestamps in such a run are per-run values, not requirements.
Actual (emulator): no matching record is ever returned; the loop never satisfies its exit condition and the harness kills the check at 180 s. The check's own marker for this outcome is DATAPLANE_UNIMPLEMENTED — 'CloudTrail had not produced the audit record ... Real AWS indexed all three within 60s.'

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create a launch template, then CreateAutoScalingGroup(MinSize=0, MaxSize=1, DesiredCapacity=0) and note the response RequestId.
3. Call CreateAutoScalingGroup again with the same name (different MaxSize) and note the RequestId of the AlreadyExists rejection; then UpdateAutoScalingGroup(MinSize=3, MaxSize=1) and note the RequestId of the ValidationError rejection.
4. Call CloudTrail LookupEvents with LookupAttributes=[{AttributeKey:'EventName', AttributeValue:'CreateAutoScalingGroup'}] over a window from 5 minutes before the calls to 5 minutes after, paging NextToken to exhaustion; repeat for 'UpdateAutoScalingGroup'.
   Expect the three records above, keyed by those RequestIds, with eventSource 'autoscaling.amazonaws.com' and the error fields present on the two rejections only.
   Observe that no record for eventSource 'autoscaling.amazonaws.com' is returned at all, so a poll for them never terminates.
5. Re-replay autoscaling__cloudtrail.obs-autoscaling-3-rejected-asg-calls-are-audited-with-their-sdk-error-and-request-parameters to confirm.
