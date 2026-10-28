Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d2be18544689ad5807caafb5cad50952f5ef0fe4

# No alarm can be created on a load balancer: the monitored-resource resolver skips load balancers on the strength of a comment the crate now contradicts

_Filed: 2026-10-28T15:11:24Z_
_Operation: PutAlarm | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d2be18544689ad5807caafb5cad50952f5ef0fe4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutAlarm` resolves `monitoredResourceName` through `crate::state::ComAmazonawsLightsailLightsail20161128State::monitored_resource`, which looks the name up in `self.instances` and then in `self.relational_databases` and gives up. Load balancers — the third of the three resource types the model and this crate's own alarm-metric catalog both name — are never consulted, so a `PutAlarm` naming a load balancer this account holds is always refused with Lightsail's `DoesNotExist` envelope.

The resolver's own doc states the reason: "a load balancer cannot be created yet, so no name resolves to one". That premise is no longer true of this crate. `CreateLoadBalancer` is implemented and stores a `StoredLoadBalancer` with its own ARN and location (`store.insert_load_balancer(...)`), `GetLoadBalancer`/`GetLoadBalancers` report them, `AttachInstancesToLoadBalancer` registers instances with them, `UpdateLoadBalancerAttribute` reconfigures them, and `GetLoadBalancerMetricData` answers their metric series. The crate's alarm catalog `ALARM_METRICS` carries twelve `LoadBalancer` rows (`ClientTLSNegotiationErrorCount`, `HealthyHostCount`, `UnhealthyHostCount`, the six `HTTPCode_*` counts, `InstanceResponseTime`, `RejectedConnectionCount`, `RequestCount`), and `alarm_metric_names("LoadBalancer")` exists only to be printed in refusals — none of those twelve rows can ever be reached by a stored alarm.

PutAlarm's own refusal sentence contradicts the resolver it calls: it tells the caller the name "does not name an Instance, LoadBalancer or RelationalDatabase of this account", naming LoadBalancer as a type the lookup covers, while the lookup never looks at a load balancer at all. The consequence is that a load balancer alarm — one third of the alarm surface — cannot be configured, read back through `GetAlarms`, driven by `TestAlarm`, or deleted by `DeleteAlarm`, and the twelve load-balancer metric rows are dead code.

## Evidence

The resolver, which never consults `self.load_balancers` — services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:13926-13946:
```
    /// The resource an alarm would monitor under `name`, or `None` when this
    /// account holds no alarmable resource of that name.
    ///
    /// See [`MonitoredResource`] for which resource types those are.
    pub fn monitored_resource(&self, name: &str) -> Option<MonitoredResource> {
        if let Some(instance) = self.instances.get(name) {
            return Some(MonitoredResource { ... resource_type: RESOURCE_TYPE_INSTANCE, ... });
        }
        let database = self.relational_databases.get(name)?;
        Some(MonitoredResource { ... resource_type: RESOURCE_TYPE_RELATIONAL_DATABASE, ... })
    }
```
The comment that justifies the omission — src/state.rs:10287-10293:
```
/// A resource an alarm monitors, as the alarm has to report it.
///
/// From the modelled `PutAlarmRequest$monitoredResourceName` documentation:
/// "Instances, load balancers, and relational databases are the only Lightsail
/// resources that can currently be monitored by alarms." Of those three, this
/// slice stores instances and relational databases; a load balancer cannot be
/// created yet, so no name resolves to one.
```
The crate DOES create and store load balancers — src/ops/create_load_balancer.rs:379-387:
```
            store.insert_load_balancer(crate::state::StoredLoadBalancer {
                name: ...,
                arn: crate::state::load_balancer_arn(&region, &id),
                ...
                state: crate::state::LOAD_BALANCER_STATE_PROVISIONING,
```
(and the record carries exactly what a `MonitoredResource` needs — src/state.rs:11071-11082: `pub name: String,` / `pub arn: String,` / `pub location: StoredLocation,`).

The crate's own alarm catalog expects load-balancer alarms — src/state.rs:10155-10167:
```
    // Load balancers.
    MetricSpec {
        name: "ClientTLSNegotiationErrorCount",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "HealthyHostCount",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Average",
    },
```
PutAlarm's refusal claims the lookup covers load balancers — src/ops/put_alarm.rs:420-425:
```
fn monitored_resource_does_not_exist(name: &str) -> PutAlarmError {
    crate::scaffolded::Scaffolded::does_not_exist(format!(
        "monitoredResourceName '{name}' does not name an Instance, LoadBalancer or \
         RelationalDatabase of this account"
    ))
}
```
The model sentence the comment itself quotes — smithy_query(query="doc", shape="PutAlarmRequest", member="monitoredResourceName"): "Instances, load balancers, and relational databases are the only Lightsail resources that can currently be monitored by alarms."

Test coverage: the alarm suite's monitored-resource enum has only two variants, with a comment that is itself stale — tests/all/alarm_configuration.rs:59-68:
```
/// From `PutAlarmRequest$monitoredResourceName`: "Instances, load balancers, and
/// relational databases are the only Lightsail resources that can currently be
/// monitored by alarms." Of those three, only these two can be created in this
/// emulator — `CreateLoadBalancer` is still a scaffold with its own issue — so a
/// load balancer metric is exercised through the refusals instead.
pub(crate) enum Monitored {
    Instance,
    RelationalDatabase,
}
```
The METRIC table (tests/all/alarm_configuration.rs:1074-1138) carries only `Monitored::Instance` and `Monitored::RelationalDatabase` rows, and the REFUSALS table (1234-1259) only refuses an unknown name and mismatched metrics — so nothing pins PutAlarm's current refusal of a real load balancer.

## Steps to Reproduce

1. Call CreateLoadBalancer with loadBalancerName="lb1", instancePort=80 and observe HTTP 200.
2. Call GetLoadBalancer with loadBalancerName="lb1" in the same Region and observe the stored load balancer, with its ARN and location.
3. Call PutAlarm with alarmName="lb1-UnhealthyHostCount", monitoredResourceName="lb1", metricName="UnhealthyHostCount", comparisonOperator="GreaterThanThreshold", threshold=1.0, evaluationPeriods=4.
4. Observe HTTP 400 NotFoundException / code DoesNotExist with the message "monitoredResourceName 'lb1' does not name an Instance, LoadBalancer or RelationalDatabase of this account" — although the account holds exactly that load balancer and the message names LoadBalancer as a resolvable type.
5. Read src/state.rs:13930-13946 beside src/state.rs:10289-10293: the lookup consults only `instances` and `relational_databases`, justified by "a load balancer cannot be created yet", while src/ops/create_load_balancer.rs:379 inserts one into `self.load_balancers`.
6. Observe that none of the twelve `RESOURCE_TYPE_LOAD_BALANCER` rows of `ALARM_METRICS` (src/state.rs:10155-10227) can ever be stored on an alarm.
