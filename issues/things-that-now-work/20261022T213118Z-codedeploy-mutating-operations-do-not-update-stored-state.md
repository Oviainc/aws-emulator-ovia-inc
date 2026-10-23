Issue-Source: trace-sweep-divergence
Issue-Trace: codedeploy-application-crud-and-github-token-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codedeploy-application-crud-and-github-token-lifecycle codedeploy-deployment-group-onprem-tag-filter-lifecycle codedeploy-on-premises-instance-registration-and-tagging codedeploy-onprem-instance-tags-round-trip codedeploy-custom-deployment-config-lifecycle
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy|GetApplication|2xx->4xx|$.application
Issue-Key: codedeploy|GetApplication|2xx->4xx|status
Issue-Key: codedeploy|GetApplication|4xx->2xx|$.__type
Issue-Key: codedeploy|GetApplication|4xx->2xx|$.message
Issue-Key: codedeploy|GetApplication|4xx->2xx|status
Issue-Key: codedeploy|CreateApplication|4xx->2xx|$.__type
Issue-Key: codedeploy|CreateApplication|4xx->2xx|$.message
Issue-Key: codedeploy|CreateApplication|4xx->2xx|status
Issue-Key: codedeploy|GetDeploymentGroup|2xx->2xx|$.deploymentGroupInfo.autoScalingGroups
Issue-Key: codedeploy|GetDeploymentGroup|2xx->2xx|$.deploymentGroupInfo.bakeTime
Issue-Key: codedeploy|GetDeploymentGroup|2xx->2xx|$.deploymentGroupInfo.deploymentStyle
Issue-Key: codedeploy|GetDeploymentGroup|2xx->2xx|$.deploymentGroupInfo.ec2TagFilters
Issue-Key: codedeploy|GetDeploymentGroup|2xx->2xx|$.deploymentGroupInfo.managedHookConfig
Issue-Key: codedeploy|GetDeploymentGroup|2xx->2xx|$.deploymentGroupInfo.onPremisesInstanceTagFilters[*].Value
Issue-Key: codedeploy|GetDeploymentGroup|2xx->2xx|$.deploymentGroupInfo.outdatedInstancesStrategy
Issue-Key: codedeploy|GetDeploymentGroup|2xx->2xx|$.deploymentGroupInfo.triggerConfigurations
Issue-Key: codedeploy|GetOnPremisesInstance|2xx->2xx|$.instanceInfo.tags[*]
Issue-Key: codedeploy|GetOnPremisesInstance|4xx->2xx|$.__type
Issue-Key: codedeploy|GetOnPremisesInstance|4xx->2xx|$.message
Issue-Key: codedeploy|GetOnPremisesInstance|4xx->2xx|status
Issue-Key: codedeploy|GetDeploymentConfig|2xx->4xx|$.deploymentConfigInfo
Issue-Key: codedeploy|GetDeploymentConfig|2xx->4xx|status
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# CodeDeploy mutating operations do not update stored state

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApplication

## Description
Eight wire divergences in which a CodeDeploy mutation (rename, filter replacement, tag removal, deregistration, custom deployment-config creation) returns success but the service's stored state is unchanged, so the following read operation answers with the pre-mutation view — or with a 400 where AWS answers 200 and vice versa. Each cause below is an independent read path that must be closed; eight causes in total.

Cause 1 of 8: an UpdateApplication rename is not reflected, so GetApplication on the NEW name is refused.
AWS: Smithy UpdateApplication(applicationName, newApplicationName) -> Unit, errors ApplicationAlreadyExistsException / ApplicationDoesNotExistException / ApplicationNameRequiredException / InvalidApplicationNameException (all HTTP 400). After a 200 rename, GetApplication(<new name>) answers HTTP 200 with an `application` member whose applicationName is the new name (recorded: application{applicationId, applicationName=<renamed>, computePlatform=Server, createTime, linkedToGitHub=false}).
Emulator: HTTP 400 and no `application` member for the renamed name.
Why: a client that renames an application and then addresses it by its new name — the documented purpose of the operation — cannot read it back at all, so rename is unusable and any workflow that renames then polls fails on a 400 AWS never sends.
Fix: the UpdateApplication handler and the application store it writes to, under services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/. Hypothesis: the open issue "Implement codedeploy UpdateApplication: unimplemented scaffold stub" is the same root cause; that issue carries no replay key for this GetApplication step, so this cluster is filed here.
Traces: codedeploy-application-crud-and-github-token-lifecycle#task#5.

Cause 2 of 8: after a rename the OLD application name still resolves.
AWS: Smithy GetApplication models ApplicationDoesNotExistException (HTTP 400). Once renamed, the pre-rename name no longer names an application and GetApplication on it is refused with HTTP 400 / ApplicationDoesNotExistException (recorded message example: 'No application found for name: sim2real-probe-codedeploy-app-78e88003').
Emulator: HTTP 200 with the application body, no `__type` and no `message`.
Why: the old name stays live, so a client sees the same application under two names; existence probes and idempotency guards built on GetApplication report the opposite of AWS.
Fix: the UpdateApplication handler's state mutation (re-key the stored application rather than leaving the old key resolvable).
Traces: codedeploy-application-crud-and-github-token-lifecycle#task#6.

Cause 3 of 8: CreateApplication for an already-existing name is accepted.
AWS: Smithy CreateApplication(applicationName required) models ApplicationAlreadyExistsException (HTTP 400). Creating an application whose name is already taken is refused with HTTP 400 / ApplicationAlreadyExistsException (recorded message example: 'sim2real-probe-codedeploy-app-renamed-78e88003 already exists.').
Emulator: HTTP 200, no `__type` / `message`, and a second record is created.
Why: a client relying on the already-exists refusal as its create-or-adopt branch takes the wrong branch and ends up with duplicate application state.
Fix: CreateApplication's uniqueness precondition against the application store. Hypothesis: in this fixture the name is free in the emulator only because Cause 1's rename never landed, so this cause may close with Cause 1 — it must still be re-replayed on its own step.
Traces: codedeploy-application-crud-and-github-token-lifecycle#task#8.

Cause 4 of 8: UpdateDeploymentGroup's onPremisesInstanceTagFilters replacement is not applied.
AWS: Smithy UpdateDeploymentGroup declares `onPremisesInstanceTagFilters` (TagFilterList); a 200 update replaces the stored filter list and GetDeploymentGroup then reports the new list (recorded: onPremisesInstanceTagFilters[0].Value == 'filter-b', the updated value).
Emulator: GetDeploymentGroup still reports the create-time value ('filter-a').
Why: a client that reconfigures a deployment group's on-premises targeting and reads it back to confirm sees the stale selector, so drift detection and IaC refresh both report the wrong configuration and deployments would target the wrong hosts.
Fix: the UpdateDeploymentGroup handler's state mutation for the tag-filter members.
Traces: codedeploy-deployment-group-onprem-tag-filter-lifecycle#task#7. (The same step additionally omits defaulted deploymentGroupInfo members; that omission is the separate response-normalization issue filed alongside this one.)

Cause 5 of 8: RemoveTagsFromOnPremisesInstances does not remove the tag.
AWS: Smithy RemoveTagsFromOnPremisesInstances(tags required, instanceNames required) -> Unit, errors InstanceNotRegisteredException / InvalidTagException / TagRequiredException (HTTP 400). After a 200 removal, GetOnPremisesInstance reports instanceInfo.tags without the removed entry (recorded: no entry at tags[0]).
Emulator: the removed tag is still present at instanceInfo.tags[0] (example: {'Key': 'Team', 'Value': 'probe'}).
Why: tag removal is a no-op, so a client cannot un-target an on-premises instance; tag-based deployment-group selection keeps matching an instance AWS would have dropped.
Fix: the RemoveTagsFromOnPremisesInstances handler and the on-premises instance tag store.
Traces: codedeploy-on-premises-instance-registration-and-tagging#task#12.

Cause 6 of 8: the same removal is a no-op for non-ASCII tag keys/values.
AWS: as Cause 5 — after removal GetOnPremisesInstance reports the instance with those tags gone, independent of the tag's character set.
Emulator: both tags remain (examples: {'Key': '\u94a5unicode-key', 'Value': '\u503cvalue-value'} and {'Key': 'caf\u00e9-key', 'Value': 'na\u00efve-\u30c6\u30b9\u30c8'}).
Why: same client impact as Cause 5, and it shows the removal path is not merely mis-encoding non-ASCII keys but not mutating state at all — a fix that only normalizes Unicode would not close it.
Fix: the RemoveTagsFromOnPremisesInstances handler (tag matching/removal, including non-ASCII keys).
Traces: codedeploy-onprem-instance-tags-round-trip#task#5.

Cause 7 of 8: DeregisterOnPremisesInstance leaves the instance registered.
AWS: Smithy DeregisterOnPremisesInstance(instanceName required) -> Unit; GetOnPremisesInstance models InstanceNotRegisteredException (HTTP 400, doc "The specified on-premises instance is not registered"). After a 200 deregistration, GetOnPremisesInstance on that name is refused with HTTP 400 / InstanceNotRegisteredException (recorded message example: 'The requested host does not exist. Host: sim2real-probe-codedeploy-onprem-fac4b8a1').
Emulator: HTTP 200 with the instance body, no `__type` / `message`.
Why: deregistration never takes effect, so cleanup is impossible and a re-registration/not-registered branch keyed on InstanceNotRegisteredException is never taken.
Fix: the DeregisterOnPremisesInstance handler's state mutation (and GetOnPremisesInstance's registration check).
Traces: codedeploy-on-premises-instance-registration-and-tagging#teardown#14.

Cause 8 of 8: a custom deployment configuration cannot be read back after creation.
AWS: Smithy GetDeploymentConfig(deploymentConfigName required) -> deploymentConfigInfo, errors DeploymentConfigDoesNotExistException / DeploymentConfigNameRequiredException (HTTP 400). For a deployment config just created in the account, GetDeploymentConfig answers HTTP 200 with deploymentConfigInfo{deploymentConfigId, deploymentConfigName, computePlatform, createTime, minimumHealthyHosts{type, value}}.
Emulator: HTTP 400 with no deploymentConfigInfo, for both the immediate read and the later re-read in the same fixture.
Why: a client that defines its own deployment configuration cannot resolve it, so every later CreateDeploymentGroup/CreateDeployment that names it is refused as well.
Fix: the CreateDeploymentConfig handler must persist into the deployment-config store that GetDeploymentConfig reads. Hypothesis: the open issue "Implement codedeploy CreateDeploymentConfig: unimplemented scaffold stub" is the root cause; it carries no replay key for these GetDeploymentConfig steps, so the cluster is filed here.
Traces: codedeploy-custom-deployment-config-lifecycle#task#3 (same cluster also at codedeploy-custom-deployment-config-lifecycle#task#5).

## Evidence
Replayed wire corpus simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, state_reset_failures=0, so these are not cross-fixture state leaks). Expected = recorded AWS response, Actual = emulator.

1. codedeploy-application-crud-and-github-token-lifecycle#task#5 GetApplication — expected 200 with $.application = {'applicationId': ..., 'applicationName': 'sim2real-probe-codedeploy-app-renamed-78e88003', 'computePlatform': 'Server', 'createTime': <shaped>, 'linkedToGitHub': False}; got 400 and $.application missing.
2. ...#task#6 GetApplication — expected 400 with $.__type 'ApplicationDoesNotExistException'; got 200 with no $.__type and no $.message.
3. ...#task#8 CreateApplication — expected 400 with $.__type 'ApplicationAlreadyExistsException'; got 200 with no $.__type / $.message.
4. codedeploy-deployment-group-onprem-tag-filter-lifecycle#task#7 GetDeploymentGroup — expected $.deploymentGroupInfo.onPremisesInstanceTagFilters[0].Value 'filter-b'; got 'filter-a'.
5. codedeploy-on-premises-instance-registration-and-tagging#task#12 GetOnPremisesInstance — expected no $.instanceInfo.tags[0]; got extra {'Key': 'Team', 'Value': 'probe'}.
6. codedeploy-onprem-instance-tags-round-trip#task#5 GetOnPremisesInstance — expected no $.instanceInfo.tags[0]/[1]; got extra {'Key': '钥unicode-key', 'Value': '值value-value'} and {'Key': 'café-key', 'Value': 'naïve-テスト'}.
7. codedeploy-on-premises-instance-registration-and-tagging#teardown#14 GetOnPremisesInstance — expected 400 with $.__type 'InstanceNotRegisteredException'; got 200.
8. codedeploy-custom-deployment-config-lifecycle#task#3 GetDeploymentConfig — expected 200 with $.deploymentConfigInfo (deploymentConfigName 'sim2real-probe-codedeploy-dpcfg-373ba7e7', minimumHealthyHosts {'type': 'HOST_COUNT', 'value': 1}); got 400 and $.deploymentConfigInfo missing. Same cluster at #task#5.

Smithy (services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json): UpdateApplication -> Unit with ApplicationAlreadyExistsException/ApplicationDoesNotExistException (400); CreateApplication models ApplicationAlreadyExistsException (400); UpdateDeploymentGroup declares onPremisesInstanceTagFilters (TagFilterList); RemoveTagsFromOnPremisesInstances and DeregisterOnPremisesInstance -> Unit; GetOnPremisesInstance models InstanceNotRegisteredException (400); GetDeploymentConfig -> deploymentConfigInfo with DeploymentConfigDoesNotExistException (400). Generated ids, createTime values and probe names above are environment-specific examples; the requirements are the status, the error code and the state transition. Recorded error messages are quoted as examples only — the model documents no message text for these exceptions.

Related open issues that do NOT cover these steps (no replay key for them): "Implement codedeploy UpdateApplication: unimplemented scaffold stub", "Implement codedeploy CreateDeploymentConfig: unimplemented scaffold stub", "Implement codedeploy UpdateDeploymentGroup: unimplemented scaffold stub", and the behavioral "CodeDeploy deployment lifecycle, response normalization and error classification diverge from AWS (6 causes)".

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus) — all eight clusters reproduce.
2. Cause 1/2/3: CreateApplication(applicationName=A); UpdateApplication(applicationName=A, newApplicationName=B); GetApplication(B) -> expect 200 with application.applicationName == B (observe 400); GetApplication(A) -> expect 400 ApplicationDoesNotExistException (observe 200); CreateApplication(applicationName=B) -> expect 400 ApplicationAlreadyExistsException (observe 200).
3. Cause 4: CreateDeploymentGroup(..., onPremisesInstanceTagFilters=[{Key:'k',Value:'filter-a',Type:'KEY_AND_VALUE'}]); UpdateDeploymentGroup(..., onPremisesInstanceTagFilters=[{...Value:'filter-b'...}]); GetDeploymentGroup -> expect Value 'filter-b'.
4. Cause 5/6: RegisterOnPremisesInstance(instanceName=H, iamUserArn=...); AddTagsToOnPremisesInstances(tags=[{Key:'Team',Value:'probe'}], instanceNames=[H]); RemoveTagsFromOnPremisesInstances(same tags, [H]); GetOnPremisesInstance(H) -> expect instanceInfo.tags with the tag gone. Repeat with non-ASCII keys/values.
5. Cause 7: DeregisterOnPremisesInstance(instanceName=H); GetOnPremisesInstance(H) -> expect 400 InstanceNotRegisteredException.
6. Cause 8: CreateDeploymentConfig(deploymentConfigName=C, minimumHealthyHosts={type:'HOST_COUNT',value:1}); GetDeploymentConfig(C) -> expect 200 with deploymentConfigInfo.
