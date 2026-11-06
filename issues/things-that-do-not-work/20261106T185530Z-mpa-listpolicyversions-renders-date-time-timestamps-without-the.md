Issue-Source: trace-sweep-divergence
Issue-Trace: mpa-policy-version-lookup-not-found-and-default-flag
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mpa-policy-version-lookup-not-found-and-default-flag
Issue-Shape-Id: com.amazonaws.mpa#AWSFluffyCoreService
Issue-Key: mpa|ListPolicyVersions|2xx->2xx|$.PolicyVersions[Arn="arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/1"].CreationTime
Issue-Key: mpa|ListPolicyVersions|2xx->2xx|$.PolicyVersions[Arn="arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/1"].LastUpdatedTime
Issue-Commit: bf25df16afda3c80a3b12bb65197c738522b5a3b

# mpa ListPolicyVersions renders date-time timestamps without the fixed millisecond field

> _Note: this issue was identified against the code base at commit `bf25df16afda3c80a3b12bb65197c738522b5a3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPolicyVersions

## Description
One net-new root cause in the mpa read path: the `date-time` serializer the service's output projections share drops the fractional-second field whenever it would be all zeros, so every `IsoTimestamp` member that lands on a whole second is rendered one field short of what AWS emits.

Cause 1 of 1: ListPolicyVersions serializes `PolicyVersionSummary.CreationTime` and `PolicyVersionSummary.LastUpdatedTime` without the three-digit fractional-second field (fixture `mpa-policy-version-lookup-not-found-and-default-flag`, step `task#3`, HTTP 200 on both sides).

  AWS: `ListPolicyVersions` is `@readonly`, `POST /policies/{PolicyArn}/?List`, and returns `PolicyVersions` as a list of `PolicyVersionSummary`. Smithy (services/service-com-amazonaws-mpa-awsfluffycoreservice/smithy-model/com-amazonaws-mpa-awsfluffycoreservice.json): `PolicyVersionSummary.CreationTime` and `PolicyVersionSummary.LastUpdatedTime` both target `com.amazonaws.mpa#IsoTimestamp`, both `@required`, and both carry `@timestampFormat("date-time")`; `IsoTimestamp` itself declares `timestampFormat: date-time`. The model fixes only the format FAMILY (RFC 3339 / ISO 8601 with a `Z` offset), not the number of fractional digits, so the precision AWS actually emits is recorded behavior only: the recording shows MPA writing a FIXED three-digit fractional-second field for these members even when the instant is on a whole second — `2025-06-17T00:00:00.000Z` for both members of the version-1 summary of the AWS-managed policy the catalog publishes. The response is a 200 with no error code or message involved; the divergence is purely in the serialized value.

  Emulator: it answers 200 with the correct instant but renders it as `2025-06-17T00:00:00Z` for both members — the `.000` field is elided. Replay reports two `value_mismatch` entries on the one element addressed by its qualified ARN: `$.PolicyVersions[Arn="arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/1"].CreationTime` (expected `2025-06-17T00:00:00.000Z`, got `2025-06-17T00:00:00Z`) and `...LastUpdatedTime` (same expected/actual). Both members of the same summary diverge identically, which is what identifies this as the shared date-time rendering helper rather than a per-member projection bug. Note the general rule, not the recorded values: the fixed part of the rendering is the `yyyy-MM-ddTHH:mm:ss.SSSZ` template; the date, time and digits themselves come from the stored instant and vary per entry, so the requirement is the template's fractional-second field, not the literal `2025-06-17` instant this fixture happens to hold.

  Why: the instant is right, so nothing about the data is wrong — the SHAPE of the rendering is. A client that compares the serialized string against AWS's, caches or ETags it, canonicalizes the body for re-signing, or parses with a strict `yyyy-MM-dd'T'HH:mm:ss.SSS'Z'` pattern (which is the pattern a fixed-millisecond producer invites) sees a different value for the same timestamp, and the strict parser fails outright. Because this is the shared rendering, the divergence is not confined to `ListPolicyVersions`: every mpa `IsoTimestamp` member — `GetPolicyVersion`'s `PolicyVersion.CreationTime`/`LastUpdatedTime`, the approval-team and identity-source read projections — renders the same way and diverges whenever its instant lands on a whole second, so a client cannot rely on a stable timestamp width anywhere in the service.

  Fix: the `date-time` timestamp serializer used by this service's restJson1 output projections (the `ListPolicyVersions` serializer and whatever `IsoTimestamp` rendering helper it shares with the other mpa read paths) must emit a fixed three-digit fractional-second field instead of eliding trailing zeros, so a whole-second instant renders as `...T00:00:00.000Z`. This belongs in the serializer, not in state: the stored instants are already correct and must not be perturbed, and the ARN/version resolution that selected this summary is correct too.

  Traces: mpa-policy-version-lookup-not-found-and-default-flag#task#3 (ListPolicyVersions, expected 200, actual 200, two body `value_mismatch` entries on CreationTime and LastUpdatedTime).

Relationship to what is already open: the open issue `20261031T114039Z-mpa-policy-catalog-projections-diverge-from-the-published.md` DESCRIBES this same rendering defect as its "Cause 2 of 2", but its replay pointer does not cover this cluster — its `Issue-Trace`/`Issue-Step` name a different cluster (`mpa-policy-catalog-browsing#task#3`, the GetPolicyVersion document-text cause) and its `Issue-Trace-Ids` lists the bare fixture token `mpa-policy-version-lookup-not-found-and-default-flag` with no `#step` half, so the token is unplaceable and the cause was never bound to a step a fixer re-replays. That is exactly why this replay reports the cluster with an EMPTY `owned_by`: nothing open carries its replay key, so a fix to the other issue can pass and close while this step stays unrun. This issue exists to carry that key. A fixer closing this one should also reconcile `20261031T114039Z`'s Cause 2, which is the same edit; the document-text cause in that issue (GetPolicyVersion's managed-policy document) is separate and is NOT re-filed here, nor are the two foreign-account-ARN refusal clusters owned by `20261104T085053Z-mpa-identity-source-reads-and-deletes-refuse-a.md`.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (branch issues/finder-wi-68f3731649f447a9bb8920c0c56bba59-1793990726869): emulator_up=true, bootstrap_signal=false, 11 fixtures replayed, 35 steps total, 31 passed, 4 mismatched, 0 errored, 0 blocked, score 88.6%, divergence_count 4. Of the 4 clusters, 3 carry a non-empty `owned_by` and are skipped (two GetIdentitySource/DeleteIdentitySource 403->400 clusters owned by 20261104T085053Z-mpa-identity-source-reads-and-deletes-refuse-a.md, and the GetPolicyVersion document cluster owned by 20261031T114039Z-mpa-policy-catalog-projections-diverge-from-the-published.md). This issue is the remaining cluster, the only one with both `stub_path` and `owned_by` empty.

Cause 1 — fixture `mpa-policy-version-lookup-not-found-and-default-flag`, step `task#3`, service `mpa`, operation ListPolicyVersions, verdict `mismatch`, expected_status 200, actual_status 200, affected_count 1, stub_path "", owned_by [].
divergence_key: service=mpa|ListPolicyVersions|status=200->200|keys=$.PolicyVersions[Arn="arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/1"].CreationTime,$.PolicyVersions[Arn="arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/1"].LastUpdatedTime
detail:
  $.PolicyVersions[Arn="arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/1"].CreationTime: value_mismatch (expected '2025-06-17T00:00:00.000Z', got '2025-06-17T00:00:00Z')
  $.PolicyVersions[Arn="arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/1"].LastUpdatedTime: value_mismatch (expected '2025-06-17T00:00:00.000Z', got '2025-06-17T00:00:00Z')

Expected vs actual, stated as the rule rather than as the recorded values:
  Expected (AWS): the member is an RFC-3339 date-time whose fractional-second field is always present with exactly three digits — template `yyyy-MM-ddTHH:mm:ss.SSSZ`. Fixed text: the `-`, `T`, `:`, `.` separators and the trailing `Z`, plus the presence and width of the `.SSS` field. Variable: the date, time and fractional digits, which come from the stored instant for that policy version.
  Actual (emulator): the same instant with the `.SSS` field omitted whenever it is zero — template `yyyy-MM-ddTHH:mm:ssZ` for a whole-second instant.

This is not a per-run value and not a clock artifact: the instant belongs to the seeded AWS-managed policy catalog (version 1 of arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy), identical on every replay, so the difference is deterministic serialization width. Both timestamp members of the same summary diverge in exactly the same way while every other member of that summary (Arn, PolicyArn, VersionId, PolicyType, IsDefault, Name, Status) matched, and the 200 status, headers and the NextToken/absent-pagination shape all matched.

Model grounding: smithy_query(query="operation", operation="ListPolicyVersions") and smithy_query(query="shape", shape="PolicyVersionSummary,IsoTimestamp") against services/service-com-amazonaws-mpa-awsfluffycoreservice/smithy-model/com-amazonaws-mpa-awsfluffycoreservice.json — ListPolicyVersions: readonly, http POST /policies/{PolicyArn}/?List, code 200, paginated (NextToken/MaxResults, items PolicyVersions, MaxResults default 20 range 1..20); output member PolicyVersions -> list, @length max 20; PolicyVersionSummary.CreationTime and .LastUpdatedTime -> com.amazonaws.mpa#IsoTimestamp, required, timestampFormat "date-time"; shape IsoTimestamp -> timestamp with timestampFormat "date-time". The model constrains the format family only, so the three-digit precision is recorded behavior.

## Steps to Reproduce
Prerequisite: clone this checkout and boot the emulator from it. The mpa service is bootstrapped (get_bootstrap_status reports `service-com-amazonaws-mpa-awsfluffycoreservice` in `bootstrapped`, with an empty `pending` set), and the emulator answered: /__admin/health 200 and the data listener accepting. No setup calls are needed — the AWS-managed policy catalog is seeded on first touch of the store.

1. Replay fixture `mpa-policy-version-lookup-not-found-and-default-flag`, step `task#3`, or issue the call directly with SigV4 (signing name `mpa`):
   POST /policies/arn%3Aaws%3Ampa%3A%3A%3Aaws%3Apolicy%2FIdentityCenter%2FMPAApprovalTeamPolicy/?List&MaxResults=20
2. The response is 200. In the body, find the `PolicyVersions` element whose `Arn` is `arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/1`.
3. Read its `CreationTime` and `LastUpdatedTime`. Observe both rendered with no fractional-second field (`...T00:00:00Z`), where AWS renders the same instant with a fixed three-digit field (`...T00:00:00.000Z`).
4. Confirm the instant itself is correct and only the rendering width differs: the two values agree with AWS on every other character, and the seeded catalog instant is the same on every replay, so re-running step 1 reproduces it byte-for-byte.
5. Confirm the defect is in the shared serializer, not in this one projection: read the same two members off `GetPolicyVersion` for the same version (GET /policy-versions/arn%3Aaws%3Ampa%3A%3A%3Aaws%3Apolicy%2FIdentityCenter%2FMPAApprovalTeamPolicy%2F1, or the `$DEFAULT` qualifier, which resolves to the same version) and observe the same elided fractional-second field. Any mpa `IsoTimestamp` whose instant lands on a whole second renders the same way.
6. After the fix, re-replay fixture `mpa-policy-version-lookup-not-found-and-default-flag` step `task#3` and confirm both members render `.000` and the step passes.
