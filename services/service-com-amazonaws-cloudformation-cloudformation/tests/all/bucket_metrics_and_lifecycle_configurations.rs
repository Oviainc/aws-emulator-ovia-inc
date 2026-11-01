// SPDX-License-Identifier: CC0-1.0

//! A bucket's CloudWatch request metrics and its object lifecycle as a stack declares them,
//! read back through Amazon S3.
//!
//! RULE: a resource CloudFormation provisions carries, in the service that OWNS it, every
//! sub-resource configuration its declaration states — each applied by the one API call that
//! service publishes for it, since none of them is a member of the create — so a read of that
//! sub-resource straight after the stack reports the resource created answers with what the
//! template declared. For an `AWS::S3::Bucket` that is one `PutBucketMetricsConfiguration`
//! per `MetricsConfigurations` entry, under the `Id` the entry names, and ONE
//! `PutBucketLifecycleConfiguration` for the whole `LifecycleConfiguration`, whose API takes
//! the rule set as a single container.
//!
//! Both properties spell the object scope FLAT where the API nests it. A metrics entry states
//! an `AccessPointArn`, a `Prefix` and `TagFilters`, while "a filter must have exactly one
//! prefix, one tag, one access point ARN, or one conjunction (MetricsAndOperator)"
//! (`smithy_query(query="shape",shape="MetricsFilter")`); a lifecycle rule states a `Prefix`,
//! `TagFilters` and the two object-size bounds, while the API's `Filter` carries one of them
//! or an `And` of several, and an EMPTY filter is the rule that "applies to all objects in the
//! bucket" (`smithy_query(query="shape",shape="LifecycleRuleFilter")`). A lifecycle rule's
//! actions are flat too: `ExpirationInDays`/`ExpirationDate` are the `Expiration` container's
//! `Days`/`Date`, and a transition's `TransitionInDays`/`TransitionDate` its `Days`/`Date`.
//!
//! The rows differ along what the rule generalises over: which FILTER form a declared scope
//! amounts to (none, each single predicate, a conjunction of two and of three), how many
//! configurations and rules one bucket declares, which ACTIONS a rule asks for (a day-based
//! expiry, a dated expiry, current- and noncurrent-version transitions, a noncurrent expiry,
//! an aborted upload), both `Status` values, and the template SPELLING (JSON and YAML).
//!
//! Every expected value is computed from what the row declares, never read back out of the
//! answer under test.

use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    create_stack, describe_stack, router, wait_for_the_transition, xml_blocks, xml_text,
};
use crate::stack_resource_provisioning::{output_value, s3};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// The access point ARN one metrics row narrows by, and the storage classes the lifecycle
/// rows transition into — values of `TransitionStorageClass`
/// (`smithy_query(query="shape",shape="TransitionStorageClass")`) whose published minimum
/// storage durations the declared day counts respect (30 days in `STANDARD_IA` before a
/// further transition).
const ACCESS_POINT_ARN: &str = "arn:aws:s3:us-east-1:000000000000:accesspoint/reports";
const STANDARD_IA: &str = "STANDARD_IA";
const GLACIER: &str = "GLACIER";

/// `GET /<bucket>?metrics` — `ListBucketMetricsConfigurations`.
async fn list_metrics(router: &MultiServiceRouter, bucket: &str) -> (u16, String) {
    let answer = s3(router, "GET", &format!("/{bucket}?metrics"), b"").await;
    (answer.status, answer.body)
}

/// `GET /<bucket>?metrics&id=<id>` — `GetBucketMetricsConfiguration`.
async fn get_metrics(router: &MultiServiceRouter, bucket: &str, id: &str) -> (u16, String) {
    let answer = s3(router, "GET", &format!("/{bucket}?metrics&id={id}"), b"").await;
    (answer.status, answer.body)
}

/// `GET /<bucket>?lifecycle` — `GetBucketLifecycleConfiguration`.
async fn get_lifecycle(router: &MultiServiceRouter, bucket: &str) -> (u16, String) {
    let answer = s3(router, "GET", &format!("/{bucket}?lifecycle"), b"").await;
    (answer.status, answer.body)
}

/// A scope as the answer spells it, for the metrics filter and the lifecycle filter alike:
/// both carry single predicates beside an `And` conjunction, and the predicates they accept
/// are the union of the two sets.
#[derive(Debug, Default, PartialEq, Eq)]
struct Scope {
    prefix: Option<String>,
    tags: Vec<(String, String)>,
    access_point_arn: Option<String>,
    object_size_greater_than: Option<String>,
    object_size_less_than: Option<String>,
    /// Whether the predicates arrived inside the `And` conjunction. A scope naming one
    /// predicate is the predicate's own member, so this tells the two apart.
    conjunction: bool,
    /// Whether the container is there at all: a `Filter` element with no predicate inside it
    /// is the lifecycle rule that covers every object, which is not the same as no `Filter`.
    present: bool,
}

/// The scope one reported container carries, read from its `Filter` block.
fn reported_scope(entry: &str) -> Scope {
    let Some(block) = xml_blocks(entry, "Filter").into_iter().next() else {
        return Scope::default();
    };
    // `And` is the conjunction's own element, so its PRESENCE is what tells a conjunction
    // from the single-predicate members.
    let (inside, conjunction) = match xml_blocks(&block, "And").into_iter().next() {
        Some(conjunction) => (conjunction, true),
        None => (block, false),
    };
    Scope {
        prefix: xml_text(&inside, "Prefix"),
        tags: xml_blocks(&inside, "Tag")
            .iter()
            .map(|tag| {
                (
                    xml_text(tag, "Key").unwrap_or_default(),
                    xml_text(tag, "Value").unwrap_or_default(),
                )
            })
            .collect(),
        access_point_arn: xml_text(&inside, "AccessPointArn"),
        object_size_greater_than: xml_text(&inside, "ObjectSizeGreaterThan"),
        object_size_less_than: xml_text(&inside, "ObjectSizeLessThan"),
        conjunction,
        present: true,
    }
}

/// A scope a row EXPECTS, in the same shape [`reported_scope`] reads one into.
fn scope(
    prefix: Option<&str>,
    tags: &[(&str, &str)],
    access_point_arn: Option<&str>,
    sizes: (Option<i64>, Option<i64>),
    conjunction: bool,
) -> Scope {
    Scope {
        prefix: prefix.map(str::to_string),
        tags: tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
        access_point_arn: access_point_arn.map(str::to_string),
        object_size_greater_than: sizes.0.map(|bytes| bytes.to_string()),
        object_size_less_than: sizes.1.map(|bytes| bytes.to_string()),
        conjunction,
        present: true,
    }
}

/// One metrics configuration as Amazon S3 reported it.
#[derive(Debug, PartialEq, Eq)]
struct ReportedMetrics {
    id: String,
    scope: Scope,
}

/// Every `MetricsConfiguration` a body reports, in document order. One parser for the listing
/// and the per-id read alike, so the two cannot drift.
fn reported_metrics(xml: &str) -> Vec<ReportedMetrics> {
    xml_blocks(xml, "MetricsConfiguration")
        .iter()
        .map(|entry| ReportedMetrics {
            id: xml_text(entry, "Id").unwrap_or_default(),
            scope: reported_scope(entry),
        })
        .collect()
}

/// One lifecycle rule as Amazon S3 reported it, read as the values a declaration fixes.
#[derive(Debug, PartialEq, Eq)]
struct ReportedRule {
    id: String,
    status: String,
    scope: Scope,
    /// The `Expiration` container's `Days` and `Date`, as reported.
    expiration_days: Option<String>,
    expiration_date: Option<String>,
    /// Each `Transition`, as its `(Days, StorageClass)`.
    transitions: Vec<(Option<String>, String)>,
    /// Each `NoncurrentVersionTransition`, as its `(NoncurrentDays, StorageClass)`.
    noncurrent_transitions: Vec<(Option<String>, String)>,
    /// The `NoncurrentVersionExpiration`, as its `(NoncurrentDays, NewerNoncurrentVersions)`.
    noncurrent_expiration: Option<(Option<String>, Option<String>)>,
    /// The `AbortIncompleteMultipartUpload`'s `DaysAfterInitiation`.
    abort_after_days: Option<String>,
}

/// Every `Rule` a `GetBucketLifecycleConfiguration` body reports, in document order.
fn reported_rules(xml: &str) -> Vec<ReportedRule> {
    xml_blocks(xml, "Rule")
        .iter()
        .map(|entry| {
            let expiration = xml_blocks(entry, "Expiration").into_iter().next();
            let noncurrent_expiration = xml_blocks(entry, "NoncurrentVersionExpiration")
                .into_iter()
                .next()
                .map(|block| {
                    (
                        xml_text(&block, "NoncurrentDays"),
                        xml_text(&block, "NewerNoncurrentVersions"),
                    )
                });
            ReportedRule {
                id: xml_text(entry, "ID").unwrap_or_default(),
                status: xml_text(entry, "Status").unwrap_or_default(),
                scope: reported_scope(entry),
                expiration_days: expiration
                    .as_deref()
                    .and_then(|block| xml_text(block, "Days")),
                expiration_date: expiration
                    .as_deref()
                    .and_then(|block| xml_text(block, "Date")),
                transitions: xml_blocks(entry, "Transition")
                    .iter()
                    .map(|block| {
                        (
                            xml_text(block, "Days"),
                            xml_text(block, "StorageClass").unwrap_or_default(),
                        )
                    })
                    .collect(),
                noncurrent_transitions: xml_blocks(entry, "NoncurrentVersionTransition")
                    .iter()
                    .map(|block| {
                        (
                            xml_text(block, "NoncurrentDays"),
                            xml_text(block, "StorageClass").unwrap_or_default(),
                        )
                    })
                    .collect(),
                noncurrent_expiration,
                abort_after_days: xml_blocks(entry, "AbortIncompleteMultipartUpload")
                    .into_iter()
                    .next()
                    .and_then(|block| xml_text(&block, "DaysAfterInitiation")),
            }
        })
        .collect()
}

/// Deploy a template and answer with the `DescribeStacks` body it completed with.
async fn deploy(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (http_status, created) = create_stack(router, stack_name, template, &[]).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: CreateStack must answer 200, got {http_status} {created}"
    );
    wait_for_the_transition().await;

    let (http_status, described) = describe_stack(router, stack_name).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: DescribeStacks must answer 200, got {http_status} {described}"
    );
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "case {case:?}: the stack must reach CREATE_COMPLETE, got {described}"
    );
    described
}

/// The stack whose bucket declares one metrics configuration of every filter form, in JSON.
const METRICS_TEMPLATE: &str = r#"{"Resources":{
     "MetricsBucket":{"Type":"AWS::S3::Bucket","Properties":{
       "BucketName":"stack-metrics-source",
       "MetricsConfigurations":[
         {"Id":"ByPrefix","Prefix":"logs/"},
         {"Id":"ByTag","TagFilters":[{"Key":"team","Value":"data"}]},
         {"Id":"ByAccessPoint",
          "AccessPointArn":"arn:aws:s3:us-east-1:000000000000:accesspoint/reports"},
         {"Id":"ByPrefixAndTag","Prefix":"archive/",
          "TagFilters":[{"Key":"tier","Value":"cold"}]},
         {"Id":"ByEverything","Prefix":"mixed/",
          "AccessPointArn":"arn:aws:s3:us-east-1:000000000000:accesspoint/reports",
          "TagFilters":[{"Key":"tier","Value":"warm"},{"Key":"owner","Value":"data"}]},
         {"Id":"WholeBucket"}]}}},
   "Outputs":{"MetricsBucketName":{"Value":{"Ref":"MetricsBucket"}}}}"#;

/// What the bucket of [`METRICS_TEMPLATE`] must report, in the ascending id order the listing
/// reports.
fn expected_metrics() -> Vec<ReportedMetrics> {
    vec![
        ReportedMetrics {
            id: "ByAccessPoint".to_string(),
            scope: scope(None, &[], Some(ACCESS_POINT_ARN), (None, None), false),
        },
        ReportedMetrics {
            id: "ByEverything".to_string(),
            scope: scope(
                Some("mixed/"),
                &[("tier", "warm"), ("owner", "data")],
                Some(ACCESS_POINT_ARN),
                (None, None),
                true,
            ),
        },
        ReportedMetrics {
            id: "ByPrefix".to_string(),
            scope: scope(Some("logs/"), &[], None, (None, None), false),
        },
        ReportedMetrics {
            id: "ByPrefixAndTag".to_string(),
            scope: scope(Some("archive/"), &[("tier", "cold")], None, (None, None), true),
        },
        ReportedMetrics {
            id: "ByTag".to_string(),
            scope: scope(None, &[("team", "data")], None, (None, None), false),
        },
        ReportedMetrics {
            id: "WholeBucket".to_string(),
            scope: Scope::default(),
        },
    ]
}

#[tokio::test]
async fn a_bucket_a_stack_declares_carries_the_metrics_configurations_it_declared() {
    let case = "one metrics configuration of every filter form, in JSON";
    let router = router();
    let described = deploy(&router, case, "stack-metrics", METRICS_TEMPLATE).await;
    let name = output_value(&described, "MetricsBucketName");
    assert!(
        !name.is_empty(),
        "case {case:?}: the stack must publish the bucket name, got {described}"
    );

    let (http_status, listing) = list_metrics(&router, &name).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: s3:ListBucketMetricsConfigurations({name:?}) must answer 200, got \
         {http_status} {listing}"
    );
    let listed = reported_metrics(&listing);
    let expected = expected_metrics();
    assert_eq!(
        listed, expected,
        "case {case:?}: the bucket {name:?} must list {expected:?}, got {listed:?} ({listing})"
    );

    // And each reads back under its own id, which is the id the entry declared.
    for configuration in &expected {
        let (http_status, body) = get_metrics(&router, &name, &configuration.id).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: s3:GetBucketMetricsConfiguration({name:?}, {:?}) must answer 200, \
             got {http_status} {body}",
            configuration.id
        );
        let read = reported_metrics(&body);
        assert_eq!(
            read.len(),
            1,
            "case {case:?}: the per-id read reports one configuration, got {read:?} ({body})"
        );
        assert_eq!(
            read[0], *configuration,
            "case {case:?}: the configuration {:?} of {name:?} must read back as \
             {configuration:?}, got {:?} ({body})",
            configuration.id, read[0]
        );
    }
}

/// The stack whose bucket declares a lifecycle rule for each action and filter form, in YAML.
const LIFECYCLE_TEMPLATE: &str = concat!(
    "Resources:\n",
    "  LifecycleBucket:\n",
    "    Type: AWS::S3::Bucket\n",
    "    Properties:\n",
    "      BucketName: stack-lifecycle-source\n",
    "      VersioningConfiguration:\n",
    "        Status: Enabled\n",
    "      LifecycleConfiguration:\n",
    "        Rules:\n",
    "          - Id: ExpireLogs\n",
    "            Status: Enabled\n",
    "            Prefix: logs/\n",
    "            ExpirationInDays: 30\n",
    "          - Id: ArchiveFinance\n",
    "            Status: Enabled\n",
    "            Prefix: documents/\n",
    "            TagFilters:\n",
    "              - Key: department\n",
    "                Value: finance\n",
    "            Transitions:\n",
    "              - StorageClass: STANDARD_IA\n",
    "                TransitionInDays: 30\n",
    "              - StorageClass: GLACIER\n",
    "                TransitionInDays: 90\n",
    "          - Id: AbortStalled\n",
    "            Status: Disabled\n",
    "            AbortIncompleteMultipartUpload:\n",
    "              DaysAfterInitiation: 7\n",
    "          - Id: PruneNoncurrent\n",
    "            Status: Enabled\n",
    "            ObjectSizeGreaterThan: 1024\n",
    "            NoncurrentVersionTransitions:\n",
    "              - StorageClass: STANDARD_IA\n",
    "                TransitionInDays: 45\n",
    "            NoncurrentVersionExpiration:\n",
    "              NoncurrentDays: 100\n",
    "              NewerNoncurrentVersions: 3\n",
    "          - Id: ExpireSizedOnADate\n",
    "            Status: Enabled\n",
    "            ObjectSizeGreaterThan: 2048\n",
    "            ObjectSizeLessThan: 8192\n",
    "            ExpirationDate: '2030-01-01T00:00:00Z'\n",
    "Outputs:\n",
    "  LifecycleBucketName:\n",
    "    Value: !Ref LifecycleBucket\n",
);

#[tokio::test]
async fn a_bucket_a_stack_declares_carries_the_lifecycle_configuration_it_declared() {
    let case = "five rules, in YAML: every action and every filter form";
    let router = router();
    let described = deploy(&router, case, "stack-lifecycle", LIFECYCLE_TEMPLATE).await;
    let name = output_value(&described, "LifecycleBucketName");
    assert!(
        !name.is_empty(),
        "case {case:?}: the stack must publish the bucket name, got {described}"
    );

    let (http_status, body) = get_lifecycle(&router, &name).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: s3:GetBucketLifecycleConfiguration({name:?}) must answer 200, got \
         {http_status} {body}"
    );
    // The rules are reported in the order the declaration states them, which is the order the
    // expectations below are written in.
    let expected = vec![
        ReportedRule {
            id: "ExpireLogs".to_string(),
            status: "Enabled".to_string(),
            scope: scope(Some("logs/"), &[], None, (None, None), false),
            expiration_days: Some("30".to_string()),
            expiration_date: None,
            transitions: Vec::new(),
            noncurrent_transitions: Vec::new(),
            noncurrent_expiration: None,
            abort_after_days: None,
        },
        ReportedRule {
            id: "ArchiveFinance".to_string(),
            status: "Enabled".to_string(),
            scope: scope(
                Some("documents/"),
                &[("department", "finance")],
                None,
                (None, None),
                true,
            ),
            expiration_days: None,
            expiration_date: None,
            transitions: vec![
                (Some("30".to_string()), STANDARD_IA.to_string()),
                (Some("90".to_string()), GLACIER.to_string()),
            ],
            noncurrent_transitions: Vec::new(),
            noncurrent_expiration: None,
            abort_after_days: None,
        },
        ReportedRule {
            id: "AbortStalled".to_string(),
            status: "Disabled".to_string(),
            // The rule narrows nothing, so the API's EMPTY filter is what it carries: present,
            // with no predicate in it.
            scope: Scope {
                present: true,
                ..Scope::default()
            },
            expiration_days: None,
            expiration_date: None,
            transitions: Vec::new(),
            noncurrent_transitions: Vec::new(),
            noncurrent_expiration: None,
            abort_after_days: Some("7".to_string()),
        },
        ReportedRule {
            id: "PruneNoncurrent".to_string(),
            status: "Enabled".to_string(),
            scope: scope(None, &[], None, (Some(1024), None), false),
            expiration_days: None,
            expiration_date: None,
            transitions: Vec::new(),
            noncurrent_transitions: vec![(Some("45".to_string()), STANDARD_IA.to_string())],
            noncurrent_expiration: Some((Some("100".to_string()), Some("3".to_string()))),
            abort_after_days: None,
        },
        ReportedRule {
            id: "ExpireSizedOnADate".to_string(),
            status: "Enabled".to_string(),
            scope: scope(None, &[], None, (Some(2048), Some(8192)), true),
            expiration_days: None,
            expiration_date: Some("2030-01-01T00:00:00Z".to_string()),
            transitions: Vec::new(),
            noncurrent_transitions: Vec::new(),
            noncurrent_expiration: None,
            abort_after_days: None,
        },
    ];
    let read = reported_rules(&body);
    assert_eq!(
        read, expected,
        "case {case:?}: the bucket {name:?} must report {expected:?}, got {read:?} ({body})"
    );
}

/// A declared configuration the provisioner cannot install, or that Amazon S3 will not take,
/// is not dropped quietly: the resource fails, so the stack does not report CREATE_COMPLETE
/// over a configuration the bucket never carried.
#[tokio::test]
async fn a_configuration_the_service_will_not_take_fails_the_bucket() {
    let rows: &[(&str, &str, &str)] = &[
        (
            "a metrics entry declaring no Id",
            "idless-metrics",
            concat!(
                "Resources:\n",
                "  Source:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: idless-metrics-source\n",
                "      MetricsConfigurations:\n",
                "        - Prefix: logs/\n",
            ),
        ),
        (
            "a lifecycle rule declaring no Status",
            "statusless-lifecycle",
            concat!(
                "Resources:\n",
                "  Source:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: statusless-lifecycle-source\n",
                "      LifecycleConfiguration:\n",
                "        Rules:\n",
                "          - Id: Incomplete\n",
                "            ExpirationInDays: 30\n",
            ),
        ),
        (
            "a lifecycle rule whose ExpirationDate is no instant",
            "undated-lifecycle",
            concat!(
                "Resources:\n",
                "  Source:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: undated-lifecycle-source\n",
                "      LifecycleConfiguration:\n",
                "        Rules:\n",
                "          - Id: Undated\n",
                "            Status: Enabled\n",
                "            ExpirationDate: the first of January\n",
            ),
        ),
        (
            "a lifecycle rule asking for no action at all, which S3 refuses",
            "actionless-lifecycle",
            concat!(
                "Resources:\n",
                "  Source:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: actionless-lifecycle-source\n",
                "      LifecycleConfiguration:\n",
                "        Rules:\n",
                "          - Id: Idle\n",
                "            Status: Enabled\n",
                "            Prefix: logs/\n",
            ),
        ),
        (
            "a transition declaring no StorageClass",
            "classless-lifecycle",
            concat!(
                "Resources:\n",
                "  Source:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: classless-lifecycle-source\n",
                "      LifecycleConfiguration:\n",
                "        Rules:\n",
                "          - Id: Incomplete\n",
                "            Status: Enabled\n",
                "            Transitions:\n",
                "              - TransitionInDays: 30\n",
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
            "case {case:?}: a bucket whose declared configuration could not be installed must \
             not complete, got {described}"
        );
    }
}
