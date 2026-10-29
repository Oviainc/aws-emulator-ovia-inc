Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.supported-instance-types-report-accurate-hardware-facts
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.supported-instance-types-report-accurate-hardware-facts|run#29|AssertionError|assert "Spark" in six_apps and "Spark" in seven_apps, (
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# DescribeReleaseLabel reports application names in all-lowercase ('spark', 'hadoop') instead of EMR's canonical mixed-case names ('Spark', 'Hadoop')

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeReleaseLabel

## Description
One cause.

Cause 1 of 1: DescribeReleaseLabel's `Applications[].Name` is spelled in all-lowercase, so no caller can find an application by the name EMR actually publishes.
Failing check ids: emr.supported-instance-types-report-accurate-hardware-facts

AWS: Recorded behavior only for the casing. `DescribeReleaseLabel` returns `Applications`, a `SimplifiedApplicationList` of `SimplifiedApplication{Name,Version}` (Smithy: com.amazonaws.emr#SimplifiedApplication, both members target `String`, no `@pattern` and no enum, so the model does not constrain the spelling). The model's member doc is actively misleading here — `SimplifiedApplication$Name` reads "The returned release label application name. For example, `hadoop`." — but the recorded EMR answer for the newest emr-6.x and emr-7.x labels uses EMR's canonical mixed-case application identifiers, the same spellings `RunJobFlow`'s `Application$Name` takes and `DescribeCluster` echoes back: `Spark`, `Hadoop`, `Hive`, `HBase`, `Hudi`, `Hue`, `Iceberg`, `JupyterEnterpriseGateway`, `JupyterHub`, `Livy`, `Oozie`, `Phoenix`, `Pig`, `Presto`, `Tez`, `Trino`, `Zeppelin`, `ZooKeeper`, `Flink`, `Ganglia`. The requirement is the canonical identifier per application, not a case transform of it: it is not uniform capitalize-first (`HBase`, `ZooKeeper`, `JupyterHub`, `JupyterEnterpriseGateway` are internally capitalized), so it must come from a per-application name in the catalog, and the catalog's membership per release line (which release first bundles an application, which drops it) is unaffected.

Emulator: every name comes back lowercased. The replay collected `Applications` for the newest emr-6.x and emr-7.x labels and got, respectively,
['flink', 'ganglia', 'hadoop', 'hbase', 'hive', 'hudi', 'hue', 'iceberg', 'jupyterenterprisegateway', 'jupyterhub', 'livy', 'oozie', 'phoenix', 'pig', 'presto', 'spark', 'tez', 'trino', 'zeppelin', 'zookeeper']
and
['flink', 'hadoop', 'hbase', 'hive', 'hudi', 'hue', 'iceberg', 'jupyterenterprisegateway', 'jupyterhub', 'livy', 'oozie', 'phoenix', 'pig', 'presto', 'spark', 'tez', 'trino', 'zeppelin', 'zookeeper'].
The HTTP status is a correct 200 and there is no error code involved — this is a representation divergence in the body, not a refusal. The per-release membership is already right (the emr-7 line correctly drops Ganglia) and the `Version` values are populated, so only the spelling of `Name` diverges. The source is the service's own reference table: `APPLICATION_CATALOG` stores each entry's `name` as a lowercase literal (`name: "spark"`, `name: "hadoop"`, `name: "ganglia"`, …) and `release_line_applications` copies it straight into `SimplifiedApplication.name`; the comment above the table says the names are spelled "as `SimplifiedApplication$Name` documents them (\"For example, `hadoop`\")", i.e. the lowercase doc example was taken as the wire spelling.

Why: the application name is an identifier, not prose. A client that reads the release catalogue to decide what to launch and then matches or re-submits those names — `"Spark" in applications`, `Applications=[{"Name": a["Name"]}]` fed back into `RunJobFlow`, a UI keyed on `HBase`/`ZooKeeper` — finds nothing and concludes the release bundles no Spark at all. This is EMR's whole reference-data surface, where the content IS the contract: there is no later call that corrects the spelling, so every capacity-planning, application-selection and release-comparison flow built on `DescribeReleaseLabel` silently reads a catalogue of applications that do not exist under those names.

Fix: the service's release/application reference state — the `APPLICATION_CATALOG` table and the projection that builds `SimplifiedApplication` from it, which `DescribeReleaseLabel`'s handler returns unchanged. Each catalog entry needs the canonical EMR application identifier as its wire name (a case transform will not produce `HBase`/`ZooKeeper`/`JupyterHub`); keep the case-insensitive comparison that `ListReleaseLabels`' `ReleaseLabelFilter$Application` predicate already does, so the documented lowercase `spark@2.1.0` filter form keeps matching after the rename.

Traces: emr.supported-instance-types-report-accurate-hardware-facts — fails at the `assert "Spark" in six_apps and "Spark" in seven_apps` step after every instance-catalogue assertion in the same check has passed. Code citations: services/service-com-amazonaws-emr-elasticmapreduce/src/state.rs lines 1302-1309 (the comment adopting the doc's lowercase example), 1395-1626 (`APPLICATION_CATALOG`, e.g. `name: "ganglia"` at 1402, `name: "hadoop"` at 1408, `name: "spark"` at 1536), 1666-1676 (`release_line_applications`, `name: Some(application.name.to_string())` at 1671), 1690-1697 (`release_label_record`); services/service-com-amazonaws-emr-elasticmapreduce/src/ops/describe_release_label.rs line 188 (`applications: Some(record.applications)`). Note the two assertions after the failing one (Spark carries a Version in both lines, and the two lines ship different Spark versions) were never reached, so they are unverified rather than passing.

## Evidence
Check: emr.supported-instance-types-report-accurate-hardware-facts — verdict: mismatch.

AssertionError raised at check_emr_supported_instance_types_report_accurate_hardware_facts.py line 242:

    assert "Spark" in six_apps and "Spark" in seven_apps, (
        f"both release lines must ship Spark; got {sorted(six_apps)!r} / {sorted(seven_apps)!r}")

  AssertionError: both release lines must ship Spark; got
    ['flink', 'ganglia', 'hadoop', 'hbase', 'hive', 'hudi', 'hue', 'iceberg',
     'jupyterenterprisegateway', 'jupyterhub', 'livy', 'oozie', 'phoenix', 'pig',
     'presto', 'spark', 'tez', 'trino', 'zeppelin', 'zookeeper']
    /
    ['flink', 'hadoop', 'hbase', 'hive', 'hudi', 'hue', 'iceberg',
     'jupyterenterprisegateway', 'jupyterhub', 'livy', 'oozie', 'phoenix', 'pig',
     'presto', 'spark', 'tez', 'trino', 'zeppelin', 'zookeeper']

Expected (recorded AWS): DescribeReleaseLabel on the newest emr-6.x label returns Applications whose Name values are the canonical EMR identifiers — Flink, Ganglia, HBase, Hadoop, Hive, Hudi, Hue, Iceberg, JupyterEnterpriseGateway, JupyterHub, Livy, Oozie, Phoenix, Pig, Presto, Spark, Tez, Trino, Zeppelin, ZooKeeper — and the same set minus Ganglia for the newest emr-7.x label.

Actual (emulator): HTTP 200, same membership and populated Version values, but every Name is lowercased ('spark', 'hadoop', 'hbase', 'zookeeper', 'jupyterhub', ...). Fixed text of the divergence: the member is Applications[].Name; the variable part is the per-application canonical identifier, which must come from the catalog entry, not from a case transform of the lowercase literal.

Source of the lowercase values in the checkout:
  services/service-com-amazonaws-emr-elasticmapreduce/src/state.rs:1307-1309
    // Application names are spelled as
    // `SimplifiedApplication$Name` documents them ("For example, `hadoop`") and as
    // `ReleaseLabelFilter$Application` filters on them ("For example, `spark@2.1.0`").
  services/service-com-amazonaws-emr-elasticmapreduce/src/state.rs:1536
    name: "spark",
  services/service-com-amazonaws-emr-elasticmapreduce/src/state.rs:1671
    name: Some(application.name.to_string()),

Smithy (services/service-com-amazonaws-emr-elasticmapreduce/smithy-model/com-amazonaws-emr-elasticmapreduce.json):
  com.amazonaws.emr#SimplifiedApplication { Name: String, Version: String } — no pattern, no enum; Name's doc example is the lowercase "hadoop", which conflicts with the recorded wire values and is what the emulator followed.

No open issue covers this: the emr/emr-serverless/emr-containers issues on file concern job-run execution, tagging stubs, managed endpoints and CloudFormation outputs, none of them the release catalogue's application names.

## Steps to Reproduce
1. Build and boot the emulator from the checkout and point an EMR client at it.
2. Find the newest label on each release line:
   emr.list_release_labels(Filters={"Prefix": "emr-6."})["ReleaseLabels"][0]
   emr.list_release_labels(Filters={"Prefix": "emr-7."})["ReleaseLabels"][0]
   (ListReleaseLabels is newest-first, so index 0 is the newest; follow NextToken to the end if present.)
3. For each of those two labels call:
   emr.describe_release_label(ReleaseLabel=<label>)
4. Inspect the returned Applications list:
   sorted(a["Name"] for a in resp["Applications"])
   Observed: every name is lowercase — 'spark', 'hadoop', 'hbase', 'zookeeper', 'jupyterhub', ...
   Expected: EMR's canonical identifiers — 'Spark', 'Hadoop', 'HBase', 'ZooKeeper', 'JupyterHub', ...
5. Equivalent one-liner assertion that reproduces the failure:
   assert "Spark" in {a["Name"] for a in resp["Applications"]}
6. Re-replay to confirm a fix: emr.supported-instance-types-report-accurate-hardware-facts (the two assertions following the failing one — Spark carries a Version in both lines, and the two lines ship different Spark versions — are only reached once the casing is fixed, so confirm they pass too).
