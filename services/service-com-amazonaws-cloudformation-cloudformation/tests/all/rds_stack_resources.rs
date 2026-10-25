// SPDX-License-Identifier: CC0-1.0

//! A stack's Amazon RDS resources in the service that OWNS them, end to end.
//!
//! RULE: a resource CloudFormation reports as created exists in the service that owns its
//! type, under the identifier the stack reports as its `PhysicalResourceId` — "CloudFormation
//! then makes the underlying service calls to provision your resources" — so the stack's view
//! and the owning service's view of the resource cannot disagree, and a client that deploys a
//! database by template can address it through that service's own API afterwards. Deleting
//! the stack deletes it again, and a resource the owning service refuses leaves the stack in
//! `*_FAILED` with that refusal as the reason, never in `*_COMPLETE`.
//!
//! The types here are `AWS::RDS::DBInstance` (whose documented `Ref` return value is the
//! `DBInstanceIdentifier`) and the `AWS::RDS::DBSubnetGroup` a DB instance's
//! `DBSubnetGroupName` names, which Amazon RDS resolves before it will create an instance in
//! it. `crate::state::provisioner` is the layer under test.
//!
//! Every case drives the real request path for BOTH services through one router — the
//! CloudFormation calls over the awsQuery wire (the harness `stack_lifecycle.rs` owns) and
//! the Amazon RDS calls as a client's SDK makes them — so what is asserted is what a client
//! that deploys a database by template and then calls `DescribeDBInstances` observes.

use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, rds, router, wait_for_the_transition, xml_text,
    xml_texts,
};

/// The `PhysicalResourceId` the stack reports for the resource `logical_id`.
async fn physical_id(router: &MultiServiceRouter, stack: &str, logical_id: &str) -> String {
    let (status_code, body) = call(
        router,
        "DescribeStackResources",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "DescribeStackResources({stack}, {logical_id}) must answer 200, got {status_code} \
         {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_default()
}

/// The `OutputValue` the stack reports for `key`, read as the pair of lists the response
/// carries (every output of every template here resolves, so the pairing cannot slip).
fn output_value(xml: &str, key: &str) -> String {
    let keys = xml_texts(xml, "OutputKey");
    let values = xml_texts(xml, "OutputValue");
    keys.iter()
        .zip(values.iter())
        .find(|(output_key, _)| output_key.as_str() == key)
        .map(|(_, value)| value.clone())
        .unwrap_or_default()
}

/// `DescribeDBInstances` on one identifier, as a client calls it.
async fn describe_db_instance(
    router: &MultiServiceRouter,
    identifier: &str,
) -> (u16, String) {
    rds(
        router,
        "DescribeDBInstances",
        &[("DBInstanceIdentifier", identifier)],
    )
    .await
}

/// One member Amazon RDS must report for the instance the stack created.
struct Reported {
    /// The element of the `DBInstance` structure.
    member: &'static str,
    /// What RDS must report in it.
    value: String,
}

fn reported(member: &'static str, value: &str) -> Reported {
    Reported {
        member,
        value: value.to_string(),
    }
}

/// One row of the provisioning table.
struct ProvisionCase<'a> {
    case: &'a str,
    stack_name: &'a str,
    template: &'a str,
    /// The logical id of the DB instance the row declares.
    instance: &'a str,
    /// The identifier the stack must report for it: `Some` where the template NAMES the
    /// instance, `None` where CloudFormation generates the name.
    declared_identifier: Option<&'a str>,
    /// The members RDS must report for that instance — the declared configuration really
    /// in force on the database.
    members: Vec<Reported>,
    /// The VPC security group ids RDS must report for the instance: the type spells the
    /// property `VPCSecurityGroups` and the API's member is `VpcSecurityGroupIds`, so a row
    /// that declares them exercises the one rename the arm makes.
    security_groups: &'a [&'a str],
    /// The logical id of a DB subnet group the row declares, if any.
    subnet_group: Option<&'a str>,
}

/// A DB instance's identifier, as Amazon RDS stores it: "this parameter is stored as a
/// lowercase string".
fn is_rds_identifier(value: &str) -> bool {
    (1..=63).contains(&value.chars().count())
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && value.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && !value.ends_with('-')
        && !value.contains("--")
}

#[tokio::test]
async fn a_db_instance_a_stack_declares_exists_in_rds_once_the_stack_reports_it_created() {
    // The rows differ along what the rule generalises over: whether the template NAMES the
    // instance or leaves CloudFormation to generate the identifier, the template spelling
    // (YAML with short-form intrinsics and JSON), how MANY properties the declaration
    // carries and of which kinds (strings, numbers, booleans, a tag list, a reference to
    // another resource of the stack), which engine the database runs, how many resources
    // one stack declares, and whether the instance is placed in a DB subnet group the same
    // stack creates — and the last row the two properties whose API member is spelled
    // differently from the resource type's property.
    let cases: &[ProvisionCase<'_>] = &[
        ProvisionCase {
            case: "an unnamed instance declaring only what the create requires, in YAML",
            stack_name: "rds-generated-name",
            template: concat!(
                "Resources:\n",
                "  Database:\n",
                "    Type: AWS::RDS::DBInstance\n",
                "    Properties:\n",
                "      Engine: mysql\n",
                "      DBInstanceClass: db.t3.micro\n",
                "Outputs:\n",
                "  InstanceId:\n",
                "    Value: !Ref Database\n",
                "  Endpoint:\n",
                "    Value: !GetAtt Database.Endpoint.Address\n",
                "  Arn:\n",
                "    Value: !GetAtt Database.DBInstanceArn\n",
            ),
            instance: "Database",
            declared_identifier: None,
            members: vec![
                reported("Engine", "mysql"),
                reported("DBInstanceClass", "db.t3.micro"),
            ],
            security_groups: &[],
            subnet_group: None,
        },
        ProvisionCase {
            case: "a named PostgreSQL instance in a DB subnet group the stack creates, in \
                   JSON",
            stack_name: "rds-subnet-group",
            template: r#"{"Resources":{
                   "Subnets":{"Type":"AWS::RDS::DBSubnetGroup","Properties":{
                     "DBSubnetGroupDescription":"the probe stack's subnets",
                     "SubnetIds":["subnet-0123456789abcdef0","subnet-0fedcba987654321f"]}},
                   "Database":{"Type":"AWS::RDS::DBInstance","Properties":{
                     "DBInstanceIdentifier":"cfn-probe-postgres",
                     "Engine":"postgres","DBInstanceClass":"db.t3.small",
                     "AllocatedStorage":"20","MasterUsername":"probeadmin",
                     "MasterUserPassword":"an-example-master-password",
                     "DBSubnetGroupName":{"Ref":"Subnets"},
                     "Port":"15432","MultiAZ":false,"StorageType":"gp3",
                     "BackupRetentionPeriod":3,
                     "Tags":[{"Key":"env","Value":"probe"}]}}},
                 "Outputs":{"InstanceId":{"Value":{"Ref":"Database"}},
                   "Endpoint":{"Value":{"Fn::GetAtt":["Database","Endpoint.Address"]}},
                   "Arn":{"Value":{"Fn::GetAtt":["Database","DBInstanceArn"]}}}}"#,
            instance: "Database",
            declared_identifier: Some("cfn-probe-postgres"),
            members: vec![
                reported("Engine", "postgres"),
                reported("DBInstanceClass", "db.t3.small"),
                reported("AllocatedStorage", "20"),
                reported("MasterUsername", "probeadmin"),
                reported("StorageType", "gp3"),
                reported("BackupRetentionPeriod", "3"),
                reported("MultiAZ", "false"),
            ],
            security_groups: &[],
            subnet_group: Some("Subnets"),
        },
        ProvisionCase {
            case: "two instances in one stack, one of them using a parameter group the \
                   template declares",
            stack_name: "rds-parameter-group",
            template: concat!(
                "Resources:\n",
                "  Params:\n",
                "    Type: AWS::RDS::DBParameterGroup\n",
                "    Properties:\n",
                "      Description: the probe stack's parameters\n",
                "      Family: mysql8.0\n",
                "  Primary:\n",
                "    Type: AWS::RDS::DBInstance\n",
                "    Properties:\n",
                "      DBInstanceIdentifier: cfn-probe-primary\n",
                "      Engine: mysql\n",
                "      DBInstanceClass: db.t3.micro\n",
                "      DBParameterGroupName: !Ref Params\n",
                "  Secondary:\n",
                "    Type: AWS::RDS::DBInstance\n",
                "    Properties:\n",
                "      Engine: mariadb\n",
                "      DBInstanceClass: db.t4g.medium\n",
                "Outputs:\n",
                "  InstanceId:\n",
                "    Value: !Ref Primary\n",
                "  Endpoint:\n",
                "    Value: !GetAtt Primary.Endpoint.Address\n",
                "  Arn:\n",
                "    Value: !GetAtt Primary.DBInstanceArn\n",
                "  SecondaryId:\n",
                "    Value: !Ref Secondary\n",
            ),
            instance: "Primary",
            declared_identifier: Some("cfn-probe-primary"),
            members: vec![
                reported("Engine", "mysql"),
                reported("DBInstanceClass", "db.t3.micro"),
            ],
            security_groups: &[],
            subnet_group: None,
        },
        ProvisionCase {
            // The two properties Amazon RDS's own `CreateDBInstance` spells differently
            // from the resource type: `VPCSecurityGroups` is the API's
            // `VpcSecurityGroupIds` and `CustomIAMInstanceProfile` its
            // `CustomIamInstanceProfile`, so a declaration carrying either reaches the
            // create only if the arm renames it.
            case: "the properties Amazon RDS spells differently from the resource type",
            stack_name: "rds-renamed-properties",
            template: r#"{"Resources":{"Database":{"Type":"AWS::RDS::DBInstance",
                     "Properties":{"DBInstanceIdentifier":"cfn-probe-renamed",
                       "Engine":"mariadb","DBInstanceClass":"db.m5.large",
                       "VPCSecurityGroups":["sg-0123456789abcdef0",
                         "sg-00112233445566778"],
                       "CustomIAMInstanceProfile":"AWSRDSCustomInstanceProfile"}}},
                 "Outputs":{"InstanceId":{"Value":{"Ref":"Database"}},
                   "Endpoint":{"Value":{"Fn::GetAtt":["Database","Endpoint.Address"]}},
                   "Arn":{"Value":{"Fn::GetAtt":["Database","DBInstanceArn"]}}}}"#,
            instance: "Database",
            declared_identifier: Some("cfn-probe-renamed"),
            members: vec![
                reported("Engine", "mariadb"),
                reported("DBInstanceClass", "db.m5.large"),
            ],
            security_groups: &["sg-0123456789abcdef0", "sg-00112233445566778"],
            subnet_group: None,
        },
    ];

    for case in cases {
        let router = router();
        let (status_code, created) =
            create_stack(&router, case.stack_name, case.template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: CreateStack must answer 200, got {status_code} {created}",
            case.case
        );
        wait_for_the_transition().await;

        let (status_code, described) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: DescribeStacks must answer 200, got {status_code} {described}",
            case.case
        );
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the stack must reach CREATE_COMPLETE, got {described}",
            case.case
        );

        // The identifier the stack reports for the instance — the same value its `Ref`
        // output publishes.
        let identifier = physical_id(&router, case.stack_name, case.instance).await;
        let published = output_value(&described, "InstanceId");
        assert_eq!(
            published, identifier,
            "case {:?}: the InstanceId output must publish the physical id of {}, got \
             {published:?} against {identifier:?}",
            case.case, case.instance
        );
        if let Some(declared) = case.declared_identifier {
            assert_eq!(
                identifier, declared,
                "case {:?}: the template named the instance, so that name is its \
                 identifier",
                case.case
            );
        }
        assert!(
            is_rds_identifier(&identifier),
            "case {:?}: {identifier:?} must be an identifier Amazon RDS accepts",
            case.case
        );

        // The whole of the rule: Amazon RDS answers for that identifier, with the
        // configuration the template declared, and the instance has reached the status a
        // client waits for.
        let (status_code, reported_instance) = describe_db_instance(&router, &identifier).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: DescribeDBInstances({identifier}) must answer 200, got \
             {status_code} {reported_instance}",
            case.case
        );
        assert_eq!(
            xml_text(&reported_instance, "DBInstanceIdentifier").as_deref(),
            Some(identifier.as_str()),
            "case {:?}: RDS must report the instance under the identifier the stack \
             published, got {reported_instance}",
            case.case
        );
        assert_eq!(
            xml_text(&reported_instance, "DBInstanceStatus").as_deref(),
            Some("available"),
            "case {:?}: the instance must have reached `available`, got \
             {reported_instance}",
            case.case
        );
        for expected in &case.members {
            let actual = xml_text(&reported_instance, expected.member);
            assert_eq!(
                actual.as_deref(),
                Some(expected.value.as_str()),
                "case {:?}: RDS must report {} as {:?}, got {actual:?} in \
                 {reported_instance}",
                case.case,
                expected.member,
                expected.value
            );
        }
        let memberships = xml_texts(&reported_instance, "VpcSecurityGroupId");
        for group in case.security_groups {
            assert!(
                memberships.iter().any(|reported| reported == group),
                "case {:?}: RDS must report {group:?} among the instance's VPC security \
                 groups, got {memberships:?} in {reported_instance}",
                case.case
            );
        }

        // The two views of the resource AGREE: the endpoint and the ARN the stack
        // publishes are the ones Amazon RDS reports for that database, not values
        // CloudFormation computed for itself.
        for (output, element) in [("Endpoint", "Address"), ("Arn", "DBInstanceArn")] {
            let stack_value = output_value(&described, output);
            let rds_value = xml_text(&reported_instance, element).unwrap_or_default();
            assert_eq!(
                stack_value, rds_value,
                "case {:?}: the {output} output must be the {element} Amazon RDS reports, \
                 got {stack_value:?} against {rds_value:?}",
                case.case
            );
        }

        // A DB subnet group the same stack declares is created too, and the instance is
        // really placed in it: an instance RDS would have refused for a group it does not
        // hold could not be there at all.
        if let Some(logical_id) = case.subnet_group {
            let group = physical_id(&router, case.stack_name, logical_id).await;
            let (status_code, reported_group) = rds(
                &router,
                "DescribeDBSubnetGroups",
                &[("DBSubnetGroupName", &group)],
            )
            .await;
            assert_eq!(
                status_code, 200,
                "case {:?}: DescribeDBSubnetGroups({group}) must answer 200, got \
                 {status_code} {reported_group}",
                case.case
            );
            assert_eq!(
                xml_text(&reported_group, "DBSubnetGroupDescription").as_deref(),
                Some("the probe stack's subnets"),
                "case {:?}: the group must carry the declared description, got \
                 {reported_group}",
                case.case
            );
            assert!(
                xml_texts(&reported_instance, "DBSubnetGroupName")
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(&group)),
                "case {:?}: the instance must be placed in {group}, got \
                 {reported_instance}",
                case.case
            );
        }
    }
}

/// The DB parameter group row's own assertion: the group's name reaches the database.
///
/// `AWS::RDS::DBParameterGroup` is a type this emulator records rather than creates (Amazon
/// RDS's `CreateDBParameterGroup` is still a stub, so there is no API to create it through
/// and the resource is left declared-only), but the instance that NAMES it is created for
/// real — so the `DBParameterGroupName` the stack resolved is the one in force on the
/// database RDS reports.
#[tokio::test]
async fn the_parameter_group_a_stack_declares_is_the_one_in_force_on_its_instance() {
    let router = router();
    let template = concat!(
        "Resources:\n",
        "  Params:\n",
        "    Type: AWS::RDS::DBParameterGroup\n",
        "    Properties:\n",
        "      Description: the probe stack's parameters\n",
        "      Family: mysql8.0\n",
        "  Database:\n",
        "    Type: AWS::RDS::DBInstance\n",
        "    Properties:\n",
        "      Engine: mysql\n",
        "      DBInstanceClass: db.t3.micro\n",
        "      DBParameterGroupName: !Ref Params\n",
    );
    create_stack(&router, "rds-effective-params", template, &[]).await;
    wait_for_the_transition().await;

    let group = physical_id(&router, "rds-effective-params", "Params").await;
    assert!(
        !group.is_empty(),
        "the stack must report a physical id for the parameter group"
    );
    let identifier = physical_id(&router, "rds-effective-params", "Database").await;
    let (status_code, reported) = describe_db_instance(&router, &identifier).await;
    assert_eq!(
        status_code, 200,
        "DescribeDBInstances({identifier}) must answer 200, got {status_code} {reported}"
    );
    assert!(
        xml_texts(&reported, "DBParameterGroupName")
            .iter()
            .any(|name| name == &group),
        "the instance must report {group:?} as its DB parameter group, got {reported}"
    );
}

#[tokio::test]
async fn deleting_a_stack_deletes_the_db_instance_and_subnet_group_it_created() {
    let router = router();
    let template = concat!(
        "Resources:\n",
        "  Subnets:\n",
        "    Type: AWS::RDS::DBSubnetGroup\n",
        "    Properties:\n",
        "      DBSubnetGroupDescription: the probe stack's subnets\n",
        "      SubnetIds:\n",
        "        - subnet-0123456789abcdef0\n",
        "        - subnet-0fedcba987654321f\n",
        "  Database:\n",
        "    Type: AWS::RDS::DBInstance\n",
        "    Properties:\n",
        "      DBInstanceIdentifier: cfn-probe-deleted\n",
        "      Engine: mysql\n",
        "      DBInstanceClass: db.t3.micro\n",
        "      DBSubnetGroupName: !Ref Subnets\n",
    );
    create_stack(&router, "rds-deleted-stack", template, &[]).await;
    wait_for_the_transition().await;
    let group = physical_id(&router, "rds-deleted-stack", "Subnets").await;
    let (status_code, _) = describe_db_instance(&router, "cfn-probe-deleted").await;
    assert_eq!(
        status_code, 200,
        "the stack's instance must exist before the delete"
    );

    let (status_code, deleted) = call(
        &router,
        "DeleteStack",
        &[member("StackName", "rds-deleted-stack")],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "DeleteStack must answer 200, got {status_code} {deleted}"
    );
    // Twice: the stack's own terminal status and the removal Amazon RDS owes the instance
    // it put in `deleting` are two clocks, and both have to be past due.
    wait_for_the_transition().await;
    wait_for_the_transition().await;

    let (status_code, reported) = describe_db_instance(&router, "cfn-probe-deleted").await;
    assert_eq!(
        status_code, 404,
        "deleting the stack must delete the instance it created, got {status_code} \
         {reported}"
    );
    assert_eq!(
        xml_text(&reported, "Code").as_deref(),
        Some("DBInstanceNotFound"),
        "the instance must be gone under RDS's own not-found code, got {reported}"
    );
    let (status_code, reported) = rds(
        &router,
        "DescribeDBSubnetGroups",
        &[("DBSubnetGroupName", &group)],
    )
    .await;
    assert_eq!(
        status_code, 404,
        "deleting the stack must delete the DB subnet group it created, got \
         {status_code} {reported}"
    );
}

#[tokio::test]
async fn a_declaration_amazon_rds_refuses_leaves_the_stack_failed_and_nothing_created() {
    let router = router();
    // A DB instance class Amazon RDS does not publish: the create is refused there, so the
    // stack cannot report the resource as created.
    let template = concat!(
        "Resources:\n",
        "  Database:\n",
        "    Type: AWS::RDS::DBInstance\n",
        "    Properties:\n",
        "      DBInstanceIdentifier: cfn-probe-refused\n",
        "      Engine: mysql\n",
        "      DBInstanceClass: not-a-db-instance-class\n",
    );
    create_stack(&router, "rds-refused", template, &[]).await;
    wait_for_the_transition().await;

    let (_, described) = describe_stack(&router, "rds-refused").await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::ROLLBACK_COMPLETE),
        "a resource the owning service refused must roll the stack back to \
         ROLLBACK_COMPLETE, got {described}"
    );
    let (status_code, reported) = describe_db_instance(&router, "cfn-probe-refused").await;
    assert_eq!(
        status_code, 404,
        "nothing must have been created for the refused declaration, got {status_code} \
         {reported}"
    );
}

#[tokio::test]
async fn a_property_this_emulator_does_not_provision_is_refused_rather_than_dropped() {
    let router = router();
    // `SourceDBInstanceIdentifier` asks for a READ REPLICA, which is Amazon RDS's own
    // `CreateDBInstanceReadReplica` — a different call, and one this emulator has not
    // written. Creating a plain DB instance instead would report a resource configured
    // differently from the declaration, so the stack fails and says so.
    let template = concat!(
        "Resources:\n",
        "  Replica:\n",
        "    Type: AWS::RDS::DBInstance\n",
        "    Properties:\n",
        "      DBInstanceIdentifier: cfn-probe-replica\n",
        "      DBInstanceClass: db.t3.micro\n",
        "      SourceDBInstanceIdentifier: some-primary\n",
    );
    create_stack(&router, "rds-unsupported", template, &[]).await;
    wait_for_the_transition().await;

    let (_, described) = describe_stack(&router, "rds-unsupported").await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::ROLLBACK_COMPLETE),
        "a property no create of this emulator sends must roll the stack back to \
         ROLLBACK_COMPLETE, got {described}"
    );
    let reason = xml_text(&described, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("SourceDBInstanceIdentifier"),
        "the reason must name the property that could not be provisioned, got {reason:?}"
    );
    let (status_code, reported) = describe_db_instance(&router, "cfn-probe-replica").await;
    assert_eq!(
        status_code, 404,
        "nothing must have been created for the refused declaration, got {status_code} \
         {reported}"
    );
}
