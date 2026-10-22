Issue-Source: trace-sweep-divergence
Issue-Trace: evs-account-settings-roundtrip
Issue-Step: task#4
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: evs-account-settings-roundtrip evs-create-environment-input-validation evs-service-catalog-discovery
Issue-Shape-Id: com.amazonaws.evs#AmazonElasticVMwareService
Issue-Key: evs|PutAccountSettings|4xx->4xx|$.message
Issue-Key: evs|CreateEnvironment|4xx->4xx|$.message
Issue-Key: evs|ListEnvironments|4xx->4xx|$.message
Issue-Commit: 360c4c1618594432a64f84ba94d0fa368ed8f619

# ValidationException message diverges from AWS constraint-violation format

> _Note: this issue was identified against the code base at commit `360c4c1618594432a64f84ba94d0fa368ed8f619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAccountSettings, CreateEnvironment, ListEnvironments

## Description
Shape-constraint violations are detected and answered with the right status (400) and error (ValidationException), but the emulator's shared constraint-violation message RENDERER builds the text by a different rule than the service: it echoes the offending value, keeps the regex anchors, pluralises and joins multiple violations differently, and reports enum violations at a different path and nesting. The message here is a requirement because it is constructed from the request and the modelled constraint, so the RULE (not the recorded text) is what must match; three operations show the same renderer defect.

Shared AWS basis: these are @pattern / enum-value violations on modelled members (Smithy: AccountSetting.value @length 1..128 @pattern ^[a-zA-Z0-9]+$; CreateEnvironmentRequest.vpcId @pattern ^vpc-[a-f0-9]{8}([a-f0-9]{9})?$ and serviceAccessSubnetId @pattern ^subnet-[a-f0-9]{8}([a-f0-9]{9})?$; ListEnvironmentsRequest.state -> EnvironmentStateList of enum EnvironmentState values [CREATING, CREATED, DELETING, DELETED, CREATE_FAILED]; all three operations bind ValidationException, @error(client), @httpError(400)). The recorded messages show the construction rule: header '<n> validation error detected: ' for one violation and '<n> validation error(s) detected: ' for several; each violation rendered as "Value at '<memberPath>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" with the offending value NOT echoed and the pattern printed WITHOUT its ^ and $ anchors; multiple violations joined with ',' and ordered by member path, not by declaration order; an enum violation reported against the LIST member path with the nested wrapper 'Member must satisfy constraint: [Member must satisfy enum value set: [...]]'. The order of the values inside that enum set differs between recordings and is an example, not a requirement.

Cause 1 of 3: PutAccountSettings, settings[].value pattern violation (fixture evs-account-settings-roundtrip, step task#4).
AWS: 400 ValidationException, message "1 validation error detected: Value at 'settings.1.member.value' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9]+".
Emulator: 400 ValidationException but message "1 validation error detected: Value 'not-a-number' at 'settings.1.member.value' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9]+$" — echoes the rejected value and keeps the ^...$ anchors.

Cause 2 of 3: CreateEnvironment, two simultaneous pattern violations on vpcId and serviceAccessSubnetId (fixture evs-create-environment-input-validation, step task#2; same cluster also covers task#3).
AWS: 400 ValidationException, message "2 validation error(s) detected: Value at 'serviceAccessSubnetId' failed to satisfy constraint: Member must satisfy regular expression pattern: subnet-[a-f0-9]{8}([a-f0-9]{9})?,Value at 'vpcId' failed to satisfy constraint: Member must satisfy regular expression pattern: vpc-[a-f0-9]{8}([a-f0-9]{9})?" — '(s)' header form, violations ordered by member path, joined with ',', no values, no anchors.
Emulator: "2 validation errors detected: Value 'vpc-zzzzzzzz' at 'vpcId' ... ^vpc-...$; Value 'subnet-zzzzzzzz' at 'serviceAccessSubnetId' ... ^subnet-...$" — 'errors' instead of 'error(s)', input declaration order instead of member-path order, '; ' instead of ',', plus the echoed values and anchors.

Cause 3 of 3: ListEnvironments, enum violation in the state list (fixture evs-service-catalog-discovery, step task#4).
AWS: 400 ValidationException, message "1 validation error detected: Value at 'state' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [...]]" — reported against the list member 'state' with the nested constraint wrapper.
Emulator: "1 validation error detected: Value 'NOT_A_REAL_STATE' at 'state.1.member' failed to satisfy constraint: Member must satisfy enum value set: [...]" — echoes the value, indexes into 'state.1.member', and drops the outer 'Member must satisfy constraint: [...]' wrapper.

Why: the status and error code are right, so this does not break control flow, but the message is the only place the service reports WHICH member failed and WHY. Tooling that parses the text (surfacing the failed field to a user, asserting on golden error output, scraping the member path out of the message) reads a different path for enum violations ('state.1.member' vs 'state'), a different separator and count-phrase when several members fail, and a pattern string that still carries anchors — so it extracts the wrong field or fails to match at all. Echoing the rejected value also leaks request content into an error the service deliberately renders without it.

Fix: the shared constraint-violation message renderer used by the evs input validation (not the individual handlers): suppress the offending value, strip the leading ^ and trailing $ from a @pattern before printing it, use the '<n> validation error(s) detected' header and ',' joiner for multi-violation messages ordered by member path, and report enum-value violations against the containing member path with the nested 'Member must satisfy constraint: [Member must satisfy enum value set: [...]]' wrapper.

Traces: evs-account-settings-roundtrip#task#4 (PutAccountSettings), evs-create-environment-input-validation#task#2 and #task#3 (CreateEnvironment), evs-service-catalog-discovery#task#4 (ListEnvironments).

## Evidence
All three steps returned the correct status 400 and error; only $.message diverged.
evs-account-settings-roundtrip#task#4: expected "1 validation error detected: Value at 'settings.1.member.value' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9]+", got "1 validation error detected: Value 'not-a-number' at 'settings.1.member.value' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9]+$".
evs-create-environment-input-validation#task#2 (and #task#3): expected "2 validation error(s) detected: Value at 'serviceAccessSubnetId' ... pattern: subnet-[a-f0-9]{8}([a-f0-9]{9})?,Value at 'vpcId' ... pattern: vpc-[a-f0-9]{8}([a-f0-9]{9})?", got "2 validation errors detected: Value 'vpc-zzzzzzzz' at 'vpcId' ... pattern: ^vpc-[a-f0-9]{8}([a-f0-9]{9})?$; Value 'subnet-zzzzzzzz' at 'serviceAccessSubnetId' ... pattern: ^subnet-[a-f0-9]{8}([a-f0-9]{9})?$".
evs-service-catalog-discovery#task#4: expected "1 validation error detected: Value at 'state' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [...]]", got "1 validation error detected: Value 'NOT_A_REAL_STATE' at 'state.1.member' failed to satisfy constraint: Member must satisfy enum value set: [...]" (the order of values inside the set is an example, not part of the claim).

## Steps to Reproduce
1. Boot the emulator and replay fixtures evs-account-settings-roundtrip, evs-create-environment-input-validation and evs-service-catalog-discovery.
2. evs-account-settings-roundtrip#task#4 calls PutAccountSettings with settings[1].value = 'not-a-number' (violates ^[a-zA-Z0-9]+$).
3. evs-create-environment-input-validation#task#2 calls CreateEnvironment with vpcId='vpc-zzzzzzzz' and serviceAccessSubnetId='subnet-zzzzzzzz' (both violate their patterns).
4. evs-service-catalog-discovery#task#4 calls ListEnvironments with state=['NOT_A_REAL_STATE'].
5. Each returns 400 ValidationException with a message built by the emulator's rule (value echoed, anchors kept, '; ' joiner, 'state.1.member' path) rather than the service's.
