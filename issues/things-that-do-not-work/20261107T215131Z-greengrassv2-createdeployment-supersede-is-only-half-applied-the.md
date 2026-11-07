Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: greengrassv2.deployment-supersede-marks-previous-inactive greengrassv2.list-deployments-history-filter-latest-only-vs-all
Issue-Shape-Id: com.amazonaws.greengrassv2#GreengrassV2
Issue-Key: greengrassv2.deployment-supersede-marks-previous-inactive|_poll_until#17|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s, last={last!r}")
Issue-Key: greengrassv2.list-deployments-history-filter-latest-only-vs-all|_poll_until#10|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s, last={last!r}")
Issue-Commit: 156e564710334729d97c597958f2499faf70dde5

# greengrassv2 CreateDeployment supersede is only half-applied: the prior deployment loses isLatestForTarget but never leaves deploymentStatus=ACTIVE

> _Note: this issue was identified against the code base at commit `156e564710334729d97c597958f2499faf70dde5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: a second CreateDeployment for a target that already has a deployment does not move the prior deployment to the INACTIVE terminal status; it only clears that deployment's isLatestForTarget flag, leaving a deployment that is simultaneously reported as ACTIVE and not-latest-for-target.

NOTE ON AN EXISTING ISSUE: the open issue 20261103T065241Z-greengrassv2-createdeployment-accepts-nonexistent-targets-and-never-supersedes.md describes a related supersede gap as its Cause 2, and names these same two checks. It does NOT cover them as filed: it asserts the prior deployment "stays ACTIVE indefinitely" with no state change at all, whereas the emulator now DOES flip isLatestForTarget to false while leaving deploymentStatus=ACTIVE, and the replay keys it carries for these two checks (`_poll_until|Timeout`) no longer match the signature they fail with (`_poll_until|AssertionError`), so the suite does not report them as owned by it. The remaining divergence is specifically the status transition, not the flag.

AWS: Smithy: CreateDeployment POST /greengrass/v2/deployments (success 201); GetDeployment GET /greengrass/v2/deployments/{deploymentId} (200) returns `deploymentStatus` (enum DeploymentStatus = ACTIVE | COMPLETED | CANCELED | FAILED | INACTIVE), `isLatestForTarget` (boolean, default false, documented "Whether or not the deployment is the latest revision for its target") and `revisionId`; ListDeployments GET /greengrass/v2/deployments takes `historyFilter` (enum DeploymentHistoryFilter = ALL | LATEST_ONLY) and returns Deployment structures carrying the same `deploymentStatus`/`isLatestForTarget` members. Recorded behavior: creating a second deployment for the same `targetArn` supersedes the first — the prior deployment settles at `deploymentStatus='INACTIVE'` AND `isLatestForTarget=false`, while the new deployment is the `ACTIVE`, `isLatestForTarget=true` deployment for that target; ListDeployments(historyFilter=LATEST_ONLY) then returns only the new deployment and historyFilter=ALL returns both revisions. The two fields move together: INACTIVE is the status AWS parks a superseded revision in, and it is a terminal status (the revision never returns to ACTIVE).

Emulator: the supersede is applied to one field only. After the second CreateDeployment, GetDeployment on the first deployment returns HTTP 200 with `isLatestForTarget: False` — so the emulator did notice the supersede and did demote the revision — but `deploymentStatus` is still `'ACTIVE'`, and it stays ACTIVE for the whole 60 s window both checks poll. No DeploymentStatus value other than ACTIVE is ever reachable for a superseded revision, so the INACTIVE state is unreachable.

Why: a client reading `deploymentStatus` to decide which deployment is current for a target is told that both the superseded and the superseding revision are ACTIVE, which is a state AWS never produces — two ACTIVE deployments on one target. Any code that waits for the previous revision to settle INACTIVE before deleting it, rolling forward, or reporting rollout completion hangs forever rather than progressing, and the self-contradictory pair (ACTIVE + isLatestForTarget=false) cannot be interpreted. Because status and the latest flag are the two independent ways AWS exposes deployment history, a ListDeployments consumer filtering on status rather than on historyFilter sees no history at all.

Fix: CreateDeployment's handler and the deployment state it writes. When recording a new deployment for a `targetArn` that already has a latest deployment, the same state transition that clears the prior record's `isLatestForTarget` must also set its `deploymentStatus` to INACTIVE, and GetDeployment's and ListDeployments' serializers must report that stored status rather than a hardcoded/initial ACTIVE. The two fields must not be able to diverge.

Traces: greengrassv2.deployment-supersede-marks-previous-inactive (polls GetDeployment(dep1) for any non-ACTIVE status, then asserts INACTIVE and isLatestForTarget=False); greengrassv2.list-deployments-history-filter-latest-only-vs-all (same poll as a precondition before comparing LATEST_ONLY against ALL, so the LATEST_ONLY/ALL comparison is never reached).

## Evidence
Check greengrassv2.deployment-supersede-marks-previous-inactive — create an IoT thing group, CreateDeployment(targetArn=<groupArn>, deploymentName='rev1'), then CreateDeployment(targetArn=<groupArn>, deploymentName='rev2'), then poll GetDeployment(rev1) until deploymentStatus != 'ACTIVE':
  expected: GetDeployment(dep1) -> deploymentStatus='INACTIVE', isLatestForTarget=False; GetDeployment(dep2) -> deploymentStatus='ACTIVE', isLatestForTarget=True
  actual: AssertionError: condition not met within 60s, last={'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'targetArn': 'arn:aws:iot:<region>:<account>:thinggroup/<group>', 'revisionId': '1', 'deploymentId': '<uuid>', 'deploymentName': 'sim2real-rev1', 'deploymentStatus': 'ACTIVE', 'iotJobId': '<uuid>', 'iotJobArn': 'arn:aws:iot:<region>:<account>:job/<uuid>', 'creationTimestamp': <ts>, 'isLatestForTarget': False, 'tags': {...}}
  Note the self-contradiction in the actual response: isLatestForTarget=False (demoted) together with deploymentStatus='ACTIVE' (not superseded).

Check greengrassv2.list-deployments-history-filter-latest-only-vs-all — two CreateDeployment calls on one thing-group target, then poll GetDeployment(dep1)['deploymentStatus'] for a non-ACTIVE value:
  expected: the poll returns 'INACTIVE', after which ListDeployments(historyFilter='LATEST_ONLY') contains only dep2 and ListDeployments(historyFilter='ALL') contains both dep1 and dep2
  actual: AssertionError: condition not met within 60s, last='ACTIVE'

The deployment ids, iot job ids, group names, account, region and timestamps above are per-run values from this replay and are not requirements; the requirement is the ACTIVE -> INACTIVE transition of the superseded revision and its agreement with isLatestForTarget=False.

## Steps to Reproduce
1. Create an IoT thing group and take its thingGroupArn: iot.create_thing_group(thingGroupName=<g>).
2. dep1 = greengrassv2.create_deployment(targetArn=<groupArn>, deploymentName='rev1'). Confirm GetDeployment(dep1) reports deploymentStatus='ACTIVE' and isLatestForTarget=True (this currently passes).
3. dep2 = greengrassv2.create_deployment(targetArn=<groupArn>, deploymentName='rev2'). Confirm the deploymentId differs from dep1.
4. Poll greengrassv2.get_deployment(deploymentId=dep1) until deploymentStatus != 'ACTIVE'. Expected: it settles at deploymentStatus='INACTIVE' with isLatestForTarget=False. Observed: isLatestForTarget is already False but deploymentStatus remains 'ACTIVE' indefinitely (the poll times out at 60 s).
5. Confirm GetDeployment(dep2) reports deploymentStatus='ACTIVE' and isLatestForTarget=True.
6. Then compare greengrassv2.list_deployments(targetArn=<groupArn>, historyFilter='LATEST_ONLY') against historyFilter='ALL': LATEST_ONLY must contain only dep2, ALL must contain both dep1 and dep2.
