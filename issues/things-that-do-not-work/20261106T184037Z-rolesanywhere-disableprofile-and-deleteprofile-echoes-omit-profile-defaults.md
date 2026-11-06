Issue-Source: trace-sweep-divergence
Issue-Trace: rolesanywhere-profile-lifecycle
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rolesanywhere-profile-lifecycle
Issue-Shape-Id: com.amazonaws.rolesanywhere#RolesAnywhere
Issue-Key: rolesanywhere|DisableProfile|2xx->2xx|$.profile.attributeMappings[*]
Issue-Key: rolesanywhere|DisableProfile|2xx->2xx|$.profile.createdBy
Issue-Key: rolesanywhere|DisableProfile|2xx->2xx|$.profile.durationSeconds
Issue-Key: rolesanywhere|DisableProfile|2xx->2xx|$.profile.managedPolicyArns
Issue-Key: rolesanywhere|DisableProfile|2xx->2xx|$.profile.requireInstanceProperties
Issue-Key: rolesanywhere|DisableProfile|2xx->2xx|$.profile.sessionPolicy
Issue-Key: rolesanywhere|DeleteProfile|2xx->2xx|$.profile.enabled
Issue-Related: 20261029T192145Z-rolesanywhere-profile-responses-drop-default-attribute-mappings-and.md
Issue-Commit: ca2413fb439d11542d93c2da1574718318f978d1

# rolesanywhere DisableProfile and DeleteProfile echoes omit profile defaults and lose post-update and re-enabled state

> _Note: this issue was identified against the code base at commit `ca2413fb439d11542d93c2da1574718318f978d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisableProfile

## Description
The two write-path ProfileDetail echoes of the profile lifecycle - DisableProfile and DeleteProfile - diverge from AWS on the SAME ProfileDetail body: the service-supplied attribute mappings and the created-by principal are missing, unset optional members are emitted instead of omitted, the members a preceding UpdateProfile set are absent, and the delete receipt reports a profile that is still disabled. Two causes, one per failing replay cluster; a fixer must close both before submitting.

Related open issues that do NOT cover these two clusters: 20261029T192145Z-rolesanywhere-profile-responses-drop-default-attribute-mappings-and.md carries replay keys for CreateProfile / GetProfile / ListProfiles and for DeleteProfile's shape members only - it has NO DisableProfile key at all and no DeleteProfile $.profile.enabled key, and the replay reports both clusters below as unowned. 20261103T020145Z-rolesanywhere-enableprofile-is-an-unimplemented-scaffold-stub-and.md describes the EnableProfile stub from the behavioral suite and carries no wire replay key, so it does not gate cause 2's response divergence.

Cause 1 of 2 (DisableProfile echo, fixture rolesanywhere-profile-lifecycle step task#7):
AWS: Smithy binds DisableProfile to POST /profile/{profileId}/disable (input ScalarProfileRequest, required @httpLabel profileId -> Uuid @length 36..36 @pattern [a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}; output ProfileDetailResponse.profile -> ProfileDetail; errors AccessDeniedException httpError 403, ResourceNotFoundException httpError 404), success 200. The echoed ProfileDetail is the resource's full state at the moment the disable was applied, i.e. exactly the body GetProfile would return: the three service-supplied attributeMappings a profile is created with - certificateField x509Issuer with mappingRules [{specifier '*'}], x509SAN with [{specifier 'DNS'},{specifier 'URI'},{specifier 'Name/*'}], x509Subject with [{specifier '*'}] (recorded behavior only; the model documents attributeMappings as 'A mapping applied to the authenticating end-entity certificate' without stating the defaults); createdBy as the ARN of the creating principal, fixed prefix 'arn:aws:iam::' plus the principal path, with the account id and principal name taken from the caller (recorded behavior only; the model's doc says only 'The Amazon Web Services account that created the profile'); the durationSeconds and sessionPolicy a preceding UpdateProfile persisted (Smithy: UpdateProfile.durationSeconds @range 900..43200, UpdateProfile.sessionPolicy @length 1..100000, PATCH /profile/{profileId}); and NO managedPolicyArns / requireInstanceProperties members when the caller never set them and the service does not default them (requireInstanceProperties is documented 'Unused, saved for future use').
Emulator: 200 with the right status and shape, but the echo carries no attributeMappings at all, createdBy is the bare 12-digit account id instead of a principal ARN, managedPolicyArns is serialized as [] and requireInstanceProperties as false where AWS omits both, durationSeconds is the create-time 3600 rather than the value the update set, and sessionPolicy is absent. The flag itself is right (enabled=false matches).
Why: a client that disables a profile and takes the returned ProfileDetail as the profile's current state - instead of re-reading it - sees a profile with no certificate-field mappings, an unusable createdBy that is not an ARN, optional members that read as explicitly empty rather than unset, and the pre-update session duration and no session policy. Any workflow that updates a profile and then disables it records the wrong trust boundary.
Fix: the DisableProfile handler's response projection and the shared ProfileDetail serializer it goes through (default attribute mappings, created-by principal ARN, omission of unset optional members), plus the UpdateProfile write into profile state that the echo reads back.
Traces: rolesanywhere-profile-lifecycle#task#7. Source: services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/disable_profile.rs lines 161-166 (record.enabled = false; projects record.to_detail()); services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/update_profile.rs lines 153-171 (scaffolded stub, never writes durationSeconds/sessionPolicy).

Cause 2 of 2 (DeleteProfile receipt reports a disabled profile where AWS reports the live one, fixture rolesanywhere-profile-lifecycle step teardown#9):
AWS: Smithy binds DeleteProfile to DELETE /profile/{profileId} (output ProfileDetailResponse.profile -> ProfileDetail; errors AccessDeniedException 403, ResourceNotFoundException 404), success 200. The deletion receipt reports the profile's state as it stood when the delete was accepted, including ProfileDetail.enabled ('Indicates whether the profile is enabled'). In the recorded lifecycle the profile is disabled and then re-enabled before the delete, so AWS's delete response reports enabled = true, together with the same full ProfileDetail body as cause 1 (default attributeMappings, principal-ARN createdBy, the updated durationSeconds and the stored sessionPolicy, and no managedPolicyArns / requireInstanceProperties members).
Emulator: 200 with the right shape, but $.profile.enabled is false, plus the same body divergences as cause 1 - attributeMappings missing, createdBy the bare account id, managedPolicyArns [] and requireInstanceProperties false where AWS omits them, durationSeconds 3600 instead of the updated value, sessionPolicy absent. The delete handler itself is faithful: it removes the record and projects it unchanged, so the stored flag was already false when the delete ran - the preceding DisableProfile writes enabled=false and nothing ever writes it back. Hypothesis: the re-enable leg of the lifecycle never lands because EnableProfile is an unimplemented scaffold that answers 501, so the profile stays disabled for the rest of the fixture.
Why: the deletion receipt is how a client learns the final state of the resource it just removed. Reporting a disabled profile where AWS reports an enabled one inverts the profile's kill switch in audit and teardown logic, and a caller that re-enables a profile before deleting it records the opposite of what happened.
Fix: the profile enabled-state transitions in service state - the enable path must write enabled=true and bump updatedAt the way the disable path writes false - so DeleteProfile's projection of the removed record reports the profile's actual last state; plus the shared ProfileDetail serializer and the UpdateProfile state write, as in cause 1.
Traces: rolesanywhere-profile-lifecycle#teardown#9. Source: services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/delete_profile.rs lines 142-155 (removes the record and projects record.to_detail() unchanged); services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/enable_profile.rs lines 137-155 (scaffolded stub, returns not_implemented, never writes the flag back).

## Evidence
Replay of wire trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator up, /__admin/health 200, score 57.6%, 85 steps, 36 mismatched). Both clusters below carry an EMPTY stub_path and an EMPTY owned_by.

Cause 1 - DisableProfile, expected 200 / actual 200, fixture rolesanywhere-profile-lifecycle step task#7 (divergence_key service=rolesanywhere|DisableProfile|status=200->200):
  $.profile.attributeMappings[0]: missing (expected {'certificateField': 'x509Issuer', 'mappingRules': [{'specifier': '*'}]}, got None)
  $.profile.attributeMappings[1]: missing (expected {'certificateField': 'x509SAN', 'mappingRules': [{'specifier': 'DNS'}, {'specifier': 'URI'}, {'specifier': 'Name/*'}]}, got None)
  $.profile.attributeMappings[2]: missing (expected {'certificateField': 'x509Subject', 'mappingRules': [{'specifier': '*'}]}, got None)
  $.profile.createdBy: value_mismatch (expected a principal ARN of the form arn:aws:iam::<account-id>:user/<user-name>, got the bare account id)
  $.profile.durationSeconds: value_mismatch (expected the value the preceding UpdateProfile set, 7200 in this run, got the create-time default 3600)
  $.profile.managedPolicyArns: type_mismatch (expected None, got [])
  $.profile.requireInstanceProperties: type_mismatch (expected None, got False)
  $.profile.sessionPolicy: missing (expected the session-policy document the preceding UpdateProfile submitted, got None)
  (The account id, user name and the 7200/policy document of the recorded run are per-run values filling those templates, not requirements.)

Cause 2 - DeleteProfile, expected 200 / actual 200, fixture rolesanywhere-profile-lifecycle step teardown#9 (divergence_key service=rolesanywhere|DeleteProfile|status=200->200, includes $.profile.enabled):
  $.profile.enabled: value_mismatch (expected True, got False)
  plus the same eight keys as cause 1: attributeMappings[0..2] missing, createdBy the bare account id, durationSeconds 3600 vs the updated value, managedPolicyArns [] vs None, requireInstanceProperties False vs None, sessionPolicy missing.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay wire trace set simparity-wire-aws-v1.
2. Cause 1: POST /profiles to create a profile, PATCH /profile/{profileId} with durationSeconds and sessionPolicy, then POST /profile/{profileId}/disable (fixture rolesanywhere-profile-lifecycle, step task#7). Expect 200 whose $.profile carries the three default attributeMappings, a principal-ARN createdBy, the updated durationSeconds and the stored sessionPolicy, and NO managedPolicyArns / requireInstanceProperties members; observe no attributeMappings, a bare account id, durationSeconds 3600, no sessionPolicy, managedPolicyArns [] and requireInstanceProperties false.
3. Cause 2: continue the same lifecycle - POST /profile/{profileId}/enable to re-enable the profile, then DELETE /profile/{profileId} (fixture rolesanywhere-profile-lifecycle, step teardown#9). Expect the delete receipt's $.profile.enabled to be true (the profile's state at delete time) along with the full ProfileDetail body; observe enabled false and the same eight body divergences.
