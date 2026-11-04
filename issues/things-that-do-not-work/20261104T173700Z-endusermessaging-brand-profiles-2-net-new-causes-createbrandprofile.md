Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: endusermessaging.create-brand-profile-seeds-default-attribute-template endusermessaging.brand-profile-deletion-protection-blocks-then-allows-delete
Issue-Shape-Id: com.amazonaws.endusermessaging#EndUserMessaging
Issue-Key: endusermessaging.create-brand-profile-seeds-default-attribute-template|run#7|AssertionError|assert seeded > 0, f"attributesCreated was {seeded!r}, expected a positive seed count"
Issue-Key: endusermessaging.brand-profile-deletion-protection-blocks-then-allows-delete|run#9|AssertionError|assert message == (
Issue-Commit: 499424707e34574cc6d85e5e4758775b0381202c

# endusermessaging brand profiles: 2 net-new causes — CreateBrandProfile seeds no default attribute template (attributesCreated always 0), and the brand-profile conflict/not-found refusals carry emulator-authored prose instead of AWS's message templates

> _Note: this issue was identified against the code base at commit `499424707e34574cc6d85e5e4758775b0381202c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBrandProfile

## Description
Two independent root causes in the endusermessaging brand-profile surface, both grounded in behavioral checks that the emulator answered (the service is bootstrapped and serving). No open endusermessaging issue exists; the only filed endusermessaging work items are the scaffold-stub issues for CreateBrandProfileAttributes and CreateNotifyCodeConfiguration, which cover different operations and do not cover either cause below.

=== Cause 1 of 2: CreateBrandProfile creates no default attributes, so attributesCreated is always 0 and ListBrandProfileAttributes is empty for a fresh profile ===
AWS: Smithy model com.amazonaws.endusermessaging#CreateBrandProfile, output member `attributesCreated` (Integer, @required), documented as "The number of default attributes that were created for the brand profile." The member exists because creating a brand profile seeds a default attribute template: AWS answers CreateBrandProfile with a positive `attributesCreated`, and ListBrandProfileAttributes (GET /v1/brand-profiles/{brandProfileId}/attributes, paginated on nextToken/maxResults, items `brandProfileAttributes`) immediately returns exactly that many BrandProfileAttributeSummary entries — distinct, non-blank `attributeName` values, each with the @required `attributeType`, `createdAt` and `updatedAt`. The exact names and count of the seeded template are recorded behavior only (the model does not enumerate them); what the model and the recording do require is that the count is positive and that it equals the number of names the paginated list returns.
Emulator: CreateBrandProfile mints the profile with an empty attribute map and derives the count from it, so `attributesCreated` is 0 for every create and the subsequent ListBrandProfileAttributes page is empty. The check fails at `assert seeded > 0` with "attributesCreated was 0, expected a positive seed count". The handler's own comment states this explicitly ("This call creates no default attributes; `attributesCreated` counts this map.").
Why: a client that reads `attributesCreated` to learn how many attributes it must fetch, reconcile or page through sees 0 and concludes the profile has no attribute template at all; any flow that updates or reads a default attribute (company name, address, logo slots) then fails with a not-found against an attribute AWS would have seeded. Pagination over the attribute list is also never exercised, because the list is always empty.
Fix: CreateBrandProfile's handler and the service's brand-profile state — the create path must seed the default attribute template into the profile's attribute map before reporting, and ListBrandProfileAttributes must page those seeded entries; `attributesCreated` must report the number actually seeded, not the size of an empty map.

=== Cause 2 of 2: brand-profile ConflictException and ResourceNotFoundException messages are emulator-authored sentences, not AWS's message templates (and resourceType uses the wrong vocabulary) ===
AWS: Smithy model declares ConflictException (HTTP 409, members message @required, resourceId, resourceType) on DeleteBrandProfile and ResourceNotFoundException (HTTP 404, same member set) on DeleteBrandProfile/GetBrandProfile. The status codes and error codes the emulator returns are correct; the message text is not. AWS words these refusals with a fixed template plus quoted variable parts taken from the request and from the server's state:
  - deletion-protection conflict: fixed text `Conflict occurred - Reason="DELETION_PROTECTION_ENABLED" ResourceType="brand-profile"`. Every part of this message is fixed text for this error class: `Conflict occurred - Reason=` and `ResourceType=`, with the reason token `DELETION_PROTECTION_ENABLED` (the state that caused the conflict) and the resource type `brand-profile` (the resource's type in AWS's hyphenated vocabulary). No request-supplied value appears in it — in particular the brand profile id does NOT.
  - not found: template `Resource not found - ResourceType="<type>" ResourceId="<id>"`, where `Resource not found - ResourceType=` and `ResourceId=` are fixed text, `<type>` is the resource's type (`brand-profile`), and `<id>` is the identifier taken verbatim from the request (the brandProfileId the caller supplied). The id is a value from the request, not part of the template.
The same wording applies across this service's resources with only the type token changing — the notify-code-configuration checks assert `Conflict occurred - Reason="DELETION_PROTECTION_ENABLED" ResourceType="notify-code-configuration"` and `Resource not found - ResourceType="notify-code-configuration" ResourceId="<id>"` — so this is the service's message convention, not one recording's text.
Emulator: DeleteBrandProfile refuses a deletion-protected profile with 409 ConflictException but the message is the emulator's own sentence `Brand profile <id> cannot be deleted while deletion protection is enabled.` — different fixed text, and it interpolates the brand profile id, which AWS's conflict message does not contain. The check fails at `assert message == ...` with "unexpected message 'Brand profile bp-... cannot be deleted while deletion protection is enabled.'". The same cause covers the not-found message on the back half of that check: the shared constructor builds `Brand profile <id> was not found.` instead of `Resource not found - ResourceType="brand-profile" ResourceId="<id>"`, so that assertion is the next one to fail once the conflict text is corrected. Both error structures also populate `resourceType` as `BRAND_PROFILE`, while AWS's own messages for this service spell the type `brand-profile`.
Why: operators and support tooling match on refusal text, and test suites and runbooks assert it; a client reading `Reason="..."` out of the conflict message to distinguish deletion protection from other 409 conflicts finds no Reason token at all and cannot tell the conflict classes apart. The hyphenated-vs-uppercase `resourceType` means a client switching on that member against AWS's vocabulary never matches.
Fix: the brand-profile refusal construction — DeleteBrandProfile's deletion-protection branch and the service's shared brand-profile not-found error constructor — must emit AWS's templates, with the resource-type token and (for not-found) the request's identifier as the only variable parts, and must report the resource type in the same hyphenated vocabulary the message uses. Apply the same convention to the notify-code-configuration refusals once that operation is implemented, so those checks do not fail on text after the stub is filled.

Traces: see trace_ids — both failing checks are endusermessaging behavioral checks under the suite's `endusermessaging` heading. Cause 1 is grounded in create-brand-profile-seeds-default-attribute-template; Cause 2 in brand-profile-deletion-protection-blocks-then-allows-delete.

## Evidence
Cause 1 — check endusermessaging.create-brand-profile-seeds-default-attribute-template [mismatch]:
  Expected (AWS): CreateBrandProfile returns attributesCreated > 0, and the paginated ListBrandProfileAttributes right after returns exactly that many distinct, non-blank attributeName values.
  Actual (emulator): attributesCreated == 0; the attribute list is empty.
  Assertion: `AssertionError: attributesCreated was 0, expected a positive seed count` (assert seeded > 0).
  Source confirming the cause: services/service-com-amazonaws-endusermessaging-endusermessaging/src/ops/create_brand_profile.rs:235-236 ("This call creates no default attributes; `attributesCreated` counts this map." / `attributes: BTreeMap::new()`) and :257 (`attributes_created: i32::try_from(profile.attributes.len())`).

Cause 2 — check endusermessaging.brand-profile-deletion-protection-blocks-then-allows-delete [mismatch]:
  Status/code: 409 ConflictException — correct.
  Expected message (AWS, fixed text in full): Conflict occurred - Reason="DELETION_PROTECTION_ENABLED" ResourceType="brand-profile"
  Actual message (emulator): Brand profile bp-480d16c1e00000001 cannot be deleted while deletion protection is enabled.   (the bp-… id is this run's value; the divergence is the whole template, plus the fact that AWS's conflict message interpolates no id at all)
  Assertion: `AssertionError: unexpected message 'Brand profile bp-480d16c1e00000001 cannot be deleted while deletion protection is enabled.'`
  Same cause, later assertion in the same check — expected: Resource not found - ResourceType="brand-profile" ResourceId="<brandProfileId from the request>" ; emulator constructs: Brand profile <id> was not found.
  Source confirming the cause: services/service-com-amazonaws-endusermessaging-endusermessaging/src/ops/delete_brand_profile.rs:183-195 (conflict message format! string) and services/service-com-amazonaws-endusermessaging-endusermessaging/src/state.rs:224 (BRAND_PROFILE_RESOURCE_TYPE = "BRAND_PROFILE") and :228-234 (brand_profile_not_found message format! string).
  Cross-resource corroboration that the templates are the service convention, not one recording: the notify-code-configuration checks assert the identical wording with ResourceType="notify-code-configuration".

## Steps to Reproduce
Cause 1:
1. POST /v1/brand-profiles with {"brandProfileName": "<any name matching ^[A-Za-z0-9_ -]*[A-Za-z0-9_-][A-Za-z0-9_ -]*$"}.
2. Read `attributesCreated` from the 200 response — the emulator reports 0, AWS reports a positive seed count.
3. GET /v1/brand-profiles/{brandProfileId}/attributes?maxResults=5 (or paginate list_brand_profile_attributes) — the emulator returns an empty `brandProfileAttributes` list; AWS returns exactly `attributesCreated` distinct, non-blank attribute names.

Cause 2:
1. POST /v1/brand-profiles with {"brandProfileName": "<any valid name>", "deletionProtectionEnabled": true}; note the returned brandProfileId.
2. DELETE /v1/brand-profiles/{brandProfileId}. The response is 409 ConflictException (correct) but Error.Message is `Brand profile <id> cannot be deleted while deletion protection is enabled.` instead of AWS's `Conflict occurred - Reason="DELETION_PROTECTION_ENABLED" ResourceType="brand-profile"`. Error.resourceType is `BRAND_PROFILE` instead of `brand-profile`.
3. PUT /v1/brand-profiles/{brandProfileId} with {"deletionProtectionEnabled": false}, then DELETE /v1/brand-profiles/{brandProfileId} (succeeds), then GET /v1/brand-profiles/{brandProfileId}. The response is 404 ResourceNotFoundException (correct) but Error.Message is `Brand profile <id> was not found.` instead of AWS's `Resource not found - ResourceType="brand-profile" ResourceId="<id>"`.
