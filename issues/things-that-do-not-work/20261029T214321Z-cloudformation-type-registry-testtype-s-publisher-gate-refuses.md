Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.type-registry-not-found-errors cloudformation.set-type-configuration-invalid-json-rejected cloudformation.list-type-registrations-invalid-filter-rejected cloudformation.list-types-invalid-visibility-filter-rejected
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|TestType|CFNRegistryException
Issue-Key: cloudformation.set-type-configuration-invalid-json-rejected|run#13|AssertionError|assert message_garbage == expected, f"expected message {expected!r}, got {message_garbage!r}"
Issue-Key: cloudformation|ListTypeRegistrations|ValidationError
Issue-Key: cloudformation|ListTypes|ValidationError
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# cloudformation type registry: TestType's publisher gate refuses a registered publisher, SetTypeConfiguration's pattern message carries regex anchors AWS omits, and the enum-filter ValidationError text diverges (3 causes)

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestType

## Description
REOPENS 20261029T131601Z-cloudformation-testtype-never-applies-the-publisher-registration-gate.md: its fix left the call refused. That issue added TestType's publisher pre-flight; the gate now fires for a caller the check has made a registered publisher, so TestType can no longer reach the registry's not-found refusal at all. Three net-new root causes of service `cloudformation`, grounded in four failing behavioral checks. Each cause is independent and must be closed separately.

=== Cause 1 of 3: TestType's publisher gate is applied unconditionally ===
AWS: Smithy models TestType with errors `CFNRegistryException` (errorKind client, httpError 400) and `TypeNotFoundException` (client, httpError 404, awsQueryError TypeNotFoundException/404). Recorded behavior: the publisher pre-flight refuses only an account that is NOT a verified extension publisher (400 CFNRegistryException, fixed message `You must register as a publisher with CloudFormation to call test-type.`). For an account that IS a registered publisher, TestType proceeds to resolve the named extension and, when nothing is registered under that name, refuses with HTTP 404, Error.Code `TypeNotFoundException`, Error.Message `The type '<Namespace::Service::Resource>' cannot be found.` — fixed text `The type '` + `' cannot be found.` with the three-part extension type name from the request as the only variable part.
Emulator: the same call still answers 400 CFNRegistryException `You must register as a publisher with CloudFormation to call test-type.` — the gate is reached regardless of the caller's publisher state, so the 404 TypeNotFoundException branch is unreachable. Hypothesis: the gate does not consult the state RegisterPublisher writes and is hard-refusing.
Why: a client that correctly follows the emulator's own first refusal (register as a publisher) gets the identical refusal afterwards, with no way to make progress and no signal that the extension name is the real problem. Error-taxonomy code also sees 400/CFNRegistryException where AWS sends 404/TypeNotFoundException, so HTTP-status-based retry layers branch differently.
Fix: the TestType handler in the cloudformation crate's type-registry ops (services/service-com-amazonaws-cloudformation-cloudformation/src/ops/test_type.rs) — the gate must read the calling account's publisher registration and pass a registered publisher through to the registry resolution, which then produces the modelled TypeNotFoundException.
Traces: cloudformation.type-registry-not-found-errors.

=== Cause 2 of 3: the typeArn pattern-constraint message includes the ^…$ anchors AWS strips ===
AWS: Smithy models SetTypeConfiguration.TypeArn (target TypeArn, @length 0..1024, pattern `^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:([0-9]{12})?:type/.+$`). Recorded behavior for a value that violates it: HTTP 400, Error.Code `ValidationError`, Error.Message `1 validation error detected: Value '<value>' at 'typeArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:([0-9]{12})?:type/.+` — AWS renders the pattern WITHOUT the leading `^` and trailing `$`. Fixed text is the whole sentence; the variable parts are the rejected value (echoed from the request) and the member name.
Emulator: identical except that it prints the modelled pattern verbatim, anchors included (`^arn:aws…type/.+$`).
Why: constraint messages are the documented, asserted contract for input validation; a client or test comparing them character-for-character against AWS fails on every member whose pattern is anchored, which is most of this service's ARN members — so this is a shared renderer defect, not a one-member typo.
Fix: the constraint-violation message renderer in the cloudformation service's input validation layer — strip the anchors when interpolating a @pattern into the 'Member must satisfy regular expression pattern:' message.
Traces: cloudformation.set-type-configuration-invalid-json-rejected.

=== Cause 3 of 3: the enum-constraint ValidationError text for the type-registry list filters is not AWS's ===
AWS: Smithy models ListTypeRegistrations.RegistrationStatusFilter (target RegistrationStatus, enum [COMPLETE, IN_PROGRESS, FAILED]) and ListTypes.Visibility (target Visibility, enum [PUBLIC, PRIVATE]); both operations declare `CFNRegistryException` (client, 400). The message AWS produces for an out-of-enum value is recorded behavior only, and the checks assert equality against it — the emulator's text is not it.
Emulator: answers ValidationError with `1 validation error detected: Value 'NOT_A_REAL_STATUS' at 'registrationStatusFilter' failed to satisfy constraint: Member must satisfy enum value set: [COMPLETE, IN_PROGRESS, FAILED]` (and the analogous `… at 'visibility' … [PUBLIC, PRIVATE]`), i.e. it renders the enum members in the model's declaration order. Both checks report this message as unexpected.
Why: these are the only signals a caller gets for a bad filter value; a client or taxonomy test comparing the refusal against AWS's recorded message diverges on every such member.
Fix: the enum-constraint message rendering for these members in the cloudformation service's input validation layer (the same renderer as Cause 2, enum branch) — reproduce AWS's recorded wording and value-set rendering rather than the model's declaration order.
Traces: cloudformation.list-type-registrations-invalid-filter-rejected, cloudformation.list-types-invalid-visibility-filter-rejected.

## Evidence
Cause 1 — cloudformation.type-registry-not-found-errors [mismatch], check line 86:
  assert error == (code, message)
  AssertionError: expected ('TypeNotFoundException', "The type 'Sim2realBehav::Probe::3df1a76f951f' cannot be found."), got ('CFNRegistryException', 'You must register as a publisher with CloudFormation to call test-type.')
  (the trailing token in the type name is a per-run value filling the message's one variable slot)
  Smithy: TestType errors = CFNRegistryException (client, httpError 400, awsQueryError CFNRegistryException/400) and TypeNotFoundException (client, httpError 404, awsQueryError TypeNotFoundException/404).

Cause 2 — cloudformation.set-type-configuration-invalid-json-rejected [mismatch]:
  AssertionError: expected message "1 validation error detected: Value 'not-a-cloudformation-type-arn-Sim2Realx49652c315684-Behav-GhostWidget' at 'typeArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:([0-9]{12})?:type/.+", got "1 validation error detected: Value 'not-a-cloudformation-type-arn-Sim2Realx49652c315684-Behav-GhostWidget' at 'typeArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:([0-9]{12})?:type/.+$"
  The only difference is the leading '^' and trailing '$'. Smithy: SetTypeConfiguration.TypeArn @length 0..1024, pattern ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:([0-9]{12})?:type/.+$; errors CFNRegistryException (400), TypeNotFoundException (404).

Cause 3 — cloudformation.list-type-registrations-invalid-filter-rejected [mismatch], check line 32:
  AssertionError: unexpected ValidationError message "1 validation error detected: Value 'NOT_A_REAL_STATUS' at 'registrationStatusFilter' failed to satisfy constraint: Member must satisfy enum value set: [COMPLETE, IN_PROGRESS, FAILED]"
  cloudformation.list-types-invalid-visibility-filter-rejected [mismatch]:
  AssertionError: unexpected ValidationError message "1 validation error detected: Value 'NOT_A_REAL_VISIBILITY' at 'visibility' failed to satisfy constraint: Member must satisfy enum value set: [PUBLIC, PRIVATE]"
  Smithy: RegistrationStatus enum values [COMPLETE, IN_PROGRESS, FAILED]; Visibility enum values [PUBLIC, PRIVATE]; ListTypeRegistrations errors = CFNRegistryException (client, 400).

## Steps to Reproduce
Cause 1:
1. Call RegisterPublisher so the calling account is a registered extension publisher.
2. Call cfn.test_type(Type='RESOURCE', TypeName='Sim2realBehav::Probe::<token>') for a type registered nowhere.
3. Observe 400 CFNRegistryException 'You must register as a publisher with CloudFormation to call test-type.'. AWS answers 404 TypeNotFoundException "The type 'Sim2realBehav::Probe::<token>' cannot be found."
4. Control that must keep working: the same call from an account that has NOT registered must still be refused 400 CFNRegistryException with that fixed sentence.

Cause 2:
1. cfn.set_type_configuration(TypeArn='not-a-cloudformation-type-arn-X', Configuration='{}').
2. Compare the Error.Message's rendered pattern against AWS's: the emulator keeps '^' and '$'.

Cause 3:
1. cfn.list_type_registrations(RegistrationStatusFilter='NOT_A_REAL_STATUS') and cfn.list_types(Visibility='NOT_A_REAL_VISIBILITY').
2. Compare Error.Message against AWS's recorded message for an out-of-enum value on those members.
3. Re-replay: cloudformation.type-registry-not-found-errors, cloudformation.set-type-configuration-invalid-json-rejected, cloudformation.list-type-registrations-invalid-filter-rejected, cloudformation.list-types-invalid-visibility-filter-rejected.
