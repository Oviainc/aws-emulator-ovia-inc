Issue-Source: trace-sweep-divergence
Issue-Trace: appfabric-ingestion-and-destination-error-paths
Issue-Step: task#15
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appfabric-ingestion-and-destination-error-paths
Issue-Shape-Id: com.amazonaws.appfabric#FabricFrontEndService
Issue-Key: appfabric|StartUserAccessTasks|2xx->2xx|$.userAccessTasksList[*]
Issue-Key: appfabric|BatchGetUserAccessTasks|2xx->4xx|$.userAccessResultsList
Issue-Key: appfabric|BatchGetUserAccessTasks|2xx->4xx|status
Issue-Commit: 3660361421cece15e150541af9cef0f01b201b94

# AppFabric user-access-task reporting diverges: empty task list and a 404 where AWS reports per-item taskError

## Resolution note

Both causes this issue owns are fixed:

- Cause 1: the start call's item for an application tenant with no authentication token now
  carries the service's fixed error (`notfound` / "No authentication token was found for this
  user. User needs to authorize."), built with no value of the request or of the store;
  the fan-out already reports one item per application tenant of the bundle.
- Cause 2: a polled task id the named app bundle holds no record for is now that entry's own
  `UserAccessResultItem` (the requested id plus the `taskError`) with the call answering 200.

RESIDUAL, tracked elsewhere: the replayed fixture step `task#15` still reports an empty
`userAccessTasksList` because its `setup#2` `CreateAppAuthorization(app=PAGERDUTY)` is refused
by the application allowlist, so the bundle holds no app authorization for the fan-out to
report. That refusal is cause 1 of the still-open issue
`20261029T200017Z-appfabric-refusal-paths-use-the-wrong-error-class.md`, which owns the
supported-application set; this issue's `Issue-Key`
`appfabric|StartUserAccessTasks|2xx->2xx|$.userAccessTasksList[*]` clears once that lands.

> _Note: this issue was identified against the code base at commit `3660361421cece15e150541af9cef0f01b201b94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartUserAccessTasks, BatchGetUserAccessTasks

## Description
Two root causes in AppFabric's user-access-task flow: StartUserAccessTasks reports no task item at all where AWS reports one per application tenant with a modeled error, and BatchGetUserAccessTasks refuses an unknown taskId with a 404 where AWS answers 200 with a per-entry taskError. A fixer must close BOTH before submitting.

--------------------------------------------------
Cause 1 of 2: StartUserAccessTasks reports an empty userAccessTasksList, and its item error is the emulator's own invention
--------------------------------------------------
AWS: StartUserAccessTasks (http 201 POST /useraccess/start) fans out over the named app bundle's applications and returns ONE UserAccessTaskItem per (app, tenantId), each either carrying a taskId that BatchGetUserAccessTasks can poll or carrying the modeled `error` and no taskId. Recorded behavior: for an app/tenant whose user has no authentication token, the item is {app: <the authorization's app>, tenantId: <the authorization's tenant identifier>, error: {errorCode: "notfound", errorMessage: "No authentication token was found for this user. User needs to authorize."}} - the errorCode is the fixed token "notfound" and the errorMessage is entirely fixed text; `app` and `tenantId` are the values of the bundle's authorization. Smithy: UserAccessTaskItem{app required, tenantId required, taskId optional UUID, error optional TaskError{errorCode, errorMessage}}; output userAccessTasksList is UserAccessTasksList; http 201.
Emulator: the response is 201 but userAccessTasksList is EMPTY - no item for the bundle's tenant at all. src/ops/start_user_access_tasks.rs builds items only from the bundle's stored app authorizations (Authorization::snapshot_of over app_authorizations_in_order), so a bundle holding no authorization record reports nothing; in the replayed fixture the PagerDuty authorization could never be created (the application-allowlist cause filed as cause 1 of the companion refusal-paths issue, appfabric-app-authorization-connect-apikey#task#2), so the fan-out had nothing to iterate. Independently of that, when the fan-out DOES produce a not-started item, the error it mints is src/state.rs user_access_task_not_started -> errorCode "AppAuthorizationNotConnected" with message "The app authorization for {app} is {status}, not Connected, so no user access task was started for it." - both the code and the sentence are the emulator's own, not AWS's, and the message interpolates request/state values where AWS's text is fixed.
Why: a client that calls StartUserAccessTasks and then polls each returned item cannot proceed at all - there is nothing to poll, and nothing tells it the user needs to authorize. When items do appear, the errorCode is a token no AWS client switches on ("notfound" is), so user-access remediation UIs and any logic branching on the code take the wrong path.
Fix: src/ops/start_user_access_tasks.rs (the fan-out must report an item per application tenant of the bundle, not only per stored-and-connected authorization record) together with src/state.rs user_access_task_not_started (errorCode and fixed text must be AWS's). Fixing the application allowlist alone is not sufficient - the item's error code and message are wrong regardless.
Traces: appfabric-ingestion-and-destination-error-paths#task#15 (201/201, $.userAccessTasksList[0] missing).

--------------------------------------------------
Cause 2 of 2: BatchGetUserAccessTasks refuses an unknown taskId with 404 instead of answering 200 with a per-entry taskError
--------------------------------------------------
AWS: BatchGetUserAccessTasks (http 200 POST /useraccess/batchget) is a BATCH poll: a taskId the app bundle holds no record for is a PER-ENTRY outcome, not a request failure. Recorded behavior: the call answers HTTP 200 and the entry for that id is a UserAccessResultItem {taskId: <the requested id>, taskError: {errorCode: "404", errorMessage: "Couldn't find a record within current appBundle with the given taskId: <that id>"}} - the errorMessage's fixed text is "Couldn't find a record within current appBundle with the given taskId: " followed by the taskId from the request, and errorCode is the fixed string "404". Smithy: output userAccessResultsList is a list of UserAccessResultItem, every member OPTIONAL including taskId/taskError, which is what lets an entry report only an id and an error; TaskIdList is @length(1..50) with no @uniqueItems; ResourceNotFoundException is modeled (404) but the recording shows it is not what an unknown taskId produces.
Emulator: the whole call is refused HTTP 404 ResourceNotFoundException (src/ops/batch_get_user_access_tasks.rs raises crate::state::user_access_task_not_found for the first id the bundle does not hold, message "The specified user access task does not exist"), so no entry of the batch is reported. The handler's own doc comment (lines ~158-165) argues explicitly for this 404 on the grounds that a task that never ran has no knowable app/tenantId/email; the recording shows AWS instead reports the entry with ONLY taskId and taskError set, which is exactly what the all-optional member set permits.
Why: this breaks the batch contract. One stale or mistyped id in a 50-id poll discards the results of the other 49, so a client polling a set of tasks loses all progress and cannot tell which id was bad; and a caller that lines the response up positionally against its request list gets no list at all instead of a per-position error.
Fix: src/ops/batch_get_user_access_tasks.rs - an id the bundle holds no task for must become a UserAccessResultItem carrying that taskId and AWS's taskError (code "404", the fixed message text plus the requested id) at its own position, with the call still answering 200; keep the one-item-per-entry positional behavior. Update the handler's doc reasoning, and leave the modeled ResourceNotFoundException for a bundle identifier the store does not hold.
Traces: appfabric-ingestion-and-destination-error-paths#task#16 (expected 200, got 404).

## Evidence
Both clusters come from replaying trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, 53 steps, 35 passed, 14 mismatched, 4 blocked, score 66%). Neither carried a stub_path or an owned_by owner.

Cause 1 - fixture appfabric-ingestion-and-destination-error-paths, step task#15, StartUserAccessTasks, status 201 expected and 201 observed:
  $.userAccessTasksList[0]: missing
    expected {'app': 'PAGERDUTY', 'tenantId': '<the bundle authorization's tenant identifier>', 'error': {'errorCode': 'notfound', 'errorMessage': 'No authentication token was found for this user. User needs to authorize.'}}
    got None
  Source: src/ops/start_user_access_tasks.rs lines 137-149 (Authorization::snapshot_of over bundle.app_authorizations_in_order) and lines 288-296 (the not-started item); src/state.rs lines 595-603 (user_access_task_not_started -> errorCode "AppAuthorizationNotConnected", message "The app authorization for {app} is {status}, not Connected, so no user access task was started for it.").
  Dependency: the same fixture's setup#2 CreateAppAuthorization(PAGERDUTY, apiKey) was itself refused 400 by the application allowlist (filed as cause 1 of the companion issue titled "AppFabric refusal paths use the wrong error class, status and message..."), which is why the bundle held no authorization for the fan-out to report. The errorCode/errorMessage divergence above is independent of that and survives it.

Cause 2 - same fixture, step task#16, BatchGetUserAccessTasks:
  status: expected 200, got 404
  $.userAccessResultsList: missing
    expected [{'taskId': '<the requested task id>', 'taskError': {'errorCode': '404', 'errorMessage': "Couldn't find a record within current appBundle with the given taskId: <that same requested id>"}}]
    got None
  Emulator refusal body message: "The specified user access task does not exist" (ResourceNotFoundException, resourceId = the polled task id, resourceType "UserAccessTask").
  Source: src/ops/batch_get_user_access_tasks.rs lines 196-204 (per-id lookup raising ResourceNotFoundException) and its doc comment at lines 158-165, which states the opposite rule; src/state.rs lines 608-614 (user_access_task_not_found).

Model evidence (services/service-com-amazonaws-appfabric-fabricfrontendservice/smithy-model/com-amazonaws-appfabric-fabricfrontendservice.json):
  StartUserAccessTasks: http 201 POST /useraccess/start; input appBundleIdentifier required, email required (sensitive); output userAccessTasksList (UserAccessTasksList).
  UserAccessTaskItem: app required, tenantId required, taskId optional UUID, error optional TaskError.
  BatchGetUserAccessTasks: @readonly, http 200 POST /useraccess/batchget; input taskIdList required TaskIdList @length(1..50), no @uniqueItems; output userAccessResultsList.
  UserAccessResultItem: EVERY member optional (app, tenantId, tenantDisplayName, taskId, resultStatus, email, userId, userFullName, userLastName, userFirstName, userStatus, taskError).
  TaskError: errorCode (String), errorMessage (String), both optional.

## Steps to Reproduce
Cause 1 (StartUserAccessTasks reports nothing):
1. CreateAppBundle, then CreateAppAuthorization in it for an application and tenant (use SLACK/oauth2 to work around the application-allowlist defect filed separately; the fixture used PAGERDUTY/apiKey).
2. POST /useraccess/start with appBundleIdentifier=<bundle> and email=<a user address>.
3. Emulator: 201 with an empty userAccessTasksList when the bundle holds no stored authorization record; and when it does hold a non-connected one, the item's error is {errorCode: "AppAuthorizationNotConnected", errorMessage: "The app authorization for <app> is <status>, not Connected, ..."}.
4. AWS: 201 with one item per (app, tenantId) of the bundle; for a user with no authentication token the item is {app, tenantId, error: {errorCode: "notfound", errorMessage: "No authentication token was found for this user. User needs to authorize."}} and no taskId.

Cause 2 (BatchGetUserAccessTasks on an unknown taskId):
1. CreateAppBundle (any bundle the store holds).
2. POST /useraccess/batchget with appBundleIdentifier=<bundle> and taskIdList=[<a fresh uuid4 no task was ever started for>].
3. Emulator: 404 ResourceNotFoundException "The specified user access task does not exist", no results list. AWS: 200 with userAccessResultsList = [{taskId: <that id>, taskError: {errorCode: "404", errorMessage: "Couldn't find a record within current appBundle with the given taskId: <that id>"}}].
4. Also check the mixed batch: taskIdList=[<a real started taskId>, <an unknown uuid4>] must answer 200 with TWO entries in request order - the real task's result and the per-entry taskError - not a single 404.

Re-replay appfabric-ingestion-and-destination-error-paths#task#15 and #task#16 to confirm both clusters pass.
