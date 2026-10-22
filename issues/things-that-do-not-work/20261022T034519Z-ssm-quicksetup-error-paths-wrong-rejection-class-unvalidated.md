Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-quicksetup-not-found-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-quicksetup-not-found-error-paths ssm-quicksetup-validation-error-paths ssm-quicksetup-create-configuration-manager-errors
Issue-Shape-Id: com.amazonaws.ssmquicksetup#QuickSetup
Issue-Key: ssm-quicksetup|GetConfigurationManager|4xx->4xx|$.message
Issue-Key: ssm-quicksetup|GetConfigurationManager|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: ssm-quicksetup|GetConfigurationManager|4xx->4xx|status
Issue-Key: ssm-quicksetup|UpdateConfigurationManager|4xx->4xx|$.message
Issue-Key: ssm-quicksetup|UpdateConfigurationManager|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: ssm-quicksetup|UpdateConfigurationManager|4xx->4xx|status
Issue-Key: ssm-quicksetup|DeleteConfigurationManager|4xx->4xx|$.message
Issue-Key: ssm-quicksetup|DeleteConfigurationManager|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: ssm-quicksetup|DeleteConfigurationManager|4xx->4xx|status
Issue-Key: ssm-quicksetup|CreateConfigurationManager|4xx->2xx|$.Message
Issue-Key: ssm-quicksetup|CreateConfigurationManager|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: ssm-quicksetup|CreateConfigurationManager|4xx->2xx|status
Issue-Key: ssm-quicksetup|CreateConfigurationManager|4xx->4xx|$.message
Issue-Key: ssm-quicksetup|ListConfigurations|4xx->4xx|$.Message
Issue-Key: ssm-quicksetup|ListConfigurationManagers|4xx->4xx|$.Message
Issue-Commit: 37e4b54b7f2f231778aa0090f22742466dbbad1b

# ssm-quicksetup error paths: wrong rejection class, unvalidated CreateConfigurationManager, and wrong constraint/not-found message rules

> _Note: this issue was identified against the code base at commit `37e4b54b7f2f231778aa0090f22742466dbbad1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConfigurationManager

## Description
Every negative path of the ssm-quicksetup control plane answers with the wrong rejection: the manager-ARN operations return a resource/validation error where AWS refuses at the front door, CreateConfigurationManager accepts a definition AWS rejects, and the messages that do reach the client are built by the wrong rule. 8 causes; all must be closed.

Cause 1 of 8: GetConfigurationManager on a well-formed ManagerArn that names no manager answers 404 ResourceNotFoundException instead of the recorded 401 UnauthorizedException.
AWS: recorded behavior only - the model binds ResourceNotFoundException (httpError 404) to this operation but does not model UnauthorizedException at all, so the 401 comes from the ssm-quicksetup REST front door refusing the request during authorization of the target ARN, before any resource lookup runs. The recording shows status 401, header x-amzn-errortype: UnauthorizedException, and the fixed front-door body $.message = 'Unauthorized' (lowercase member, unlike the service's own modeled Message).
Emulator: status 404, header x-amzn-errortype: ResourceNotFoundException, and no $.message member at all.
Why: a client cannot distinguish 'you may not touch this resource' from 'this resource does not exist'. 401 is non-retryable and drives credential/permission re-acquisition, while 404 drives create-if-missing logic - so a client that polls for a manager it is not entitled to will loop or silently re-create instead of surfacing an authorization failure.
Fix: the request-authorization stage that fronts the {ManagerArn} routes in the ssm-quicksetup service crate, which must refuse an unauthorized/unknown target ARN with 401 UnauthorizedException before the handler's resource lookup.
Traces: ssm-quicksetup-not-found-error-paths#task#1

Cause 2 of 8: UpdateConfigurationManager has the same defect on the same input.
AWS: recorded behavior only - PUT /configurationManager/{ManagerArn} with an ARN naming no manager answers 401, x-amzn-errortype: UnauthorizedException, $.message = 'Unauthorized'.
Emulator: 404 with x-amzn-errortype: ResourceNotFoundException and no $.message.
Why: as cause 1, and worse on a write path - a caller retrying an update after a 404 will attempt a create it is not authorized for.
Fix: same front-door authorization stage, applied to the UpdateConfigurationManager route.
Traces: ssm-quicksetup-not-found-error-paths#task#2

Cause 3 of 8: DeleteConfigurationManager has the same defect on the same input.
AWS: recorded behavior only - DELETE /configurationManager/{ManagerArn} with an ARN naming no manager answers 401, x-amzn-errortype: UnauthorizedException, $.message = 'Unauthorized'.
Emulator: 404 with x-amzn-errortype: ResourceNotFoundException and no $.message.
Why: as cause 1; a delete that reports 404 reads as 'already gone' and lets a cleanup routine declare success on a resource it never had access to.
Fix: same front-door authorization stage, applied to the DeleteConfigurationManager route.
Traces: ssm-quicksetup-not-found-error-paths#task#3

Cause 4 of 8: a syntactically invalid ManagerArn is pattern-validated instead of being refused at the front door.
AWS: recorded behavior only - GET /configurationManager/not-an-arn answers 401 with $.message = 'Unauthorized'. The recording establishes the ordering: front-door authorization of the ARN path label runs BEFORE the member's pattern constraint, so a malformed ARN never reaches validation. (Smithy: GetConfigurationManagerInput.ManagerArn is @httpLabel, @required, length min 1, pattern ^arn:aws:ssm-quicksetup:([^:]+):(\d{12}):configuration-manager/<uuid>$ - the constraint exists, but the recording shows it is not what answers this request.)
Emulator: status 400 with x-amzn-errortype: ValidationException and a constraint message echoing the rejected value and the anchored pattern.
Why: a client that sends a mistyped or stale ARN is told its input is malformed when AWS tells it the call was not authorized; retry classification, metrics and alarm routing all differ between 400 and 401.
Fix: same front-door authorization stage - it must precede the GetConfigurationManager input validation, not follow it.
Traces: ssm-quicksetup-validation-error-paths#task#1

Cause 5 of 8: CreateConfigurationManager accepts a configuration definition that names no deployment target and returns 200 where AWS returns 400 ValidationException.
AWS: POST /configurationManager with a ConfigurationDefinitions entry whose Parameters map defines neither TargetAccounts nor TargetOrganizationalUnits is rejected with status 400, header x-amzn-errortype: ValidationException, and $.Message = 'Either TargetAccounts or TargetOrganizationalUnits must be defined in parameters.' This is a cross-member rule the per-member constraints cannot express; the model states its basis (Smithy: ConfigurationDefinitionInput.Parameters is @required and its documentation lists TargetOrganizationalUnits / TargetAccounts plus TargetRegions as required parameters for each Quick Setup type), and the recording supplies the status, error code and the fixed sentence.
Emulator: status 200 - it mints a configuration manager and returns a ManagerArn, with no $.Message and no x-amzn-errortype header. Three recorded steps that each expect this rejection all succeed instead.
Why: a client's negative test passes and, worse, a manager is created that targets nothing; downstream reads then describe a manager AWS would never have accepted, so the emulator's state diverges from the recording for every later step in the scenario.
Fix: CreateConfigurationManager input validation in the ssm-quicksetup service crate - it needs the cross-member target rule over each ConfigurationDefinitions[].Parameters entry, raising ValidationException before any state is written.
Traces: ssm-quicksetup-create-configuration-manager-errors#task#2 (plus the two further steps in that fixture that expect the same rejection)

Cause 6 of 8: the ValidationException constraint message renders the Smithy pattern with its ^ and $ anchors.
AWS: the message is built from the request - '1 validation error detected: Value '<rejected value>' at '<member path>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>' - and AWS renders the BARE pattern, with the leading ^ and trailing $ stripped: for ConfigurationDefinitionInput.Type (Smithy pattern ^[a-zA-Z0-9_\-.:/]{3,200}$) the recorded text ends 'pattern: [a-zA-Z0-9_\-.:/]{3,200}'. Because every part of the sentence is derived from the request and the modeled constraint, the rendering rule is the requirement here, not the one recorded string.
Emulator: emits the anchored form '^[a-zA-Z0-9_\-.:/]{3,200}$'. The same defect shows on GetConfigurationManager's managerArn pattern in cause 4, so it is the shared renderer and not one message.
Why: clients and test harnesses that assert on, parse or log AWS validation text see a pattern that is not the one AWS reports; anything comparing the reported constraint against the service model's own rendering mismatches.
Fix: the ValidationException message renderer shared by ssm-quicksetup's input validation - strip the anchors when formatting a pattern constraint.
Traces: ssm-quicksetup-create-configuration-manager-errors#task#1 (same cluster also covers ssm-quicksetup-validation-error-paths#task#3)

Cause 7 of 8: ListConfigurations' not-found message does not follow AWS's template.
AWS: POST /listConfigurations with a ManagerArn naming no manager answers 404 ResourceNotFoundException (Smithy: ListConfigurations binds ResourceNotFoundException, httpError 404, member Message) and the message is built from the request by interpolating the supplied ARN into the fixed template 'Quick setup manager with arn: <ManagerArn> could not be found.' The template plus the request-supplied ARN is the requirement; the recorded ARN value is only an example.
Emulator: the status and error class are right, but $.Message is 'no configuration manager with ARN <arn> exists' - a different sentence.
Why: the status is usable, but a client that surfaces or matches on the service's not-found text sees wording AWS never emits; it is the same renderer feeding every ssm-quicksetup not-found path, so the divergence is systematic.
Fix: the ResourceNotFoundException message construction in the ListConfigurations handler (and the not-found helper it shares) in the ssm-quicksetup service crate.
Traces: ssm-quicksetup-not-found-error-paths#task#6

Cause 8 of 8: ListConfigurationManagers' invalid-pagination-token message is locally invented.
AWS: POST /listConfigurationManagers with a StartingToken the service did not issue answers 400 ValidationException (Smithy: ListConfigurationManagersInput.StartingToken, length max 1024, pattern ^[A-Za-z0-9+=@_\/\s-]*$; operation binds ValidationException, httpError 400; paginated inputToken=StartingToken/outputToken=NextToken) with the fixed, request-independent sentence $.Message = 'Provided starting token is not valid' - recorded behavior only, but invariant: it interpolates nothing from the request and nothing environment-specific.
Emulator: status and class are right; $.Message is 'StartingToken is not a pagination token issued by ListConfigurationManagers'.
Why: a paginating client that distinguishes an expired/foreign token from other validation failures by the service's own sentence cannot do so against the emulator.
Fix: the StartingToken validation in the ListConfigurationManagers handler - emit the service's fixed invalid-token sentence.
Traces: ssm-quicksetup-validation-error-paths#task#2

No open ssm-quicksetup issue covers any of these; the only existing ssm-quicksetup issue is the (resolved) bootstrap issue.

## Evidence
Cause 1 - ssm-quicksetup-not-found-error-paths#task#1, GetConfigurationManager: expected status 401, $.message 'Unauthorized', x-amzn-errortype 'UnauthorizedException'; actual status 404, $.message missing, x-amzn-errortype 'ResourceNotFoundException'.
Cause 2 - ssm-quicksetup-not-found-error-paths#task#2, UpdateConfigurationManager: expected 401/'Unauthorized'/UnauthorizedException; actual 404/missing/ResourceNotFoundException.
Cause 3 - ssm-quicksetup-not-found-error-paths#task#3, DeleteConfigurationManager: expected 401/'Unauthorized'/UnauthorizedException; actual 404/missing/ResourceNotFoundException.
Cause 4 - ssm-quicksetup-validation-error-paths#task#1, GetConfigurationManager with ManagerArn 'not-an-arn': expected status 401 with $.message 'Unauthorized'; actual status 400, x-amzn-errortype 'ValidationException', $.message "1 validation error detected: Value 'not-an-arn' at 'managerArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws:ssm-quicksetup:([^:]+):(\d{12}):configuration-manager/[0-9a-fA-F]{8}\b-...$".
Cause 5 - ssm-quicksetup-create-configuration-manager-errors#task#2 (and task#3, task#4), CreateConfigurationManager: expected status 400, x-amzn-errortype 'ValidationException', $.Message 'Either TargetAccounts or TargetOrganizationalUnits must be defined in parameters.'; actual status 200, no $.Message, no x-amzn-errortype header.
Cause 6 - ssm-quicksetup-create-configuration-manager-errors#task#1 (also ssm-quicksetup-validation-error-paths#task#3), CreateConfigurationManager: expected $.message "... at 'configurationDefinitions.1.member.type' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_\-.:/]{3,200}"; actual the same sentence ending 'pattern: ^[a-zA-Z0-9_\-.:/]{3,200}$' (status 400 on both sides).
Cause 7 - ssm-quicksetup-not-found-error-paths#task#6, ListConfigurations: expected $.Message 'Quick setup manager with arn: <ManagerArn> could not be found.'; actual 'no configuration manager with ARN <arn> exists' (status 404 on both sides).
Cause 8 - ssm-quicksetup-validation-error-paths#task#2, ListConfigurationManagers: expected $.Message 'Provided starting token is not valid'; actual 'StartingToken is not a pagination token issued by ListConfigurationManagers' (status 400 on both sides).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1); no state-reset failures were reported, so each fixture starts clean.
2. Causes 1-3: with a well-formed but unknown configuration-manager ARN, call GET, then PUT, then DELETE on /configurationManager/{ManagerArn} (fixture ssm-quicksetup-not-found-error-paths, steps task#1, task#2, task#3). Each answers 404 ResourceNotFoundException instead of 401 UnauthorizedException with body {'message': 'Unauthorized'}.
3. Cause 4: GET /configurationManager/not-an-arn (fixture ssm-quicksetup-validation-error-paths, step task#1). Answers 400 ValidationException instead of 401.
4. Cause 5: POST /configurationManager with ConfigurationDefinitions=[{Type: <valid type>, Parameters: {...no TargetAccounts and no TargetOrganizationalUnits...}}] (fixture ssm-quicksetup-create-configuration-manager-errors, step task#2). Returns 200 and a ManagerArn instead of 400 ValidationException.
5. Cause 6: POST /configurationManager with a ConfigurationDefinitions[1].Type of 'ab' (too short for the Type pattern) (same fixture, step task#1). The 400 body's constraint text carries the anchored pattern.
6. Cause 7: POST /listConfigurations with ManagerArn set to an ARN naming no manager (fixture ssm-quicksetup-not-found-error-paths, step task#6). 404 body carries the wrong not-found sentence.
7. Cause 8: POST /listConfigurationManagers with a StartingToken the emulator never issued (fixture ssm-quicksetup-validation-error-paths, step task#2). 400 body carries the wrong invalid-token sentence.
