Issue-Source: trace-sweep-divergence
Issue-Trace: personalize-dataset-lifecycle-and-validation
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: personalize-dataset-lifecycle-and-validation
Issue-Shape-Id: com.amazonaws.personalize#AmazonPersonalize
Issue-Key: personalize|DescribeDataset|2xx->2xx|$.dataset.datasetType
Issue-Commit: b4a0d642bb551c1144d98d8c876f3d4bc2002490

# DescribeDataset rewrites the caller's datasetType instead of echoing it

> _Note: this issue was identified against the code base at commit `b4a0d642bb551c1144d98d8c876f3d4bc2002490`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDataset

## Description
ONE cause.

Cause 1 of 1: DescribeDataset reports a canonicalized spelling of datasetType rather than the spelling the dataset was created with.
AWS: recorded behavior — HTTP 200 and $.dataset.datasetType echoes the datasetType string the CreateDataset request supplied, character for character. Smithy: CreateDataset.datasetType targets DatasetType, which is a plain string (@length 0..256, @pattern ^[A-Za-z_]+$) and NOT an enum, so no canonical casing is modeled; the Dataset.datasetType documentation merely lists the value names in prose (Interactions, Items, Users, Actions, Action interactions). Because the shape is a free string the recorded round-trip is the requirement: whatever casing the caller stored is what Describe returns.
Emulator: CreateDataset passes the input through canonical_dataset_type() and stores the canonical spelling (src/ops/create_dataset.rs:178-185, 252), so a dataset created as the all-caps form is read back title-cased — the recorded step expected 'INTERACTIONS' and got 'Interactions'. Everything else in the reply, including the dataset ARN (which does keep the upper-case type segment), matched.
Why: a client that compares the returned datasetType against the constant it sent, keys a map on it, or routes per dataset type on an exact string match fails to recognize its own dataset; code that round-trips a Describe result back into another request also mutates the value silently. Case-insensitive ACCEPTANCE at CreateDataset is correct and should stay — only the stored/echoed spelling is wrong.
Fix: CreateDataset's state write — keep canonical_dataset_type() for validation and for deriving the ARN segment, but persist the caller's original datasetType string in DatasetRecord so DescribeDataset (and ListDatasets) echo it verbatim.
Traces: personalize-dataset-lifecycle-and-validation#task#5.

## Evidence
personalize-dataset-lifecycle-and-validation#task#5, DescribeDataset: status 200 as expected; $.dataset.datasetType value_mismatch — expected 'INTERACTIONS' (the spelling the fixture's CreateDataset supplied), got 'Interactions'. No other member diverged. Smithy: DatasetType = string, @length 0..256, @pattern ^[A-Za-z_]+$, no @enum; Dataset.datasetType documented only as a prose list of value names. Source: services/service-com-amazonaws-personalize-amazonpersonalize/src/ops/create_dataset.rs:178-185 (canonical_dataset_type) and :252 (dataset_type stored as the canonical form).

## Steps to Reproduce
1. Replay fixture personalize-dataset-lifecycle-and-validation: CreateSchema, CreateDatasetGroup, then CreateDataset with datasetType spelled in all caps (the Interactions type).
2. At task#5 call DescribeDataset with the returned datasetArn -> $.dataset.datasetType comes back title-cased ('Interactions') instead of the all-caps string that was sent ('INTERACTIONS').

## Resolution

Fixed, but NOT by the rule stated above: the report's "echoes the request character for character"
is a claim, and the recording disagrees with it. Echoing the request's own string was implemented
first, and the recorded step still diverged on $.dataset.datasetType — so the recorded value is not
a copy of what that request wrote. What the recording does report is the type's UPPER-CASE name,
which is also the spelling the same reply's datasetArn carries as its last segment, so the rule
implemented is: a dataset's type is stored and reported in the upper-case name Amazon Personalize
identifies the type by, whatever casing the (case-insensitively accepted) create wrote. Case
insensitive ACCEPTANCE is unchanged, and a group still holds at most one dataset per type however
each create spelled it. Covered by tests/all/dataset_type_casing.rs (every documented type, one
differing casing per row) and tests/all/dataset_type_echo.rs (one type recreated under four
casings; the dataset's free-string `name` is still echoed verbatim).

