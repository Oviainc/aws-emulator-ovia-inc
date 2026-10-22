Issue-Source: trace-sweep-divergence
Issue-Trace: application-signals-slo-exclusion-windows
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: application-signals-slo-exclusion-windows application-signals-slo-lifecycle-period-based
Issue-Shape-Id: com.amazonaws.applicationsignals#ApplicationSignals
Issue-Key: application-signals|CreateServiceLevelObjective|2xx->2xx|$.Slo.RequestBasedSli.RequestBasedSliMetric.MetricType
Issue-Key: application-signals|CreateServiceLevelObjective|2xx->2xx|$.Slo.Sli.SliMetric.MetricType
Issue-Key: application-signals|GetServiceLevelObjective|2xx->2xx|$.Slo.Sli.SliMetric.MetricType
Issue-Key: application-signals|UpdateServiceLevelObjective|2xx->2xx|$.Slo.Sli.SliMetric.MetricType
Issue-Commit: 6d3a954d57304c873e4e6a3ccc977c1ac1664bbd

# application-signals SLO responses fabricate SliMetric.MetricType = AVAILABILITY where AWS omits the member

> _Note: this issue was identified against the code base at commit `6d3a954d57304c873e4e6a3ccc977c1ac1664bbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateServiceLevelObjective

## Description
Four causes, one fault: whenever the emulator serialises a ServiceLevelObjective it fills in the optional MetricType member of the SLI metric with 'AVAILABILITY', while AWS leaves it absent for the SLOs these fixtures create. The member is present on every read path, so it is the serializer (or the stored SLO record), not one operation.

AWS (shared by all four causes): Smithy ServiceLevelIndicatorMetric.MetricType and RequestBasedServiceLevelIndicatorMetric.MetricType are OPTIONAL enum members, documented as "If the SLO monitors either the LATENCY or AVAILABILITY metric that Application Signals collects, this field displays which of those metrics is used." — i.e. it is populated only for an SLO built on an Application Signals standard metric. Recorded behavior: for the SLOs these fixtures create, every AWS response omits MetricType entirely (expected None). Hypothesis: these SLO configs specify their metric explicitly (MetricDataQueries / a custom metric) rather than selecting a standard metric, which is why AWS has no metric type to report.
Emulator (shared): emits MetricType = 'AVAILABILITY' in the SLI metric of the returned Slo. Status is 200 and the rest of the body matches.
Why (shared): a client reading MetricType concludes the SLO tracks the Application Signals standard availability metric when it does not. Round-tripping a Get into an Update then sends a MetricType the customer never configured, and dashboards/assertions that treat the member's presence as "standard metric SLO" mis-classify every SLO this emulator returns.
Fix (shared): the ServiceLevelObjective serializer / the SLO record built by the create handler — only populate MetricType when the request's SliConfig (or RequestBasedSliConfig) actually selected a standard metric type; otherwise leave the member absent.

Cause 1 of 4: CreateServiceLevelObjective, request-based SLI — $.Slo.RequestBasedSli.RequestBasedSliMetric.MetricType is 'AVAILABILITY', expected absent. Traces: application-signals-slo-exclusion-windows#setup#1.
Cause 2 of 4: CreateServiceLevelObjective, period-based SLI — $.Slo.Sli.SliMetric.MetricType is 'AVAILABILITY', expected absent. Traces: application-signals-slo-lifecycle-period-based#setup#1.
Cause 3 of 4: GetServiceLevelObjective — the stored SLO is read back with the same fabricated $.Slo.Sli.SliMetric.MetricType, so the defect is persisted, not only echoed by create. Traces: application-signals-slo-lifecycle-period-based#task#2.
Cause 4 of 4: UpdateServiceLevelObjective — the post-update Slo also carries the fabricated $.Slo.Sli.SliMetric.MetricType. Traces: application-signals-slo-lifecycle-period-based#task#6.

## Evidence
Cause 1 — application-signals-slo-exclusion-windows#setup#1 CreateServiceLevelObjective (200 both sides): $.Slo.RequestBasedSli.RequestBasedSliMetric.MetricType type_mismatch — expected None, got 'AVAILABILITY'.
Cause 2 — application-signals-slo-lifecycle-period-based#setup#1 CreateServiceLevelObjective (200 both sides): $.Slo.Sli.SliMetric.MetricType — expected None, got 'AVAILABILITY'.
Cause 3 — application-signals-slo-lifecycle-period-based#task#2 GetServiceLevelObjective (200 both sides): $.Slo.Sli.SliMetric.MetricType — expected None, got 'AVAILABILITY'.
Cause 4 — application-signals-slo-lifecycle-period-based#task#6 UpdateServiceLevelObjective (200 both sides): $.Slo.Sli.SliMetric.MetricType — expected None, got 'AVAILABILITY'.
Smithy citations: ServiceLevelIndicatorMetric.MetricType and RequestBasedServiceLevelIndicatorMetric.MetricType target the ServiceLevelIndicatorMetricType enum and are optional (not in any required set); their docs scope the member to SLOs monitoring the LATENCY or AVAILABILITY metric Application Signals collects.

## Steps to Reproduce
1. Replay application-signals-slo-exclusion-windows step setup#1: CreateServiceLevelObjective with a RequestBasedSliConfig. The returned Slo.RequestBasedSli.RequestBasedSliMetric contains MetricType='AVAILABILITY'; AWS omits the member.
2. Replay application-signals-slo-lifecycle-period-based step setup#1 (CreateServiceLevelObjective with SliConfig), then step task#2 (GetServiceLevelObjective) and step task#6 (UpdateServiceLevelObjective): Slo.Sli.SliMetric.MetricType is 'AVAILABILITY' in all three responses.
