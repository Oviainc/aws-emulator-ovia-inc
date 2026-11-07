Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rolesanywhere.put-attribute-mapping-invalid-specifier-rejected
Issue-Shape-Id: com.amazonaws.rolesanywhere#RolesAnywhere
Issue-Key: rolesanywhere.put-attribute-mapping-invalid-specifier-rejected|_client_error#7|AssertionError|raise AssertionError(f"expected PutAttributeMapping to be rejected, but it SUCCEEDED: {result!r}")
Issue-Commit: c48413f08eb6ef52de10538e8da3115aa7206fd4

# rolesanywhere PutAttributeMapping accepts a specifier the certificateField does not support instead of refusing it with ValidationException

> _Note: this issue was identified against the code base at commit `c48413f08eb6ef52de10538e8da3115aa7206fd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAttributeMapping

## Description
One net-new root cause on the rolesanywhere attribute-mapping write path.

Cause 1 of 1 (PutAttributeMapping does not validate the mapping-rule specifier against the certificateField it is being written under):

AWS: Smithy binds PutAttributeMapping to PUT /profiles/{profileId}/mappings (input PutAttributeMappingRequest: required @httpLabel profileId -> Uuid @length 36..36 @pattern [a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}; required certificateField -> CertificateField, an enum whose only values are x509Subject | x509Issuer | x509SAN; required mappingRules -> MappingRules, a list of MappingRule whose required member specifier is a String @length max 60). Output PutAttributeMappingResponse with required profile -> ProfileDetail. Modeled errors: AccessDeniedException httpError 403, ResourceNotFoundException httpError 404, ValidationException httpError 400. The model constrains the certificate FIELD to an enum but leaves `specifier` an unconstrained string, so the specifier-vs-field compatibility rule is not expressible in the model: the set of specifiers each certificate field accepts is a service-side rule (the operation's own documentation states a mapping 'specifies a certificate field and one or more specifiers that have contextual meanings', i.e. the specifier is only meaningful relative to the field). Recorded behavior: for certificateField x509SAN, which supports only the subject-alternative-name specifiers DNS, URI and Name, a mappingRules entry with specifier CN is refused with HTTP 400 ValidationException and nothing is persisted; a supported specifier (DNS) on the same profile is accepted. The refusal message is "Unable to validate specifier (CN) on the target (SUBJECT_ALTERNATIVE_NAME)". In that message the FIXED text for this error class is "Unable to validate specifier (" ... ") on the target (" ... ")"; the two variable parts are taken from the request: the rejected `specifier` value verbatim (here CN) and the internal name of the addressed certificateField (here SUBJECT_ALTERNATIVE_NAME for x509SAN). CN is not part of the template — it is simply the specifier the request sent.

Emulator: the call SUCCEEDS with HTTP 200 and the unsupported specifier is persisted. The replayed check observes 200 with profile.attributeMappings = [{certificateField: 'x509SAN', mappingRules: [{specifier: 'CN'}]}], i.e. the emulator both failed to refuse and wrote the rejected rule into profile state, so the subsequent GetProfile reports a x509SAN mapping carrying CN. The handler validates only the profile's existence (404 on an unknown profileId) and the merged rule COUNT against MAX_MAPPING_RULES_PER_CERTIFICATE_FIELD; there is no check at all that each submitted specifier is one the addressed certificateField supports, so every string within the 60-character length bound is accepted for every field.

Why: a client relying on AWS's behavior gets two things wrong. First, a mapping configuration that real RolesAnywhere rejects outright is reported as applied, so code that validates user-supplied attribute mappings by submitting them (a console form, an IaC plan/apply, a conformance test) sees a false success and promotes a profile that cannot exist in AWS. Second, the profile's persisted attributeMappings now contain a rule AWS would never store, so every later read (GetProfile, ListProfiles, the echoes from Disable/Delete/Put) serves state that diverges from AWS, and a client exercising the negative path to assert ValidationException gets a 200 and no error to branch on — an overly permissive acceptance that silently hides a real misconfiguration until the same call is made against AWS.

Fix: PutAttributeMapping's input validation, in the operation's handler (services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/put_attribute_mapping.rs) — before any write to profile state, reject each mappingRules entry whose specifier is not in the set the submitted certificateField supports, as a 400 ValidationException whose message renders AWS's template with the offending specifier and the field's internal target name; the supported-specifier sets per certificate field and the target-name rendering belong beside the existing mapping helpers in crate::state. The refused call must persist nothing (the count check already runs before the write, so the field check belongs in the same pre-write position).

Traces: behavioral check rolesanywhere.put-attribute-mapping-invalid-specifier-rejected.

Related but NOT covering: open issue 20261106T145115Z-rolesanywhere-refusal-messages-and-delete-echoes-diverge-on.md covers the DeleteAttributeMapping not-found message and trust-anchor paths, not PutAttributeMapping specifier validation, and carries no replay key for this check.

## Evidence
Replay of behavioral suite simparity-behavioral-aws-v1 against the emulator built from this checkout (rolesanywhere 10/26 passed; emulator up and serving — this check reached the handler and got a 2xx).

[mismatch] rolesanywhere.put-attribute-mapping-invalid-specifier-rejected
  AssertionError: expected PutAttributeMapping to be rejected, but it SUCCEEDED: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'profile': {'profileId': '...', 'name': 'sim2real-ra-profile-...', 'enabled': True, 'roleArns': ['arn:aws:iam::...:role/sim2real-ra-role-...'], 'attributeMappings': [{'certificateField': 'x509SAN', 'mappingRules': [{'specifier': 'CN'}]}], ...}}

Expected (AWS): HTTP 400, Error.Code 'ValidationException', Error.Message 'Unable to validate specifier (CN) on the target (SUBJECT_ALTERNATIVE_NAME)'.
  Fixed text of that message: 'Unable to validate specifier (' + ') on the target (' + ')'.
  Variable parts, from the request: the rejected specifier value (CN here) and the internal target name of the addressed certificateField (SUBJECT_ALTERNATIVE_NAME for x509SAN).
Actual (emulator): HTTP 200, no error, and the rejected rule persisted — the profile ids, ARNs, names and timestamps in the quoted body are per-run values, not requirements.

Positive control in the same check passes against AWS semantics: specifier DNS on the same profile/x509SAN field is accepted and visible via GetProfile, so this is specifically the missing per-field specifier check, not a blanket refusal.

Source: services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/put_attribute_mapping.rs lines 144-195 — the handler checks only profile existence (lines 153-159) and the merged rule count against crate::state::MAX_MAPPING_RULES_PER_CERTIFICATE_FIELD (lines 164-182), then writes unconditionally via crate::state::put_attribute_mapping (lines 184-188). No specifier/certificateField compatibility check exists anywhere on the path.

Smithy: PutAttributeMapping PUT /profiles/{profileId}/mappings; certificateField -> CertificateField enum values [x509Subject, x509Issuer, x509SAN]; mappingRules -> MappingRules of MappingRule{specifier: String @length max 60, required}; errors ValidationException httpError 400, ResourceNotFoundException 404, AccessDeniedException 403.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay behavioral check rolesanywhere.put-attribute-mapping-invalid-specifier-rejected.
2. CreateProfile with a name and one roleArns entry; take profileId from the response.
3. Call PutAttributeMapping with that profileId, certificateField='x509SAN' and mappingRules=[{'specifier': 'CN'}] — CN is a subject/issuer specifier, not one of the DNS/URI/Name specifiers x509SAN supports.
   Expect: HTTP 400 ValidationException with message 'Unable to validate specifier (CN) on the target (SUBJECT_ALTERNATIVE_NAME)' (the specifier and target name interpolated from the request) and nothing persisted.
   Observe: HTTP 200 with profile.attributeMappings containing {certificateField: 'x509SAN', mappingRules: [{specifier: 'CN'}]}.
4. Call GetProfile with the same profileId and observe the x509SAN mapping still carries {'specifier': 'CN'} — the refused rule leaked into profile state.
5. Positive control (must keep working after the fix): PutAttributeMapping with the same profileId, certificateField='x509SAN', mappingRules=[{'specifier': 'DNS'}] succeeds and GetProfile reports the DNS specifier under x509SAN.
