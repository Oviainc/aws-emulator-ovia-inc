Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-rshealth-cluster-option-catalog-filters
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-rshealth-cluster-option-catalog-filters redshift-rshealth-describe-events-scoping redshift-rshealth-describe-identifier-mistakes
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|DescribeOrderableClusterOptions|4xx->2xx|$.ErrorResponse
Issue-Key: redshift|DescribeOrderableClusterOptions|4xx->2xx|status
Issue-Key: redshift|DescribeEvents|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DescribeEvents|4xx->2xx|$.ErrorResponse
Issue-Key: redshift|DescribeEvents|4xx->2xx|status
Issue-Key: redshift|DescribeClusters|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: redshift|DescribeClusters|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DescribeClusters|4xx->4xx|status
Issue-Key: redshift|DescribeClusters|2xx->4xx|$.DescribeClustersResponse
Issue-Key: redshift|DescribeClusters|2xx->4xx|status
Issue-Key: redshift|DescribeClusterSnapshots|2xx->4xx|$.DescribeClusterSnapshotsResponse
Issue-Key: redshift|DescribeClusterSnapshots|2xx->4xx|status
Issue-Key: redshift|DescribeClusterSnapshots|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: redshift|DescribeClusterSnapshots|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 5e177a4f4d28f6dc4714f734dc35ed5ad192c177

# Redshift describe-path request validation answers the wrong status and the wrong error taxonomy

> _Note: this issue was identified against the code base at commit `5e177a4f4d28f6dc4714f734dc35ed5ad192c177`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusters

## Description
Eight net-new root causes in the redshift service, all on the Describe* read path, all about the refusal the emulator authors for a filter/identifier the request supplies: it either accepts a filter AWS refuses (HTTP 200 where AWS answers 400), refuses a read AWS serves (400/404 where AWS answers 200 with an empty collection), raises a resource-not-found fault where AWS raises a parameter fault, or emits the generic non-AWS code ValidationException where Redshift's query protocol emits InvalidParameterValue / InvalidParameterCombination. Each cause is a separate validation site and must be closed separately. Open issue 20261029T200555Z-redshift-request-validation-answers-a-generic-validationexception-or.md covers a neighbouring set of validation sites (MaxRecords on other operations, DeleteCluster, parameter-group domains) and does NOT carry the replay keys below.

Cause 1 of 8: DescribeOrderableClusterOptions does not validate the ClusterVersion filter. AWS: a ClusterVersion that is not an existing Redshift cluster version is refused HTTP 400 with code InvalidParameterCombination and the message template "Cannot find Cluster version " + the ClusterVersion value from the request (fixed prefix, one variable part). Smithy: DescribeOrderableClusterOptions.ClusterVersion is an unconstrained String and the operation models NO errors at all, so this refusal is a service-level rule — recorded behavior only. Emulator: HTTP 200 with an OrderableClusterOptions list. Why: a client probing which versions exist by filtering gets a non-empty catalog back for a version that does not exist, so version-selection logic picks an unprovisionable version. Fix: the DescribeOrderableClusterOptions handler's filter validation. Traces: redshift-rshealth-cluster-option-catalog-filters#task#2.

Cause 2 of 8: the DescribeEvents SourceIdentifier/SourceType combination message is emulator-authored. AWS: DescribeEvents with SourceIdentifier but no SourceType is refused HTTP 400, code InvalidParameterCombination, message "Cannot specify source identifier without source type" — entirely fixed text, no variable parts and no trailing period. Smithy: DescribeEvents models no errors and SourceType is the SourceType enum, so the pairing rule is a service-level rule (recorded behavior). Emulator: the right status and code but the message "SourceType must also be provided when SourceIdentifier is supplied." — different fixed text and an added trailing period. Why: operators and log scrapers matching AWS's wording see text no AWS region emits. Fix: the DescribeEvents handler's parameter-combination refusal message. Traces: redshift-rshealth-describe-events-scoping#task#1 (cluster also covers #task#2).

Cause 3 of 8: DescribeEvents does not enforce the 14-day event-retention window. AWS: a StartTime further in the past than the 14-day event retention window is refused HTTP 400, code InvalidParameterCombination, message "Events occurring more than 14 days in the past are not available." — entirely fixed text (the 14-day limit is AWS's documented event retention period, not a per-request value). Emulator: HTTP 200 with an events payload. Why: a monitoring client that asks for a 30-day event history is silently served a window AWS refuses, so it concludes no events occurred in a period it was never allowed to query. Fix: the DescribeEvents handler's time-range validation. Traces: redshift-rshealth-describe-events-scoping#task#3 (cluster also covers #task#4).

Cause 4 of 8: a syntactically invalid ClusterIdentifier is answered as not-found instead of as a bad parameter. AWS: DescribeClusters whose ClusterIdentifier violates the identifier grammar (must begin with a letter; only ASCII letters, digits and hyphens; no trailing hyphen, no two consecutive hyphens) is refused HTTP 400 with code InvalidParameterValue and the fixed message "The parameter DBInstanceIdentifier is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens." (fixed text; AWS names the parameter DBInstanceIdentifier here, inherited from the shared RDS-family validator, and does NOT echo the offending value). Smithy: DescribeClusters.ClusterIdentifier is an unconstrained String and the only modeled 400 is InvalidTagFault, so the grammar check is a service-level rule (recorded behavior). Emulator: HTTP 404 with code ClusterNotFound and the message "Cluster <identifier> not found.". Why: a malformed identifier is reported as a missing cluster, so a client retries/creates against a name AWS will never accept instead of correcting it; the status also changes 400->404, which flips client-side retry classification. Fix: the DescribeClusters handler's identifier-grammar validation, before the state lookup. Traces: redshift-rshealth-describe-identifier-mistakes#task#1 (cluster also covers #task#2).

Cause 5 of 8: the DescribeClusters MaxRecords refusal uses an unmodelled code and emulator-authored text. AWS: a MaxRecords below the enforced minimum is refused HTTP 400 with code InvalidParameterValue and the fixed message "Invalid value for max records. Must be between 20 and 100" — fixed text with the documented bounds, no echo of the supplied value and no trailing period. Smithy: MaxRecords targets IntegerOptional with no @range, so the bound is a service rule, not a modeled constraint (recorded behavior). Emulator: HTTP 400 but code ValidationException — which is not in Redshift's query-protocol error vocabulary at all — and the message "Invalid value for MaxRecords: <n>. MaxRecords must be between 20 and 100.", which echoes the request value and adds a period. Why: a paginating client cannot classify ValidationException and so cannot distinguish "shrink the page size" from a malformed request. Fix: the MaxRecords validation in the DescribeClusters handler (shared Describe* validator). Traces: redshift-rshealth-describe-identifier-mistakes#task#3.

Cause 6 of 8: DescribeClusters refuses a read AWS serves with an empty collection. AWS: this DescribeClusters variant answers HTTP 200 with an empty Clusters collection (serialized as the empty element in the query protocol). Emulator: HTTP 400 and no DescribeClustersResponse at all. Hypothesis: it is the above-documented-maximum MaxRecords case — AWS does not enforce the upper bound of 100 and serves the call, while the emulator's symmetric bounds check rejects it. Why: the client is refused a call AWS answers, so a pagination loop that legitimately requests a large page aborts instead of returning results. Fix: the same shared Describe* MaxRecords/filter validator — it must not refuse inputs AWS accepts. Traces: redshift-rshealth-describe-identifier-mistakes#task#5.

Cause 7 of 8: DescribeClusterSnapshots raises not-found where AWS filters to an empty result. AWS: this DescribeClusterSnapshots variant answers HTTP 200 with an empty Snapshots collection. Smithy: DescribeClusterSnapshots models ClusterNotFoundFault (awsQueryError ClusterNotFound, 404) and ClusterSnapshotNotFoundFault (404), but for this request AWS raises neither and serves an empty list. Emulator: HTTP 404 and no DescribeClusterSnapshotsResponse. Hypothesis: the emulator resolves the ClusterIdentifier/SnapshotIdentifier filter as a required resource lookup instead of treating it as a filter predicate. Why: an inventory client that enumerates snapshots for an identifier it has not yet created gets a 404 it must special-case, instead of the empty list AWS returns, so snapshot-polling loops crash rather than wait. Fix: the DescribeClusterSnapshots handler's filter handling — filter, do not resolve. Traces: redshift-rshealth-describe-identifier-mistakes#task#7.

Cause 8 of 8: the DescribeClusterSnapshots SnapshotType domain refusal uses an unmodelled code and emulator-authored text. AWS: a SnapshotType outside the allowed domain is refused HTTP 400 with code InvalidParameterValue and the message template "Invalid snapshot type '" + the supplied SnapshotType + "'.  Must be blank or one of: manual, automated." — fixed text with one variable part (the supplied value), note AWS's two spaces after the first period and that blank is explicitly allowed. Smithy: SnapshotType is an unconstrained String, so the domain is a service rule (recorded behavior). Emulator: HTTP 400 but code ValidationException and the message "Invalid value for SnapshotType: '<value>'. Valid Values: automated | manual" — a different fixed text that also omits the "may be blank" half of the rule. Why: the caller gets a code outside Redshift's error vocabulary and is not told that an omitted SnapshotType is legal. Fix: the DescribeClusterSnapshots handler's SnapshotType validation. Traces: redshift-rshealth-describe-identifier-mistakes#task#8.

## Evidence
Cause 1 — redshift-rshealth-cluster-option-catalog-filters#task#2 (DescribeOrderableClusterOptions): status expected 400, got 200; $.ErrorResponse missing — expected {'Error': {'Type': 'Sender', 'Code': 'InvalidParameterCombination', 'Message': 'Cannot find Cluster version 2.0'}}, got None.

Cause 2 — redshift-rshealth-describe-events-scoping#task#1 (DescribeEvents, cluster of 2): $.ErrorResponse.Error.Message expected 'Cannot specify source identifier without source type', got 'SourceType must also be provided when SourceIdentifier is supplied.'

Cause 3 — redshift-rshealth-describe-events-scoping#task#3 (DescribeEvents, cluster of 2): status expected 400, got 200; $.ErrorResponse missing — expected code 'InvalidParameterCombination', message 'Events occurring more than 14 days in the past are not available.', got None.

Cause 4 — redshift-rshealth-describe-identifier-mistakes#task#1 (DescribeClusters, cluster of 2): status expected 400, got 404; Code expected 'InvalidParameterValue', got 'ClusterNotFound'; Message expected 'The parameter DBInstanceIdentifier is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.', got 'Cluster <identifier> not found.'

Cause 5 — redshift-rshealth-describe-identifier-mistakes#task#3 (DescribeClusters): Code expected 'InvalidParameterValue', got 'ValidationException'; Message expected 'Invalid value for max records. Must be between 20 and 100', got 'Invalid value for MaxRecords: <n>. MaxRecords must be between 20 and 100.'

Cause 6 — redshift-rshealth-describe-identifier-mistakes#task#5 (DescribeClusters): status expected 200, got 400; $.DescribeClustersResponse missing — expected {'DescribeClustersResult': {'Clusters': ''}}, got None.

Cause 7 — redshift-rshealth-describe-identifier-mistakes#task#7 (DescribeClusterSnapshots): status expected 200, got 404; $.DescribeClusterSnapshotsResponse missing — expected {'DescribeClusterSnapshotsResult': {'Snapshots': ''}}, got None.

Cause 8 — redshift-rshealth-describe-identifier-mistakes#task#8 (DescribeClusterSnapshots): Code expected 'InvalidParameterValue', got 'ValidationException'; Message expected "Invalid snapshot type 'auto'.  Must be blank or one of: manual, automated.", got "Invalid value for SnapshotType: 'auto'. Valid Values: automated | manual".

## Steps to Reproduce
1. Boot the emulator and point a redshift (query-protocol) client at it.
2. Cause 1: DescribeOrderableClusterOptions(ClusterVersion='2.0') -> expect HTTP 400 InvalidParameterCombination 'Cannot find Cluster version 2.0'; today HTTP 200 with a catalog.
3. Cause 2: DescribeEvents(SourceIdentifier=<any>) with no SourceType -> the message must read exactly 'Cannot specify source identifier without source type'.
4. Cause 3: DescribeEvents(StartTime=<more than 14 days ago>) -> expect HTTP 400 InvalidParameterCombination 'Events occurring more than 14 days in the past are not available.'; today HTTP 200.
5. Cause 4: DescribeClusters(ClusterIdentifier='Sim2real_Probe_Bad') (underscores, leading capital) -> expect HTTP 400 InvalidParameterValue with the identifier-grammar message; today HTTP 404 ClusterNotFound.
6. Cause 5: DescribeClusters(MaxRecords=10) -> expect HTTP 400 InvalidParameterValue 'Invalid value for max records. Must be between 20 and 100'; today ValidationException with different text.
7. Cause 6: the DescribeClusters variant of redshift-rshealth-describe-identifier-mistakes#task#5 (an above-maximum MaxRecords) -> expect HTTP 200 with an empty Clusters collection; today HTTP 400.
8. Cause 7: DescribeClusterSnapshots filtered on an identifier with no snapshots -> expect HTTP 200 with an empty Snapshots collection; today HTTP 404.
9. Cause 8: DescribeClusterSnapshots(SnapshotType='auto') -> expect HTTP 400 InvalidParameterValue "Invalid snapshot type 'auto'.  Must be blank or one of: manual, automated."
10. Re-replay fixtures redshift-rshealth-cluster-option-catalog-filters, redshift-rshealth-describe-events-scoping and redshift-rshealth-describe-identifier-mistakes to confirm.
