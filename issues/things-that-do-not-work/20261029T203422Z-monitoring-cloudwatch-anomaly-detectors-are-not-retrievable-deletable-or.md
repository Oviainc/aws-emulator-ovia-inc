Issue-Source: trace-sweep-divergence
Issue-Trace: cwa-range-operator-threshold-metric-id
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cwa-range-operator-threshold-metric-id monitoring-anomaly-detector-lifecycle cwp-delete-anomaly-detector-describe-empty
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|DescribeAnomalyDetectors|2xx->2xx|$.AnomalyDetectors[*]
Issue-Key: monitoring|DeleteAnomalyDetector|2xx->4xx|status
Issue-Key: monitoring|PutAnomalyDetector|2xx->2xx|$.AnomalyDetectorId
Issue-Key: monitoring|DescribeAnomalyDetectors|2xx->2xx|$.AnomalyDetectors[*].AnomalyDetectorId
Issue-Key: monitoring|DescribeAnomalyDetectors|2xx->2xx|$.AnomalyDetectors[*].Configuration
Issue-Key: monitoring|DescribeAnomalyDetectors|2xx->2xx|$.AnomalyDetectors[*].Dimensions
Issue-Key: monitoring|DescribeAnomalyDetectors|2xx->2xx|$.AnomalyDetectors[*].SingleMetricAnomalyDetector.AccountId
Issue-Key: monitoring|DescribeAnomalyDetectors|2xx->2xx|$.AnomalyDetectors[*].SingleMetricAnomalyDetector.Dimensions
Issue-Key: monitoring|PutAnomalyDetector|4xx->4xx|$.message
Issue-Key: monitoring|DeleteAnomalyDetector|4xx->4xx|$.message
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CloudWatch anomaly detectors are not retrievable, deletable or correctly identified

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAnomalyDetectors

## Description
6 causes in the anomaly-detector subsystem: a detector created implicitly is not visible, the delete of a visible detector 404s, the detector id and projection are wrong, and the missing-identifier refusals are emulator-authored.

Cause 1 of 6: DescribeAnomalyDetectors after an alarm with a ThresholdMetricId / ANOMALY_DETECTION_BAND has implicitly created the detector. AWS: 200 with one AnomalyDetector carrying AnomalyDetectorId, Configuration (with ExcludedTimeRanges, [] when none), Dimensions ([] when none), MetricName, Namespace, Stat, StateValue 'PENDING_TRAINING' and a SingleMetricAnomalyDetector mirror carrying AccountId, Dimensions, MetricName, Namespace and Stat. Smithy: DescribeAnomalyDetectors output AnomalyDetectors targets AnomalyDetectors; AnomalyDetector members include Configuration, StateValue (AnomalyDetectorStateValue) and SingleMetricAnomalyDetector. Emulator: 200 with an EMPTY AnomalyDetectors list. Why: an anomaly-band alarm's detector cannot be inspected, so training state and exclusion windows are invisible and the detector cannot be managed. Fix: the anomaly-detector state and the implicit-create path behind PutMetricAlarm/ThresholdMetricId. Trace: cwa-range-operator-threshold-metric-id#task#7.

Cause 2 of 6: DeleteAnomalyDetector for that implicitly created detector. AWS: 200 (the detector exists and is removed). Emulator: 404. Why: teardown of an anomaly-band alarm leaves a detector the client cannot delete, and the 404 is reported as a hard failure. Fix: the same detector state plus DeleteAnomalyDetector's lookup. Trace: cwa-range-operator-threshold-metric-id#teardown#8.

Cause 3 of 6: PutAnomalyDetector's returned AnomalyDetectorId. AWS: an identifier matching the modelled shape — Smithy: PutAnomalyDetector output AnomalyDetectorId @length(min 1, max 128) @pattern '^[A-Za-z0-9_./:%()+-]+$' — of the form the service mints (the recorded value is a per-run id, not a requirement; the recording asserts only its shape). Emulator: a 64-character lowercase hex digest derived from the detector definition, which does not match the shape the recording asserts. Why: the id is content-addressed rather than service-minted, so two logically distinct detectors can collide and a client cannot distinguish a re-created detector. Fix: AnomalyDetectorId minting in the PutAnomalyDetector handler/state. Trace: cwp-delete-anomaly-detector-describe-empty#task#1.

Cause 4 of 6: DescribeAnomalyDetectors for an explicitly created detector. AWS: the entry carries Configuration ({'ExcludedTimeRanges': []} when none configured), Dimensions ([]), and SingleMetricAnomalyDetector.AccountId (the caller's account) and .Dimensions ([]). Emulator: all four members absent, plus the wrong id form from Cause 3. Why: a client round-tripping a DescribeAnomalyDetectors result into PutAnomalyDetector loses the configuration and the account scoping. Fix: the AnomalyDetector output projection. Trace: cwp-delete-anomaly-detector-describe-empty#task#2.

Cause 5 of 6: PutAnomalyDetector with neither the deprecated single-metric members nor SingleMetricAnomalyDetector nor MetricMathAnomalyDetector. AWS: 400 with the message 'Input missing required parameter.' — entirely fixed text. Smithy: PutAnomalyDetector models MissingRequiredParameterException and InvalidParameterCombinationException; none of the identification members is individually required, so the service refuses the combination with this fixed sentence. Emulator: its own prose 'Either single metric attributes, SingleMetricAnomalyDetector or MetricMathAnomalyDetector must be set.' Fix: PutAnomalyDetector's identification validation message. Trace: monitoring-anomaly-detector-lifecycle#task#3.

Cause 6 of 6: DeleteAnomalyDetector with no identifier at all. AWS: 400 with the message 'Either an AnomalyDetectorId or a metric definition must be provided to identify the anomaly detector to delete.' — fixed text, and note it is DIFFERENT from PutAnomalyDetector's text in Cause 5 because the delete path also accepts AnomalyDetectorId. Emulator: reuses the Put-side prose 'Either single metric attributes, SingleMetricAnomalyDetector or MetricMathAnomalyDetector must be set.', which omits AnomalyDetectorId as an option. Why: the message actively misleads — it tells the caller to supply a metric definition when the id alone would do. Fix: DeleteAnomalyDetector's own identification validation message (do not share the Put message). Trace: monitoring-anomaly-detector-lifecycle#task#4.

## Evidence
Replay (simparity-wire-aws-v1), 6 clusters.
1. DescribeAnomalyDetectors, cwa-range-operator-threshold-metric-id#task#7: $.AnomalyDetectors[0] missing (expected AnomalyDetectorId, Configuration {'ExcludedTimeRanges': []}, Dimensions [], MetricName, Namespace, SingleMetricAnomalyDetector {AccountId, Dimensions [], MetricName, Namespace, Stat}, Stat, StateValue 'PENDING_TRAINING'), got None. (also monitoring-anomaly-detector-lifecycle#task#5)
2. DeleteAnomalyDetector, cwa-range-operator-threshold-metric-id#teardown#8: status expected 200, got 404.
3. PutAnomalyDetector, cwp-delete-anomaly-detector-describe-empty#task#1: $.AnomalyDetectorId shape mismatch — expected a service-minted id, got 'd2229afec083dc08adbed4a4675775e33bb95715f856c595042875df052440b4'.
4. DescribeAnomalyDetectors, cwp-delete-anomaly-detector-describe-empty#task#2: AnomalyDetectors[0].Configuration expected {'ExcludedTimeRanges': []}, got None; .Dimensions expected [], got None; .SingleMetricAnomalyDetector.AccountId expected the account id, got None; .SingleMetricAnomalyDetector.Dimensions expected [], got None; plus the same id shape mismatch.
5. PutAnomalyDetector, monitoring-anomaly-detector-lifecycle#task#3: $.message expected 'Input missing required parameter.', got 'Either single metric attributes, SingleMetricAnomalyDetector or MetricMathAnomalyDetector must be set.'
6. DeleteAnomalyDetector, monitoring-anomaly-detector-lifecycle#task#4: $.message expected 'Either an AnomalyDetectorId or a metric definition must be provided to identify the anomaly detector to delete.', got 'Either single metric attributes, SingleMetricAnomalyDetector or MetricMathAnomalyDetector must be set.'

## Steps to Reproduce
1. Boot the emulator from mainline and replay the simparity-wire-aws-v1 corpus.
2. PutMetricAlarm with a ThresholdMetricId backed by an ANOMALY_DETECTION_BAND expression, then DescribeAnomalyDetectors (cwa-range-operator-threshold-metric-id#task#7) — observe an empty list — and DeleteAnomalyDetector (teardown#8) — observe 404.
3. PutAnomalyDetector with a SingleMetricAnomalyDetector (cwp-delete-anomaly-detector-describe-empty#task#1) and inspect AnomalyDetectorId; then DescribeAnomalyDetectors (task#2) and check Configuration, Dimensions and SingleMetricAnomalyDetector.AccountId/.Dimensions.
4. PutAnomalyDetector with no identification members (monitoring-anomaly-detector-lifecycle#task#3) and DeleteAnomalyDetector with none (task#4); compare $.message.
