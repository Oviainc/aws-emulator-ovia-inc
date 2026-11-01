// SPDX-License-Identifier: CC0-1.0

//! A bucket's S3 Intelligent-Tiering configurations as a stack declares them, read back
//! through Amazon S3.
//!
//! RULE: a resource CloudFormation provisions carries, in the service that OWNS it, every
//! sub-resource configuration its declaration states — each applied by the one API call that
//! service publishes for it, since none of them is a member of the create — so a read of that
//! sub-resource straight after the stack reports the resource created answers with what the
//! template declared. For an `AWS::S3::Bucket`'s `IntelligentTieringConfigurations` that is
//! one `PutBucketIntelligentTieringConfiguration` per declared entry, under the `Id` the entry
//! names, so `s3:ListBucketIntelligentTieringConfigurations` on the bucket the stack published
//! reports exactly the declared ids and `s3:GetBucketIntelligentTieringConfiguration` reports
//! each one's members.
//!
//! The object scope is the part of the template's spelling that is FLAT where the API nests
//! it: it travels as `Prefix` and `TagFilters` where the API takes a single `Filter` carrying
//! a prefix, a single `Tag` or an `And` conjunction whose "operator must have at least two
//! predicates" (`smithy_query(query="shape",shape="IntelligentTieringFilter")`). So a scope
//! naming ONE predicate is reported as that predicate's own member, and a scope naming
//! several as the conjunction of them, the prefix and each tag counting as one predicate
//! each. `Status` and `Tierings` are spelled the same on both sides and are reported as
//! declared.
//!
//! The rows differ along what the rule generalises over: HOW MANY configurations one bucket
//! declares (none, one, three), which FILTER form the declared scope amounts to (none, a
//! prefix, one tag, a prefix with a tag, two tags, a prefix with two tags), how many TIERINGS
//! an entry declares and which access tier each names, the `Days` each names (each published
//! floor, the published maximum and a value inside the range), the `Status` (both values the
//! enum takes), the template SPELLING (JSON and YAML) and how many buckets one stack
//! declares. A second table drives what a declaration the service will NOT take does to the
//! stack, including a day count one past each published bound.
//!
//! Every expected value is computed from what the row declares, never read back out of the
//! answer under test.

use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    create_stack, describe_stack, router, wait_for_the_transition, xml_blocks, xml_text,
};
use crate::stack_resource_provisioning::{output_value, s3};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// The two values `IntelligentTieringAccessTier` takes
/// (`smithy_query(query="shape",shape="IntelligentTieringAccessTier")`).
const ARCHIVE_ACCESS: &str = "ARCHIVE_ACCESS";
const DEEP_ARCHIVE_ACCESS: &str = "DEEP_ARCHIVE_ACCESS";

/// The two values `IntelligentTieringStatus` takes
/// (`smithy_query(query="shape",shape="IntelligentTieringStatus")`).
const ENABLED: &str = "Enabled";
const DISABLED: &str = "Disabled";

/// The day range `Tiering$Days` publishes: "the minimum number of days specified for Archive
/// Access tier must be at least 90 days and Deep Archive Access tier must be at least 180
/// days … the maximum can be up to 2 years (730 days)"
/// (`smithy_query(query="doc",shape="Tiering",member="Days")`).
const ARCHIVE_ACCESS_FLOOR: i32 = 90;
const DEEP_ARCHIVE_ACCESS_FLOOR: i32 = 180;
const MAX_DAYS: i32 = 730;

/// The `IntelligentTieringFilter` the API reports for the scope a declaration states flat.
#[derive(Debug, PartialEq, Eq)]
enum Filter {
    /// No `Filter` element at all — the configuration applies to every object in the bucket.
    Unfiltered,
    /// The one-prefix member.
    Prefix(&'static str),
    /// The one-tag member, as its `(Key, Value)`.
    Tag(&'static str, &'static str),
    /// The conjunction: the prefix it carries, if any, and its tags in declared order.
    And(Option<&'static str>, &'static [(&'static str, &'static str)]),
}

/// One configuration a row expects the bucket to carry.
struct Expected {
    id: &'static str,
    filter: Filter,
    status: &'static str,
    /// The tierings, as the `(Days, AccessTier)` pairs each declares, in declared order.
    tierings: &'static [(i32, &'static str)],
}

/// One bucket of a row: the stack output that publishes its name, and the configurations
/// Amazon S3 must report for it.
struct Bucket {
    output_key: &'static str,
    configurations: &'static [Expected],
}

/// One row of the table.
struct TieringCase {
    case: &'static str,
    stack_name: &'static str,
    template: &'static str,
    buckets: &'static [Bucket],
}

/// One `IntelligentTieringConfiguration` as Amazon S3 reported it, read as the values a
/// declaration fixes so the two sides are compared as one value.
#[derive(Debug, PartialEq, Eq)]
struct Reported {
    id: String,
    filter: ReportedFilter,
    status: String,
    tierings: Vec<(String, String)>,
}

/// [`Filter`] as the answer spells it.
#[derive(Debug, PartialEq, Eq)]
enum ReportedFilter {
    Unfiltered,
    Prefix(String),
    Tag(String, String),
    And(Option<String>, Vec<(String, String)>),
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
        status: configuration.status.to_string(),
        tierings: configuration
            .tierings
            .iter()
            .map(|(days, access_tier)| (days.to_string(), (*access_tier).to_string()))
            .collect(),
    }
}

/// Every `IntelligentTieringConfiguration` a body reports, in document order.
///
/// One parser for BOTH reads: a `GetBucketIntelligentTieringConfiguration` body carries one
/// container and a `ListBucketIntelligentTieringConfigurations` page as many as it holds, so
/// the two cannot drift on how a container is read. Each nested list is taken as its own
/// BLOCK first and scanned inside it, because `Prefix` is a member of the filter and of the
/// conjunction alike.
fn reported(xml: &str) -> Vec<Reported> {
    xml_blocks(xml, "IntelligentTieringConfiguration")
        .iter()
        .map(|entry| Reported {
            id: xml_text(entry, "Id").unwrap_or_default(),
            filter: reported_filter(entry),
            status: xml_text(entry, "Status").unwrap_or_default(),
            tierings: reported_tierings(entry),
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

/// The `(Days, AccessTier)` of every `Tiering` one reported container carries, in document
/// order.
fn reported_tierings(entry: &str) -> Vec<(String, String)> {
    xml_blocks(entry, "Tiering")
        .iter()
        .map(|tiering| {
            (
                xml_text(tiering, "Days").unwrap_or_default(),
                xml_text(tiering, "AccessTier").unwrap_or_default(),
            )
        })
        .collect()
}

/// `GET /<bucket>?intelligent-tiering` — `ListBucketIntelligentTieringConfigurations`.
async fn list_tiering(router: &MultiServiceRouter, bucket: &str) -> (u16, String) {
    let answer = s3(router, "GET", &format!("/{bucket}?intelligent-tiering"), b"").await;
    (answer.status, answer.body)
}

/// `GET /<bucket>?intelligent-tiering&id=<id>` —
/// `GetBucketIntelligentTieringConfiguration`.
async fn get_tiering(router: &MultiServiceRouter, bucket: &str, id: &str) -> (u16, String) {
    let answer = s3(
        router,
        "GET",
        &format!("/{bucket}?intelligent-tiering&id={id}"),
        b"",
    )
    .await;
    (answer.status, answer.body)
}

/// Deploy a row's stack and answer with the `DescribeStacks` body it completed with.
async fn deploy(router: &MultiServiceRouter, case: &TieringCase) -> String {
    let (http_status, created) = create_stack(router, case.stack_name, case.template, &[]).await;
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
fn cases() -> &'static [TieringCase] {
    &[
        TieringCase {
            // Three arms in JSON, declared in an order that is NOT the ascending id order the
            // listing reports: a scope naming a prefix AND a tag (so the conjunction), a
            // scope naming a prefix alone, and one narrowing nothing. The day counts are the
            // two published floors, the published maximum and a value well inside the range,
            // and both `Status` values appear.
            case: "three arms, in JSON: a prefix with a tag, a prefix alone and no scope",
            stack_name: "json-tiering",
            template: r#"{"Resources":{
                 "TieredBucket":{"Type":"AWS::S3::Bucket","Properties":{
                   "BucketName":"json-tiering-source",
                   "IntelligentTieringConfigurations":[
                     {"Id":"ScopedToDocuments",
                      "Prefix":"documents/",
                      "TagFilters":[{"Key":"department","Value":"finance"}],
                      "Status":"Enabled",
                      "Tierings":[{"Days":90,"AccessTier":"ARCHIVE_ACCESS"},
                                  {"Days":180,"AccessTier":"DEEP_ARCHIVE_ACCESS"}]},
                     {"Id":"ScopedToLogs",
                      "Prefix":"logs/",
                      "Status":"Disabled",
                      "Tierings":[{"Days":730,"AccessTier":"ARCHIVE_ACCESS"}]},
                     {"Id":"WholeBucket",
                      "Status":"Enabled",
                      "Tierings":[{"Days":200,"AccessTier":"DEEP_ARCHIVE_ACCESS"}]}]}}},
               "Outputs":{"TieredBucketName":{"Value":{"Ref":"TieredBucket"}}}}"#,
            buckets: &[Bucket {
                output_key: "TieredBucketName",
                configurations: &[
                    Expected {
                        id: "ScopedToDocuments",
                        filter: Filter::And(Some("documents/"), &[("department", "finance")]),
                        status: ENABLED,
                        tierings: &[
                            (ARCHIVE_ACCESS_FLOOR, ARCHIVE_ACCESS),
                            (DEEP_ARCHIVE_ACCESS_FLOOR, DEEP_ARCHIVE_ACCESS),
                        ],
                    },
                    Expected {
                        id: "ScopedToLogs",
                        filter: Filter::Prefix("logs/"),
                        status: DISABLED,
                        tierings: &[(MAX_DAYS, ARCHIVE_ACCESS)],
                    },
                    Expected {
                        id: "WholeBucket",
                        filter: Filter::Unfiltered,
                        status: ENABLED,
                        tierings: &[(200, DEEP_ARCHIVE_ACCESS)],
                    },
                ],
            }],
        },
        TieringCase {
            // The remaining filter forms, in YAML: one tag alone, two tags, and a prefix with
            // two tags — beside a second bucket of the same stack that declares no
            // configuration at all, so each bucket carries its own.
            case: "three scopes, in YAML: one tag, two tags and a prefix with two tags, \
                   beside a bucket declaring none",
            stack_name: "yaml-tiering",
            template: concat!(
                "Resources:\n",
                "  Plain:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: yaml-tiering-plain\n",
                "  Tiered:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: yaml-tiering-source\n",
                "      IntelligentTieringConfigurations:\n",
                "        - Id: ByTag\n",
                "          TagFilters:\n",
                "            - Key: tier\n",
                "              Value: cold\n",
                "          Status: Enabled\n",
                "          Tierings:\n",
                "            - Days: 95\n",
                "              AccessTier: ARCHIVE_ACCESS\n",
                "        - Id: ByTwoTags\n",
                "          TagFilters:\n",
                "            - Key: tier\n",
                "              Value: warm\n",
                "            - Key: owner\n",
                "              Value: data\n",
                "          Status: Disabled\n",
                "          Tierings:\n",
                "            - Days: 180\n",
                "              AccessTier: DEEP_ARCHIVE_ACCESS\n",
                "        - Id: ByPrefixAndTwoTags\n",
                "          Prefix: archive/\n",
                "          TagFilters:\n",
                "            - Key: tier\n",
                "              Value: frozen\n",
                "            - Key: owner\n",
                "              Value: platform\n",
                "          Status: Enabled\n",
                "          Tierings:\n",
                "            - Days: 90\n",
                "              AccessTier: ARCHIVE_ACCESS\n",
                "            - Days: 730\n",
                "              AccessTier: DEEP_ARCHIVE_ACCESS\n",
                "Outputs:\n",
                "  TieredBucketName:\n",
                "    Value: !Ref Tiered\n",
                "  PlainBucketName:\n",
                "    Value: !Ref Plain\n",
            ),
            buckets: &[
                Bucket {
                    output_key: "TieredBucketName",
                    configurations: &[
                        Expected {
                            id: "ByPrefixAndTwoTags",
                            filter: Filter::And(
                                Some("archive/"),
                                &[("tier", "frozen"), ("owner", "platform")],
                            ),
                            status: ENABLED,
                            tierings: &[
                                (ARCHIVE_ACCESS_FLOOR, ARCHIVE_ACCESS),
                                (MAX_DAYS, DEEP_ARCHIVE_ACCESS),
                            ],
                        },
                        Expected {
                            id: "ByTag",
                            filter: Filter::Tag("tier", "cold"),
                            status: ENABLED,
                            tierings: &[(95, ARCHIVE_ACCESS)],
                        },
                        Expected {
                            id: "ByTwoTags",
                            filter: Filter::And(None, &[("tier", "warm"), ("owner", "data")]),
                            status: DISABLED,
                            tierings: &[(DEEP_ARCHIVE_ACCESS_FLOOR, DEEP_ARCHIVE_ACCESS)],
                        },
                    ],
                },
                Bucket {
                    output_key: "PlainBucketName",
                    configurations: &[],
                },
            ],
        },
    ]
}

#[tokio::test]
async fn a_bucket_a_stack_declares_carries_the_intelligent_tiering_configurations_it_declared() {
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

            let (http_status, listing) = list_tiering(&router, &name).await;
            assert_eq!(
                http_status, 200,
                "case {:?}: s3:ListBucketIntelligentTieringConfigurations({name:?}) must answer \
                 200, got {http_status} {listing}",
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
                let (http_status, body) = get_tiering(&router, &name, configuration.id).await;
                assert_eq!(
                    http_status, 200,
                    "case {:?}: s3:GetBucketIntelligentTieringConfiguration({name:?}, {:?}) must \
                     answer 200, got {http_status} {body}",
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

/// A declared configuration the provisioner cannot install — because it states no member the
/// API requires, or names a day count outside the range the access tier publishes — is not
/// dropped quietly: the resource fails, so the stack does not report CREATE_COMPLETE over a
/// tiering policy the bucket never carried.
///
/// The day rows are each ONE past a published bound: one below the `ARCHIVE_ACCESS` floor,
/// one below the `DEEP_ARCHIVE_ACCESS` floor and one past the maximum either tier allows.
#[tokio::test]
async fn a_configuration_the_service_will_not_take_fails_the_bucket() {
    let rows: &[(&str, &str, String)] = &[
        (
            "an entry declaring no Id",
            "idless-tiering",
            tiering_template(
                "idless-tiering-source",
                concat!(
                    "        - Status: Enabled\n",
                    "          Tierings:\n",
                    "            - Days: 90\n",
                    "              AccessTier: ARCHIVE_ACCESS\n",
                ),
            ),
        ),
        (
            "an entry declaring no Status",
            "statusless-tiering",
            tiering_template(
                "statusless-tiering-source",
                concat!(
                    "        - Id: Incomplete\n",
                    "          Tierings:\n",
                    "            - Days: 90\n",
                    "              AccessTier: ARCHIVE_ACCESS\n",
                ),
            ),
        ),
        (
            "an entry declaring no Tierings",
            "tieringless-tiering",
            tiering_template(
                "tieringless-tiering-source",
                concat!("        - Id: Incomplete\n", "          Status: Enabled\n"),
            ),
        ),
        (
            "a tiering declaring no AccessTier",
            "tierless-tiering",
            tiering_template(
                "tierless-tiering-source",
                concat!(
                    "        - Id: Incomplete\n",
                    "          Status: Enabled\n",
                    "          Tierings:\n",
                    "            - Days: 90\n",
                ),
            ),
        ),
        (
            "one day below the ARCHIVE_ACCESS floor",
            "short-archive-tiering",
            tiering_template(
                "short-archive-tiering-source",
                &one_tiering_entry("TooSoon", ARCHIVE_ACCESS_FLOOR - 1, ARCHIVE_ACCESS),
            ),
        ),
        (
            "one day below the DEEP_ARCHIVE_ACCESS floor",
            "short-deep-tiering",
            tiering_template(
                "short-deep-tiering-source",
                &one_tiering_entry(
                    "TooSoon",
                    DEEP_ARCHIVE_ACCESS_FLOOR - 1,
                    DEEP_ARCHIVE_ACCESS,
                ),
            ),
        ),
        (
            "one day past the published maximum",
            "long-tiering",
            tiering_template(
                "long-tiering-source",
                &one_tiering_entry("TooLate", MAX_DAYS + 1, ARCHIVE_ACCESS),
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
            "case {case:?}: a bucket whose declared Intelligent-Tiering configuration could not \
             be installed must not complete, got {described}"
        );
    }
}

/// One `IntelligentTieringConfigurations` entry under `id`, enabled, transitioning into
/// `access_tier` after `days` consecutive days of no access.
fn one_tiering_entry(id: &str, days: i32, access_tier: &str) -> String {
    format!(
        "        - Id: {id}\n          \
                   Status: Enabled\n          \
                   Tierings:\n            \
                     - Days: {days}\n              \
                       AccessTier: {access_tier}\n"
    )
}

/// One bucket declaring the `IntelligentTieringConfigurations` entries `entries` spells, as a
/// YAML template body.
fn tiering_template(bucket_name: &str, entries: &str) -> String {
    format!(
        "Resources:\n  \
           Source:\n    \
             Type: AWS::S3::Bucket\n    \
             Properties:\n      \
               BucketName: {bucket_name}\n      \
               IntelligentTieringConfigurations:\n{entries}"
    )
}
