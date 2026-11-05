Issue-Source: trace-sweep-divergence
Issue-Trace: amplify-afs-traj-app-id-mistakes
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amplify-afs-traj-app-id-mistakes amplify-afs-traj-branch-before-deploy-and-retry amplify-afs-traj-create-app-request-mistakes amplify-afs-traj-manual-deploy-job-sequencing
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify|GetApp|4xx->4xx|$.message
Issue-Key: amplify|CreateBranch|4xx->4xx|$.message
Issue-Key: amplify|CreateApp|4xx->4xx|$.message
Issue-Key: amplify|StartDeployment|4xx->4xx|$.message
Issue-Key: amplify|CreateBranch|4xx->2xx|$.message
Issue-Key: amplify|CreateBranch|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: amplify|CreateBranch|4xx->2xx|status
Issue-Key: amplify|CreateDeployment|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: amplify|CreateDeployment|4xx->4xx|status
Issue-Key: amplify|CreateDeployment|4xx->2xx|$.message
Issue-Key: amplify|CreateDeployment|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: amplify|CreateDeployment|4xx->2xx|status
Issue-Commit: 798fb32aa8eb1e75369deb4b48c94746eb1ca248

# amplify request refusals: wrong validation message text, wrong error class, and three refusals never made

> _Note: this issue was identified against the code base at commit `798fb32aa8eb1e75369deb4b48c94746eb1ca248`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApp

## Description
Seven net-new root causes in the amplify control plane's request-refusal contract, each grounded in a replayed wire fixture step. They share one fault: the emulator decides WHETHER to refuse a request, and words the refusal it does send, differently from AWS. They are independent defects and must each be closed before this issue is submitted.

Cause 1 of 7: the regular-expression constraint refusal renders the model's anchored pattern instead of the pattern AWS quotes.
  AWS: Smithy models GetApp.appId (-> AppId, @httpLabel, @required, @length 1..20, pattern "^d[a-z0-9]+$") with BadRequestException (HTTP 400) among its errors. Recorded behavior: an appId that does not match is refused with HTTP 400 and the message "1 validation error detected: Value '<appId>' at 'appId' failed to satisfy constraint: Member must satisfy regular expression pattern: d[a-z0-9]+". Everything except <appId> is fixed text: <appId> is the rejected value copied from the request, 'appId' is the member that failed, and AWS renders the pattern UNANCHORED - d[a-z0-9]+ - not the model's ^...$ form.
  Emulator: same status and same sentence, but the pattern is rendered '^d[a-z0-9]+$' (the model's literal pattern trait).
  Why: a client (or operator) that parses the allowed pattern out of the refusal text, or that asserts on a refusal recorded against real AWS, sees a different contract than AWS states; message-equality checks against an AWS baseline fail on every appId validation error of this service.
  Fix: the shared constraint-refusal renderer this service's input validation feeds - strip the anchors when quoting a pattern constraint in the refusal message.
  Traces: amplify-afs-traj-app-id-mistakes#task#1.

Cause 2 of 7: the enum-constraint refusal for CreateBranch.stage renders the allowed value set in the model's declaration order instead of AWS's order.
  AWS: Smithy models CreateBranch.stage -> enum Stage, whose declaration order is PRODUCTION, BETA, DEVELOPMENT, EXPERIMENTAL, PULL_REQUEST (smithy_query shape=Stage). Recorded behavior: an unmodelled stage is refused with HTTP 400 and the message "1 validation error detected: Value '<stage>' at 'stage' failed to satisfy constraint: Member must satisfy enum value set: [EXPERIMENTAL, BETA, PULL_REQUEST, PRODUCTION, DEVELOPMENT]". Fixed text: everything except <stage>, the rejected value from the request; the bracketed list is part of the fixed text and AWS emits it in that order, which is NOT the model's order.
  Emulator: identical sentence but the set is rendered '[PRODUCTION, BETA, DEVELOPMENT, EXPERIMENTAL, PULL_REQUEST]'.
  Why: as Cause 1 - the refusal message is the contract a client reads the valid values from, and an AWS-recorded assertion on it fails.
  Fix: the enum-constraint refusal renderer behind this operation's validation must emit the value order AWS uses for Stage rather than the model's member order.
  Traces: amplify-afs-traj-branch-before-deploy-and-retry#task#3.

Cause 3 of 7: the same enum-order defect on CreateApp.platform.
  AWS: Smithy models CreateApp.platform -> enum Platform, declaration order WEB, WEB_DYNAMIC, WEB_COMPUTE. Recorded behavior: an unknown platform is refused with HTTP 400 and "1 validation error detected: Value '<platform>' at 'platform' failed to satisfy constraint: Member must satisfy enum value set: [WEB_COMPUTE, WEB_DYNAMIC, WEB]" - fixed text apart from <platform> from the request.
  Emulator: renders '[WEB, WEB_DYNAMIC, WEB_COMPUTE]'.
  Why: as Cause 1.
  Fix: the same enum-constraint refusal renderer, for Platform.
  Note on an existing issue: this is the same root cause as Cause 1 of the open behavioral issue 20261102T085926Z-amplify-control-plane-8-net-new-root-causes.md, which carries only behavioral check ids and no wire replay key; it is restated here so this wire step is carried as a replay key. A fixer closing either closes both and should re-replay the step named below.
  Traces: amplify-afs-traj-create-app-request-mistakes#task#2.

Cause 4 of 7: StartDeployment with neither jobId nor sourceUrl is refused with the emulator's own wording instead of AWS's.
  AWS: Smithy models StartDeployment -> POST /apps/{appId}/branches/{branchName}/deployments/start with optional `jobId` and `sourceUrl` and BadRequestException (HTTP 400) among its errors. Recorded behavior: a call carrying neither member is refused with HTTP 400 and the message "Must specify jobId or sourceUrl for StartDeploymentRequest." - entirely fixed text, carrying nothing from the request or state.
  Emulator: the status and error class match, but the message is "A deployment requires either the jobId of a CreateDeployment request or a sourceUrl."
  Why: the message is part of the error contract; a client that matches on it (or a recorded-baseline assertion) does not recognize the refusal, and an operator reading emulator output sees text AWS never emits.
  Fix: the StartDeployment handler's missing-argument refusal message.
  Traces: amplify-afs-traj-manual-deploy-job-sequencing#task#5.

Cause 5 of 7: CreateBranch applies no character-set validation to branchName, so a name AWS refuses is accepted with 200.
  AWS: Smithy models CreateBranch.branchName @required, @length 1..255, pattern ^(?s).+$ and BadRequestException (HTTP 400) among its errors. Recorded behavior: a branch name containing a space is refused with HTTP 400, a shaped x-amzn-errortype header and the message "The requested branch name is not supported. Failed to create branch. The branch name <branchName> is invalid. The name can only contain letters, numbers, and the symbols /_.-" - all fixed text except <branchName>, copied from the request.
  Emulator: answers HTTP 200 with a Branch, no `message` and no x-amzn-errortype header, and stores the branch.
  Why: the branch name becomes the branch's domain prefix, so the emulator hands back a branch that can never be addressed, and a client relying on AWS's refusal to validate user input never sees it. The stored branch then leaks into ListBranches (see the companion issue's ListBranches cause).
  Fix: the CreateBranch handler's validation, before the branch is built or stored - refuse a branchName containing any character outside letters, digits and /_.- with 400 BadRequestException and AWS's message.
  Note on an existing issue: same root cause as Cause 4 of the open behavioral issue 20261102T085926Z-amplify-control-plane-8-net-new-root-causes.md, which carries no wire replay key.
  Traces: amplify-afs-traj-branch-before-deploy-and-retry#task#4.

Cause 6 of 7: CreateDeployment's not-found refusal uses the wrong error class - 400 BadRequestException instead of 404.
  AWS: Smithy models CreateDeployment -> POST /apps/{appId}/branches/{branchName}/deployments; its modeled error list is BadRequestException (400), InternalFailureException (500), LimitExceededException (429), UnauthorizedException (401) and does NOT include NotFoundException, so this is recorded behavior only and the recording overrides the model's list: deploying to an app/branch that does not exist is refused with HTTP 404 and the not-found error type in x-amzn-errortype. The message matched at replay and is not at issue.
  Emulator: answers HTTP 400 with x-amzn-errortype 'BadRequestException' and the same message.
  Why: clients branch on the error CODE, not the message - a not-found target is created or retried on 404 but treated as a malformed request on 400, so a generated `except NotFoundException` path never fires and error-class control flow breaks.
  Fix: the CreateDeployment handler's not-found refusal - answer the 404 not-found error for an absent app or branch, keeping the message.
  Note on an existing issue: same root cause as Cause 5 of the open behavioral issue 20261102T085926Z-amplify-control-plane-8-net-new-root-causes.md, which carries no wire replay key.
  Traces: amplify-afs-traj-app-id-mistakes#task#4, and amplify-afs-traj-branch-before-deploy-and-retry#task#2 in the same cluster.

Cause 7 of 7: CreateDeployment is accepted (200) while the branch's previous deployment job is still unfinished, where AWS refuses it with 400.
  AWS: Smithy models CreateDeployment with BadRequestException (HTTP 400) among its errors. Recorded behavior: with an unfinished job on the branch, a further CreateDeployment is refused with HTTP 400, a shaped x-amzn-errortype header and the message "The last job(deployment) <jobId> was not finished, please wait until it finish or use StopJob operation to cancel it before creating a new deployment." - fixed text apart from <jobId>, which is the id of the branch's last unfinished job taken from emulator state (the recording shows '2' for that run; the number is not part of the requirement).
  Emulator: answers HTTP 200 with a fresh jobId, no `message` and no x-amzn-errortype header, minting a second concurrent deployment on the branch.
  Why: AWS serializes deployments per branch. A client is told a second deployment was reserved while another is in flight, so it polls an id whose publish can never be ordered against the first, and the "cancel the previous job first" instruction AWS's message exists to give is never delivered.
  Fix: the CreateDeployment handler's precondition on the branch's outstanding job - refuse with 400 BadRequestException and AWS's message when the branch's last job has not reached a terminal status, before any job id is reserved.
  Traces: amplify-afs-traj-manual-deploy-job-sequencing#task#4, and amplify-afs-traj-manual-deploy-job-sequencing#task#9 in the same cluster.

## Evidence
Cause 1 (amplify-afs-traj-app-id-mistakes#task#1, GetApp, HTTP 400 both sides): $.message expected "1 validation error detected: Value 'frontend-app' at 'appId' failed to satisfy constraint: Member must satisfy regular expression pattern: d[a-z0-9]+"; actual "...regular expression pattern: ^d[a-z0-9]+$". 'frontend-app' is the request's value filling the template; only the pattern's anchors diverge.

Cause 2 (amplify-afs-traj-branch-before-deploy-and-retry#task#3, CreateBranch, HTTP 400 both sides): $.message expected "1 validation error detected: Value 'production' at 'stage' failed to satisfy constraint: Member must satisfy enum value set: [EXPERIMENTAL, BETA, PULL_REQUEST, PRODUCTION, DEVELOPMENT]"; actual "... [PRODUCTION, BETA, DEVELOPMENT, EXPERIMENTAL, PULL_REQUEST]". 'production' is the request's value; only the bracketed order diverges.

Cause 3 (amplify-afs-traj-create-app-request-mistakes#task#2, CreateApp, HTTP 400 both sides): $.message expected "... Value 'web' at 'platform' ... enum value set: [WEB_COMPUTE, WEB_DYNAMIC, WEB]"; actual "... [WEB, WEB_DYNAMIC, WEB_COMPUTE]".

Cause 4 (amplify-afs-traj-manual-deploy-job-sequencing#task#5, StartDeployment, HTTP 400 both sides): $.message expected 'Must specify jobId or sourceUrl for StartDeploymentRequest.' (all fixed text); actual 'A deployment requires either the jobId of a CreateDeployment request or a sourceUrl.'

Cause 5 (amplify-afs-traj-branch-before-deploy-and-retry#task#4, CreateBranch): expected HTTP 400, a shaped x-amzn-errortype header and $.message 'The requested branch name is not supported. Failed to create branch. The branch name main site is invalid. The name can only contain letters, numbers, and the symbols /_.-' ('main site' is the request's branchName; the rest is fixed). Actual: HTTP 200, no $.message, no x-amzn-errortype header.

Cause 6 (amplify-afs-traj-app-id-mistakes#task#4 and amplify-afs-traj-branch-before-deploy-and-retry#task#2, CreateDeployment): expected status 404 with the not-found error type in x-amzn-errortype; actual status 400 with x-amzn-errortype 'BadRequestException'.

Cause 7 (amplify-afs-traj-manual-deploy-job-sequencing#task#4 and #task#9, CreateDeployment): expected HTTP 400, a shaped x-amzn-errortype header and $.message 'The last job(deployment) 2 was not finished, please wait until it finish or use StopJob operation to cancel it before creating a new deployment.' (the '2' is the last unfinished job's id from state; the rest is fixed). Actual: HTTP 200, no $.message, no x-amzn-errortype header.

## Steps to Reproduce
Cause 1:
1. amplify.get_app(appId='frontend-app') -> expect HTTP 400 with the pattern quoted unanchored as d[a-z0-9]+; observed '^d[a-z0-9]+$'.

Cause 2:
1. app = amplify.create_app(name=<unique>)['app'].
2. amplify.create_branch(appId=app['appId'], branchName='main', stage='production') -> expect 400 with the enum set rendered [EXPERIMENTAL, BETA, PULL_REQUEST, PRODUCTION, DEVELOPMENT].

Cause 3:
1. amplify.create_app(name=<unique>, platform='web') -> expect 400 with the enum set rendered [WEB_COMPUTE, WEB_DYNAMIC, WEB].

Cause 4:
1. Create an app and branch 'main'.
2. amplify.start_deployment(appId, branchName='main') with neither jobId nor sourceUrl -> expect 400 'Must specify jobId or sourceUrl for StartDeploymentRequest.'

Cause 5:
1. app = amplify.create_app(name=<unique>)['app'].
2. amplify.create_branch(appId=app['appId'], branchName='main site') -> expect 400 BadRequestException with the message above; observed 200 and a stored branch.
3. amplify.list_branches(appId=app['appId']) must not list 'main site'.

Cause 6:
1. amplify.create_deployment(appId=<an appId that exists but has no such branch, or no such app>, branchName='main') -> expect HTTP 404 with the not-found error type; observed 400 BadRequestException with the same message.

Cause 7:
1. Create an app and branch 'main'; d1 = amplify.create_deployment(appId, 'main') (job left unfinished).
2. amplify.create_deployment(appId, 'main') again -> expect 400 BadRequestException 'The last job(deployment) <d1 jobId> was not finished, please wait until it finish or use StopJob operation to cancel it before creating a new deployment.'; observed 200 with a new jobId.
3. After amplify.stop_job(appId, 'main', d1['jobId']), the identical create_deployment must succeed.
