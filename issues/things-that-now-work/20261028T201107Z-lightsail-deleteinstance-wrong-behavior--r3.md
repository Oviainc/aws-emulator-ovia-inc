Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: db5b8cf4b40010fa5fa0137914026d79724270f1

# An alarm keeps monitoring an instance (or database) the delete removed — the one reference the removal does not release, and one PutAlarm itself refuses to create

_Filed: 2026-10-28T20:11:07Z_
_Operation: DeleteInstance | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `db5b8cf4b40010fa5fa0137914026d79724270f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`remove_instance` is documented as releasing EVERY reference the store holds to the instance it removes, and its helper enumerates the four kinds of record that can hold one (disk, static IP, load-balancer registration, bucket access grant). A fifth kind does hold one: a stored alarm keeps a `MonitoredResource` — name, ARN and resource type — of the instance it watches, and `StoredAlarm::view` reports it as `monitoredResourceInfo` on every `GetAlarms` answer, unconditionally. Nothing in the crate removes or rewrites those alarms when the monitored resource is deleted.

So after `DeleteInstance` (or `DeleteRelationalDatabase`) the store holds, and `GetAlarms` reports, an alarm whose `monitoredResourceInfo` names a resource the account no longer holds — exactly the state `PutAlarm` refuses to create: it resolves `monitoredResourceName` through `store.monitored_resource(..)` and answers `DoesNotExist` ("does not name an Instance, LoadBalancer or RelationalDatabase of this account") when the name resolves to nothing. This is the same defect shape as the already-fixed bucket grant ("a bucket keeps reporting a deleted instance in resourcesReceivingAccess"), with the alarm being the reference the release walk was never extended to.

It is not only a cosmetic report: `alarms_on_metric` counts those stale alarms, and `PutAlarm` enforces the per-metric alarm budget from that count. Two alarms left over from a deleted instance named `i1` therefore refuse every new alarm on the same metric of a NEW instance created under the name `i1`, naming a limit that is spent entirely on a resource that does not exist.

`remove_relational_database` makes the stronger claim for the database path ("Nothing else in the store names a database it is attached to ... so this removes exactly one record"), while `monitored_resource` resolves relational databases and so an alarm can name one.

## Evidence

The release walk's own universal claim and its enumeration — services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:13498-13509:
```
    /// Release every reference the store holds to the instance `name`,
    /// answering one entry per release so the caller can log it.
    ///
    /// Four kinds of record can hold one: a block storage disk (`attachedTo`
    /// plus the device `path` it is mounted at), a static IP (`attachedTo`), a
    /// load balancer (the instance registered with it), and a bucket (the
    /// instance it reports in `resourcesReceivingAccess`, ...
```
and the handler repeating it — src/ops/delete_instance.rs:176-189:
```
            // Removing the instance also releases every live reference another
            // stored resource holds to it — a block storage disk, a static IP,
            // a load balancer's registration, a bucket's access grant — ...
            // can observe an attachment to, or an access held by, an instance
            // this account no longer holds.
            let Some(deleted) = store.remove_instance(&name) else {
```
`release_attachments_to_instance` (src/state.rs:13515-13592) walks disks, static IPs, load balancers and buckets and never touches `self.alarms`.

An alarm DOES hold that reference — src/state.rs:10379-10380:
```
    /// The resource this alarm watches, as it stood when the alarm was put.
    pub monitored_resource: MonitoredResource,
```
and reports it on every read — src/state.rs:10410:
```
            monitored_resource_info: Some(self.monitored_resource.view()),
```
while the resolver that creates it only ever resolves records the account HOLDS — src/state.rs:14066-14081:
```
    pub fn monitored_resource(&self, name: &str) -> Option<MonitoredResource> {
        if let Some(instance) = self.instances.get(name) {
...
        let database = self.relational_databases.get(name)?;
```
and the write path refuses anything else — src/ops/put_alarm.rs:304-306 and :420-424:
```
            let monitored = store
                .monitored_resource(&input.monitored_resource_name)
                .ok_or_else(|| monitored_resource_does_not_exist(&input.monitored_resource_name))?;
```
```
fn monitored_resource_does_not_exist(name: &str) -> PutAlarmError {
    crate::scaffolded::Scaffolded::does_not_exist(format!(
        "monitoredResourceName '{name}' does not name an Instance, LoadBalancer or \
         RelationalDatabase of this account"
```
The stale alarms still spend the per-metric budget — src/state.rs:14010-14024 (`alarms_on_metric` filters only on `alarm.monitored_resource.name`), used by src/ops/put_alarm.rs:323-331.

The database path's claim — src/state.rs:13792-13803:
```
    /// Nothing else in the store names a database it is attached to — a database
    /// attaches to no instance, disk or load balancer — so this removes exactly
    /// one record.
...
    pub fn remove_relational_database(
```

Test coverage: tests/all/alarm_configuration.rs exercises PutAlarm/GetAlarms/DeleteAlarm (its delete section is at :819-:916) and tests/all/instance_delete_attachments.rs + tests/all/instance_delete_bucket_access.rs pin what DeleteInstance releases — no test deletes a monitored instance or database, so no case pins the current behaviour (grep for `DeleteInstance` in tests/all/alarm_configuration.rs returns nothing).

## Steps to Reproduce

1. CreateInstances with instanceNames ["i1"], a blueprint and bundle of the catalog, availabilityZone "us-east-2a".
2. PutAlarm with alarmName "cpu-1", monitoredResourceName "i1", metricName "CPUUtilization", comparisonOperator "GreaterThanThreshold", threshold 80, evaluationPeriods 2 — answered 200.
3. PutAlarm again with alarmName "cpu-2" and the same monitored resource and metric — answered 200 (two alarms, the per-metric maximum).
4. DeleteInstance with instanceName "i1" — answered 200.
5. GetAlarms: both alarms are still reported, each with monitoredResourceInfo naming the Instance "i1", while GetInstance for "i1" answers the DoesNotExist envelope.
6. PutAlarm with alarmName "cpu-3" and monitoredResourceName "i1": refused as "does not name an Instance, LoadBalancer or RelationalDatabase of this account" — the state the two surviving alarms report is one this same operation will not create.
7. CreateInstances again with instanceNames ["i1"], then PutAlarm with alarmName "cpu-3", monitoredResourceName "i1", metricName "CPUUtilization": refused with "i1 already has the 2 alarms a single metric accepts on 'CPUUtilization'" — the budget is spent by alarms left over from the deleted instance.
8. The same sequence with CreateRelationalDatabase / PutAlarm (metricName "DatabaseConnections") / DeleteRelationalDatabase leaves the same stale alarm, against remove_relational_database's "this removes exactly one record".
