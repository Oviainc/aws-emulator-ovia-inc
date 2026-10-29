Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rolesanywhere.get-trust-anchor-malformed-id-validation-rejected rolesanywhere.create-profile-role-arns-exceed-max-rejected rolesanywhere.get-trust-anchor-nonexistent-not-found rolesanywhere.trust-anchor-delete-removes-resource rolesanywhere.profile-delete-removes-resource rolesanywhere.create-profile-defaults-reflect-documented-values rolesanywhere.put-and-reset-notification-settings
Issue-Shape-Id: com.amazonaws.rolesanywhere#RolesAnywhere
Issue-Key: rolesanywhere.get-trust-anchor-malformed-id-validation-rejected|run#10|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: rolesanywhere.create-profile-role-arns-exceed-max-rejected|run#8|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: rolesanywhere.get-trust-anchor-nonexistent-not-found|run#11|AssertionError|assert err.get("Message") == "TrustAnchor not found", (
Issue-Key: rolesanywhere.trust-anchor-delete-removes-resource|run#12|AssertionError|assert error.get("Message") == "TrustAnchor not found", (
Issue-Key: rolesanywhere.profile-delete-removes-resource|run#13|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: rolesanywhere.create-profile-defaults-reflect-documented-values|run#9|AssertionError|assert created["enabled"] is False, (
Issue-Key: rolesanywhere.put-and-reset-notification-settings|run#9|AssertionError|assert default_entry is not None, "fresh trust anchor has no CA_CERTIFICATE_EXPIRY/ALL setting"
Issue-Commit: 5b71589754f95f05f88c1a0ee9ccd691bbc5f701

# rolesanywhere refusal messages and resource defaults diverge from AWS wording and documented values

> _Note: this issue was identified against the code base at commit `5b71589754f95f05f88c1a0ee9ccd691bbc5f701`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTrustAnchor

## Description
Five root causes where the status and error class are right but the message text, or a resource's service-supplied default, diverges from AWS. Five causes, numbered below; a fixer must close all five before submitting.

Cause 1 of 5 (GetTrustAnchor - the rendered trustAnchorId pattern omits the ARN alternative):
AWS: GetTrustAnchor.trustAnchorId is an @httpLabel targeting Uuid with @length 36..36 and @pattern [a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12} (Smithy: GetTrustAnchor input trustAnchorId, errors ValidationException httpError 400, ResourceNotFoundException httpError 404). An id that is not a well-formed UUID is refused 400 ValidationException with the standard constraint envelope '1 validation error detected: Value '<submitted value>' at '<member name>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>'. The pattern AWS reports for this member is the alternation it actually accepts - ([a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}|arn:aws(-[^:]+)?:rolesanywhere(:.*){2}:(trust-anchor|alias/trust-anchor)/.*) - i.e. a bare UUID OR a trust-anchor ARN/alias, unanchored. Fixed text: the envelope and the pattern; variable parts: the submitted value and the member name ('trustAnchorId').
Emulator: same 400 and same envelope, but renders only the model's narrow UUID pattern '[a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}', dropping the ARN alternative entirely.
Why: the message is part of the error contract. A caller reading the reported constraint out of the refusal concludes only a bare UUID is ever accepted, and an exact-match comparison against AWS's refusal fails even though the rejection itself is correct. Related to but not covered by open issue 20261029T192145Z-rolesanywhere-validation-refusals-render-patterns-and-enum-sets.md, whose cause 1 is about stripping ^...$ anchors on CreateProfile.name; here the anchors are already absent and the defect is the missing alternation for the resource-id members.
Fix: the shared constraint-violation message renderer / the id-member validation for resource-id labels - render the service's accepted pattern (UUID or trust-anchor ARN/alias) for trustAnchorId-style members, not the model's UUID-only pattern.
Traces: rolesanywhere.get-trust-anchor-malformed-id-validation-rejected.

Cause 2 of 5 (CreateProfile - the roleArns length-violation message does not match AWS's envelope):
AWS: CreateProfile.roleArns is @required and targets RoleArnList with @length 0..250 (Smithy: CreateProfile.roleArns -> RoleArnList length min 0 max 250; errors ValidationException httpError 400). A list of 251 role ARNs is refused 400 ValidationException with the constraint envelope '1 validation error detected: Value '[<arn>, <arn>, ...]' at 'roleArns' failed to satisfy constraint: Member must have length less than or equal to 250', where the fixed text is the envelope plus the phrase 'Member must have length less than or equal to ' and the declared maximum, and the variable parts are the submitted list rendered as a bracketed, comma-and-space separated sequence of its elements and the member name ('roleArns'). No profile is created, so the submitted name does not appear in ListProfiles.
Emulator: the error Code assertion passed, so the emulator does refuse with 400 ValidationException - only the Message diverges from that envelope (the exact emulator text is not quotable here: the replay report truncated that very long single-line detail after the expected value). Hypothesis: the list value is not rendered into the '[a, b, ...]' form, or the length phrase differs from 'Member must have length less than or equal to <max>'.
Why: as cause 1 - a client or test comparing the reported constraint against AWS's text never matches, and a human reading the refusal cannot tell which member or which limit was violated.
Fix: the shared constraint-violation message renderer for @length on list members (value rendering of the submitted list and the 'Member must have length less than or equal to <max>' phrase), used by CreateProfile's request validation.
Traces: rolesanywhere.create-profile-role-arns-exceed-max-rejected.

Cause 3 of 5 (ResourceNotFoundException messages are emulator-authored prose):
AWS: GetTrustAnchor on an unknown trustAnchorId answers 404 ResourceNotFoundException (Smithy: GetTrustAnchor errors ResourceNotFoundException httpError 404) whose message is the FIXED text 'TrustAnchor not found' - no variable part, the requested id is NOT interpolated. GetProfile on an unknown profileId answers 404 ResourceNotFoundException with a differently shaped message: "The item doesn't exist. arn: <profile ARN> ; aborting." - fixed parts "The item doesn't exist. arn:" and " ; aborting.", the single variable part being the profile ARN composed from the requested profileId plus the endpoint's region and the caller's account (Smithy: ProfileArn @pattern ^arn:aws(-[^:]+)?:rolesanywhere(:.*){2}(:profile.*)$). The same wording applies after a successful Delete*, which makes the resource permanently unreadable.
Emulator: 404 and the right error class, but hand-written prose - 'Trust anchor <trustAnchorId> not found' for trust anchors and 'Profile <profileId> not found' for profiles - with none of AWS's fixed text and a bare id where AWS reports either nothing or a full ARN.
Why: a client that distinguishes not-found conditions by message text never matches, and the profile message's ARN - which identifies which profile was missing - is simply absent. Three behavioral checks fail on exactly this. The open wire issue 20261029T192145Z-rolesanywhere-not-found-refusals-carry-emulator-authored-messages.md describes the same defect from the wire corpus (GetTrustAnchor/GetProfile/DeleteProfile/DeleteTrustAnchor) but does not carry these behavioral checks' replay keys, so they are recorded here; fixing that issue should resolve this cause and these three checks are the replay to confirm it.
Fix: the not-found error construction on the trust-anchor and profile read/delete paths (their handlers / the service's error channel) - the fixed 'TrustAnchor not found' string with no interpolation for trust anchors, and the ARN-bearing "The item doesn't exist. arn: <arn> ; aborting." string for profiles.
Traces: rolesanywhere.get-trust-anchor-nonexistent-not-found, rolesanywhere.trust-anchor-delete-removes-resource, rolesanywhere.profile-delete-removes-resource.

Cause 4 of 5 (CreateProfile defaults `enabled` to true instead of false):
AWS: CreateProfile.enabled is an optional Boolean with no modelled default (Smithy: CreateProfile.enabled -> Boolean). A profile created without it comes back disabled: enabled=false, alongside the documented durationSeconds default 3600 and acceptRoleSessionName=false, and GetProfile reports the identical values with no drift from the create echo (the enabled default is recorded behavior only - the model states nothing).
Emulator: returns enabled=true for a create that omitted the member (durationSeconds=3600 and acceptRoleSessionName=false did match).
Why: a caller that creates a profile and only later enables it believes the profile is inert while the emulator has it live - the exact inversion of the kill switch, and a client that asserts a new profile is disabled before wiring credentials to it fails or, worse, ships a live profile. Same defect as cause 1 of open wire issue 20261029T192145Z-rolesanywhere-profile-responses-drop-default-attribute-mappings-and.md, which does not carry this check's replay key.
Fix: the CreateProfile handler's defaulting of the enabled flag when the request omits it (state write), so every later read reports false.
Traces: rolesanywhere.create-profile-defaults-reflect-documented-values.

Cause 5 of 5 (a fresh trust anchor carries no default notification settings):
AWS: a trust anchor created without notificationSettings is still returned - by CreateTrustAnchor and by GetTrustAnchor - with the service-default NotificationSettingDetail entries, including event CA_CERTIFICATE_EXPIRY with channel ALL, enabled=true, threshold=45 and configuredBy='rolesanywhere.amazonaws.com' (the model states the configuredBy rule: default settings configured by IAM Roles Anywhere carry rolesanywhere.amazonaws.com, customized ones the account id; the threshold and channel are recorded behavior only). PutNotificationSettings then overrides the threshold for an event and flips that entry's configuredBy to the caller's account id, and ResetNotificationSettings reverts both to threshold 45 and configuredBy rolesanywhere.amazonaws.com.
Emulator: notificationSettings on a freshly created trust anchor is an empty list, so there is no CA_CERTIFICATE_EXPIRY/ALL entry to read; the whole put/reset round trip is unobservable.
Why: a client cannot discover that certificate-expiry notifications are on by default or at what threshold, so notification tooling built against the emulator sees an unmonitored trust anchor, and read-after-write does not hold for PutNotificationSettings/ResetNotificationSettings. Same defect as cause 1 of open wire issue 20261029T192145Z-rolesanywhere-trust-anchor-responses-omit-default-notification-settings.md, which does not carry this check's replay key.
Fix: the CreateTrustAnchor handler must seed the default notification settings into trust-anchor state, and the PutNotificationSettings / ResetNotificationSettings writes plus the shared TrustAnchorDetail serializer must round-trip them (including the configuredBy switch).
Traces: rolesanywhere.put-and-reset-notification-settings.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout. rolesanywhere 2/26 (8%), 24 failed (12 blocked by already-filed unimplemented stubs). The seven mismatches below are status-correct and diverge only in message text or in a service-supplied default.

Cause 1 - rolesanywhere.get-trust-anchor-malformed-id-validation-rejected [mismatch]:
  AssertionError: expected message "1 validation error detected: Value 'zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz' at 'trustAnchorId' failed to satisfy constraint: Member must satisfy regular expression pattern: ([a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}|arn:aws(-[^:]+)?:rolesanywhere(:.*){2}:(trust-anchor|alias/trust-anchor)/.*)", got "1 validation error detected: Value 'zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz' at 'trustAnchorId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}"
  The submitted value is the variable part; the difference is solely the missing '(...|arn:aws...trust-anchor...)' alternation in the rendered pattern.

Cause 2 - rolesanywhere.create-profile-role-arns-exceed-max-rejected [mismatch]:
  Code assertion passed (ValidationException); Message assertion failed at check line 29/37.
  Expected: "1 validation error detected: Value '[<251 comma-separated role ARNs>]' at 'roleArns' failed to satisfy constraint: Member must have length less than or equal to 250".
  Actual: diverged - the report's detail line was truncated after the (very long) expected value, so the emulator's exact message text is not reproduced here; the Message inequality is the recorded fact.

Cause 3 - three checks, all 404 with the right error class and only the message diverging:
  rolesanywhere.get-trust-anchor-nonexistent-not-found: AssertionError: expected message 'TrustAnchor not found', got 'Trust anchor <trustAnchorId> not found'.
  rolesanywhere.trust-anchor-delete-removes-resource: AssertionError: expected GetTrustAnchor message 'TrustAnchor not found', got 'Trust anchor <trustAnchorId> not found'.
  rolesanywhere.profile-delete-removes-resource: AssertionError: expected GetProfile message "The item doesn't exist. arn: arn:aws:rolesanywhere:<region>:<account>:profile/<profileId> ; aborting.", got 'Profile <profileId> not found'.
  (The ids in those messages are per-run values filling the template, not requirements.)

Cause 4 - rolesanywhere.create-profile-defaults-reflect-documented-values [mismatch]:
  AssertionError: expected default enabled=False on CreateProfile, got True (durationSeconds==3600 and acceptRoleSessionName==False passed before it).

Cause 5 - rolesanywhere.put-and-reset-notification-settings [mismatch]:
  AssertionError: fresh trust anchor has no CA_CERTIFICATE_EXPIRY/ALL setting - GetTrustAnchor right after CreateTrustAnchor returns notificationSettings with no entry for event CA_CERTIFICATE_EXPIRY / channel ALL (the create echo shows notificationSettings: []). Expected that entry with configuredBy 'rolesanywhere.amazonaws.com', threshold 45, enabled true; the later PutNotificationSettings(threshold=100 -> configuredBy=<account id>) and ResetNotificationSettings(-> threshold 45, configuredBy rolesanywhere.amazonaws.com) legs were never reached.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay suite simparity-behavioral-aws-v1.
2. Cause 1: GET /trustanchor/zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz (check rolesanywhere.get-trust-anchor-malformed-id-validation-rejected). Expect 400 ValidationException whose message renders the UUID-or-trust-anchor-ARN alternation; observe only the bare UUID pattern.
3. Cause 2: POST /profiles with name set and roleArns holding 251 role ARNs (check rolesanywhere.create-profile-role-arns-exceed-max-rejected). Expect 400 ValidationException with the '[...]' value rendering and 'Member must have length less than or equal to 250'; compare the Message byte for byte.
4. Cause 3: GET /trustanchor/{random uuid} and GET /profile/{random uuid}, and repeat both after deleting a real trust anchor / profile (checks rolesanywhere.get-trust-anchor-nonexistent-not-found, rolesanywhere.trust-anchor-delete-removes-resource, rolesanywhere.profile-delete-removes-resource). Compare $.message with 'TrustAnchor not found' and with "The item doesn't exist. arn: <profile arn> ; aborting.".
5. Cause 4: POST /profiles with name and roleArns only - no enabled, no durationSeconds, no acceptRoleSessionName (check rolesanywhere.create-profile-defaults-reflect-documented-values) - then GET /profile/{profileId}. Expect enabled=false in both; observe true.
6. Cause 5: POST /trustanchors with name and a genuine CERTIFICATE_BUNDLE source and no notificationSettings, then GET /trustanchor/{trustAnchorId} (check rolesanywhere.put-and-reset-notification-settings). Expect a CA_CERTIFICATE_EXPIRY/ALL setting with threshold 45, enabled true, configuredBy rolesanywhere.amazonaws.com; then PutNotificationSettings(threshold 100) and ResetNotificationSettings and re-read to confirm the configuredBy/threshold round trip.
