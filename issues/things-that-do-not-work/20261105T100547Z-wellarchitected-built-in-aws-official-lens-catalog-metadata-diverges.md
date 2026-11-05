Issue-Source: trace-sweep-divergence
Issue-Trace: wellarchitected-catalog-list-lenses
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wellarchitected-catalog-list-lenses wellarchitected-lens-association-lifecycle
Issue-Shape-Id: com.amazonaws.wellarchitected#WellArchitectedApiServiceLambda
Issue-Key: wellarchitected|ListLenses|2xx->2xx|$.LensSummaries[*]
Issue-Key: wellarchitected|ListLensReviews|2xx->2xx|$.LensReviewSummaries[*].LensVersion
Issue-Commit: 2e710d778938761c1f752c2d9288acfa8834ac9b

# Built-in AWS official lens catalog metadata diverges from the published lens catalog

> _Note: this issue was identified against the code base at commit `2e710d778938761c1f752c2d9288acfa8834ac9b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListLenses

## Description
This bundle has 2 net-new causes, both in service wellarchitected, both grounded in wire fixtures of trace set simparity-wire-aws-v1. Both are the same fault seen from two operations: the emulator's built-in catalog of AWS_OFFICIAL lenses carries its own authored metadata (descriptions, creation/update timestamps, lens versions) rather than the metadata AWS publishes for those lenses, so every surface that projects a lens's catalog metadata reports wrong values. Neither divergence was marked owned or stub-blocked by the replay. No existing open issue covers lens catalog metadata: 20261103T090801Z-wellarchitected-exportlens-re-serializes-the-lens-document-instead-of.md concerns the ExportLens document of a CUSTOM lens, and 20261029T192731Z-wellarchitected-validationexception-envelopes-use-generic-framework-text-and-omit.md concerns error envelopes.

Cause 1 of 2: ListLenses returns AWS_OFFICIAL lens summaries whose Description, CreatedAt, UpdatedAt and LensVersion are the emulator's own authored values, not the catalog AWS serves.
AWS: Smithy (services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json): ListLenses GET /lenses, code 200, output ListLensesOutput{LensSummaries, NextToken}; LensSummary{LensArn, LensAlias (@length 1..128), LensName (@length 1..128), LensType -> LensType enum, Description -> LensDescription (@length 1..1024), CreatedAt -> Timestamp, UpdatedAt -> Timestamp, LensVersion (@length 1..32), Owner -> AwsAccountId, LensStatus -> LensStatus enum}. The model does not enumerate the catalog contents, so for the per-lens values this is recorded behavior only: for each AWS_OFFICIAL lens the recording shows one fixed summary — the alias/ARN/name AWS assigns, the description AWS publishes for that lens, the lens's own publication CreatedAt/UpdatedAt instants and its published LensVersion (e.g. the framework lens is described as providing 'a set of foundational questions for you to consider for all of your cloud architectures' and carries distinct CreatedAt and UpdatedAt values; the Connected Mobility lens is at LensVersion 2024-03-26, not 2025-01-15). Owner is 'AWS' and LensStatus 'PUBLISHED' for all of them, which the emulator already gets right.
Emulator: for all 16 catalog entries ($.LensSummaries[0] through $.LensSummaries[15]) the summary differs from the recorded one: the Description is re-worded prose the emulator authored, CreatedAt and UpdatedAt are set to the same synthetic instant per lens (both equal, e.g. one shared epoch value for the framework lens) instead of the lens's real publication and revision instants, and LensVersion is a different date string for at least the Connected Mobility lens.
Why: a client that keys off a lens's published LensVersion to decide whether a review must be upgraded, or that renders the catalog description and 'last updated' date to a user, reads values that do not exist in AWS; version comparisons against the real catalog never line up, and UpdatedAt==CreatedAt hides that a lens was ever revised.
Fix: the AWS_OFFICIAL lens catalog data the ListLenses handler and the lens-summary serializer read — populate description, created/updated timestamps and version from the published catalog per lens instead of authored placeholders.
Traces: wellarchitected-catalog-list-lenses#task#1.

Cause 2 of 2: ListLensReviews reports the wrong LensVersion for a lens review of an AWS official lens, because the review summary inherits the same wrong catalog version.
AWS: Smithy: ListLensReviews GET /workloads/{WorkloadId}/lensReviews, code 200, output ListLensReviewsOutput{WorkloadId, MilestoneNumber, LensReviewSummaries, NextToken}; LensReviewSummary{LensAlias, LensArn, LensVersion (@length 1..32), LensName, LensStatus, UpdatedAt, RiskCounts, Profiles, PrioritizedRiskCounts}. Recorded behavior: LensVersion on a review summary is the version of the lens the review was created against, i.e. the catalog version of that AWS official lens (for the lens in this fixture, '2020-02-04').
Emulator: returns '2025-01-15' for the same review's LensVersion ($.LensReviewSummaries[1].LensVersion), the version its own catalog entry carries; the rest of the summary matches.
Why: the lens version on a review is what tells a client whether the review is behind the current lens and must be upgraded; a wrong version makes upgrade decisions and version-pinned reporting wrong, and it disagrees with the version the same emulator would report for the lens elsewhere.
Fix: the same AWS_OFFICIAL lens catalog data, read by the ListLensReviews handler / lens-review summary serializer; once the catalog carries the published versions, the review summary inherits the right one.
Traces: wellarchitected-lens-association-lifecycle#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 12 fixtures replayed, 94 steps, 71 passed, 23 mismatched, score 75.5%, 19 divergence clusters; emulator_up true, bootstrap_signal false. The two clusters below carry empty owned_by and empty stub_path.

Cause 1 — fixture wellarchitected-catalog-list-lenses, step task#1, ListLenses, expected status 200 / actual 200, verdict mismatch. Divergence keys: $.LensSummaries[0] .. $.LensSummaries[15] (all 16 entries). Examples from the detail: LensSummaries[0] expected Description 'The AWS Well-Architected Framework Lens provides a set of foundational questions for you to consider for all of your cloud architectures.' with CreatedAt 1568405610 and UpdatedAt 1790853813, got Description 'The Amazon Web Services Well-Architected Framework provides a consistent approach for customers and partners to evaluate architectures, and implement designs that scale over time.' with CreatedAt 1740441600 and UpdatedAt 1740441600. LensSummaries[1] (connectedmobility) expected Description 'Best practices for Connected Mobility workload', LensVersion '2024-03-26', CreatedAt/UpdatedAt 1711050610, got a long authored description, LensVersion '2025-01-15' and CreatedAt/UpdatedAt 1736899200. The individual epoch numbers above are the recorded catalog instants for those lenses, not per-run values; the requirement is that each lens's description/version/instants come from the published catalog, with UpdatedAt tracking revisions separately from CreatedAt.

Cause 2 — fixture wellarchitected-lens-association-lifecycle, step task#3, ListLensReviews, expected status 200 / actual 200, verdict mismatch. Detail: $.LensReviewSummaries[1].LensVersion: value_mismatch (expected '2020-02-04', got '2025-01-15').

Smithy citations: ListLenses GET /lenses -> 200 {LensSummaries, NextToken}; LensSummary{..., Description -> LensDescription @length 1..1024, CreatedAt/UpdatedAt -> Timestamp, LensVersion @length 1..32, Owner -> AwsAccountId, LensStatus}; ListLensReviews GET /workloads/{WorkloadId}/lensReviews -> 200 {..., LensReviewSummaries}; LensReviewSummary{LensAlias, LensArn, LensVersion, LensName, LensStatus, UpdatedAt, RiskCounts, ...}.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 and look at service wellarchitected; the two clusters below are the ones with no owned_by and no stub_path.

Cause 1: GET /lenses (ListLenses) with no filters. Compare each returned LensSummary against the recorded catalog: the Description text, the CreatedAt/UpdatedAt pair (the emulator emits one synthetic instant for both) and LensVersion all differ, for every one of the 16 AWS_OFFICIAL entries. Re-replay wellarchitected-catalog-list-lenses#task#1.

Cause 2: CreateWorkload, associate a second AWS official lens, then GET /workloads/{WorkloadId}/lensReviews (ListLensReviews). The second review summary's LensVersion is the emulator's catalog version ('2025-01-15') instead of the version the review was created against ('2020-02-04'). Re-replay wellarchitected-lens-association-lifecycle#task#3.
