Issue-Source: trace-sweep-divergence
Issue-Trace: synthetics-canary-create-validation-errors
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: synthetics-canary-create-validation-errors synthetics-group-canary-association-errors synthetics-group-lifecycle-and-tags
Issue-Shape-Id: com.amazonaws.synthetics#Synthetics
Issue-Key: synthetics|CreateCanary|4xx->4xx|$.Message
Issue-Key: synthetics|AssociateResource|4xx->4xx|$.Message
Issue-Key: synthetics|CreateGroup|4xx->4xx|$.Message
Issue-Commit: dbd8b6da40b3283b718ba9768f41bda1f397bd18

# synthetics: client-error messages are emulator-authored instead of AWS's wording (3 causes: CreateCanary pattern validation, AssociateResource canary-not-found, CreateGroup name conflict)

> _Note: this issue was identified against the code base at commit `dbd8b6da40b3283b718ba9768f41bda1f397bd18`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCanary

## Description
Three distinct causes in the emulated CloudWatch Synthetics service, all of the same family: the HTTP status and the error code are right, but the error MESSAGE is rewritten by the emulator rather than produced by AWS's rule for that error class. All three must be closed before this issue is resubmitted. (The open behavioral issue 20261029T184613Z-synthetics-4-net-new-root-causes-duplicate-associateresource.md reports the same family for StopCanary/DeleteCanary/StartCanary ConflictException text; it does NOT cover any of the three operations below, and these are wire divergences from a different corpus.)

=== Cause 1 of 3: CreateCanary's pattern ValidationException quotes the regex WITHOUT the anchors the model declares ===
AWS: Smithy: CreateCanary.Name -> CanaryName, @length min 1 max 255, @pattern "^[0-9a-z_\\-]+$"; modeled error ValidationException (httpError 400, errorKind client, documented "A parameter could not be validated."). A name that fails the member's pattern is refused with HTTP 400 ValidationException whose message is the standard AWS constraint-violation template: fixed text "1 validation error detected: Value '" + "' at '" + "' failed to satisfy constraint: Member must satisfy regular expression pattern: "; the variable parts are the rejected value as supplied in the request, the lowerCamel member name of the failing input member (here `name`), and the pattern AS THE MODEL DECLARES IT — i.e. including the leading `^` and trailing `$`.
Emulator: the same status, code and template, but the pattern it interpolates is the un-anchored body of the regex. Replay saw $.Message = "1 validation error detected: Value 'Invalid_Canary_NAME!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-z_\\-]+" where AWS answers "... regular expression pattern: ^[0-9a-z_\\-]+$". (The rejected value shown is this fixture's request value, not part of the requirement; the requirement is that the pattern is echoed verbatim from the model, anchors included.)
Why: clients and test suites that assert on the constraint message, and tooling that re-compiles the echoed pattern to explain the failure to a user, get a regex that is not the modeled one — an un-anchored pattern matches any string containing a legal run, so a consumer re-validating with the echoed pattern accepts names AWS rejects.
Fix: the shared constraint-violation message builder used by the centralized validate stage of the request walk (the @pattern violation formatter) — it must render the member's pattern trait verbatim rather than a stripped/un-anchored form; CreateCanary's own handler is not the defect site.
Traces: synthetics-canary-create-validation-errors#task#2.

=== Cause 2 of 3: AssociateResource's ResourceNotFoundException for a missing canary uses the fixed, identifier-free sentence instead of echoing the requested ARN ===
AWS: Smithy: AssociateResource (PATCH /group/{GroupIdentifier}/associate) models ResourceNotFoundException (httpError 404, errorKind client, "One of the specified resources was not found."); the model documents no message text, so recorded behavior: a ResourceArn that resolves to no canary is refused with HTTP 404 ResourceNotFoundException and the message form "Canary does not exist: <ResourceArn>" — fixed text "Canary does not exist: " followed by the canary ARN exactly as the request supplied it. This is the same construction the emulator already implements for groups (state::group_not_found -> "Group does not exist: <identifier>"), so the canary side of AssociateResource echoes the identifier too.
Emulator: answers the right status and code with the service-wide constant state::CANARY_NOT_FOUND = "Canary not found." (services/service-com-amazonaws-synthetics-synthetics/src/state.rs, documented there as "request-independent ... a constant here, not built from the lookup key"). Replay saw $.Message = 'Canary not found.' where AWS answered 'Canary does not exist: <canary arn>'. The ARN in the recorded fixture is a per-run value and is not itself the requirement; the requirement is that the message is built from the request's own ResourceArn.
Why: a caller that associates a canary by ARN and parses the refusal cannot tell WHICH arn was unresolvable (relevant when associating several), and any assertion on AWS's wording fails; the emulator is also internally inconsistent, echoing the identifier for groups but not for canaries on the same operation.
Fix: the canary-lookup failure path of the AssociateResource handler (src/ops/associate_resource.rs) plus a canary_not_found(identifier) helper in src/state.rs alongside the existing group_not_found, used by the operations whose recorded wording echoes the identifier; CANARY_NOT_FOUND stays for the operations whose recorded wording is the fixed sentence.
Traces: synthetics-group-canary-association-errors#task#3.

=== Cause 3 of 3: CreateGroup's duplicate-name ConflictException message is reworded and drops the quotes around the name ===
AWS: Smithy: CreateGroup (POST /group) models ConflictException (httpError 409, errorKind client, "A conflicting operation is already in progress."); CreateGroupRequest.Name is documented "The names for all groups in your account, across all Regions, must be unique." Recorded behavior: creating a group whose name is already held is refused with HTTP 409 ConflictException and the message form "Group with the name '<Name>' already exists" — fixed text "Group with the name '" and "' already exists", with the single variable part the request's Name, SINGLE-QUOTED.
Emulator: same status and code, different text: src/ops/create_group.rs builds format!("Group with name {} already exists", stored.name), i.e. it drops "the" and the quotes around the name. Replay saw $.Message = 'Group with name sim2real-probe-synthetics-group-5c6bf8ef already exists' where AWS answers "Group with the name 'sim2real-probe-synthetics-group-5c6bf8ef' already exists" (that group name is this fixture's value, not part of the template).
Why: group names may contain any Unicode characters including spaces, so the quoting is what makes the name delimitable; a client that matches AWS's sentence, or parses the name back out of it, does not recognise the emulator's refusal.
Fix: the ConflictException construction in the duplicate-name branch of the CreateGroup handler (src/ops/create_group.rs).
Traces: synthetics-group-lifecycle-and-tags#task#4.

## Evidence
Corpus simparity-wire-aws-v1, 33 steps, 19 passed, score 57.6%. All three causes are status-and-code-correct, message-text-wrong mismatches.

Cause 1 — fixture synthetics-canary-create-validation-errors, step task#2, CreateCanary, expected 400 / actual 400:
  $.Message: value_mismatch
    expected "1 validation error detected: Value 'Invalid_Canary_NAME!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-z_\\-]+$"
    got      "1 validation error detected: Value 'Invalid_Canary_NAME!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-z_\\-]+"
  Model: CanaryName @pattern "^[0-9a-z_\\-]+$" — the anchors are part of the declared pattern.

Cause 2 — fixture synthetics-group-canary-association-errors, step task#3, AssociateResource, expected 404 / actual 404:
  $.Message: value_mismatch
    expected 'Canary does not exist: arn:aws:synthetics:us-west-2:000000000000:canary:sim2real-probe-synthetics-assoc-canary-9c17c16d'
    got      'Canary not found.'
  Source: src/state.rs CANARY_NOT_FOUND ("Canary not found.") returned by the canary lookup in src/ops/associate_resource.rs; compare src/state.rs group_not_found() which already echoes the identifier.

Cause 3 — fixture synthetics-group-lifecycle-and-tags, step task#4, CreateGroup, expected 409 / actual 409:
  $.Message: value_mismatch
    expected "Group with the name 'sim2real-probe-synthetics-group-5c6bf8ef' already exists"
    got      'Group with name sim2real-probe-synthetics-group-5c6bf8ef already exists'
  Source: src/ops/create_group.rs, duplicate-name branch: format!("Group with name {} already exists", stored.name).

The canary ARN, group name and rejected canary name quoted above are per-run fixture values; only the surrounding fixed text and the rule that produces the variable parts are the requirement.

## Steps to Reproduce
Cause 1: CreateCanary with Name="Invalid_Canary_NAME!" (any value violating CanaryName's @pattern) and otherwise valid Code/ArtifactS3Location/ExecutionRoleArn/Schedule/RuntimeVersion -> 400 ValidationException; read Message and compare the echoed regular expression: the emulator prints the pattern without its ^ and $ anchors.

Cause 2: CreateGroup(Name=g); AssociateResource(GroupIdentifier=g, ResourceArn=arn:aws:synthetics:<region>:<account>:canary:<a-canary-that-does-not-exist>) -> 404 ResourceNotFoundException with Message 'Canary not found.'; AWS answers 'Canary does not exist: <that same ResourceArn>'.

Cause 3: CreateGroup(Name=g); CreateGroup(Name=g) again -> 409 ConflictException with Message 'Group with name g already exists'; AWS answers "Group with the name 'g' already exists".
