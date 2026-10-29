Issue-Source: trace-sweep-divergence
Issue-Trace: rolesanywhere-create-profile-defaults
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rolesanywhere-create-profile-defaults rolesanywhere-profile-lifecycle rolesanywhere-tag-lifecycle
Issue-Shape-Id: com.amazonaws.rolesanywhere#RolesAnywhere
Issue-Key: rolesanywhere|CreateProfile|2xx->2xx|$.profile.attributeMappings[*]
Issue-Key: rolesanywhere|CreateProfile|2xx->2xx|$.profile.createdAt
Issue-Key: rolesanywhere|CreateProfile|2xx->2xx|$.profile.createdBy
Issue-Key: rolesanywhere|CreateProfile|2xx->2xx|$.profile.enabled
Issue-Key: rolesanywhere|CreateProfile|2xx->2xx|$.profile.managedPolicyArns
Issue-Key: rolesanywhere|CreateProfile|2xx->2xx|$.profile.requireInstanceProperties
Issue-Key: rolesanywhere|CreateProfile|2xx->2xx|$.profile.updatedAt
Issue-Key: rolesanywhere|GetProfile|2xx->2xx|$.profile.attributeMappings[*]
Issue-Key: rolesanywhere|GetProfile|2xx->2xx|$.profile.createdAt
Issue-Key: rolesanywhere|GetProfile|2xx->2xx|$.profile.createdBy
Issue-Key: rolesanywhere|GetProfile|2xx->2xx|$.profile.enabled
Issue-Key: rolesanywhere|GetProfile|2xx->2xx|$.profile.managedPolicyArns
Issue-Key: rolesanywhere|GetProfile|2xx->2xx|$.profile.requireInstanceProperties
Issue-Key: rolesanywhere|GetProfile|2xx->2xx|$.profile.updatedAt
Issue-Key: rolesanywhere|ListProfiles|2xx->2xx|$.profiles[name="sim2real-probe-rolesanywhere-profile-61048731"].attributeMappings[*]
Issue-Key: rolesanywhere|ListProfiles|2xx->2xx|$.profiles[name="sim2real-probe-rolesanywhere-profile-61048731"].createdAt
Issue-Key: rolesanywhere|ListProfiles|2xx->2xx|$.profiles[name="sim2real-probe-rolesanywhere-profile-61048731"].createdBy
Issue-Key: rolesanywhere|ListProfiles|2xx->2xx|$.profiles[name="sim2real-probe-rolesanywhere-profile-61048731"].managedPolicyArns
Issue-Key: rolesanywhere|ListProfiles|2xx->2xx|$.profiles[name="sim2real-probe-rolesanywhere-profile-61048731"].requireInstanceProperties
Issue-Key: rolesanywhere|ListProfiles|2xx->2xx|$.profiles[name="sim2real-probe-rolesanywhere-profile-61048731"].updatedAt
Issue-Key: rolesanywhere|GetProfile|2xx->2xx|$.profile.durationSeconds
Issue-Key: rolesanywhere|GetProfile|2xx->2xx|$.profile.sessionPolicy
Issue-Key: rolesanywhere|DeleteProfile|2xx->2xx|$.profile.attributeMappings[*]
Issue-Key: rolesanywhere|DeleteProfile|2xx->2xx|$.profile.createdAt
Issue-Key: rolesanywhere|DeleteProfile|2xx->2xx|$.profile.createdBy
Issue-Key: rolesanywhere|DeleteProfile|2xx->2xx|$.profile.durationSeconds
Issue-Key: rolesanywhere|DeleteProfile|2xx->2xx|$.profile.managedPolicyArns
Issue-Key: rolesanywhere|DeleteProfile|2xx->2xx|$.profile.requireInstanceProperties
Issue-Key: rolesanywhere|DeleteProfile|2xx->2xx|$.profile.sessionPolicy
Issue-Key: rolesanywhere|DeleteProfile|2xx->2xx|$.profile.updatedAt
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# rolesanywhere profile responses drop default attribute mappings and serialize timestamps as epoch floats

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProfile

## Description
The ProfileDetail the emulator serializes diverges from AWS on every profile-returning operation (CreateProfile, GetProfile, ListProfiles, DeleteProfile), and UpdateProfile's edits are not reflected in later reads. Eight causes, one per failing replay cluster.

Cause 1 of 8 (CreateProfile, fixture rolesanywhere-create-profile-defaults step task#2):
AWS: ProfileDetail.createdAt/updatedAt are Timestamp members carrying @timestampFormat date-time (Smithy: ProfileDetail.createdAt/updatedAt, doc 'The ISO-8601 timestamp when the profile was created'), so restJson1 serializes them as ISO-8601 strings. A newly created profile carries three service-supplied attributeMappings - certificateField x509Issuer with mappingRules [{specifier '*'}], x509SAN with [{specifier 'DNS'},{specifier 'URI'},{specifier 'Name/*'}], and x509Subject with [{specifier '*'}] (recorded behavior only; the model does not document the defaults). createdBy is the ARN of the principal that created the profile, of the form arn:aws:iam::<account-id>:user/<user-name> - the fixed part is the arn:aws:iam:: prefix and the principal path, the account id and user name come from the caller (recorded behavior only; the model's doc says only 'The Amazon Web Services account that created the profile'). Members the request did not set and the service does not default - managedPolicyArns, requireInstanceProperties - are OMITTED from the response rather than emitted empty/false. When the request omits `enabled`, the created profile is disabled: enabled=false.
Emulator: createdAt/updatedAt are emitted as JSON epoch floats (e.g. a bare number instead of an ISO-8601 string); attributeMappings is absent entirely; createdBy is the bare 12-digit account id instead of a principal ARN; managedPolicyArns is emitted as [] and requireInstanceProperties as false where AWS omits both; enabled is true although the request omitted it.
Why: a client parsing createdAt/updatedAt with an ISO-8601/date-time deserializer fails outright (an SDK raises a deserialization error on a number where a date-time string is modeled), and a client that reads attributeMappings to learn how certificate fields map sees no mapping at all. A caller that creates a profile without `enabled` believes it is inert, but the emulator has it live.
Fix: the CreateProfile handler and the shared ProfileDetail serializer - date-time formatting for createdAt/updatedAt, the created-by principal ARN, the three default attribute mappings seeded at create time, omission of unset optional members, and the enabled=false default.
Traces: rolesanywhere-create-profile-defaults#task#2.

Cause 2 of 8 (GetProfile, fixture rolesanywhere-create-profile-defaults step task#3):
AWS: GetProfile returns the same ProfileDetail that CreateProfile returned, with ISO-8601 createdAt/updatedAt, the three default attributeMappings, the principal-ARN createdBy, enabled=false for a profile created without `enabled`, and managedPolicyArns/requireInstanceProperties omitted (Smithy: GetProfile output ProfileDetailResponse.profile -> ProfileDetail).
Emulator: the read serves the same defective record as the create - epoch-float timestamps, no attributeMappings, account-id createdBy, enabled=true, [] / false for the unset members.
Why: the defect is not confined to the create echo; it is the stored record, so every read of the profile is wrong too.
Fix: the ProfileDetail serializer shared by GetProfile, plus the state record it reads.
Traces: rolesanywhere-create-profile-defaults#task#3.

Cause 3 of 8 (CreateProfile, fixture rolesanywhere-profile-lifecycle step setup#2):
AWS: same ProfileDetail contract as cause 1 for a create that DOES pass enabled - the three default attributeMappings are still present, timestamps are still ISO-8601 date-time strings, createdBy is still the principal ARN, and managedPolicyArns/requireInstanceProperties are still omitted when unset.
Emulator: attributeMappings missing, createdAt/updatedAt epoch floats, createdBy the bare account id, managedPolicyArns [] and requireInstanceProperties false.
Why: confirms the shape divergence is independent of which optional request members the caller sets.
Fix: as cause 1, in the CreateProfile handler / ProfileDetail serializer.
Traces: rolesanywhere-profile-lifecycle#setup#2 (also rolesanywhere-tag-lifecycle#setup#2).

Cause 4 of 8 (GetProfile, fixture rolesanywhere-profile-lifecycle step task#3):
AWS: as cause 2 - the read echoes the full ProfileDetail with default attributeMappings, date-time timestamps, principal-ARN createdBy, and unset optional members omitted.
Emulator: same five defects on the read path.
Fix: the ProfileDetail serializer / state record.
Traces: rolesanywhere-profile-lifecycle#task#3 (also rolesanywhere-tag-lifecycle#task#3).

Cause 5 of 8 (ListProfiles, fixture rolesanywhere-profile-lifecycle step task#4):
AWS: ListProfiles returns ProfileDetails - a list of the SAME ProfileDetail shape (Smithy: ListProfiles output ListProfilesResponse.profiles -> ProfileDetails), so each element carries the default attributeMappings, ISO-8601 createdAt/updatedAt, principal-ARN createdBy, and omits unset managedPolicyArns/requireInstanceProperties.
Emulator: each listed element reproduces all of those defects.
Why: a client that enumerates profiles instead of reading them one by one is equally broken, so the list serializer must be fixed with the detail serializer, not separately.
Fix: the ListProfiles handler's use of the shared ProfileDetail serializer.
Traces: rolesanywhere-profile-lifecycle#task#4.

Cause 6 of 8 (GetProfile after UpdateProfile, fixture rolesanywhere-profile-lifecycle step task#6):
AWS: UpdateProfile persists the members it was given - durationSeconds (Smithy: UpdateProfile.durationSeconds @range 900..43200) and sessionPolicy (Smithy: UpdateProfile.sessionPolicy @length 1..100000) - and it bumps updatedAt, so the following GetProfile returns the UPDATED durationSeconds, the stored sessionPolicy document verbatim, and an updatedAt strictly later than createdAt.
Emulator: the read still reports the pre-update durationSeconds default and omits sessionPolicy entirely, i.e. the update was not applied to state; on top of that the timestamps are epoch floats, attributeMappings is missing and createdBy is the account id.
Why: read-after-write does not hold - a client that updates a profile's session duration or session policy and then reads it back sees the old profile, so any configuration workflow silently no-ops.
Fix: the UpdateProfile handler's write into the profile state record (durationSeconds, sessionPolicy, updatedAt), plus the shared ProfileDetail serializer.
Traces: rolesanywhere-profile-lifecycle#task#6.

Cause 7 of 8 (DeleteProfile, fixture rolesanywhere-profile-lifecycle step teardown#9):
AWS: DeleteProfile answers 200 with the deleted profile's final ProfileDetail (Smithy: DeleteProfile output ProfileDetailResponse.profile), including the default attributeMappings, the updated durationSeconds/sessionPolicy, ISO-8601 timestamps and the principal-ARN createdBy.
Emulator: the delete echo carries the same defects as the reads, and likewise reports the pre-update durationSeconds and no sessionPolicy.
Fix: the DeleteProfile handler's response serialization (shared ProfileDetail serializer).
Traces: rolesanywhere-profile-lifecycle#teardown#9.

Cause 8 of 8 (DeleteProfile, fixture rolesanywhere-tag-lifecycle step teardown#8):
AWS: as cause 7 for a profile that was only tagged, not updated - the delete echo still carries default attributeMappings, ISO-8601 createdAt/updatedAt, principal-ARN createdBy and no managedPolicyArns/requireInstanceProperties members.
Emulator: same divergences on the delete echo.
Fix: the DeleteProfile handler / shared ProfileDetail serializer.
Traces: rolesanywhere-tag-lifecycle#teardown#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator, 8 mismatch clusters, all status-matched but body-diverged.

Cause 1 - rolesanywhere-create-profile-defaults#task#2 CreateProfile, expected 201 / actual 201:
  $.profile.attributeMappings[0]: missing (expected {'certificateField': 'x509Issuer', 'mappingRules': [{'specifier': '*'}]}, got None)
  $.profile.attributeMappings[1]: missing (expected {'certificateField': 'x509SAN', 'mappingRules': [{'specifier': 'DNS'}, {'specifier': 'URI'}, {'specifier': 'Name/*'}]}, got None)
  $.profile.attributeMappings[2]: missing (expected {'certificateField': 'x509Subject', 'mappingRules': [{'specifier': '*'}]}, got None)
  $.profile.createdAt: value_mismatch (expected an ISO-8601 date-time string, got 1793301476.3125048)
  $.profile.createdBy: value_mismatch (expected 'arn:aws:iam::<account>:user/<user>', got '<account>')
  $.profile.enabled: value_mismatch (expected False, got True)
  $.profile.managedPolicyArns: type_mismatch (expected None, got [])
  $.profile.requireInstanceProperties: type_mismatch (expected None, got False)
  $.profile.updatedAt: value_mismatch (expected an ISO-8601 date-time string, got 1793301476.3125048)

Cause 2 - rolesanywhere-create-profile-defaults#task#3 GetProfile, 200/200: identical nine-key diff on $.profile.

Cause 3 - rolesanywhere-profile-lifecycle#setup#2 CreateProfile, 201/201 (cluster also covers rolesanywhere-tag-lifecycle#setup#2): attributeMappings[0..2] missing, createdAt/updatedAt epoch float, createdBy account id, managedPolicyArns [] vs None, requireInstanceProperties False vs None.

Cause 4 - rolesanywhere-profile-lifecycle#task#3 GetProfile, 200/200 (also rolesanywhere-tag-lifecycle#task#3): same eight-key diff.

Cause 5 - rolesanywhere-profile-lifecycle#task#4 ListProfiles, 200/200: same eight keys under $.profiles[name="<created profile name>"].

Cause 6 - rolesanywhere-profile-lifecycle#task#6 GetProfile, 200/200, after UpdateProfile:
  $.profile.durationSeconds: value_mismatch (expected 7200, got 3600)
  $.profile.sessionPolicy: missing (expected the submitted policy JSON document, got None)
  plus attributeMappings[0..2] missing, createdAt/updatedAt epoch floats, createdBy account id, managedPolicyArns []/None, requireInstanceProperties False/None.

Cause 7 - rolesanywhere-profile-lifecycle#teardown#9 DeleteProfile, 200/200: same ten-key diff as cause 6.

Cause 8 - rolesanywhere-tag-lifecycle#teardown#8 DeleteProfile, 200/200: same eight-key shape diff.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 wire corpus against the emulator built from this checkout.
2. Cause 1/2: POST /profiles with name+roleArns only (no enabled) and then GET /profile/{profileId} - fixture rolesanywhere-create-profile-defaults, steps task#2 and task#3. Observe attributeMappings absent, enabled true, createdAt/updatedAt numeric, createdBy a bare account id, managedPolicyArns [] and requireInstanceProperties false.
3. Cause 3/4/5: fixture rolesanywhere-profile-lifecycle steps setup#2 (CreateProfile), task#3 (GetProfile) and task#4 (ListProfiles).
4. Cause 6/7: fixture rolesanywhere-profile-lifecycle - PATCH /profile/{profileId} with durationSeconds and sessionPolicy, then step task#6 (GetProfile) and teardown#9 (DeleteProfile); the updated durationSeconds and the sessionPolicy are absent from both.
5. Cause 8: fixture rolesanywhere-tag-lifecycle step teardown#8 (DeleteProfile echo).
