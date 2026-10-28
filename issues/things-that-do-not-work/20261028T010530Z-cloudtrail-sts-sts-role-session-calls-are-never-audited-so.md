Issue-Severity: HIGH
Issue-Commit: 2882fa01447cddf34bfea2947358b39ec0b8885e

# STS role-session calls are never audited, so AssumeRole's sourceIdentity never appears in requestParameters and no session call carries userIdentity.sessionContext.sourceIdentity

> _Note: this issue was identified against the code base at commit `2882fa01447cddf34bfea2947358b39ec0b8885e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRole

## Description
ONE root cause in this issue.

Cause 1 of 1: neither sts AssumeRole nor a subsequent call made with the assumed session's credentials produces an event-history record, so the source-identity propagation contract is unobservable.
AWS: AssumeRole accepts SourceIdentity (sts model: SourceIdentity, pattern of alphanumerics plus +=,.@-, and its own member doc states 'The source identity value persists across chained role sessions'), and CloudTrail records it. Documented: IAM User Guide, 'Monitor and control actions taken with assumed roles' -> 'Viewing source identity in CloudTrail' - 'You can find the sourceIdentity information in the requestParameters key in your CloudTrail log', alongside roleArn and roleSessionName; CloudTrail User Guide, 'CloudTrail userIdentity element' -> 'AWS STS source identity' - for the service API calls the session then makes, 'sourceIdentity is found in the sessionContext block', with userIdentity.type 'AssumedRole' and sessionContext.sessionIssuer.arn equal to the ROLE arn. A session assumed WITHOUT SourceIdentity records none - that twin control is what separates propagation from always stamping a value. Recorded behavior for latency: regional management-event indexing measured 80-136 s elsewhere in this corpus.
Emulator: LookupEvents returns no record at all for the AssumeRole call or for the session's own service call, so the check polls event history under _INDEX_BUDGET_S=255 and is cut off by the harness 60 s cap ('Check timed out after 60.0s'). No status or error-code divergence is involved - LookupEvents answers 200 with an empty Events list - and the propagation, persistence-across-chaining and absent-when-unset assertions are never reached.
Why: source identity is the control that ties an action taken through a shared role back to the human who assumed it; organizations gate role trust policies on sts:SourceIdentity and then prove attribution from the audit record. On this emulator that proof is permanently unavailable, and a client building attribution or a chained-session audit sees an empty history rather than a refusal.
Fix: the shared management-event recording seam that CloudTrail's LookupEvents handler reads must be driven from STS's served operations (AssumeRole and the credential-bearing calls a session makes), writing requestParameters.roleArn/roleSessionName/sourceIdentity, responseElements.assumedRoleUser.arn, and userIdentity.type/sessionContext (sessionIssuer.arn = role arn, sourceIdentity when the session carries one, omitted when it does not).
Traces: cloudtrail__sts.source-identity-propagates-from-assume-role-into-the-sessions-own-audit-records.

Related open issues that do NOT cover this: 20261024T114720Z-cloudtrail-event-history-is-empty-for-everything-but.md names the same recording seam but its replay set contains no sts leg, and 20261027T001707Z-sts-3-net-new-behavioral-root-causes-assumerolewithwebidentity.md is about AssumeRoleWithWebIdentity credential issuance, not auditing.

## Evidence
cloudtrail__sts.source-identity-propagates-from-assume-role-into-the-sessions-own-audit-records - verdict 'mismatch', detail 'Check timed out after 60.0s'. The check carries DATAPLANE_MARKER = 'DATAPLANE_UNIMPLEMENTED' precisely for a target that audits no session identity, declares _INDEX_BUDGET_S=255 for the event-history poll (citing 80-100 s and 135-136 s measured for regional management events elsewhere in this corpus) plus _PROPAGATION_BUDGET_S=90 for trust-policy propagation, and its _await_record() helper pages LookupEvents to exhaustion (_PAGE_CEILING=40) looking for the AssumeRole record and then the session's own service-call record. It is still inside that poll when the harness cap ends it, i.e. the records are absent rather than wrong.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create a role whose trust policy the caller can assume; call sts AssumeRole with RoleSessionName and SourceIdentity='simparity<token>'.
3. With the returned session credentials, make one management call (e.g. cloudtrail DescribeTrails), then chain a second AssumeRole from that session supplying NO SourceIdentity and make another call with it.
4. Call cloudtrail LookupEvents with LookupAttributes [{AttributeKey: 'EventName', AttributeValue: 'AssumeRole'}] and then for the session call's EventName, over a window covering them.
5. Expect: the AssumeRole record with requestParameters.sourceIdentity / roleSessionName / roleArn and responseElements.assumedRoleUser.arn; each session call's record with userIdentity.type 'AssumedRole', sessionContext.sessionIssuer.arn equal to the role arn, sessionContext.sourceIdentity carrying the same value on both the direct and the chained session, and NO sourceIdentity for a session assumed without one. Observe: Events: [] for every lookup.
