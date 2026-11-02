Issue-Source: trace-sweep-divergence
Issue-Trace: appstream-asvpce-traj-access-endpoints-request-validation
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appstream-asvpce-traj-access-endpoints-request-validation appstream-asvpce-traj-invented-vpce-id-rejected appstream-asvpce-traj-rerun-create-stack appstream-asvpce-traj-update-stack-attributes-to-delete-enum-rejected
Issue-Shape-Id: com.amazonaws.appstream#PhotonAdminProxyService
Issue-Key: appstream|CreateStack|4xx->2xx|$.__type
Issue-Key: appstream|CreateStack|4xx->2xx|$.message
Issue-Key: appstream|CreateStack|4xx->2xx|status
Issue-Key: appstream|CreateStack|4xx->4xx|$.message
Issue-Key: appstream|CreateStack|4xx->4xx|$.__type
Issue-Key: appstream|UpdateStack|4xx->4xx|$.message
Issue-Commit: 798fb32aa8eb1e75369deb4b48c94746eb1ca248

# appstream CreateStack/UpdateStack request validation diverges in class, wording and coverage

> _Note: this issue was identified against the code base at commit `798fb32aa8eb1e75369deb4b48c94746eb1ca248`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Four net-new causes sharing one fault: the synchronous request-validation layer for the stack write operations. One constraint is not checked at all, one refusal uses the wrong exception class and emits no message, and two refusal messages are rendered by the emulator's generic validator instead of AWS's sentence.

Cause 1 of 4: CreateStack accepts an AccessEndpoints entry whose VpceId is null, answering HTTP 200 instead of refusing the request.
AWS: Smithy: CreateStack.AccessEndpoints -> AccessEndpointList (@length min 1 max 4) of AccessEndpoint{EndpointType required, VpceId String @length min 1}; a null VpceId violates the member's not-null requirement and AWS refuses the request before any state change. Recorded response: HTTP 400, `__type: ValidationException`, `message` = "1 validation error detected: Value null at 'accessEndpoints.<n>.member.vpceId' failed to satisfy constraint: Member must not be null" — fixed text apart from the 1-based index of the offending list entry and the member path, both taken from the request.
Emulator: answers HTTP 200 with no `__type` and no `message` (replay detail: "status: expected 400, got 200; $.__type: missing (expected 'ValidationException'); $.message: missing"), i.e. it creates the stack from a malformed endpoint list.
Why: a client that submits a partially-populated endpoint entry (a very common templating bug) gets a stack that AWS would never have created, with an endpoint entry carrying no VpceId; the error surfaces much later, or not at all, and the emulator's stored state is unreachable on AWS.
Fix: CreateStack's input validation — enforce the required/not-null members of AccessEndpoint before the handler writes state.
Traces: appstream-asvpce-traj-access-endpoints-request-validation#task#2 (cluster also covers appstream-asvpce-traj-invented-vpce-id-rejected#task#6 and appstream-asvpce-traj-rerun-create-stack#task#8).

Cause 2 of 4: the list-length refusal message renders the rejected value as an item count instead of the value itself.
AWS: Smithy: CreateStack.AccessEndpoints @length min 1. An empty list is refused with HTTP 400 ValidationException whose message is "1 validation error detected: Value '[]' at 'accessEndpoints' failed to satisfy constraint: Member must have length greater than or equal to 1" — the fixed text is the whole sentence; the variable parts are the rendered rejected value, the member path and the limit (1, from the model's @length min).
Emulator: same status and class, but the message renders the value as "'0 items'": "1 validation error detected: Value '0 items' at 'accessEndpoints' failed to satisfy constraint: Member must have length greater than or equal to 1".
Why: the message is a documented part of the contract; clients and test suites that match on AWS's text (and users reading the error) see a value rendering AWS never produces, and the generic "<n> items" form shows the emulator renders collection values by cardinality rather than by value everywhere this validator fires.
Fix: the shared validation-message renderer for list-length violations (the service's validation layer), which must render the rejected collection value, not its size.
Traces: appstream-asvpce-traj-access-endpoints-request-validation#task#3 (cluster also covers #task#4).

Cause 3 of 4: CreateStack with an AccessEndpoints VpceId that names no VPC endpoint is refused as ResourceAlreadyExistsException with no message, instead of InvalidParameterValueException with AWS's sentence.
AWS: the endpoint id is resolved against the account's VPC endpoints and an unknown id is refused as an invalid parameter value. Recorded response: HTTP 400, `__type: InvalidParameterValueException`, `message` = "The VPC endpoint <vpce-id> is not valid. Specify a valid VPC endpoint that exists in your AWS account, and try again." — fixed text with the rejected endpoint id interpolated from the request. (Smithy: CreateStack's modeled error list carries the parameter-class refusals InvalidParameterCombinationException/InvalidRoleException and ResourceNotFoundException; the InvalidParameterValueException class and the sentence are grounded in the recording, so for the class name this is recorded behavior.)
Emulator: HTTP 400 but `__type: ResourceAlreadyExistsException` and no `message` at all (replay detail: "$.__type: value_mismatch (expected 'InvalidParameterValueException', got 'ResourceAlreadyExistsException'); $.message: missing").
Why: the error class says the opposite of what happened — a client reading ResourceAlreadyExistsException concludes the stack name is taken and will rename and retry forever, when the actual problem is an unresolvable VPC endpoint id; and with no message there is nothing to diagnose from.
Fix: CreateStack's AccessEndpoints validation — resolve the VpceId and raise the invalid-parameter-value refusal with AWS's sentence, rather than falling through to the name-collision path.
Traces: appstream-asvpce-traj-invented-vpce-id-rejected#task#7.

Cause 4 of 4: UpdateStack's AttributesToDelete enum refusal reports the wrong value, the wrong member path and an incomplete enum value set.
AWS: Smithy: UpdateStack.AttributesToDelete -> StackAttributes, a list whose member targets the StackAttribute enum. A lower-case entry is refused with HTTP 400 ValidationException whose message reports the violation against the LIST: "1 validation error detected: Value '[<entries>]' at 'attributesToDelete' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [<the service's full StackAttribute value list>]]" — fixed text, with the rendered rejected list, the member path and the enum value set as variable parts; the enum set is the service's own and includes values (AGENT_ACCESS_CONFIG_COMPUTER_USE_INTERFACE_ID, WEB_AUTHN_REDIRECTION, AGENT_ACCESS_CONFIG_KMS_KEY_ARN) the emulator's set omits.
Emulator: same status/class, but reports the single member rather than the list, at the indexed path, with the inner constraint clause flattened and an enum value set missing three values: "1 validation error detected: Value 'access_endpoints' at 'attributesToDelete.1.member' failed to satisfy constraint: Member must satisfy enum value set: [...]".
Why: the message is wrong in three independent ways a client can observe — the path it blames, the value it echoes, and the set of legal values it advertises. A caller that reads the advertised set to discover which attributes it may delete is told three real StackAttribute values do not exist, so a valid later request is never attempted; and the missing outer "Member must satisfy constraint: [...]" nesting is how AWS signals a list-member violation rather than a scalar one.
Fix: the UpdateStack AttributesToDelete validation plus the shared enum-violation message renderer — blame the list for a list-member violation, render the rejected list value, and source the advertised value set from the model's StackAttribute enum so no value is dropped.
Traces: appstream-asvpce-traj-update-stack-attributes-to-delete-enum-rejected#task#2 (cluster also covers #task#3).

Note: the previously filed "appstream refusal messages are emulator-authored: four refusals carry the wrong sentence" (20261029T194603Z) is RESOLVED and covers different refusals; it does not cover any of the four causes above.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator.

Cause 1 — CreateStack, fixture appstream-asvpce-traj-access-endpoints-request-validation step task#2:
  status: expected 400, got 200
  $.__type: missing (expected 'ValidationException', got None)
  $.message: missing (expected "1 validation error detected: Value null at 'accessEndpoints.1.member.vpceId' failed to satisfy constraint: Member must not be null", got None)
  divergence_key service=appstream|CreateStack|status=400->200|keys=$.__type,$.message,status; affected_count 3.

Cause 2 — CreateStack, fixture appstream-asvpce-traj-access-endpoints-request-validation step task#3 (400 both sides):
  $.message: value_mismatch
    expected "1 validation error detected: Value '[]' at 'accessEndpoints' failed to satisfy constraint: Member must have length greater than or equal to 1"
    got      "1 validation error detected: Value '0 items' at 'accessEndpoints' failed to satisfy constraint: Member must have length greater than or equal to 1"
  affected_count 2.

Cause 3 — CreateStack, fixture appstream-asvpce-traj-invented-vpce-id-rejected step task#7 (400 both sides):
  $.__type: value_mismatch (expected 'InvalidParameterValueException', got 'ResourceAlreadyExistsException')
  $.message: missing (expected 'The VPC endpoint vpce-bad is not valid. Specify a valid VPC endpoint that exists in your AWS account, and try again.', got None) — 'vpce-bad' is this run's rejected id filling the fixed sentence.
  affected_count 1.

Cause 4 — UpdateStack, fixture appstream-asvpce-traj-update-stack-attributes-to-delete-enum-rejected step task#2 (400 both sides):
  $.message: value_mismatch
    expected "1 validation error detected: Value '[access_endpoints]' at 'attributesToDelete' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [AGENT_ACCESS_CONFIG_COMPUTER_USE_INTERFACE_ID, THEME_NAME, AGENT_ACCESS_CONFIG, ACCESS_ENDPOINTS, CONTENT_REDIRECTION, WEB_AUTHN_REDIRECTION, STORAGE_CONNECTORS, REDIRECT_URL, STORAGE_CONNECTOR_AUTHENTICATION, STORAGE_CONNECTOR_HOMEFOLDERS, AGENT_ACCESS_CONFIG_KMS_KEY_ARN, FEEDBACK_URL, USER_SETTINGS, STORAGE_CONNECTOR_GOOGLE_DRIVE, STREAMING_EXPERIENCE_SETTINGS, STORAGE_CONNECTOR_ONE_DRIVE, IAM_ROLE_ARN, EMBED_HOST_DOMAINS]]"
    got      "1 validation error detected: Value 'access_endpoints' at 'attributesToDelete.1.member' failed to satisfy constraint: Member must satisfy enum value set: [STORAGE_CONNECTORS, STORAGE_CONNECTOR_HOMEFOLDERS, STORAGE_CONNECTOR_GOOGLE_DRIVE, STORAGE_CONNECTOR_ONE_DRIVE, REDIRECT_URL, FEEDBACK_URL, THEME_NAME, USER_SETTINGS, EMBED_HOST_DOMAINS, IAM_ROLE_ARN, ACCESS_ENDPOINTS, STREAMING_EXPERIENCE_SETTINGS, CONTENT_REDIRECTION, AGENT_ACCESS_CONFIG, STORAGE_CONNECTOR_AUTHENTICATION]"
  Differences: blamed path (list vs 'attributesToDelete.1.member'), echoed value (list vs single member), the omitted outer "Member must satisfy constraint: [...]" nesting, and three enum values absent from the emulator's advertised set (AGENT_ACCESS_CONFIG_COMPUTER_USE_INTERFACE_ID, WEB_AUTHN_REDIRECTION, AGENT_ACCESS_CONFIG_KMS_KEY_ARN).
  affected_count 2.

Smithy evidence: CreateStack.AccessEndpoints -> AccessEndpointList @length min 1 max 4; AccessEndpoint {EndpointType required enum, VpceId String @length min 1}; UpdateStack.AttributesToDelete -> StackAttributes (list of StackAttribute enum); CreateStack errors include InvalidParameterCombinationException (400), ResourceAlreadyExistsException (400), ResourceNotFoundException.

## Steps to Reproduce
1. git clone the project repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: replay fixture appstream-asvpce-traj-access-endpoints-request-validation, step task#2 — CreateStack whose AccessEndpoints second entry has a null VpceId; observe HTTP 200 instead of 400 ValidationException.
4. Cause 2: same fixture, step task#3 — CreateStack with AccessEndpoints: []; observe the message renders the value as '0 items' instead of '[]'.
5. Cause 3: replay fixture appstream-asvpce-traj-invented-vpce-id-rejected, step task#7 — CreateStack whose AccessEndpoints VpceId names no VPC endpoint; observe __type ResourceAlreadyExistsException and no message, instead of InvalidParameterValueException with the 'The VPC endpoint <id> is not valid...' sentence.
6. Cause 4: replay fixture appstream-asvpce-traj-update-stack-attributes-to-delete-enum-rejected, step task#2 — UpdateStack with AttributesToDelete ['access_endpoints']; compare the ValidationException message's blamed path, echoed value and advertised enum set with the recorded one.
