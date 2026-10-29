// SPDX-License-Identifier: CC0-1.0

//! One stack NAME deployed into two Regions: each deployment is its own stack and really
//! creates its own `AWS::Glue::Job` in its own Region, and retiring one of them leaves the
//! other's job exactly where it is.
//!
//! The rule under test: **a stack is a regional resource, and the resources it declares are
//! created in the Region that stack was deployed in — so the same template deployed under
//! the same name into two Regions is two independent stacks, each publishing a handle the
//! owning service resolves through that Region's endpoint alone, and deleting one releases
//! only the resources that one created.** For a job definition the handle is its NAME
//! ("Ref returns the job name"), so each Region's `glue GetJob` answers 200 for the name
//! its own stack exported with `Job.Role` that stack's role ARN, answers
//! `EntityNotFoundException` for the other Region's name, and keeps answering for the
//! surviving deployment after the other one is deleted.
//!
//! This is the deploy a client makes when a stack was rolled out to the wrong Region: the
//! template is redeployed under the same name in the Region it belongs in, both halves
//! complete, each Region's job is verified, and the misdeployed one is then retired. The
//! single-Region half of the rule — a stack's job carrying the declaration's own properties
//! and tags, a changed declaration reaching AWS Glue's update, a refused declaration
//! rolling the stack back — is [`crate::cloudformation_provisions_glue_jobs`]; this suite
//! is the half two deployments of ONE name have to agree on.
//!
//! The rows differ along what the rule generalises over: which pair of Regions the two
//! deployments land in (same geography or not), which of the two is deployed first, and
//! which of the two is the one retired at the end.
//!
//! The router, the awsQuery/awsJson wire helpers and the XML readers are the test crate's
//! own ([`crate::cloudformation_provisions_glue_jobs`], [`crate::query_wire`]), not another
//! copy.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_provisions_glue_jobs::{
    build_router, cloudformation, error_code, glue, listed_names, ACCOUNT, PAST_STACK_SPAN,
    TRUST_POLICY,
};
use crate::query_wire::{opt_element, outputs_of, physical_ids_of};

/// The logical id every row's template gives its job, so the physical id is read from the
/// one resource the rule is about.
const JOB_LOGICAL_ID: &str = "GlueJob";

/// The output keys every row's template publishes: the deployed job's name, the ARN of the
/// role it runs as, and the Region the deployment landed in.
const NAME_OUTPUT: &str = "GlueJobName";
const ROLE_OUTPUT: &str = "GlueJobRoleArn";
const REGION_OUTPUT: &str = "DeploymentRegion";

/// One row: the stack name both deployments share, the base both derived names are built
/// from, the two Regions in the order they are deployed, and which of the two is retired.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    base: &'static str,
    regions: [&'static str; 2],
    /// Index into `regions` of the deployment that is deleted at the end — the misdeployed
    /// one in the first row, the later deployment in the second.
    retired: usize,
}

/// The rows: three stack names, each deployed into two Regions.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the misdeployed Region first, then the expected one, retiring the first",
            stack_name: "gjwr-redeploy-stack",
            base: "gjwr-RoleReport",
            regions: ["us-east-2", "us-east-1"],
            retired: 0,
        },
        Row {
            case: "two Regions on different continents, retiring the later deployment",
            stack_name: "gjwr-redeploy-far-stack",
            base: "gjwr-ingest",
            regions: ["ap-southeast-1", "eu-west-1"],
            retired: 1,
        },
        Row {
            case: "two Regions of one geography, retiring the first deployment",
            stack_name: "gjwr-redeploy-near-stack",
            base: "gjwr-aggregate",
            regions: ["us-west-1", "us-west-2"],
            retired: 0,
        },
    ]
}

/// The template both deployments of a row use: a role AWS Glue can assume and the job that
/// runs as it, with BOTH names derived from the Region the deployment lands in, so the two
/// deployments of one stack name ask for two different physical resources — exactly as a
/// template written for a multi-Region rollout does.
fn template(base: &str) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "a Glue ETL job and the role it runs as, named per Region",
        "Resources": {
            "GlueJobRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": { "Fn::Sub": format!("{base}-role-${{AWS::Region}}") },
                    "AssumeRolePolicyDocument": serde_json::from_str::<Value>(TRUST_POLICY)
                        .expect("the trust policy is a document"),
                }
            },
            JOB_LOGICAL_ID: {
                "Type": "AWS::Glue::Job",
                "Properties": {
                    "Name": {
                        "Fn::Sub": format!("{base}-${{AWS::AccountId}}-${{AWS::Region}}")
                    },
                    "Role": { "Fn::GetAtt": ["GlueJobRole", "Arn"] },
                    "Command": {
                        "Name": "glueetl",
                        "PythonVersion": "3",
                        "ScriptLocation": "s3://sim2real-etl/report.py"
                    },
                    "GlueVersion": "4.0"
                }
            }
        },
        "Outputs": {
            NAME_OUTPUT: { "Value": { "Ref": JOB_LOGICAL_ID } },
            ROLE_OUTPUT: { "Value": { "Fn::GetAtt": ["GlueJobRole", "Arn"] } },
            REGION_OUTPUT: { "Value": { "Ref": "AWS::Region" } }
        }
    })
    .to_string()
}

/// What one completed deployment published: the job name it exported and the ARN of the role
/// that job runs as.
struct Deployed {
    job_name: String,
    role_arn: String,
}

/// Deploy `template` under `stack_name` in `region`, wait for the stack's clock-owed span to
/// come due, and read back what it published — asserting on the way that the deployment
/// completed, that its stack id names its own Region, and that the job's physical id is the
/// name the template asked for.
async fn deploy(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    stack_name: &str,
    base: &str,
) -> Deployed {
    let body = template(base);
    let (status, answer) = cloudformation(
        router,
        region,
        "CreateStack",
        &[
            ("StackName", stack_name),
            ("TemplateBody", &body),
            // The template names the IAM role its job runs as.
            ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?} / {region}: expected HTTP 200 from CreateStack, got {status} with body \
         {answer}"
    );

    tokio::time::sleep(PAST_STACK_SPAN).await;
    let (status, described) =
        cloudformation(router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?} / {region}: expected HTTP 200 from DescribeStacks, got {status} with body \
         {described}"
    );
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?} / {region}: expected the deployment to reach CREATE_COMPLETE, got \
         {described}"
    );

    // A stack is a regional resource, so its id names the Region it was deployed in.
    let stack_id = opt_element(&described, "StackId").unwrap_or_default();
    let expected_prefix = format!("arn:aws:cloudformation:{region}:");
    assert!(
        stack_id.starts_with(&expected_prefix),
        "case {case:?} / {region}: expected the stack id to name its own Region \
         ({expected_prefix:?}), got {stack_id:?}"
    );

    let outputs = outputs_of(&described);
    let job_name = outputs
        .get(NAME_OUTPUT)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?} / {region}: no {NAME_OUTPUT} output in {outputs:?}")
        });
    let role_arn = outputs
        .get(ROLE_OUTPUT)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?} / {region}: no {ROLE_OUTPUT} output in {outputs:?}")
        });
    assert_eq!(
        outputs.get(REGION_OUTPUT).map(String::as_str),
        Some(region),
        "case {case:?} / {region}: expected output {REGION_OUTPUT:?} to be the deployment's own \
         Region, got {outputs:?}"
    );
    // The name the template derived from the account and the Region pseudo-parameters.
    assert_eq!(
        job_name,
        format!("{base}-{ACCOUNT}-{region}"),
        "case {case:?} / {region}: expected the exported job name to be the one the template \
         derived, got {job_name:?}"
    );

    // "Ref returns the job name": the resource's physical id is that same name.
    let (status, resources) = cloudformation(
        router,
        region,
        "DescribeStackResources",
        &[("StackName", stack_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?} / {region}: expected HTTP 200 from DescribeStackResources, got {status} \
         with body {resources}"
    );
    assert_eq!(
        physical_ids_of(&resources).get(JOB_LOGICAL_ID),
        Some(&job_name),
        "case {case:?} / {region}: expected {JOB_LOGICAL_ID} to carry the exported name \
         {job_name:?}, got {resources}"
    );

    Deployed { job_name, role_arn }
}

/// `GetJob` in `region` for `job_name` must answer 200 with the job the stack there created,
/// carrying `role_arn`.
async fn job_is_held(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    job_name: &str,
    role_arn: &str,
) {
    let (status, read) = glue(router, region, "GetJob", &json!({ "JobName": job_name })).await;
    assert_eq!(
        status, 200,
        "case {case:?} / {region}: expected HTTP 200 from GetJob on the name the stack there \
         exported, got {status} with body {read}"
    );
    let job = read.get("Job").cloned().unwrap_or(Value::Null);
    assert_eq!(
        job.get("Name").and_then(Value::as_str),
        Some(job_name),
        "case {case:?} / {region}: expected the job to be named {job_name:?}, got {job}"
    );
    assert_eq!(
        job.get("Role").and_then(Value::as_str),
        Some(role_arn),
        "case {case:?} / {region}: expected the job to run as the role the stack created \
         ({role_arn:?}), got {job}"
    );
}

/// `GetJob` in `region` for `job_name` must be refused as a job that does not exist.
async fn job_is_absent(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    job_name: &str,
) {
    let (status, read) = glue(router, region, "GetJob", &json!({ "JobName": job_name })).await;
    assert_eq!(
        status, 400,
        "case {case:?} / {region}: expected HTTP 400 from GetJob for a job this Region holds \
         nothing under, got {status} with body {read}"
    );
    let code = error_code(&read);
    assert_eq!(
        code, "EntityNotFoundException",
        "case {case:?} / {region}: expected EntityNotFoundException for {job_name:?}, got \
         {code:?} with body {read}"
    );
}

/// Every row: both deployments of one stack name complete, each Region holds its own job and
/// only its own, and retiring one of the two leaves the other's job untouched.
#[tokio::test]
async fn one_stack_name_deployed_into_two_regions_provisions_a_glue_job_in_each() {
    let router = build_router();

    for row in rows() {
        let case = row.case;

        // Both deployments, in the row's own order: same template, same stack name, two
        // Regions.
        let mut deployed: Vec<Deployed> = Vec::new();
        for region in row.regions {
            deployed.push(deploy(&router, case, region, row.stack_name, row.base).await);
        }
        assert_ne!(
            deployed[0].job_name, deployed[1].job_name,
            "case {case:?}: expected the two deployments to publish different job names, got \
             {:?} twice",
            deployed[0].job_name
        );

        // Each Region holds the job its own deployment created, and holds nothing under the
        // other Region's name: AWS Glue is regional, and so is the stack that asked for it.
        for (index, region) in row.regions.iter().enumerate() {
            let own = &deployed[index];
            let other = &deployed[1 - index];
            job_is_held(&router, case, region, &own.job_name, &own.role_arn).await;
            job_is_absent(&router, case, region, &other.job_name).await;

            let listed = listed_names(&router, case, region).await;
            assert!(
                listed.contains(&own.job_name),
                "case {case:?} / {region}: expected ListJobs to name this Region's job \
                 {:?}, got {listed:?}",
                own.job_name
            );
            assert!(
                !listed.contains(&other.job_name),
                "case {case:?} / {region}: expected ListJobs not to name the other Region's \
                 job {:?}, got {listed:?}",
                other.job_name
            );

            // And CloudFormation resolves the job it created to the stack that owns it,
            // while the other Region's job id resolves to nothing here.
            let (status, owned) = cloudformation(
                &router,
                region,
                "DescribeStackResources",
                &[("PhysicalResourceId", &own.job_name)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} / {region}: expected HTTP 200 resolving {:?} to its stack, got \
                 {status} with body {owned}",
                own.job_name
            );
            assert_eq!(
                opt_element(&owned, "StackName").as_deref(),
                Some(row.stack_name),
                "case {case:?} / {region}: expected {:?} to resolve to stack {:?}, got {owned}",
                own.job_name,
                row.stack_name
            );
            let (status, foreign) = cloudformation(
                &router,
                region,
                "DescribeStackResources",
                &[("PhysicalResourceId", &other.job_name)],
            )
            .await;
            let resolved = status == 200
                && physical_ids_of(&foreign)
                    .values()
                    .any(|id| id == &other.job_name);
            assert!(
                !resolved,
                "case {case:?} / {region}: expected the other Region's job id {:?} to resolve \
                 to no stack here, got {status} with body {foreign}",
                other.job_name
            );
        }

        // Retire one of the two deployments. Only the resources IT created go away.
        let retired_region = row.regions[row.retired];
        let surviving_region = row.regions[1 - row.retired];
        let retired = &deployed[row.retired];
        let surviving = &deployed[1 - row.retired];
        let (status, body) = cloudformation(
            &router,
            retired_region,
            "DeleteStack",
            &[("StackName", row.stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?} / {retired_region}: expected HTTP 200 from DeleteStack, got {status} \
             with body {body}"
        );
        tokio::time::sleep(PAST_STACK_SPAN).await;

        job_is_absent(&router, case, retired_region, &retired.job_name).await;
        let listed = listed_names(&router, case, retired_region).await;
        assert!(
            !listed.contains(&retired.job_name),
            "case {case:?} / {retired_region}: expected ListJobs to name the deleted stack's job \
             {:?} no longer, got {listed:?}",
            retired.job_name
        );

        // The deployment that was kept is untouched: its stack still reports complete and
        // AWS Glue still holds its job, with the role it was given.
        let (status, described) = cloudformation(
            &router,
            surviving_region,
            "DescribeStacks",
            &[("StackName", row.stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?} / {surviving_region}: expected HTTP 200 from DescribeStacks for the \
             surviving deployment, got {status} with body {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?} / {surviving_region}: expected the surviving deployment to stay \
             CREATE_COMPLETE, got {described}"
        );
        job_is_held(
            &router,
            case,
            surviving_region,
            &surviving.job_name,
            &surviving.role_arn,
        )
        .await;
    }
}
