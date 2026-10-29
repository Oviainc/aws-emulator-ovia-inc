Issue-Source: trace-sweep-divergence
Issue-Trace: evs-catalog-get-versions
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: evs-catalog-get-versions evs-create-environment-input-validation evs-service-catalog-discovery
Issue-Shape-Id: com.amazonaws.evs#AmazonElasticVMwareService
Issue-Key: evs|GetVersions|2xx->2xx|$.instanceTypeEsxVersions[instanceType="i4i.metal"].esxVersions[*]
Issue-Key: evs|GetVersions|2xx->2xx|$.instanceTypeEsxVersions[instanceType="i7i.metal-24xl"].esxVersions[*]
Issue-Key: evs|GetVersions|2xx->2xx|$.instanceTypeEsxVersions[instanceType="i7i.metal-48xl"].esxVersions[*]
Issue-Key: evs|GetVersions|2xx->2xx|$.vcfVersions[*]
Issue-Key: evs|CreateEnvironment|4xx->4xx|$.fieldList[*]
Issue-Key: evs|CreateEnvironment|4xx->4xx|$.fieldList[*].message
Issue-Key: evs|CreateEnvironment|4xx->4xx|$.message
Issue-Key: evs|ListEnvironments|4xx->4xx|$.message
Issue-Commit: 6c894fcf9edc6b5b7d1e0a3892ed526add3b28c2

# evs wire responses diverge on GetVersions catalogue content and on ValidationException message construction (4 causes)

> _Note: this issue was identified against the code base at commit `6c894fcf9edc6b5b7d1e0a3892ed526add3b28c2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetVersions

## Description
Four net-new root causes in the evs crate, all grounded in mismatching steps of the simparity-wire-aws-v1 replay (evs: 28/32 steps passed, 4 mismatches, 0 errors, 0 blocked; none of the four was marked stub-blocked or owned by an open issue). Cause 1 is the service version catalogue's CONTENT; causes 2-4 are three independent defects in how a ValidationException body is constructed (the aggregated clause list, and two constraint-violation message templates). Each needs its own fix; all four must be closed before this issue is submitted.

=== Cause 1 of 4: the GetVersions catalogue reports bare vSphere version numbers, one per instance type, where AWS reports a list of ESXi build ids ===
AWS: Smithy GetVersions output members `vcfVersions` (VcfVersionList) and `instanceTypeEsxVersions` (InstanceTypeEsxVersionsList) are both @required; EsxVersion is a string with @length max 128; VcfVersionInfo.defaultEsxVersion is @required and documented as "The default ESX version for this VCF version. It is based on Broadcom's Bill Of Materials (BOM)". Beyond that the model does not constrain the values, so: recorded behavior only. The recording shows that for EVERY instance type the service offers, `instanceTypeEsxVersions[].esxVersions` is a MULTI-ENTRY list of ESXi build identifiers of the form `ESXi-<vsphere-release>-<build-number>` (e.g. `ESXi-8.0U3b-24280767`, `ESXi-9.0.2.0.25148076`), not bare vSphere version numbers, and the same build-id form is used for `vcfVersions[].defaultEsxVersion`. The particular build ids are service-catalogue data that changes as Broadcom publishes new BOMs; the requirement is the FORM (an `ESXi-` build identifier), the fact that each instance type carries the full set of offered builds rather than a single value, and the internal consistency the model implies (a VCF version's defaultEsxVersion appears among the esxVersions of every instance type that version lists).
Emulator: each instance type's esxVersions list holds exactly ONE entry, a bare vSphere version string with no `ESXi-` build id: for `i4i.metal` the replay observed `esxVersions[0] = '8.0.3'` where the recording has `ESXi-8.0U3b-24280767`, and `esxVersions[1..8]` are absent entirely (`missing (expected 'ESXi-8.0U3g-24859861', got None)` and so on through `ESXi-8.0U3k-25595708`). The same single bare value appears for `i7i.metal-24xl` and `i7i.metal-48xl`. The cluster's key list additionally names `$.vcfVersions[0]`, `$.vcfVersions[1]` and `$.vcfVersions[2]`, i.e. all three VCF-version catalogue entries diverge as well, consistent with one wrong catalogue feeding both halves of the response (get_versions.rs computes instanceTypeEsxVersions as the transpose of the VCF version list).
Why: a caller that selects, pins or parses a host ESX version — and anything that round-trips the value back into CreateEnvironmentHost — is built against values production never returns, and against a one-element list where production offers a choice. Version-selection logic ("take the newest 9.x build", "match the environment's defaultEsxVersion") cannot be exercised at all, and build-id parsing fails in production.
Emulator: see above.
Fix: the service version catalogue in crate::state — `crate::state::provided_vcf_versions` and `crate::state::instance_type_esx_versions`, read verbatim by the handler in services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/get_versions.rs. The catalogue entries must carry ESXi build ids and the full per-instance-type list, and the transpose must keep each VCF version's defaultEsxVersion present in every instance type it lists.
Traces: evs-catalog-get-versions#task#1 (GetVersions, 200 -> 200, body mismatch).

=== Cause 2 of 4: CreateEnvironment's aggregated validation failure omits the AWS Premium Support entitlement clause and words the termsAccepted clause in the emulator's own terms ===
AWS: Smithy CreateEnvironment models exactly one error, ValidationException (HTTP 400), whose body carries @required `message` plus `fieldList` (a list of ValidationExceptionField). termsAccepted is @required Boolean and vpcId is @required with pattern `^vpc-[a-f0-9]{8}([a-f0-9]{9})?$`. The model says nothing about support plans or about clause text, so: recorded behavior only. On the recorded account class (no AWS Business/Enterprise(-On-Ramp) Support plan) every CreateEnvironment request first collects the entitlement clause `You must be subscribed to AWS Premium Support`, which is wholly fixed text interpolating nothing from the request. All collected clauses are reported BOTH as one `fieldList` entry each, in collection order with the entitlement clause FIRST, AND joined into `message` as `Request validation failure: ` + clauses joined by ` | `. The unaccepted-terms clause is the fixed text `Terms and Conditions must be accepted before creating an environment` (it names no member and interpolates nothing). The nonexistent-VPC clause is `The VPC id <vpcId> is not valid or does not exist. Please verify the VPC id exists and is valid.` where `<vpcId>` is the value taken from the request and the rest is fixed text.
Emulator: for a request with `termsAccepted=false` and a nonexistent vpcId it answers HTTP 400 ValidationException (status and code correct) with only TWO fieldList entries and a two-clause message: the entitlement clause is absent entirely (`$.fieldList[2]: missing`, and the whole list shifts up by one), and the terms clause reads `termsAccepted must be accepted (true) before an environment can be created` instead of AWS's `Terms and Conditions must be accepted before creating an environment`. Observed `$.message`: `Request validation failure: termsAccepted must be accepted (true) before an environment can be created | The VPC id vpc-01234567 is not valid or does not exist. Please verify the VPC id exists and is valid.` where the recording has `Request validation failure: You must be subscribed to AWS Premium Support | Terms and Conditions must be accepted before creating an environment | The VPC id vpc-01234567 is not valid or does not exist. Please verify the VPC id exists and is valid.` (the `vpc-01234567` there is the id taken from that request, not part of the template). The ` | ` join and the VPC clause are correct.
Why: a client that keys on the refusal text to tell an entitlement failure from an unaccepted-terms failure from a bad-VPC failure is routed to the wrong branch, and support-plan onboarding paths are never reached because the gate clause never appears. Clients reading `fieldList` positionally get a different field at every index than production returns. The leaked member-name phrasing (`termsAccepted must be accepted (true)`) is implementation wording that production never emits.
Fix: CreateEnvironment's validation in src/ops/create_environment.rs (the termsAccepted clause text, and collecting the account-entitlement clause as the FIRST failure) together with the shared validation-failure aggregation in crate::state that builds `message` and `fieldList` from the collected clauses. NOTE: open issue 20261029T183050Z-evs-5-net-new-root-causes-createenvironment-never.md describes the same two behaviours from the behavioral suite, but it carries no wire replay key, so this wire step is not gated by it; closing one should close both and this binding is what re-replays it.
Traces: evs-create-environment-input-validation#task#1 (CreateEnvironment, 400 -> 400, body mismatch).

=== Cause 3 of 4: the enum-constraint refusal for the body member vcfVersion uses the generic SDK template instead of AWS's member-named one ===
AWS: Smithy CreateEnvironment.vcfVersion is @required and targets the enum VcfVersion with values [VCF-5.2.1, VCF-5.2.2, SELF_DEPLOYED]; ValidationException (400) is the only modeled error. The message template is recorded behavior only: for an unmodeled value on this body enum member AWS answers HTTP 400 with `<N> validation error(s) detected: Member <memberName> must satisfy enum value set: [<declared values, comma-space separated>]`. `validation error(s) detected: Member `, ` must satisfy enum value set: [` and `]` are fixed text; `<N>` is the number of failures detected, `<memberName>` is the member from the request, and the value list is the enum's declared value set.
Emulator: same status (400) and error code, but the generic shape-level template: `1 validation error detected: Value at 'vcfVersion' failed to satisfy constraint: Member must satisfy enum value set: [VCF-5.2.1, VCF-5.2.2, SELF_DEPLOYED]` — it writes `validation error detected` (no `(s)`), wraps the member in `Value at '...' failed to satisfy constraint:` and drops AWS's `Member <name> must satisfy ...` phrasing. The value set itself matches.
Why: a message that differs from AWS's wording breaks clients and tests that match the refusal text, and anything extracting the offending member name from the message looks for it in the wrong position (quoted after `Value at` vs bare after `Member`). Hypothesis: the crate renders every constraint violation through one generic SDK-style formatter, so a body-level enum member gets the template AWS uses for the nested list-member case instead (cause 4 shows the nested template is also in use).
Fix: the enum/constraint violation message formatter used by CreateEnvironment's generated validation path (the constraint-violation rendering in crate::state / the request-validation layer feeding src/ops/create_environment.rs), so a direct body enum member renders AWS's `Member <name> must satisfy enum value set: [...]` text.
Traces: evs-create-environment-input-validation#task#3 (CreateEnvironment, 400 -> 400, `$.message` mismatch).

=== Cause 4 of 4: the enum value set in ListEnvironments' state-constraint refusal is listed in model-declaration order instead of AWS's order ===
AWS: Smithy ListEnvironments.state targets EnvironmentStateList (member enum EnvironmentState) and ValidationException (400) is its only modeled error. Message template is recorded behavior only: an unmodeled state filter is refused with HTTP 400 and `<N> validation error detected: Value at '<memberName>' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [CREATE_FAILED, DELETING, CREATED, DELETED, CREATING]]`. Every word of that is fixed text for this error class except `<N>` (failure count) and `<memberName>` (the member from the request); in particular the value set and ITS ORDER are fixed service text, not derived from the request or from state — the recording lists CREATE_FAILED, DELETING, CREATED, DELETED, CREATING.
Emulator: same status, same error code, same nested template, same five values — but ordered `[CREATING, CREATED, DELETING, DELETED, CREATE_FAILED]`, which is the order the enum is declared in the Smithy model (verified: EnvironmentState values are CREATING, CREATED, DELETING, DELETED, CREATE_FAILED). So the emulator renders the set from the model's declaration order where AWS renders it in its own fixed order.
Why: clients and test suites that compare this refusal message byte-for-byte against production fail, and operators diffing emulator output against recorded AWS output see a spurious difference on every paginated/filtered list refusal. The same formatter serves the other list-of-enum filters in the service, so the divergence is not confined to ListEnvironments.
Fix: the enum value-set rendering in the constraint-violation formatter (crate::state / the request-validation layer feeding src/ops/list_environments.rs) — the order AWS reports must be stated explicitly for the enum rather than taken from the model's declaration order. Lowest-severity of the four: the status, the error code and the value set are all already correct.
Traces: evs-service-catalog-discovery#task#4 (ListEnvironments, 400 -> 400, `$.message` mismatch).

## Evidence
All evidence is from the simparity-wire-aws-v1 replay against the emulator built from this checkout (emulator_up=true, 10 fixtures replayed, 32 steps, 28 passed, 4 mismatched, 0 errored, 0 blocked, score 87.5%). No divergence carried a stub_path or an owned_by.

Cause 1 — evs-catalog-get-versions#task#1, GetVersions, expected 200 / actual 200, body mismatch:
  $.instanceTypeEsxVersions[instanceType="i4i.metal"].esxVersions[0]: value_mismatch (expected 'ESXi-8.0U3b-24280767', got '8.0.3')
  $.instanceTypeEsxVersions[instanceType="i4i.metal"].esxVersions[1..8]: missing (expected 'ESXi-8.0U3g-24859861', 'ESXi-9.0.2.0.25148076', 'ESXi-9.0.2.0100.25595025', 'ESXi-9.1.0.0100.25433460', 'ESXi-9.1.0.0200.25557999', 'ESXi-8.0U3i-25205845', 'ESXi-8.0U3j-25429389', 'ESXi-8.0U3k-25595708'; got None)
  identical pattern for instanceType "i7i.metal-24xl" and "i7i.metal-48xl" (esxVersions[0] = '8.0.3' instead of the ESXi build id, [1..8] missing)
  the cluster's key list also names $.vcfVersions[0], $.vcfVersions[1], $.vcfVersions[2]
  (the specific build ids are catalogue data; the divergence is the bare-version form and the single-entry list.)

Cause 2 — evs-create-environment-input-validation#task#1, CreateEnvironment, expected 400 / actual 400:
  $.fieldList[0].message: expected 'You must be subscribed to AWS Premium Support', got 'termsAccepted must be accepted (true) before an environment can be created'
  $.fieldList[1].message: expected 'Terms and Conditions must be accepted before creating an environment', got 'The VPC id vpc-01234567 is not valid or does not exist. Please verify the VPC id exists and is valid.'
  $.fieldList[2]: missing (expected {'message': 'The VPC id vpc-01234567 is not valid or does not exist. Please verify the VPC id exists and is valid.'}, got None)
  $.message: expected 'Request validation failure: You must be subscribed to AWS Premium Support | Terms and Conditions must be accepted before creating an environment | The VPC id vpc-01234567 is not valid or does not exist. Please verify the VPC id exists and is valid.', got 'Request validation failure: termsAccepted must be accepted (true) before an environment can be created | The VPC id vpc-01234567 is not valid or does not exist. Please verify the VPC id exists and is valid.'
  (vpc-01234567 is the id supplied by that request; the divergence is the missing first clause and the terms-clause wording.)

Cause 3 — evs-create-environment-input-validation#task#3, CreateEnvironment, expected 400 / actual 400:
  $.message: expected '1 validation error(s) detected: Member vcfVersion must satisfy enum value set: [VCF-5.2.1, VCF-5.2.2, SELF_DEPLOYED]', got "1 validation error detected: Value at 'vcfVersion' failed to satisfy constraint: Member must satisfy enum value set: [VCF-5.2.1, VCF-5.2.2, SELF_DEPLOYED]"

Cause 4 — evs-service-catalog-discovery#task#4, ListEnvironments, expected 400 / actual 400:
  $.message: expected "1 validation error detected: Value at 'state' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [CREATE_FAILED, DELETING, CREATED, DELETED, CREATING]]", got "1 validation error detected: Value at 'state' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [CREATING, CREATED, DELETING, DELETED, CREATE_FAILED]]"
  (same values, emulator order == Smithy declaration order of EnvironmentState.)

## Steps to Reproduce
Replay the corpus: the four steps below are fixtures evs-catalog-get-versions, evs-create-environment-input-validation and evs-service-catalog-discovery of trace set simparity-wire-aws-v1.

Cause 1:
1. Call evs GetVersions. Inspect every instanceTypeEsxVersions[].esxVersions list and every vcfVersions[].defaultEsxVersion. Emulator: one bare entry per instance type ('8.0.3'). AWS: a multi-entry list of `ESXi-<release>-<build>` build ids, with each VCF version's defaultEsxVersion present among the esxVersions of every instance type that version lists.

Cause 2:
2. Call evs CreateEnvironment with client-side parameter validation disabled, termsAccepted=false and a vpcId that does not exist (e.g. vpc-01234567), everything else well formed. Compare the ValidationException body: AWS returns three fieldList entries — the AWS Premium Support entitlement clause first, then the Terms and Conditions clause, then the VPC clause — and a `message` of `Request validation failure: ` plus those three clauses joined by ` | `. The emulator returns two entries and omits the entitlement clause, and words the terms clause as `termsAccepted must be accepted (true) before an environment can be created`.

Cause 3:
3. Call CreateEnvironment (client-side validation disabled) with vcfVersion set to a value outside the VcfVersion enum. Compare `message` against AWS's `1 validation error(s) detected: Member vcfVersion must satisfy enum value set: [VCF-5.2.1, VCF-5.2.2, SELF_DEPLOYED]`.

Cause 4:
4. Call ListEnvironments with a `state` filter containing a value outside the EnvironmentState enum. The status (400), the error code (ValidationException) and the value set are correct; compare the ORDER of the enum value set in `message` against AWS's `[CREATE_FAILED, DELETING, CREATED, DELETED, CREATING]`.
