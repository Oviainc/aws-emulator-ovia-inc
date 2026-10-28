// SPDX-License-Identifier: CC0-1.0

//! What a stack claims about an `AWS::Glue::Job` AWS Glue was never asked to create, seen
//! from CloudFormation's side alone.
//!
//! The rule under test: **a resource type this emulator PROVISIONS is reported as created
//! only where the service that owns it created it** — so a job definition whose owning
//! service does not answer fails the resource and rolls the stack back, naming it, instead
//! of a `CREATE_COMPLETE` stack whose output publishes a job name AWS Glue holds nothing
//! under. This router does not carry AWS Glue (`stack_lifecycle::router`), which is exactly
//! the shape of "the call did not happen" for the type it owns.
//!
//! The rows differ along what the rule generalises over: how the template names the job
//! (through `Fn::Sub` over the Region pseudo-parameter, literally, or not at all), which
//! command kind it declares, the template spelling, and how many jobs one template declares.
//!
//! Nothing half-created survives the rollback either: each row's job declares the role it
//! runs as, that role IS created (IAM is mounted here), and the rollback releases it — so a
//! failed deploy leaves neither the job nor the role behind.
//!
//! The half where AWS Glue IS mounted and the create really happens — the stack completing,
//! `glue GetJob` resolving the name the stack exported, the job carrying the role ARN the
//! template passed, a changed declaration reaching AWS Glue's own update, and the stack's
//! deletion deleting the job there — is
//! `core/router/tests/all/cloudformation_provisions_glue_jobs.rs`: a crate-local suite can
//! only ever see one side of a two-crate call.

use serde_json::json;

use crate::stack_lifecycle::{
    call, iam, member, wait_for_the_transition, xml_text, xml_texts,
};

/// One row: a template declaring one or more Glue jobs, the logical id whose failure the
/// stack has to name, and the role that template creates on the way there.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    /// The logical ids of the jobs the template declares. The walk stops at the FIRST
    /// refusal, and which job that is follows from the deployment order the dependency
    /// graph gives it, so what the failure has to name is one of these.
    declared_jobs: &'static [&'static str],
    /// The role the template names, which must not survive the rollback.
    role_name: &'static str,
}

/// The trust policy every row's role carries: AWS Glue has to be able to assume the role a
/// job runs as.
fn trust_policy() -> serde_json::Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "glue.amazonaws.com" },
            "Action": "sts:AssumeRole",
        }],
    })
}

/// One `AWS::IAM::Role` declaration under `role_name`, which every row's job runs as.
fn role(role_name: &str) -> serde_json::Value {
    json!({
        "Type": "AWS::IAM::Role",
        "Properties": {
            "RoleName": role_name,
            "AssumeRolePolicyDocument": trust_policy(),
        }
    })
}

fn rows() -> Vec<Row> {
    vec![
        // The issue's own template: the job's name derived through `Fn::Sub` over the
        // account and the Region, its role read back with `Fn::GetAtt`, in JSON.
        Row {
            case: "a job named through Fn::Sub, with a Spark ETL command, in JSON",
            stack_name: "glue-job-unreachable-stack",
            template: json!({
                "Resources": {
                    "GlueJobRole": role("gjwr-unreachable-report"),
                    "GlueJob": {
                        "Type": "AWS::Glue::Job",
                        "Properties": {
                            "Name": {
                                "Fn::Sub": "gjwr-RoleReport-${AWS::AccountId}-${AWS::Region}"
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
                "Outputs": { "GlueJobName": { "Value": { "Ref": "GlueJob" } } }
            })
            .to_string(),
            declared_jobs: &["GlueJob"],
            role_name: "gjwr-unreachable-report",
        },
        // The same shape in YAML, with the short-form intrinsic tags, a literal name and a
        // Python shell command.
        Row {
            case: "a job named literally, with a Python shell command, in YAML",
            stack_name: "glue-job-unreachable-yaml-stack",
            template: [
                "Resources:",
                "  GlueJobRole:",
                "    Type: AWS::IAM::Role",
                "    Properties:",
                "      RoleName: gjwr-unreachable-ingest",
                "      AssumeRolePolicyDocument:",
                "        Version: '2012-10-17'",
                "        Statement:",
                "          - Effect: Allow",
                "            Principal:",
                "              Service: glue.amazonaws.com",
                "            Action: sts:AssumeRole",
                "  Ingest:",
                "    Type: AWS::Glue::Job",
                "    Properties:",
                "      Name: gjwr-unreachable-ingest-job",
                "      Role: !GetAtt GlueJobRole.Arn",
                "      Command:",
                "        Name: pythonshell",
                "        ScriptLocation: s3://sim2real-etl/ingest.py",
                "Outputs:",
                "  GlueJobName:",
                "    Value: !Ref Ingest",
                "",
            ]
            .join("\n"),
            declared_jobs: &["Ingest"],
            role_name: "gjwr-unreachable-ingest",
        },
        // Two jobs in one stack, one of them unnamed (CloudFormation generates its name):
        // the first refusal stops the walk, so the stack names it — and no job name is
        // published for either.
        Row {
            case: "two jobs, one of them the template does not name at all",
            stack_name: "glue-job-unreachable-pair-stack",
            template: json!({
                "Resources": {
                    "GlueJobRole": role("gjwr-unreachable-pair"),
                    "First": {
                        "Type": "AWS::Glue::Job",
                        "Properties": {
                            "Name": "gjwr-unreachable-first",
                            "Role": { "Fn::GetAtt": ["GlueJobRole", "Arn"] },
                            "Command": {
                                "Name": "gluestreaming",
                                "ScriptLocation": "s3://sim2real-etl/one.py"
                            }
                        }
                    },
                    "Second": {
                        "Type": "AWS::Glue::Job",
                        "Properties": {
                            "Role": { "Fn::GetAtt": ["GlueJobRole", "Arn"] },
                            "Command": {
                                "Name": "glueetl",
                                "ScriptLocation": "s3://sim2real-etl/two.py"
                            }
                        }
                    }
                },
                "Outputs": {
                    "FirstName": { "Value": { "Ref": "First" } },
                    "SecondName": { "Value": { "Ref": "Second" } }
                }
            })
            .to_string(),
            declared_jobs: &["First", "Second"],
            role_name: "gjwr-unreachable-pair",
        },
    ]
}

/// A declared Glue job whose owning service never created it is not reported as created: the
/// stack fails, names the resource, publishes no job name, and leaves nothing behind.
#[tokio::test]
async fn a_glue_job_the_owning_service_never_created_is_not_reported_as_created() {
    let router = crate::stack_lifecycle::router();

    for row in rows() {
        let case = row.case;
        let (status, body) = call(
            &router,
            "CreateStack",
            &[
                member("StackName", row.stack_name),
                member("TemplateBody", &row.template),
                // Every row names the role its job runs as.
                member("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );
        wait_for_the_transition().await;

        let (status, body) = call(
            &router,
            "DescribeStacks",
            &[member("StackName", row.stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
        );
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some("ROLLBACK_COMPLETE"),
            "case {case:?}: expected the failed create to be rolled back rather than report a \
             job nothing created, got {body}"
        );
        let reason = xml_text(&body, "StackStatusReason").unwrap_or_default();
        assert!(
            row.declared_jobs.iter().any(|job| reason.contains(job)),
            "case {case:?}: expected the reason to name the job that failed (one of {:?}), got \
             {reason:?}",
            row.declared_jobs
        );
        let values = xml_texts(&body, "OutputValue");
        assert!(
            values.is_empty(),
            "case {case:?}: expected no output value for a job nothing created, got {values:?}"
        );

        // And the role the template created on the way to that failure is released: the
        // rollback leaves no half-created resource behind.
        let (status, read) = iam(
            &router,
            "GetRole",
            &[("RoleName", row.role_name)],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the rolled-back stack to have released the role \
             {:?}, got {status} with body {read}",
            row.role_name
        );
    }
}
