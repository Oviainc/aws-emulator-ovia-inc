// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares the AWS Backup resource types really creates them
//! in AWS Backup, and the handles the stack hands back are the ones that service answers
//! to.
//!
//! The rule under test: **a resource type AWS publishes for a service this emulator ships
//! is created, when a stack declares it, in the service that OWNS it, through that
//! service's own public API, under the identifier that type's published return values
//! define — so the id a stack's output carries is one `backup get-backup-plan`,
//! `backup list-backup-selections`, `backup get-backup-selection` and
//! `backup describe-backup-vault` resolve to the resource the template declared, a member
//! the declaration left out carries the owning service's own default for it, and deleting
//! the stack deletes the resources there.** Two crates
//! have to agree for that to hold (CloudFormation makes the call, AWS Backup serves it), so
//! the suite lives here rather than in either crate.
//!
//! The rows differ along what the rule generalises over: which of the three types the
//! template declares, whether the vault is NAMED or left for CloudFormation to generate a
//! name, how many rules the plan carries and which of a rule's optional members each one
//! states (both backup windows, a lifecycle, a copy action, a timezone), how the selection
//! picks its resources (tag conditions against resource ARNs with exclusions), whether tags
//! are declared, the Region deployed in, both template spellings, and which intrinsic the
//! outputs read a resource with (`Ref`, `Fn::GetAtt`).
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the two identifiers AWS Backup MINTS (the plan id and
//! the selection id) and the GENERATED vault name of the row that declares none, which are
//! read off what the stack reports and then checked against what that service resolves.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{form_encode, opt_element, outputs_of, physical_ids_of};

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to come
/// due, so the stack reports the template it deployed.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The backup window AWS Backup fills a rule's in with where the declaration states none:
/// 8 hours for the start window and 7 days for the completion window (that service's own
/// `DEFAULT_START_WINDOW_MINUTES` / `DEFAULT_COMPLETION_WINDOW_MINUTES`).
const DEFAULT_START_WINDOW_MINUTES: i64 = 480;
const DEFAULT_COMPLETION_WINDOW_MINUTES: i64 = 10_080;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every
/// other suite here; the in-process callback CloudFormation makes signs its request either
/// way (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_backup_cryocontrollerusermanager::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request and what each handler reads its Region from. The signature is
/// never checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one AWS Backup operation over its restJson1 wire, signed for `region`: the method
/// and URI its model binds the operation to.
async fn backup(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("backup.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("backup", region))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// The ARN of an AWS Backup resource in `region`: the same total function of the Region and
/// the resource path both services compute it from.
fn backup_arn(region: &str, resource: &str) -> String {
    format!("arn:aws:backup:{region}:{ACCOUNT}:{resource}")
}

/// A backup vault's ARN, as that service's own `backup_vault_arn` spells it.
fn vault_arn(region: &str, name: &str) -> String {
    backup_arn(region, &format!("backup-vault:{name}"))
}

/// A backup plan's ARN, as that service's own `backup_plan_arn` spells it.
fn plan_arn(region: &str, plan_id: &str) -> String {
    backup_arn(region, &format!("backup-plan:{plan_id}"))
}

/// Whether `name` is inside the name form AWS Backup publishes for a vault
/// (`^[a-zA-Z0-9\-\_]{2,50}$`): 2 to 50 letters, digits, hyphens and underscores.
fn is_vault_name(name: &str) -> bool {
    (2..=50).contains(&name.chars().count())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// One rule the row's plan declares, and what `GetBackupPlan` must then report for it.
struct ExpectedRule {
    rule_name: &'static str,
    /// The schedule the declaration states, where it states one.
    schedule: Option<&'static str>,
    /// The two backup windows the read must report: the declared value, or the default the
    /// service fills in for a rule that states none.
    start_window: i64,
    completion_window: i64,
    /// `Lifecycle.DeleteAfterDays`, where the declaration states a lifecycle.
    delete_after_days: Option<i64>,
    /// The destination vault ARN of the one copy action the rule declares, if any.
    copy_destination: Option<&'static str>,
}

/// One row of the table: a template, and what the rule says the resources it declares must
/// then be in AWS Backup.
struct Row {
    case: &'static str,
    /// The Region the row deploys in: the resources are created there, and it is the Region
    /// their ARNs carry.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// The vault's logical id, and the name the template gives it — `None` for a template
    /// that names none, whose vault carries the name CloudFormation generated.
    vault_logical_id: &'static str,
    declared_vault: Option<&'static str>,
    /// The plan's logical id and the name its body declares.
    plan_logical_id: &'static str,
    plan_name: &'static str,
    /// What the plan's rules must be read back as, in declaration order.
    rules: Vec<ExpectedRule>,
    /// The selection's logical id, and the body `GetBackupSelection` must report for it.
    selection_logical_id: &'static str,
    selection: fn() -> Value,
    /// The output keys whose value must be the plan id AWS Backup minted, each written with
    /// a different intrinsic over the same resource.
    plan_id_outputs: &'static [&'static str],
    /// The output key whose value must be the vault's name, and the one whose value must be
    /// its ARN.
    vault_name_output: &'static str,
    vault_arn_output: &'static str,
}

/// The IAM role every row's selection names. A literal ARN: the role itself is no part of
/// what this suite is about, and AWS Backup takes the ARN the declaration states.
const BACKUP_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AWSBackupDefaultServiceRole";

/// The vault a copy action in the second row copies into: another Region's vault, which is
/// what a cross-region lifecycle declares, stated as the literal ARN the declaration
/// carries.
const COPY_DESTINATION_ARN: &str =
    "arn:aws:backup:us-east-1:000000000000:backup-vault:refarch-bkp-dr";

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a named vault, a one-rule plan with no windows and a tag selection, in JSON",
            region: "us-west-2",
            template: || {
                json!({
                    "AWSTemplateFormatVersion": "2010-09-09",
                    "Description": "a backup estate: a vault, a plan and a selection",
                    "Resources": {
                        "Vault": {
                            "Type": "AWS::Backup::BackupVault",
                            "Properties": {
                                "BackupVaultName": "refarch-bkp-vault",
                                "BackupVaultTags": {"Owner": "platform"}
                            }
                        },
                        "Plan": {
                            "Type": "AWS::Backup::BackupPlan",
                            "Properties": {
                                "BackupPlan": {
                                    "BackupPlanName": "refarch-bkp-plan",
                                    "BackupPlanRule": [{
                                        "RuleName": "DailyBackups",
                                        "TargetBackupVault": {"Ref": "Vault"},
                                        "ScheduleExpression": "cron(0 0 * * ? *)",
                                        "Lifecycle": {"DeleteAfterDays": 35}
                                    }]
                                },
                                "BackupPlanTags": {"Owner": "platform"}
                            }
                        },
                        "Selection": {
                            "Type": "AWS::Backup::BackupSelection",
                            "Properties": {
                                "BackupPlanId": {"Ref": "Plan"},
                                "BackupSelection": {
                                    "SelectionName": "tagged-resources",
                                    "IamRoleArn": BACKUP_ROLE_ARN,
                                    "ListOfTags": [{
                                        "ConditionType": "STRINGEQUALS",
                                        "ConditionKey": "backup",
                                        "ConditionValue": "daily"
                                    }]
                                }
                            }
                        }
                    },
                    "Outputs": {
                        "BackupVaultName": {"Value": {"Ref": "Vault"}},
                        "BackupVaultArn": {
                            "Value": {"Fn::GetAtt": ["Vault", "BackupVaultArn"]}
                        },
                        "BackupPlanId": {"Value": {"Ref": "Plan"}},
                        "PlanIdAttribute": {
                            "Value": {"Fn::GetAtt": ["Plan", "BackupPlanId"]}
                        }
                    }
                })
                .to_string()
            },
            vault_logical_id: "Vault",
            declared_vault: Some("refarch-bkp-vault"),
            plan_logical_id: "Plan",
            plan_name: "refarch-bkp-plan",
            rules: vec![ExpectedRule {
                rule_name: "DailyBackups",
                schedule: Some("cron(0 0 * * ? *)"),
                start_window: DEFAULT_START_WINDOW_MINUTES,
                completion_window: DEFAULT_COMPLETION_WINDOW_MINUTES,
                delete_after_days: Some(35),
                copy_destination: None,
            }],
            selection_logical_id: "Selection",
            selection: || {
                json!({
                    "SelectionName": "tagged-resources",
                    "IamRoleArn": BACKUP_ROLE_ARN,
                    "ListOfTags": [{
                        "ConditionType": "STRINGEQUALS",
                        "ConditionKey": "backup",
                        "ConditionValue": "daily"
                    }]
                })
            },
            plan_id_outputs: &["BackupPlanId", "PlanIdAttribute"],
            vault_name_output: "BackupVaultName",
            vault_arn_output: "BackupVaultArn",
        },
        Row {
            case: "a vault CloudFormation names, two rules with windows and a copy action, \
                   an ARN selection, in YAML",
            region: "eu-west-1",
            template: || {
                [
                    "Resources:",
                    "  Vault:",
                    "    Type: AWS::Backup::BackupVault",
                    "  Plan:",
                    "    Type: AWS::Backup::BackupPlan",
                    "    Properties:",
                    "      BackupPlan:",
                    "        BackupPlanName: cross-region-plan",
                    "        BackupPlanRule:",
                    "          - RuleName: Hourly",
                    "            TargetBackupVault: !Ref Vault",
                    "            ScheduleExpression: cron(0 * * * ? *)",
                    "            ScheduleExpressionTimezone: Etc/UTC",
                    "            StartWindowMinutes: 120",
                    "            CompletionWindowMinutes: 240",
                    "          - RuleName: WeeklyWithCopy",
                    "            TargetBackupVault: !Ref Vault",
                    "            ScheduleExpression: cron(0 5 ? * SUN *)",
                    "            Lifecycle:",
                    "              DeleteAfterDays: 90",
                    "            CopyActions:",
                    "              - DestinationBackupVaultArn: \
                       arn:aws:backup:us-east-1:000000000000:backup-vault:refarch-bkp-dr",
                    "  Selection:",
                    "    Type: AWS::Backup::BackupSelection",
                    "    Properties:",
                    "      BackupPlanId: !Ref Plan",
                    "      BackupSelection:",
                    "        SelectionName: listed-resources",
                    "        IamRoleArn: arn:aws:iam::000000000000:role/\
                       AWSBackupDefaultServiceRole",
                    "        Resources:",
                    "          - arn:aws:s3:::refarch-bkp-data",
                    "        NotResources:",
                    "          - arn:aws:s3:::refarch-bkp-logs",
                    "Outputs:",
                    "  VaultName:",
                    "    Value: !Ref Vault",
                    "  VaultArn:",
                    "    Value: !GetAtt Vault.BackupVaultArn",
                    "  PlanId:",
                    "    Value: !Ref Plan",
                    "",
                ]
                .join("\n")
            },
            vault_logical_id: "Vault",
            declared_vault: None,
            plan_logical_id: "Plan",
            plan_name: "cross-region-plan",
            rules: vec![
                ExpectedRule {
                    rule_name: "Hourly",
                    schedule: Some("cron(0 * * * ? *)"),
                    start_window: 120,
                    completion_window: 240,
                    delete_after_days: None,
                    copy_destination: None,
                },
                ExpectedRule {
                    rule_name: "WeeklyWithCopy",
                    schedule: Some("cron(0 5 ? * SUN *)"),
                    start_window: DEFAULT_START_WINDOW_MINUTES,
                    completion_window: DEFAULT_COMPLETION_WINDOW_MINUTES,
                    delete_after_days: Some(90),
                    copy_destination: Some(COPY_DESTINATION_ARN),
                },
            ],
            selection_logical_id: "Selection",
            selection: || {
                json!({
                    "SelectionName": "listed-resources",
                    "IamRoleArn": BACKUP_ROLE_ARN,
                    "Resources": ["arn:aws:s3:::refarch-bkp-data"],
                    "NotResources": ["arn:aws:s3:::refarch-bkp-logs"]
                })
            },
            plan_id_outputs: &["PlanId"],
            vault_name_output: "VaultName",
            vault_arn_output: "VaultArn",
        },
    ]
}

/// Every row: the estate the stack reports is the estate AWS Backup holds — the plan id the
/// outputs carry is the one that service minted, the plan it reads back is the one the
/// template declared (with the service's own defaults where the declaration stated none),
/// the selection and the vault read back the same way, and deleting the stack deletes all
/// three there.
#[tokio::test]
async fn a_stack_provisions_the_backup_resources_its_template_declares() {
    let router = build_router();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("backup-estate-{index}");
        let template = (row.template)();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        // The stack reports the template it deployed once the create COMPLETES.
        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        let (status, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let reported = physical_ids_of(&resources);
        let physical = |logical: &str| -> String {
            reported
                .get(logical)
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: no physical id for {logical:?} in {resources}"))
        };
        let outputs: BTreeMap<String, String> = outputs_of(&described);

        // "Ref returns the backup vault name": the vault's physical id is the name the
        // template asked for, or the one CloudFormation generated for a template that
        // named none — which has to be a name AWS Backup accepts, or the vault could not
        // have been created there at all.
        let vault = physical(row.vault_logical_id);
        if let Some(declared) = row.declared_vault {
            assert_eq!(
                vault, declared,
                "case {case:?}: expected the vault's physical id to be the declared name \
                 {declared:?}, got {vault:?}"
            );
        }
        assert!(
            is_vault_name(&vault),
            "case {case:?}: expected a vault name inside the published name form, got \
             {vault:?}"
        );
        let expected_vault_arn = vault_arn(region, &vault);
        assert_eq!(
            outputs.get(row.vault_name_output),
            Some(&vault),
            "case {case:?}: expected output {:?} to be the vault name {vault:?}, got {:?}",
            row.vault_name_output,
            outputs.get(row.vault_name_output)
        );
        assert_eq!(
            outputs.get(row.vault_arn_output),
            Some(&expected_vault_arn),
            "case {case:?}: expected output {:?} to be the vault ARN {expected_vault_arn:?}, \
             got {:?}",
            row.vault_arn_output,
            outputs.get(row.vault_arn_output)
        );

        // The plan's physical id is the one AWS Backup MINTED for it, and every output
        // naming the plan carries that id rather than a name CloudFormation computed.
        let plan_id = physical(row.plan_logical_id);
        for key in row.plan_id_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&plan_id),
                "case {case:?}: expected output {key:?} to be the minted plan id \
                 {plan_id:?}, got {:?}",
                outputs.get(*key)
            );
        }

        // ...and AWS Backup itself resolves that id to the plan the template declared.
        let (status, answered) =
            backup(&router, region, "GET", &format!("/backup/plans/{plan_id}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetBackupPlan on the id the stack \
             published, got {status} with body {answered}"
        );
        assert_eq!(
            answered.get("BackupPlanId").and_then(Value::as_str),
            Some(plan_id.as_str()),
            "case {case:?}: expected the read to report the plan asked for, got {answered}"
        );
        assert_eq!(
            answered.get("BackupPlanArn").and_then(Value::as_str),
            Some(plan_arn(region, &plan_id).as_str()),
            "case {case:?}: expected the plan ARN to name the minted id, got {answered}"
        );
        assert_eq!(
            answered
                .pointer("/BackupPlan/BackupPlanName")
                .and_then(Value::as_str),
            Some(row.plan_name),
            "case {case:?}: expected the declared plan name, got {answered}"
        );
        let read_rules = answered
            .pointer("/BackupPlan/Rules")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            read_rules.len(),
            row.rules.len(),
            "case {case:?}: expected {} rule(s) read back, got {answered}",
            row.rules.len()
        );
        for (expected, rule) in row.rules.iter().zip(read_rules.iter()) {
            let named = expected.rule_name;
            assert_eq!(
                rule.get("RuleName").and_then(Value::as_str),
                Some(named),
                "case {case:?}: expected rule {named:?}, got {rule}"
            );
            // The one member the type spells differently from the API it reaches:
            // `TargetBackupVault` in a template, `TargetBackupVaultName` on the wire.
            assert_eq!(
                rule.get("TargetBackupVaultName").and_then(Value::as_str),
                Some(vault.as_str()),
                "case {case:?}: expected rule {named:?} to target the declared vault \
                 {vault:?}, got {rule}"
            );
            assert_eq!(
                rule.get("ScheduleExpression").and_then(Value::as_str),
                expected.schedule,
                "case {case:?}: expected rule {named:?} to carry the declared schedule, got \
                 {rule}"
            );
            assert_eq!(
                rule.get("StartWindowMinutes").and_then(Value::as_i64),
                Some(expected.start_window),
                "case {case:?}: expected rule {named:?} to carry StartWindowMinutes {}, got \
                 {rule}",
                expected.start_window
            );
            assert_eq!(
                rule.get("CompletionWindowMinutes").and_then(Value::as_i64),
                Some(expected.completion_window),
                "case {case:?}: expected rule {named:?} to carry CompletionWindowMinutes \
                 {}, got {rule}",
                expected.completion_window
            );
            assert_eq!(
                rule.pointer("/Lifecycle/DeleteAfterDays")
                    .and_then(Value::as_i64),
                expected.delete_after_days,
                "case {case:?}: expected rule {named:?} to carry the declared lifecycle, got \
                 {rule}"
            );
            assert_eq!(
                rule.pointer("/CopyActions/0/DestinationBackupVaultArn")
                    .and_then(Value::as_str),
                expected.copy_destination,
                "case {case:?}: expected rule {named:?} to carry the declared copy action, \
                 got {rule}"
            );
            // Every stored rule carries the `RuleId` only the service can mint.
            assert!(
                rule.get("RuleId")
                    .and_then(Value::as_str)
                    .is_some_and(|id| !id.is_empty()),
                "case {case:?}: expected rule {named:?} to carry a RuleId, got {rule}"
            );
        }

        // The selection: its physical id is the `SelectionId` AWS Backup minted, and that
        // service reports the body the template declared under the plan the stack created.
        let selection_id = physical(row.selection_logical_id);
        let (status, listed) = backup(
            &router,
            region,
            "GET",
            &format!("/backup/plans/{plan_id}/selections"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListBackupSelections on the plan the \
             stack published, got {status} with body {listed}"
        );
        let listed_ids: Vec<&str> = listed
            .get("BackupSelectionsList")
            .and_then(Value::as_array)
            .map(|members| {
                members
                    .iter()
                    .filter_map(|member| member.get("SelectionId").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            listed_ids,
            vec![selection_id.as_str()],
            "case {case:?}: expected the plan to list the selection the stack created, got \
             {listed}"
        );
        let (status, answered) = backup(
            &router,
            region,
            "GET",
            &format!("/backup/plans/{plan_id}/selections/{selection_id}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetBackupSelection on the ids the stack \
             published, got {status} with body {answered}"
        );
        assert_eq!(
            answered.get("BackupPlanId").and_then(Value::as_str),
            Some(plan_id.as_str()),
            "case {case:?}: expected the selection to belong to the deployed plan, got \
             {answered}"
        );
        let declared_selection = (row.selection)();
        for (member, value) in declared_selection
            .as_object()
            .expect("a declared selection is an object")
        {
            assert_eq!(
                answered.pointer("/BackupSelection").and_then(|body| body.get(member)),
                Some(value),
                "case {case:?}: expected the selection's {member} to be {value}, got \
                 {answered}"
            );
        }

        // The vault: AWS Backup resolves the name the stack published to the vault it
        // created, under the ARN the stack reported for it.
        let (status, answered) =
            backup(&router, region, "GET", &format!("/backup-vaults/{vault}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeBackupVault on the name the stack \
             published, got {status} with body {answered}"
        );
        assert_eq!(
            answered.get("BackupVaultArn").and_then(Value::as_str),
            Some(expected_vault_arn.as_str()),
            "case {case:?}: expected the read to report {expected_vault_arn}, got {answered}"
        );
        assert_eq!(
            answered.get("BackupVaultName").and_then(Value::as_str),
            Some(vault.as_str()),
            "case {case:?}: expected the read to report the vault name {vault:?}, got \
             {answered}"
        );

        // Deleting the stack deletes what AWS Backup holds for it — the selection before
        // the plan that holds it and the vault, which is the order the resources were
        // created in reversed.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (status, gone) = backup(
            &router,
            region,
            "GET",
            &format!("/backup/plans/{plan_id}/selections/{selection_id}"),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected GetBackupSelection to refuse the deleted selection, got \
             {status} with body {gone}"
        );
        // A deleted plan is no longer there to read: AWS Backup records the deletion on
        // the version the account held and refuses every later read of it, so this is how
        // the stack's plan is observed gone.
        let (status, gone) =
            backup(&router, region, "GET", &format!("/backup/plans/{plan_id}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected GetBackupPlan to refuse the deleted plan, got {status} \
             with body {gone}"
        );
        assert_eq!(
            gone.get("__type").and_then(Value::as_str),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected the modelled not-found refusal for the deleted plan, \
             got {gone}"
        );
        let (status, gone) =
            backup(&router, region, "GET", &format!("/backup-vaults/{vault}")).await;
        assert_ne!(
            status, 200,
            "case {case:?}: expected no backup vault named {vault:?} once the stack deleted \
             it, the read answered {status} with body {gone}"
        );
    }
}

/// The refusal a plan id nothing created still gets: `GetBackupPlan` answers HTTP 400
/// `ResourceNotFoundException` for an id no stack and no client ever minted — which is what
/// makes the 200 above evidence that the stack really created the plan rather than that the
/// read answers anything at all.
#[tokio::test]
async fn an_id_no_stack_created_is_still_refused() {
    let router = build_router();
    let region = "us-east-2";
    for (case, plan_id) in [
        ("an id of the form the service mints", "8f81f553-3a74-4a3f-b93d-b3360dc80c50"),
        ("a name of the form CloudFormation generates", "backup-estate-Plan-ABCDEFGH1234"),
    ] {
        let (status, refused) =
            backup(&router, region, "GET", &format!("/backup/plans/{plan_id}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from GetBackupPlan, got {status} with body \
             {refused}"
        );
        assert!(
            refused
                .get("__type")
                .and_then(Value::as_str)
                .is_some_and(|code| code.ends_with("ResourceNotFoundException")),
            "case {case:?}: expected ResourceNotFoundException, got {refused}"
        );
    }
}
