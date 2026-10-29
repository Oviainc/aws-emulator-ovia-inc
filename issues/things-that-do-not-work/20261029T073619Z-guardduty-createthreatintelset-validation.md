Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2b09bad8439b0b0ac599f9d638bea059710099b4

# ExpectedBucketOwner is stored unchecked on the IPSet and ThreatIntelSet creates/updates, although the crate already enforces the 12-digit account-id form for the identically documented member

_Filed: 2026-10-29T07:36:19Z_
_Operation: CreateThreatIntelSet | Class: validation_

> _Note: this issue was identified against the code base at commit `2b09bad8439b0b0ac599f9d638bea059710099b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ExpectedBucketOwner` is the same fact on five of this service's resources — "The Amazon Web Services account ID that owns the Amazon S3 bucket specified in the location parameter" — and this crate enforces one form for it in exactly one place and skips it in four handlers.

ENFORCED: the publishing-destination path runs the value through `crate::state::validate_destination_property_forms`, which refuses anything that is not exactly 12 ASCII digits, with a comment stating that the handler-side check exists precisely so the helper "states the whole rule" rather than relying on the model's length constraint. The entity-set family (`CreateThreatEntitySet` / `CreateTrustedEntitySet` / their updates) gets the same rule at the request boundary, because their `ExpectedBucketOwner` member targets a shape the model gives `pattern ^[0-9]+$`.

NOT ENFORCED: `CreateThreatIntelSet`, `UpdateThreatIntelSet`, `CreateIPSet` and `UpdateIPSet` bind `ExpectedBucketOwner` to `AccountId`, which the model constrains by LENGTH ONLY (`len 12..12`) and by no characters — so the boundary validator is not the enforcement point for these four, and the handler is. All four store `input.expected_bucket_owner.clone()` verbatim, and `GetIPSet` / `GetThreatIntelSet` hand the value straight back. A 12-character value that cannot be an account id at all (`"abcdefghijkl"`, `"owner1234567"`, `"arn:aws:s3:::"`) is accepted, persisted and reported as the account that owns the bucket.

The crate already owns the reader this needs: `crate::state::is_account_id` (12 ASCII digits, form only), which every other account-id-valued member of this service is held to (`EnableOrganizationAdminAccount` / `DisableOrganizationAdminAccount` for `AdminAccountId`, `AcceptInvitation` / `AcceptAdministratorInvitation` for `MasterId` / `AdministratorId`, and `account_ids_rejection` for every `AccountIds` list). `CreateIPSet` even states the pattern this finding is about, in its own comment, for the sibling member `Name`: "The documented input rule the model cannot express is checked BEFORE anything is written … while the modelled constraint on the member is a length only" — and then stores `ExpectedBucketOwner`, whose modelled constraint is also a length only, without any such check.

The four handlers are one gap; `CreateThreatIntelSet` is named as the anchor because neither the ThreatIntelSet create nor its update applies the check.

## Evidence

THE RULE THE CRATE ALREADY IMPLEMENTS FOR THIS MEMBER — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs:5893-5902 (`validate_destination_property_forms`):
```
    if let Some(owner) = properties.expected_bucket_owner.as_deref() {
        // "The 12-digit Amazon Web Services account ID of the account that owns the
        // Amazon S3 bucket"
        // (`smithy_query(query="doc",shape="DestinationProperties",member="ExpectedBucketOwner")`).
        // The model's own `len 12..12` refuses another length at the boundary; the
        // length is checked here too so this helper states the whole rule.
        if owner.len() != ACCOUNT_ID_LEN || !owner.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid_parameter_message(EXPECTED_BUCKET_OWNER_MEMBER));
        }
    }
```

THE READER THE CRATE ALREADY HAS — state.rs:1561-1569:
```
/// Is `id` a syntactically well-formed account id — exactly [`ACCOUNT_ID_LEN`]
/// decimal digits?
///
/// Form only: a well-formed id need not name an account this administrator has a
/// member record for, which is a different question and a different answer (see
/// [`not_a_member_message`]).
pub fn is_account_id(id: &str) -> bool {
    id.len() == ACCOUNT_ID_LEN && id.bytes().all(|b| b.is_ascii_digit())
}
```

THE FOUR HANDLERS THAT SKIP IT:
* src/ops/create_threat_intel_set.rs:235 — `expected_bucket_owner: input.expected_bucket_owner.clone(),` (nothing earlier in `handle` reads `input.expected_bucket_owner`; the only pre-write refusals are the tag ceiling at :172, the detector lookup at :183 and the quota at :218).
* src/ops/update_threat_intel_set.rs:237-239 —
```
        if let Some(owner) = input.expected_bucket_owner {
            set.expected_bucket_owner = Some(owner);
        }
```
* src/ops/create_ip_set.rs:237 — `input.expected_bucket_owner.clone(),`
* src/ops/update_ip_set.rs:214-217 —
```
        if let Some(owner) = input.expected_bucket_owner.as_ref() {
            changed |= ip_set.expected_bucket_owner.as_deref() != Some(owner.as_str());
            ip_set.expected_bucket_owner = Some(owner.clone());
        }
```

THE SAME FILE STATING THE PATTERN IT THEN SKIPS — src/ops/create_ip_set.rs:164-174:
```
        // The documented input rule the model cannot express is checked BEFORE
        // anything is written or the clock is read, so a refused request leaves no
        // IPSet behind: "allowed characters are alphanumeric, whitespace, dash (-),
        // and underscores (_)"
        // (`smithy_query(query="doc",shape="CreateIPSetRequest",member="Name")`),
        // while the modelled constraint on the member is a length only.
        if !crate::state::is_ip_set_name(&input.name) {
```

WHY THE BOUNDARY DOES NOT COVER THESE FOUR — `smithy_query(query="shape", shape="CreateIPSetRequest,CreateThreatIntelSetRequest,UpdateIPSetRequest,UpdateThreatIntelSetRequest,CreateThreatEntitySetRequest")`:
* `CreateIPSetRequest.ExpectedBucketOwner`: `{"target": "AccountId", "jsonName": "expectedBucketOwner", "length": {"min": 12, "max": 12}, "doc": "<p>The Amazon Web Services account ID that owns the Amazon S3 bucket specified in the <b>location</b> parameter.</p>"}` — no `pattern`. Identical for `CreateThreatIntelSetRequest`, `UpdateIPSetRequest` and `UpdateThreatIntelSetRequest`.
* `CreateThreatEntitySetRequest.ExpectedBucketOwner`: `{"target": "ExpectedBucketOwner", "jsonName": "expectedBucketOwner", "length": {"min": 12, "max": 12}, "pattern": "^[0-9]+$", "doc": "<p>The Amazon Web Services account ID that owns the Amazon S3 bucket specified in the <b>location</b> parameter.</p>"}` — same sentence, digits-only value space declared.

THE READ THAT HANDS THE UNCHECKED VALUE BACK — src/ops/get_threat_intel_set.rs:202 `expected_bucket_owner: set.expected_bucket_owner.clone(),` and src/ops/get_ip_set.rs:187 `expected_bucket_owner: ip_set.expected_bucket_owner.clone(),`.

TEST COVERAGE: the rule IS pinned on the publishing-destination path — tests/all/publishing_destination_lifecycle.rs:559-561 `"expectedBucketOwner is 12 characters but not 12 digits" … json!({… "expectedBucketOwner": "owner1234567"}) … "expectedBucketOwner"` and tests/all/publishing_destination_update.rs:449-451 (same row). On the IPSet/ThreatIntelSet path every row uses a digit string (tests/all/ip_set_lifecycle.rs:174 `"expectedBucketOwner": "123456789012"`, tests/all/ip_set_update_and_delete.rs:197/234/239, tests/all/threat_intel_sets.rs:202, tests/all/threat_intel_set_update.rs:203/241/246), so NO test pins acceptance of a non-digit value, and the suite doc that enumerates the member classes (tests/all/ip_set_lifecycle.rs:124-131) states no reason for leaving it unchecked.

## Steps to Reproduce

1. Create a detector: `POST /detector` with `{"enable": true}`; note the returned `detectorId`.
2. Create a threat list with a 12-character non-digit owner: `POST /detector/{detectorId}/threatintelset` with `{"name": "list", "format": "TXT", "location": "s3://bucket/list.txt", "activate": true, "expectedBucketOwner": "abcdefghijkl"}`. It answers HTTP 200 with a minted `threatIntelSetId` (create_threat_intel_set.rs:235 stores the value verbatim).
3. Read it back: `GET /detector/{detectorId}/threatintelset/{threatIntelSetId}` reports `"expectedBucketOwner": "abcdefghijkl"` (get_threat_intel_set.rs:202) — a value reported as the account that owns the bucket that cannot be an account id.
4. Repeat against `POST /detector/{detectorId}/ipset`, `POST /detector/{detectorId}/ipset/{ipSetId}` (UpdateIPSet) and `POST /detector/{detectorId}/threatintelset/{threatIntelSetId}` (UpdateThreatIntelSet) with the same value: all four accept it.
5. Contrast with the sibling entity set: `POST /detector/{detectorId}/threatentityset` carrying `"expectedBucketOwner": "abcdefghijkl"` is refused at the request boundary by the model's `^[0-9]+$`.
6. Contrast with the publishing destination: `POST /detector/{detectorId}/publishingDestination` with `destinationProperties.expectedBucketOwner = "abcdefghijkl"` is refused HTTP 400 `InvalidInputException` naming `expectedBucketOwner`, at state.rs:5899 — the check the four handlers above never reach.
