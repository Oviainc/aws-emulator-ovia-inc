// SPDX-License-Identifier: CC0-1.0

//! A bucket's storage class analyses as a stack declares them, read back through Amazon S3.
//!
//! RULE: a resource CloudFormation provisions carries, in the service that OWNS it, every
//! sub-resource configuration its declaration states — each applied by the one API call that
//! service publishes for it, since none of them is a member of the create — so a read of that
//! sub-resource straight after the stack reports the resource created answers with what the
//! template declared. For an `AWS::S3::Bucket`'s `AnalyticsConfigurations` that is one
//! `PutBucketAnalyticsConfiguration` per declared entry, under the `Id` the entry names, so
//! `s3:ListBucketAnalyticsConfigurations` on the bucket the stack published reports exactly the
//! declared ids and `s3:GetBucketAnalyticsConfiguration` reports each one's members.
//!
//! Two parts of the template's spelling are FLAT where the API nests them, and the rule covers
//! both. The object scope travels as `Prefix` and `TagFilters` where the API takes the single
//! `Filter` union — "a filter must have exactly one prefix, one tag, or one conjunction
//! (AnalyticsAndOperator)", whose operator "must have at least two predicates"
//! (`smithy_query(query="shape",shape="AnalyticsFilter")`) — and the export destination travels
//! as `{ BucketArn, BucketAccountId, Format, Prefix }` where the API reports it one level deeper
//! as `Destination.S3BucketDestination` with `BucketArn` spelled `Bucket`, "the Amazon Resource
//! Name (ARN) of the bucket to which data is exported"
//! (`smithy_query(query="shape",shape="AnalyticsS3BucketDestination")`). `StorageClassAnalysis`
//! is required and its one `DataExport` member is not, so a declaration stating an EMPTY
//! analysis is reported with no `DataExport` member at all.
//!
//! The rows differ along what the rule generalises over: HOW MANY configurations one bucket
//! declares (none, one, three, four), whether an entry declares a data EXPORT or an empty
//! analysis, which destination MEMBERS it states (an account id and a prefix, each present or
//! absent), where the destination values COME FROM (a literal, a stack parameter, `Ref
//! AWS::AccountId`, an `Fn::GetAtt` on a bucket the same template declares), which FILTER form
//! the declared scope amounts to (none, a prefix, one tag, a prefix and a tag, two tags), the
//! template SPELLING (JSON and YAML, long- and short-form intrinsics), and how many buckets one
//! stack declares.
//!
//! Every expected value is computed from what the row declares — the account id the emulator
//! mints and the `arn:aws:s3:::<name>` form an `AWS::S3::Bucket`'s `Arn` attribute publishes —
//! never read back out of the analytics answer under test.

use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    create_stack, describe_stack, member, router, wait_for_the_transition, xml_blocks, xml_text,
    ACCOUNT_ID,
};
use crate::stack_resource_provisioning::{output_value, s3};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// The destination bucket ARN the parameterised row passes in, and the account that is NOT the
/// deploying one — so a row's cross-account arm cannot pass by accident.
const DESTINATION_ARN: &str = "arn:aws:s3:::xacct-analytics-exports";
const FOREIGN_ACCOUNT_ID: &str = "210987654321";

/// The one value `AnalyticsS3ExportFileFormat` takes and the one
/// `StorageClassAnalysisSchemaVersion` takes
/// (`smithy_query(query="shape",shape="AnalyticsS3ExportFileFormat,StorageClassAnalysisSchemaVersion")`).
const CSV: &str = "CSV";
const V_1: &str = "V_1";

/// The `AnalyticsFilter` the API reports for the scope a declaration states flat.
#[derive(Debug, PartialEq, Eq)]
enum Filter {
    /// No `Filter` element at all — "if no filter is provided, all objects will be considered
    /// in any analysis".
    Unfiltered,
    /// The one-prefix member.
    Prefix(&'static str),
    /// The one-tag member, as its `(Key, Value)`.
    Tag(&'static str, &'static str),
    /// The conjunction: the prefix it carries, if any, and its tags in declared order.
    And(Option<&'static str>, &'static [(&'static str, &'static str)]),
}

/// The export one configuration must report, flattened to the members a declaration states.
#[derive(Debug, PartialEq, Eq)]
struct Export {
    schema_version: &'static str,
    format: &'static str,
    /// Where the destination bucket ARN comes from, and so what `Bucket` must read back as.
    bucket: Arn,
    /// Where the destination's owning account comes from, or `None` for a declaration that
    /// states none ("if no account ID is provided, the owner is not validated").
    account_id: Option<Account>,
    prefix: Option<&'static str>,
}

/// Where a declared destination bucket ARN comes from.
#[derive(Debug, PartialEq, Eq)]
enum Arn {
    /// An ARN the row hands the stack as a parameter, or spells as a literal.
    Given(&'static str),
    /// An `AWS::S3::Bucket` the same template declares under this `BucketName`, reached through
    /// `Fn::GetAtt … Arn`: "an Amazon S3 bucket ARN carries neither a region nor an account id".
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

/// Which account a declared `BucketAccountId` names.
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
    filter: Filter,
    /// `None` for an entry declaring an empty `StorageClassAnalysis`.
    export: Option<Export>,
}

/// One bucket of a row: the stack output that publishes its name, and the configurations
/// Amazon S3 must report for it.
struct Bucket {
    output_key: &'static str,
    configurations: &'static [Expected],
}

/// One row of the table.
struct AnalyticsCase {
    case: &'static str,
    stack_name: &'static str,
    template: &'static str,
    /// The stack parameters the row supplies, as `(ParameterKey, ParameterValue)`.
    parameters: &'static [(&'static str, &'static str)],
    buckets: &'static [Bucket],
}

/// One `AnalyticsConfiguration` as Amazon S3 reported it, read as the values a declaration
/// fixes so the two sides are compared as one value.
#[derive(Debug, PartialEq, Eq)]
struct Reported {
    id: String,
    filter: ReportedFilter,
    export: Option<ReportedExport>,
}

/// [`Filter`] as the answer spells it.
#[derive(Debug, PartialEq, Eq)]
enum ReportedFilter {
    Unfiltered,
    Prefix(String),
    Tag(String, String),
    And(Option<String>, Vec<(String, String)>),
}

/// [`Export`] as the answer spells it.
#[derive(Debug, PartialEq, Eq)]
struct ReportedExport {
    schema_version: String,
    format: String,
    bucket: String,
    account_id: Option<String>,
    prefix: Option<String>,
}

/// What a row EXPECTS, in the same shape the answer is read into.
fn wanted(configuration: &Expected) -> Reported {
    Reported {
        id: configuration.id.to_string(),
        filter: match &configuration.filter {
            Filter::Unfiltered => ReportedFilter::Unfiltered,
            Filter::Prefix(prefix) => ReportedFilter::Prefix((*prefix).to_string()),
            Filter::Tag(key, value) => {
                ReportedFilter::Tag((*key).to_string(), (*value).to_string())
            }
            Filter::And(prefix, tags) => ReportedFilter::And(
                prefix.map(str::to_string),
                tags.iter()
                    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                    .collect(),
            ),
        },
        export: configuration.export.as_ref().map(|export| ReportedExport {
            schema_version: export.schema_version.to_string(),
            format: export.format.to_string(),
            bucket: export.bucket.text(),
            account_id: export.account_id.as_ref().map(Account::text),
            prefix: export.prefix.map(str::to_string),
        }),
    }
}

/// Every `AnalyticsConfiguration` a body reports, in document order.
///
/// One parser for BOTH reads: a `GetBucketAnalyticsConfiguration` body carries one container
/// and a `ListBucketAnalyticsConfigurations` page as many as it holds, so the two cannot drift
/// on how a container is read. Each nested list is taken as its own BLOCK first and scanned
/// inside it, because `Prefix` is a member of both the filter and the export destination.
fn reported(xml: &str) -> Vec<Reported> {
    xml_blocks(xml, "AnalyticsConfiguration")
        .iter()
        .map(|entry| Reported {
            id: xml_text(entry, "Id").unwrap_or_default(),
            filter: reported_filter(entry),
            export: reported_export(entry),
        })
        .collect()
}

/// The filter one reported container carries.
fn reported_filter(entry: &str) -> ReportedFilter {
    let Some(block) = xml_blocks(entry, "Filter").into_iter().next() else {
        return ReportedFilter::Unfiltered;
    };
    // `And` is the conjunction's own element, so its PRESENCE is what tells a conjunction from
    // the single-predicate members.
    if let Some(conjunction) = xml_blocks(&block, "And").into_iter().next() {
        return ReportedFilter::And(xml_text(&conjunction, "Prefix"), reported_tags(&conjunction));
    }
    match reported_tags(&block).into_iter().next() {
        Some((key, value)) => ReportedFilter::Tag(key, value),
        None => ReportedFilter::Prefix(xml_text(&block, "Prefix").unwrap_or_default()),
    }
}

/// The `(Key, Value)` of every `Tag` in one block, in document order.
fn reported_tags(block: &str) -> Vec<(String, String)> {
    xml_blocks(block, "Tag")
        .iter()
        .map(|tag| {
            (
                xml_text(tag, "Key").unwrap_or_default(),
                xml_text(tag, "Value").unwrap_or_default(),
            )
        })
        .collect()
}

/// The export one reported container carries, or `None` for an analysis reported with no
/// `DataExport` member at all.
fn reported_export(entry: &str) -> Option<ReportedExport> {
    let analysis = xml_blocks(entry, "StorageClassAnalysis").into_iter().next()?;
    let export = xml_blocks(&analysis, "DataExport").into_iter().next()?;
    let destination = xml_blocks(&export, "S3BucketDestination")
        .into_iter()
        .next()
        .unwrap_or_default();
    Some(ReportedExport {
        schema_version: xml_text(&export, "OutputSchemaVersion").unwrap_or_default(),
        format: xml_text(&destination, "Format").unwrap_or_default(),
        bucket: xml_text(&destination, "Bucket").unwrap_or_default(),
        account_id: xml_text(&destination, "BucketAccountId"),
        prefix: xml_text(&destination, "Prefix"),
    })
}

/// `GET /<bucket>?analytics` — `ListBucketAnalyticsConfigurations`.
async fn list_analytics(router: &MultiServiceRouter, bucket: &str) -> (u16, String) {
    let answer = s3(router, "GET", &format!("/{bucket}?analytics"), b"").await;
    (answer.status, answer.body)
}

/// `GET /<bucket>?analytics&id=<id>` — `GetBucketAnalyticsConfiguration`.
async fn get_analytics(router: &MultiServiceRouter, bucket: &str, id: &str) -> (u16, String) {
    let answer = s3(router, "GET", &format!("/{bucket}?analytics&id={id}"), b"").await;
    (answer.status, answer.body)
}

/// `DELETE /<bucket>?analytics&id=<id>` — `DeleteBucketAnalyticsConfiguration`.
async fn delete_analytics(router: &MultiServiceRouter, bucket: &str, id: &str) -> u16 {
    s3(
        router,
        "DELETE",
        &format!("/{bucket}?analytics&id={id}"),
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
                member(
                    &format!("Parameters.member.{position}.ParameterKey"),
                    key,
                ),
                member(
                    &format!("Parameters.member.{position}.ParameterValue"),
                    value,
                ),
            ]
        })
        .collect()
}

/// Deploy a row's stack and answer with the `DescribeStacks` body it completed with.
async fn deploy(router: &MultiServiceRouter, case: &AnalyticsCase) -> String {
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
fn cases() -> &'static [AnalyticsCase] {
    &[
        AnalyticsCase {
            // The cross-account shape: a destination ARN and a foreign account handed in as
            // stack parameters, an owner-account arm resolving `Ref AWS::AccountId`, and an
            // arm whose analysis declares no export at all. Declared in an order that is NOT
            // the ascending id order the listing reports.
            case: "three arms, in JSON: a parameterised cross-account export, an owner-account \
                   export and an empty analysis",
            stack_name: "xacct-analytics",
            template: r#"{"Parameters":{
                 "DestinationArn":{"Type":"String"},
                 "ForeignAccount":{"Type":"String"}},
               "Resources":{
                 "AnalyticsBucket":{"Type":"AWS::S3::Bucket","Properties":{
                   "BucketName":"xacct-analytics-source",
                   "AnalyticsConfigurations":[
                     {"Id":"CrossAccountExport","StorageClassAnalysis":{"DataExport":{
                        "OutputSchemaVersion":"V_1",
                        "Destination":{"BucketArn":{"Ref":"DestinationArn"},
                                       "BucketAccountId":{"Ref":"ForeignAccount"},
                                       "Format":"CSV"}}}},
                     {"Id":"OwnerAccountExport","StorageClassAnalysis":{"DataExport":{
                        "OutputSchemaVersion":"V_1",
                        "Destination":{"BucketArn":{"Ref":"DestinationArn"},
                                       "BucketAccountId":{"Ref":"AWS::AccountId"},
                                       "Format":"CSV"}}}},
                     {"Id":"NoExport","StorageClassAnalysis":{}}]}}},
               "Outputs":{"AnalyticsBucketName":{"Value":{"Ref":"AnalyticsBucket"}}}}"#,
            parameters: &[
                ("DestinationArn", DESTINATION_ARN),
                ("ForeignAccount", FOREIGN_ACCOUNT_ID),
            ],
            buckets: &[Bucket {
                output_key: "AnalyticsBucketName",
                configurations: &[
                    Expected {
                        id: "CrossAccountExport",
                        filter: Filter::Unfiltered,
                        export: Some(Export {
                            schema_version: V_1,
                            format: CSV,
                            bucket: Arn::Given(DESTINATION_ARN),
                            account_id: Some(Account::Foreign),
                            prefix: None,
                        }),
                    },
                    Expected {
                        id: "NoExport",
                        filter: Filter::Unfiltered,
                        export: None,
                    },
                    Expected {
                        id: "OwnerAccountExport",
                        filter: Filter::Unfiltered,
                        export: Some(Export {
                            schema_version: V_1,
                            format: CSV,
                            bucket: Arn::Given(DESTINATION_ARN),
                            account_id: Some(Account::Deploying),
                            prefix: None,
                        }),
                    },
                ],
            }],
        },
        AnalyticsCase {
            // Every form the flat scope folds into: a prefix alone, one tag alone, a prefix
            // beside a tag and two tags — the last two being the conjunction, since "the
            // operator must have at least two predicates" and prefix and tags count one each.
            // The destination here states a `Prefix` and no `BucketAccountId`.
            case: "four scopes, in YAML: a prefix, one tag, a prefix with a tag, and two tags",
            stack_name: "scoped-analytics",
            template: concat!(
                "Resources:\n",
                "  Scoped:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: scoped-analytics-source\n",
                "      AnalyticsConfigurations:\n",
                "        - Id: ByPrefix\n",
                "          Prefix: logs/\n",
                "          StorageClassAnalysis:\n",
                "            DataExport:\n",
                "              OutputSchemaVersion: V_1\n",
                "              Destination:\n",
                "                BucketArn: arn:aws:s3:::scoped-analytics-exports\n",
                "                Format: CSV\n",
                "                Prefix: reports/\n",
                "        - Id: ByTag\n",
                "          TagFilters:\n",
                "            - Key: tier\n",
                "              Value: cold\n",
                "          StorageClassAnalysis:\n",
                "            DataExport:\n",
                "              OutputSchemaVersion: V_1\n",
                "              Destination:\n",
                "                BucketArn: arn:aws:s3:::scoped-analytics-exports\n",
                "                Format: CSV\n",
                "        - Id: ByPrefixAndTag\n",
                "          Prefix: archive/\n",
                "          TagFilters:\n",
                "            - Key: team\n",
                "              Value: platform\n",
                "          StorageClassAnalysis: {}\n",
                "        - Id: ByTwoTags\n",
                "          TagFilters:\n",
                "            - Key: tier\n",
                "              Value: warm\n",
                "            - Key: owner\n",
                "              Value: data\n",
                "          StorageClassAnalysis: {}\n",
                "Outputs:\n",
                "  ScopedBucketName:\n",
                "    Value: !Ref Scoped\n",
            ),
            parameters: &[],
            buckets: &[Bucket {
                output_key: "ScopedBucketName",
                configurations: &[
                    Expected {
                        id: "ByPrefix",
                        filter: Filter::Prefix("logs/"),
                        export: Some(Export {
                            schema_version: V_1,
                            format: CSV,
                            bucket: Arn::Given("arn:aws:s3:::scoped-analytics-exports"),
                            account_id: None,
                            prefix: Some("reports/"),
                        }),
                    },
                    Expected {
                        id: "ByPrefixAndTag",
                        filter: Filter::And(Some("archive/"), &[("team", "platform")]),
                        export: None,
                    },
                    Expected {
                        id: "ByTag",
                        filter: Filter::Tag("tier", "cold"),
                        export: Some(Export {
                            schema_version: V_1,
                            format: CSV,
                            bucket: Arn::Given("arn:aws:s3:::scoped-analytics-exports"),
                            account_id: None,
                            prefix: None,
                        }),
                    },
                    Expected {
                        id: "ByTwoTags",
                        filter: Filter::And(None, &[("tier", "warm"), ("owner", "data")]),
                        export: None,
                    },
                ],
            }],
        },
        AnalyticsCase {
            // The destination ARN read off a bucket the SAME template declares, beside a
            // second bucket that declares no analyses at all: each bucket carries its own.
            case: "one configuration exporting to a sibling bucket, beside a bucket declaring \
                   none",
            stack_name: "sibling-analytics",
            template: concat!(
                "Resources:\n",
                "  ExportsBucket:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: sibling-analytics-exports\n",
                "  SourceBucket:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: sibling-analytics-source\n",
                "      AnalyticsConfigurations:\n",
                "        - Id: WholeBucket\n",
                "          StorageClassAnalysis:\n",
                "            DataExport:\n",
                "              OutputSchemaVersion: V_1\n",
                "              Destination:\n",
                "                BucketArn: !GetAtt ExportsBucket.Arn\n",
                "                BucketAccountId: !Ref AWS::AccountId\n",
                "                Format: CSV\n",
                "Outputs:\n",
                "  SourceBucketName:\n",
                "    Value: !Ref SourceBucket\n",
                "  ExportsBucketName:\n",
                "    Value: !Ref ExportsBucket\n",
            ),
            parameters: &[],
            buckets: &[
                Bucket {
                    output_key: "SourceBucketName",
                    configurations: &[Expected {
                        id: "WholeBucket",
                        filter: Filter::Unfiltered,
                        export: Some(Export {
                            schema_version: V_1,
                            format: CSV,
                            bucket: Arn::Bucket("sibling-analytics-exports"),
                            account_id: Some(Account::Deploying),
                            prefix: None,
                        }),
                    }],
                },
                Bucket {
                    output_key: "ExportsBucketName",
                    configurations: &[],
                },
            ],
        },
    ]
}

#[tokio::test]
async fn a_bucket_a_stack_declares_carries_the_analytics_configurations_it_declared() {
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

            let (http_status, listing) = list_analytics(&router, &name).await;
            assert_eq!(
                http_status, 200,
                "case {:?}: s3:ListBucketAnalyticsConfigurations({name:?}) must answer 200, got \
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
                let (http_status, body) = get_analytics(&router, &name, configuration.id).await;
                assert_eq!(
                    http_status, 200,
                    "case {:?}: s3:GetBucketAnalyticsConfiguration({name:?}, {:?}) must answer \
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

/// Deleting every configuration a stack installed returns the bucket to the state a bucket is
/// created in: the listing reports completeness and carries no configuration at all.
#[tokio::test]
async fn deleting_the_installed_configurations_empties_the_listing() {
    let case = &cases()[0];
    let router = router();
    let described = deploy(&router, case).await;
    let bucket = &case.buckets[0];
    let name = output_value(&described, bucket.output_key);

    for configuration in bucket.configurations {
        let http_status = delete_analytics(&router, &name, configuration.id).await;
        assert_eq!(
            http_status, 204,
            "s3:DeleteBucketAnalyticsConfiguration({name:?}, {:?}) must answer 204, got \
             {http_status}",
            configuration.id
        );
    }

    let (http_status, listing) = list_analytics(&router, &name).await;
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
        !listing.contains("<AnalyticsConfiguration"),
        "the listing must omit AnalyticsConfigurationList entirely, got {listing}"
    );
    assert_eq!(
        xml_text(&listing, "IsTruncated").as_deref(),
        Some("false"),
        "the listing must report the list as complete, got {listing}"
    );
}

/// A declared configuration the provisioner cannot install is not dropped quietly: the
/// resource fails, so the stack does not report CREATE_COMPLETE over an analysis the bucket
/// never carried.
#[tokio::test]
async fn a_configuration_missing_a_required_member_fails_the_bucket() {
    let rows: &[(&str, &str, &str)] = &[
        (
            "an entry declaring no Id",
            "idless-analytics",
            concat!(
                "Resources:\n",
                "  Source:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: idless-analytics-source\n",
                "      AnalyticsConfigurations:\n",
                "        - StorageClassAnalysis: {}\n",
            ),
        ),
        (
            "an export whose destination declares no BucketArn",
            "arnless-analytics",
            concat!(
                "Resources:\n",
                "  Source:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: arnless-analytics-source\n",
                "      AnalyticsConfigurations:\n",
                "        - Id: Incomplete\n",
                "          StorageClassAnalysis:\n",
                "            DataExport:\n",
                "              OutputSchemaVersion: V_1\n",
                "              Destination:\n",
                "                Format: CSV\n",
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
            "case {case:?}: a bucket whose declared analytics configuration could not be \
             installed must not complete, got {described}"
        );
    }
}
