// SPDX-License-Identifier: CC0-1.0

//! A bucket's S3 Inventory reports as a stack declares them, read back through Amazon S3.
//!
//! RULE: a resource CloudFormation provisions carries, in the service that OWNS it, every
//! sub-resource configuration its declaration states — each applied by the one API call that
//! service publishes for it, since none of them is a member of the create — so a read of that
//! sub-resource straight after the stack reports the resource created answers with what the
//! template declared. For an `AWS::S3::Bucket`'s `InventoryConfigurations` that is one
//! `PutBucketInventoryConfiguration` per declared entry, under the `Id` the entry names, so
//! `s3:ListBucketInventoryConfigurations` on the bucket the stack published reports exactly the
//! declared ids and `s3:GetBucketInventoryConfiguration` reports each one's members.
//!
//! The template's spelling is FLAT where the API nests it, and the rule covers the whole fold.
//! An entry states its object scope as a single `Prefix` where the API takes the `Filter` whose
//! one REQUIRED member is that prefix, and its schedule as a single `ScheduleFrequency` where
//! the API takes the `Schedule` whose one REQUIRED member is that frequency
//! (`smithy_query(query="shape",shape="InventoryFilter,InventorySchedule")`); `Enabled` is the
//! API's `IsEnabled`, modelled a BOOLEAN; and the destination is reported one level deeper as
//! `Destination.S3BucketDestination`, whose REQUIRED `Bucket` is "the Amazon Resource Name (ARN)
//! of the bucket where inventory results will be published" beside the optional `AccountId` and
//! `Prefix`, "the prefix that is prepended to all inventory results"
//! (`smithy_query(query="shape",shape="InventoryS3BucketDestination")`). An optional member no
//! declaration states is reported by NO element at all, never as an empty one — so a report that
//! sets no destination prefix can be told from one that sets it.
//!
//! The rows differ along what the rule generalises over: HOW MANY configurations one bucket
//! declares (none, one, two), whether the entry declares an object scope at all, whether the
//! report is enabled or disabled and in which spelling the template gives that boolean (JSON's
//! own and the quoted text), which `ScheduleFrequency`, `IncludedObjectVersions` and `Format`
//! value it names, how many `OptionalFields` it asks for (none, an empty list, one, two), which
//! optional destination
//! members it states (an account id and a prefix, each present or absent), where the destination
//! ARN COMES FROM (a stack parameter, an `Fn::GetAtt` on a bucket the same template declares),
//! the template SPELLING (JSON and YAML, long- and short-form intrinsics), and how many buckets
//! one stack declares.
//!
//! The enum values the rows name come from this service's own catalogues
//! (`smithy_query(query="shape",shape="InventoryFrequency,InventoryIncludedObjectVersions,InventoryFormat,InventoryOptionalField")`),
//! and cover entries the issue behind this suite did not cite (`Weekly`, `All`, `ORC`,
//! `Parquet`). Every expected value is computed from what the row declares — the account id the
//! emulator mints and the `arn:aws:s3:::<name>` form an `AWS::S3::Bucket`'s `Arn` attribute
//! publishes — never read back out of the inventory answer under test.

use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    create_stack, describe_stack, member, router, wait_for_the_transition, xml_blocks, xml_text,
    xml_texts, ACCOUNT_ID,
};
use crate::stack_resource_provisioning::{output_value, s3};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// The destination bucket ARN the parameterised row hands in, and the account that is NOT the
/// deploying one — so a row's cross-account arm cannot pass by accident.
const DESTINATION_ARN: &str = "arn:aws:s3:::xacct-inventory-reports";
const FOREIGN_ACCOUNT_ID: &str = "210987654321";

/// Where a declared destination bucket ARN comes from.
#[derive(Debug, PartialEq, Eq)]
enum Arn {
    /// An ARN the row hands the stack as a parameter, or spells as a literal.
    Given(&'static str),
    /// An `AWS::S3::Bucket` the same template declares under this `BucketName`, reached through
    /// `Fn::GetAtt … Arn`: an Amazon S3 bucket ARN carries neither a region nor an account id.
    Bucket(&'static str),
}

impl Arn {
    fn text(&self) -> String {
        match self {
            Self::Given(arn) => (*arn).to_string(),
            Self::Bucket(name) => format!("arn:aws:s3:::{name}"),
        }
    }
}

/// Which account a declared destination account id names.
#[derive(Debug, PartialEq, Eq)]
enum Account {
    /// An account id the row spells out, which is not the deploying one.
    Foreign,
    /// `Ref AWS::AccountId` — the account the stack is deployed into.
    Deploying,
}

impl Account {
    fn text(&self) -> String {
        match self {
            Self::Foreign => FOREIGN_ACCOUNT_ID.to_string(),
            Self::Deploying => ACCOUNT_ID.to_string(),
        }
    }
}

/// One configuration a row expects the bucket to carry.
struct Expected {
    id: &'static str,
    enabled: bool,
    frequency: &'static str,
    versions: &'static str,
    /// The `Filter`'s prefix, or `None` for an entry declaring no object scope — which is the
    /// report that covers every object.
    filter_prefix: Option<&'static str>,
    format: &'static str,
    bucket: Arn,
    account_id: Option<Account>,
    destination_prefix: Option<&'static str>,
    optional_fields: &'static [&'static str],
}

/// One bucket of a row: the stack output that publishes its name, and the configurations
/// Amazon S3 must report for it.
struct Bucket {
    output_key: &'static str,
    configurations: &'static [Expected],
}

/// One row of the table.
struct InventoryCase {
    case: &'static str,
    stack_name: &'static str,
    template: &'static str,
    /// The stack parameters the row supplies, as `(ParameterKey, ParameterValue)`.
    parameters: &'static [(&'static str, &'static str)],
    buckets: &'static [Bucket],
}

/// One `InventoryConfiguration` as Amazon S3 reported it, read as the values a declaration
/// fixes so the two sides are compared as one value.
///
/// Each optional member is read as an `Option`, so an element the answer OMITS is distinguished
/// from one it carries empty.
#[derive(Debug, PartialEq, Eq)]
struct Reported {
    id: Option<String>,
    /// `IsEnabled` as the answer spells the boolean, so `true` is asserted rather than any text.
    enabled: Option<String>,
    frequency: Option<String>,
    versions: Option<String>,
    filter_prefix: Option<String>,
    format: Option<String>,
    bucket: Option<String>,
    account_id: Option<String>,
    destination_prefix: Option<String>,
    optional_fields: Vec<String>,
}

/// What a row EXPECTS, in the same shape the answer is read into.
fn wanted(configuration: &Expected) -> Reported {
    Reported {
        id: Some(configuration.id.to_string()),
        enabled: Some(configuration.enabled.to_string()),
        frequency: Some(configuration.frequency.to_string()),
        versions: Some(configuration.versions.to_string()),
        filter_prefix: configuration.filter_prefix.map(str::to_string),
        format: Some(configuration.format.to_string()),
        bucket: Some(configuration.bucket.text()),
        account_id: configuration.account_id.as_ref().map(Account::text),
        destination_prefix: configuration.destination_prefix.map(str::to_string),
        optional_fields: configuration
            .optional_fields
            .iter()
            .map(|field| (*field).to_string())
            .collect(),
    }
}

/// Every `InventoryConfiguration` a body reports, in document order.
///
/// One parser for BOTH reads: a `GetBucketInventoryConfiguration` body carries one container and
/// a `ListBucketInventoryConfigurations` page as many as it holds, so the two cannot drift on how
/// a container is read. Each nested structure is taken as its own BLOCK first and scanned inside
/// it, because `Prefix` is a member of both the filter and the destination.
fn reported(xml: &str) -> Vec<Reported> {
    xml_blocks(xml, "InventoryConfiguration")
        .iter()
        .map(|entry| {
            let filter = xml_blocks(entry, "Filter").into_iter().next();
            let schedule = xml_blocks(entry, "Schedule").into_iter().next();
            let destination = xml_blocks(entry, "S3BucketDestination").into_iter().next();
            let optional_fields = xml_blocks(entry, "OptionalFields")
                .into_iter()
                .next()
                // `InventoryOptionalFields`'s member carries the xmlName `Field`.
                .map(|block| xml_texts(&block, "Field"))
                .unwrap_or_default();
            Reported {
                id: xml_text(entry, "Id"),
                enabled: xml_text(entry, "IsEnabled"),
                frequency: schedule.as_deref().and_then(|block| xml_text(block, "Frequency")),
                versions: xml_text(entry, "IncludedObjectVersions"),
                filter_prefix: filter.as_deref().and_then(|block| xml_text(block, "Prefix")),
                format: destination
                    .as_deref()
                    .and_then(|block| xml_text(block, "Format")),
                bucket: destination
                    .as_deref()
                    .and_then(|block| xml_text(block, "Bucket")),
                account_id: destination
                    .as_deref()
                    .and_then(|block| xml_text(block, "AccountId")),
                destination_prefix: destination
                    .as_deref()
                    .and_then(|block| xml_text(block, "Prefix")),
                optional_fields,
            }
        })
        .collect()
}

/// `GET /<bucket>?inventory` — `ListBucketInventoryConfigurations`.
async fn list_inventory(router: &MultiServiceRouter, bucket: &str) -> (u16, String) {
    let answer = s3(router, "GET", &format!("/{bucket}?inventory"), b"").await;
    (answer.status, answer.body)
}

/// `GET /<bucket>?inventory&id=<id>` — `GetBucketInventoryConfiguration`.
async fn get_inventory(router: &MultiServiceRouter, bucket: &str, id: &str) -> (u16, String) {
    let answer = s3(router, "GET", &format!("/{bucket}?inventory&id={id}"), b"").await;
    (answer.status, answer.body)
}

/// `DELETE /<bucket>?inventory&id=<id>` — `DeleteBucketInventoryConfiguration`.
async fn delete_inventory(router: &MultiServiceRouter, bucket: &str, id: &str) -> u16 {
    s3(
        router,
        "DELETE",
        &format!("/{bucket}?inventory&id={id}"),
        b"",
    )
    .await
    .status
}

/// The stack parameters a row supplies, as the form members `CreateStack` takes them in.
fn parameter_members(parameters: &[(&str, &str)]) -> Vec<(String, String)> {
    parameters
        .iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let position = index + 1;
            [
                member(&format!("Parameters.member.{position}.ParameterKey"), key),
                member(
                    &format!("Parameters.member.{position}.ParameterValue"),
                    value,
                ),
            ]
        })
        .collect()
}

/// Deploy a row's stack and answer with the `DescribeStacks` body it completed with.
async fn deploy(router: &MultiServiceRouter, case: &InventoryCase) -> String {
    let (http_status, created) = create_stack(
        router,
        case.stack_name,
        case.template,
        &parameter_members(case.parameters),
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {:?}: CreateStack must answer 200, got {http_status} {created}",
        case.case
    );
    wait_for_the_transition().await;

    let (http_status, described) = describe_stack(router, case.stack_name).await;
    assert_eq!(
        http_status, 200,
        "case {:?}: DescribeStacks must answer 200, got {http_status} {described}",
        case.case
    );
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "case {:?}: the stack must reach CREATE_COMPLETE, got {described}",
        case.case
    );
    described
}

/// The rows of the table, shared by the listing case and the read-back case so neither can
/// drift on what a declaration means.
fn cases() -> &'static [InventoryCase] {
    &[
        InventoryCase {
            // The IaC shape a client manages one report with: the id and the source prefix are
            // stack parameters, the destination ARN is read off the sibling bucket the same
            // template declares, and NO destination prefix is declared — so none may be
            // reported. A second bucket declares no report at all.
            case: "one parameterised report in JSON, exporting to a sibling bucket with no \
                   destination prefix",
            stack_name: "inventory-e2e",
            template: r#"{"Parameters":{
                 "InventoryConfigurationId":{"Type":"String"},
                 "SourcePrefix":{"Type":"String"}},
               "Resources":{
                 "DestinationBucket":{"Type":"AWS::S3::Bucket","Properties":{
                   "BucketName":"inventory-e2e-destination"}},
                 "SourceBucket":{"Type":"AWS::S3::Bucket","Properties":{
                   "BucketName":"inventory-e2e-source",
                   "InventoryConfigurations":[
                     {"Id":{"Ref":"InventoryConfigurationId"},
                      "Enabled":true,
                      "Prefix":{"Ref":"SourcePrefix"},
                      "ScheduleFrequency":"Daily",
                      "IncludedObjectVersions":"Current",
                      "Destination":{
                        "BucketArn":{"Fn::GetAtt":["DestinationBucket","Arn"]},
                        "Format":"CSV"}}]}}},
               "Outputs":{
                 "SourceBucketName":{"Value":{"Ref":"SourceBucket"}},
                 "DestinationBucketName":{"Value":{"Ref":"DestinationBucket"}},
                 "DestinationBucketArn":{"Value":{"Fn::GetAtt":["DestinationBucket","Arn"]}}}}"#,
            parameters: &[
                ("InventoryConfigurationId", "daily-with-prefix"),
                ("SourcePrefix", "data/"),
            ],
            buckets: &[
                Bucket {
                    output_key: "SourceBucketName",
                    configurations: &[Expected {
                        id: "daily-with-prefix",
                        enabled: true,
                        frequency: "Daily",
                        versions: "Current",
                        filter_prefix: Some("data/"),
                        format: "CSV",
                        bucket: Arn::Bucket("inventory-e2e-destination"),
                        account_id: None,
                        destination_prefix: None,
                        optional_fields: &[],
                    }],
                },
                Bucket {
                    output_key: "DestinationBucketName",
                    configurations: &[],
                },
            ],
        },
        InventoryCase {
            // Two reports on one bucket, declared in an order that is NOT the ascending id
            // order the listing reports: a weekly one over every object version, exporting
            // cross-account with its own destination prefix and two optional fields, and a
            // DISABLED one whose enabled flag is the quoted text a template may spell a
            // boolean as.
            case: "two reports in YAML: a weekly cross-account export with optional fields, \
                   and a disabled one",
            stack_name: "xacct-inventory",
            template: concat!(
                "Parameters:\n",
                "  DestinationArn:\n",
                "    Type: String\n",
                "  ForeignAccount:\n",
                "    Type: String\n",
                "Resources:\n",
                "  Source:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: xacct-inventory-source\n",
                "      InventoryConfigurations:\n",
                "        - Id: WeeklyAllVersions\n",
                "          Enabled: true\n",
                "          ScheduleFrequency: Weekly\n",
                "          IncludedObjectVersions: All\n",
                "          OptionalFields:\n",
                "            - Size\n",
                "            - LastModifiedDate\n",
                "          Destination:\n",
                "            BucketArn: !Ref DestinationArn\n",
                "            BucketAccountId: !Ref ForeignAccount\n",
                "            Format: Parquet\n",
                "            Prefix: reports/weekly/\n",
                "        - Id: DisabledDaily\n",
                "          Enabled: \"false\"\n",
                "          Prefix: archive/\n",
                "          ScheduleFrequency: Daily\n",
                "          IncludedObjectVersions: Current\n",
                "          Destination:\n",
                "            BucketArn: !Ref DestinationArn\n",
                "            BucketAccountId: !Ref AWS::AccountId\n",
                "            Format: CSV\n",
                "Outputs:\n",
                "  SourceBucketName:\n",
                "    Value: !Ref Source\n",
            ),
            parameters: &[
                ("DestinationArn", DESTINATION_ARN),
                ("ForeignAccount", FOREIGN_ACCOUNT_ID),
            ],
            buckets: &[Bucket {
                output_key: "SourceBucketName",
                configurations: &[
                    Expected {
                        id: "DisabledDaily",
                        enabled: false,
                        frequency: "Daily",
                        versions: "Current",
                        filter_prefix: Some("archive/"),
                        format: "CSV",
                        bucket: Arn::Given(DESTINATION_ARN),
                        account_id: Some(Account::Deploying),
                        destination_prefix: None,
                        optional_fields: &[],
                    },
                    Expected {
                        id: "WeeklyAllVersions",
                        enabled: true,
                        frequency: "Weekly",
                        versions: "All",
                        filter_prefix: None,
                        format: "Parquet",
                        bucket: Arn::Given(DESTINATION_ARN),
                        account_id: Some(Account::Foreign),
                        destination_prefix: Some("reports/weekly/"),
                        optional_fields: &["Size", "LastModifiedDate"],
                    },
                ],
            }],
        },
        InventoryCase {
            // A third shape of the same fold: the destination ARN read off a sibling bucket
            // again, but with a destination `Prefix` declared, a third output format, every
            // object version included, and the two ways a declaration can ask for no
            // optional field — one asking for one, the other asking for an empty set —
            // beside the destination bucket, which declares no report at all.
            case: "two reports in YAML: one with a destination prefix and one optional field, \
                   and one asking for an empty set of optional fields",
            stack_name: "prefixed-inventory",
            template: concat!(
                "Resources:\n",
                "  Reports:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: prefixed-inventory-reports\n",
                "  Source:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: prefixed-inventory-source\n",
                "      InventoryConfigurations:\n",
                "        - Id: OrcEveryObject\n",
                "          Enabled: true\n",
                "          ScheduleFrequency: Daily\n",
                "          IncludedObjectVersions: All\n",
                "          OptionalFields:\n",
                "            - StorageClass\n",
                "          Destination:\n",
                "            BucketArn: !GetAtt Reports.Arn\n",
                "            Format: ORC\n",
                "            Prefix: reports/daily/\n",
                "        - Id: EmptyOptionalFields\n",
                "          Enabled: true\n",
                "          ScheduleFrequency: Weekly\n",
                "          IncludedObjectVersions: Current\n",
                "          OptionalFields: []\n",
                "          Destination:\n",
                "            BucketArn: !GetAtt Reports.Arn\n",
                "            Format: CSV\n",
                "Outputs:\n",
                "  SourceBucketName:\n",
                "    Value: !Ref Source\n",
                "  ReportsBucketName:\n",
                "    Value: !Ref Reports\n",
            ),
            parameters: &[],
            buckets: &[
                Bucket {
                    output_key: "SourceBucketName",
                    configurations: &[
                        // An EMPTY `OptionalFields` asks for the same as a declaration
                        // stating none at all, so neither reports the member.
                        Expected {
                            id: "EmptyOptionalFields",
                            enabled: true,
                            frequency: "Weekly",
                            versions: "Current",
                            filter_prefix: None,
                            format: "CSV",
                            bucket: Arn::Bucket("prefixed-inventory-reports"),
                            account_id: None,
                            destination_prefix: None,
                            optional_fields: &[],
                        },
                        Expected {
                            id: "OrcEveryObject",
                            enabled: true,
                            frequency: "Daily",
                            versions: "All",
                            filter_prefix: None,
                            format: "ORC",
                            bucket: Arn::Bucket("prefixed-inventory-reports"),
                            account_id: None,
                            destination_prefix: Some("reports/daily/"),
                            optional_fields: &["StorageClass"],
                        },
                    ],
                },
                Bucket {
                    output_key: "ReportsBucketName",
                    configurations: &[],
                },
            ],
        },
    ]
}

#[tokio::test]
async fn a_bucket_a_stack_declares_carries_the_inventory_configurations_it_declared() {
    for case in cases() {
        let router = router();
        let described = deploy(&router, case).await;

        for bucket in case.buckets {
            let name = output_value(&described, bucket.output_key);
            assert!(
                !name.is_empty(),
                "case {:?}: the stack must publish {:?}, got {described}",
                case.case,
                bucket.output_key
            );

            let (http_status, listing) = list_inventory(&router, &name).await;
            assert_eq!(
                http_status, 200,
                "case {:?}: s3:ListBucketInventoryConfigurations({name:?}) must answer 200, got \
                 {http_status} {listing}",
                case.case
            );
            // The listing reports the configurations in ascending id order, which is the order
            // the rows list what they expect.
            let listed = reported(&listing);
            let expected: Vec<Reported> = bucket.configurations.iter().map(wanted).collect();
            assert_eq!(
                listed, expected,
                "case {:?}: the bucket {name:?} must list {expected:?}, got {listed:?} \
                 ({listing})",
                case.case
            );
            assert_eq!(
                xml_text(&listing, "IsTruncated").as_deref(),
                Some("false"),
                "case {:?}: the bucket {name:?} carries one page, so the listing must report it \
                 complete, got {listing}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn every_declared_configuration_reads_back_under_its_own_id() {
    for case in cases() {
        let router = router();
        let described = deploy(&router, case).await;

        for bucket in case.buckets {
            let name = output_value(&described, bucket.output_key);
            for configuration in bucket.configurations {
                let (http_status, body) = get_inventory(&router, &name, configuration.id).await;
                assert_eq!(
                    http_status, 200,
                    "case {:?}: s3:GetBucketInventoryConfiguration({name:?}, {:?}) must answer \
                     200, got {http_status} {body}",
                    case.case, configuration.id
                );
                let read = reported(&body);
                let expected = vec![wanted(configuration)];
                assert_eq!(
                    read, expected,
                    "case {:?}: the configuration {:?} of {name:?} must read back as \
                     {expected:?}, got {read:?} ({body})",
                    case.case, configuration.id
                );
            }
        }
    }
}

/// A declaration that sets no destination prefix is told from one that sets it by the ABSENCE
/// of the element, not by an empty one — which is what a client filtering the listing on it
/// selects by.
#[tokio::test]
async fn a_report_declaring_no_destination_prefix_reports_no_prefix_element() {
    let rows: &[(&str, usize, usize, bool)] = &[
        ("the parameterised report, declaring no destination prefix", 0, 0, false),
        ("the weekly cross-account report, declaring one", 1, 1, true),
    ];

    for (case, case_index, configuration_index, prefixed) in rows {
        let row = &cases()[*case_index];
        let router = router();
        let described = deploy(&router, row).await;
        let bucket = &row.buckets[0];
        let name = output_value(&described, bucket.output_key);
        let configuration = &bucket.configurations[*configuration_index];

        let (http_status, body) = get_inventory(&router, &name, configuration.id).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: the read must answer 200, got {http_status} {body}"
        );
        let destination = xml_blocks(&body, "S3BucketDestination")
            .into_iter()
            .next()
            .unwrap_or_default();
        let reported_prefix = xml_text(&destination, "Prefix");
        assert_eq!(
            reported_prefix.is_some(),
            *prefixed,
            "case {case:?}: a destination Prefix element must be reported only where one was \
             declared, got {reported_prefix:?} ({body})"
        );
    }
}

/// Deleting every configuration a stack installed returns the bucket to the state a bucket is
/// created in: the listing reports completeness and carries no configuration at all.
#[tokio::test]
async fn deleting_the_installed_configurations_empties_the_listing() {
    let case = &cases()[1];
    let router = router();
    let described = deploy(&router, case).await;
    let bucket = &case.buckets[0];
    let name = output_value(&described, bucket.output_key);

    for configuration in bucket.configurations {
        let http_status = delete_inventory(&router, &name, configuration.id).await;
        assert_eq!(
            http_status, 204,
            "s3:DeleteBucketInventoryConfiguration({name:?}, {:?}) must answer 204, got \
             {http_status}",
            configuration.id
        );
    }

    let (http_status, listing) = list_inventory(&router, &name).await;
    assert_eq!(
        http_status, 200,
        "the listing must answer 200, got {http_status} {listing}"
    );
    let listed = reported(&listing);
    assert!(
        listed.is_empty(),
        "every configuration was deleted, so the listing must carry none: got {listed:?} \
         ({listing})"
    );
    assert!(
        !listing.contains("<InventoryConfiguration"),
        "the listing must omit InventoryConfigurationList entirely, got {listing}"
    );
    assert_eq!(
        xml_text(&listing, "IsTruncated").as_deref(),
        Some("false"),
        "the listing must report the list as complete, got {listing}"
    );
}

/// A declared configuration the provisioner cannot install is not dropped quietly: the resource
/// fails, so the stack does not report CREATE_COMPLETE over a report the bucket never carried.
#[tokio::test]
async fn a_configuration_missing_a_required_member_fails_the_bucket() {
    let rows: &[(&str, &str, &str)] = &[
        (
            "an entry declaring no Id",
            "idless-inventory",
            concat!(
                "Resources:\n",
                "  Source:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: idless-inventory-source\n",
                "      InventoryConfigurations:\n",
                "        - Enabled: true\n",
                "          ScheduleFrequency: Daily\n",
                "          IncludedObjectVersions: Current\n",
                "          Destination:\n",
                "            BucketArn: arn:aws:s3:::idless-inventory-reports\n",
                "            Format: CSV\n",
            ),
        ),
        (
            "an entry whose destination declares no BucketArn",
            "arnless-inventory",
            concat!(
                "Resources:\n",
                "  Source:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: arnless-inventory-source\n",
                "      InventoryConfigurations:\n",
                "        - Id: Incomplete\n",
                "          Enabled: true\n",
                "          ScheduleFrequency: Daily\n",
                "          IncludedObjectVersions: Current\n",
                "          Destination:\n",
                "            Format: CSV\n",
            ),
        ),
        (
            "an entry declaring no ScheduleFrequency",
            "scheduleless-inventory",
            concat!(
                "Resources:\n",
                "  Source:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: scheduleless-inventory-source\n",
                "      InventoryConfigurations:\n",
                "        - Id: Incomplete\n",
                "          Enabled: true\n",
                "          IncludedObjectVersions: Current\n",
                "          Destination:\n",
                "            BucketArn: arn:aws:s3:::scheduleless-inventory-reports\n",
                "            Format: CSV\n",
            ),
        ),
        (
            "an entry declaring no Destination at all",
            "destinationless-inventory",
            concat!(
                "Resources:\n",
                "  Source:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: destinationless-inventory-source\n",
                "      InventoryConfigurations:\n",
                "        - Id: Incomplete\n",
                "          Enabled: true\n",
                "          ScheduleFrequency: Daily\n",
                "          IncludedObjectVersions: Current\n",
            ),
        ),
        (
            "an entry declaring no Enabled flag",
            "flagless-inventory",
            concat!(
                "Resources:\n",
                "  Source:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: flagless-inventory-source\n",
                "      InventoryConfigurations:\n",
                "        - Id: Incomplete\n",
                "          ScheduleFrequency: Daily\n",
                "          IncludedObjectVersions: Current\n",
                "          Destination:\n",
                "            BucketArn: arn:aws:s3:::flagless-inventory-reports\n",
                "            Format: CSV\n",
            ),
        ),
    ];

    for (case, stack_name, template) in rows {
        let router = router();
        let (http_status, created) = create_stack(&router, stack_name, template, &[]).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: CreateStack must answer 200, got {http_status} {created}"
        );
        wait_for_the_transition().await;

        let (http_status, described) = describe_stack(&router, stack_name).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: DescribeStacks must answer 200, got {http_status} {described}"
        );
        let reported_status = xml_text(&described, "StackStatus").unwrap_or_default();
        assert_ne!(
            reported_status,
            status::CREATE_COMPLETE,
            "case {case:?}: a bucket whose declared inventory configuration could not be \
             installed must not complete, got {described}"
        );
    }
}
