Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue__s3.emr-cluster-glue-catalog-missing-permission-negative
Issue-Key: emr__glue__s3.emr-cluster-glue-catalog-missing-permission-negative|run#13|AssertionError|assert state == "FAILED", f"hive query unexpectedly succeeded without glue:GetTable permission: {state}"
Issue-Commit: 0a1e14ce9c5d04d4281ea317c037d5ece999a2b8

# emr__glue__s3: an EMR step is reported COMPLETED although the cluster's instance role is explicitly denied glue:*, so the Glue Data Catalog authorization boundary is unmeasurable (1 cause)

> _Note: this issue was identified against the code base at commit `0a1e14ce9c5d04d4281ea317c037d5ece999a2b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddJobFlowSteps

## Description
Cause 1 of 1 — a shell step whose Hive query resolves a table through the Glue Data Catalog is reported COMPLETED even though the EMR cluster's EC2 instance role carries an explicit Deny of glue:*.
AWS: an EMR step runs on the cluster under the EC2 instance profile. With the Glue Data Catalog configured as the Hive metastore (hive-site classification, hive.metastore.client.factory.class = com.amazonaws.glue.catalog.metastore.AWSGlueDataCatalogHiveClientFactory) a Hive query must call Glue GetDatabase/GetTable as that role; an explicit Deny makes those calls fail, the `hive -e` command exits non-zero and the step settles Status.State = FAILED, with the Hive output the step copied to S3 containing an AccessDenied signal from Glue. Removing the Deny from the same instance role and re-running the identical step on the same cluster/table then succeeds. Recorded behavior only for the exact Hive log text; the modelled part is the step's terminal state (DescribeStep/ListSteps Status.State) and the Glue refusal's AccessDeniedException code.
Emulator: the step's terminal state is COMPLETED. Nothing authorizes the step's catalog access against the instance role, and the step's declared S3 log object is not produced with any Glue refusal in it, so the negative control cannot be distinguished from the positive one.
Why: a client validating that an EMR cluster's instance role is correctly scoped (a least-privilege review, or a guardrail test that a cluster cannot read the catalog) is told the restricted cluster succeeded. Every permission-boundary assertion over EMR + Glue silently passes, and a mis-scoped production role would never be caught.
Fix: the EMR step executor — a step's work must be performed as the cluster's EC2 instance role so Glue (and S3) calls are authorized, and the step's terminal state must follow the exit status of the work rather than being set to COMPLETED on acceptance. Related open issues that do NOT carry this check: 20261029T184212Z-emr-clusters-accept-steps-but-never-execute-them.md and 20261105T074335Z-emr-glue-s3-emr-cluster-steps-never-reach.md.
Traces: emr__glue__s3.emr-cluster-glue-catalog-missing-permission-negative.

## Evidence
Expected: the step that runs `hive -S -e "SELECT count(*) FROM <db>.<table>;"` on a cluster whose instance role explicitly denies glue:* settles with Status.State = FAILED, and the uploaded Hive log contains 'AccessDenied'.
Actual: the step settles COMPLETED. Replay detail: `AssertionError: hive query unexpectedly succeeded without glue:GetTable permission: COMPLETED`.

## Steps to Reproduce
1. Create a bucket, upload data/sample.csv, create a Glue database and a CSV table whose StorageDescriptor.Location is s3://<bucket>/data/.
2. Create EMR service and EC2 instance roles where the instance role carries an explicit Deny on glue:* .
3. RunJobFlow with Configurations=[{Classification:'hive-site', Properties:{hive.metastore.client.factory.class:'com.amazonaws.glue.catalog.metastore.AWSGlueDataCatalogHiveClientFactory', hive.metastore.schema.verification:'false'}}] and wait for the cluster to be ready.
4. AddJobFlowSteps with a shell step running `hive -S -e "SELECT count(*) FROM <db>.<table>;" > /tmp/out.txt 2>&1; rc=$?; aws s3 cp /tmp/out.txt s3://<bucket>/results/denied.txt; exit $rc` and poll DescribeStep to terminal.
5. Expect FAILED and an AccessDenied line in results/denied.txt; observe COMPLETED.
6. Replay: emr__glue__s3.emr-cluster-glue-catalog-missing-permission-negative.
