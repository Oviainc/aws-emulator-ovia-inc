// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Verified Access instance LOGGING slice:
//! `ModifyVerifiedAccessInstanceLoggingConfiguration` and
//! `DescribeVerifiedAccessInstanceLoggingConfigurations`, driven over the wire the service
//! actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. an instance NOTHING has configured carries one configuration: every destination
//!    disabled, no trust data in the logs, no log schema chosen and no delivery status — and
//!    describing it stores nothing, so a later caller sees exactly what this one saw;
//! 2. `ModifyVerifiedAccessInstanceLoggingConfiguration` SETS the configuration from the
//!    options the request carries: a destination block it carries is that destination's whole
//!    configuration, an option it does not carry keeps the stored value, and the
//!    configuration it answers is the one
//!    `DescribeVerifiedAccessInstanceLoggingConfigurations` then reports for that instance;
//! 3. the modify refuses what it cannot store: a log version the model does not publish, an
//!    enabled destination that names nowhere to deliver to, a bucket owner that is no account
//!    id, a client token past the published bound, and an instance id the account holds no
//!    instance for — and a `DryRun` reports the outcome and writes nothing;
//! 4. one `ClientToken` names one update: a repeat applies nothing further, and the same token
//!    carrying a different update is refused;
//! 5. the describe REPORTS THE STORE: narrowed by `VerifiedAccessInstanceIds` and by every
//!    filter it answers (AND across filters, OR within one, `*` wildcards), refusing an id
//!    the account holds no instance for and a filter name it does not answer, and paged by
//!    `MaxResults`/`NextToken` over the modelled `range 1..10`.
//!
//! Each table row is a distinct input along what the rule generalises over — which
//! destination, how many of them, enabled or not, which published log version, whether trust
//! context is asked for, which option the repeat leaves out — so no row is the specification.

use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, MAX_CLIENT_TOKEN_CHARS};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message, filters};
use super::wire::{child, child_text, document};

/// The Region every call here is signed for: a Region that is not the emulator's default is
/// the stronger case, and the instance the configuration belongs to is a regional entity.
const REGION: &str = "ap-southeast-2";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// Stand up an emulator with nothing seeded: the account starts with no Verified Access
/// instance and no logging configuration at all.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// Create one Verified Access instance that must succeed, and answer the id it was minted.
async fn seed_instance(router: &MultiServiceRouter, case: &str) -> String {
    let (status, body) = call(router, REGION, "CreateVerifiedAccessInstance", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateVerifiedAccessInstance must succeed, got {status} with body \
         {body}"
    );
    let instance_id = element(&body, "verifiedAccessInstanceId").unwrap_or_default();
    assert!(
        instance_id.starts_with("vai-"),
        "case {case:?}: expected a minted instance id, got {instance_id:?}"
    );
    instance_id
}

/// Ask `ModifyVerifiedAccessInstanceLoggingConfiguration` with `params`.
async fn modify(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(
        router,
        REGION,
        "ModifyVerifiedAccessInstanceLoggingConfiguration",
        params,
    )
    .await
}

/// Ask `DescribeVerifiedAccessInstanceLoggingConfigurations` with `params`.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(
        router,
        REGION,
        "DescribeVerifiedAccessInstanceLoggingConfigurations",
        params,
    )
    .await
}

/// How many logging configurations the store holds — the map the describe reads, so a read
/// that wrote a default into the store would show up here.
fn stored_configurations(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .verified_access_instance_logging_configurations
        .len()
}

/// The members of one reported `<accessLogs>` block, as `path -> value` pairs sorted by path:
/// a destination's own member is `s3/enabled`, a member of the block itself is `logVersion`.
///
/// Read through the XML tree rather than by substring, so a destination's `enabled` is never
/// confused with another destination's.
fn members(access_logs: roxmltree::Node<'_, '_>) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for member in access_logs.children().filter(roxmltree::Node::is_element) {
        let name = member.tag_name().name().to_string();
        let nested: Vec<_> = member
            .children()
            .filter(roxmltree::Node::is_element)
            .collect();
        if nested.is_empty() {
            pairs.push((name, member.text().unwrap_or_default().to_string()));
        } else {
            for inner in nested {
                pairs.push((
                    format!("{name}/{}", inner.tag_name().name()),
                    inner.text().unwrap_or_default().to_string(),
                ));
            }
        }
    }
    pairs.sort();
    pairs
}

/// Every logging configuration a response carries: the instance it belongs to and the members
/// of its `<accessLogs>` block, in the order the response reported them.
///
/// One reader for both operations: a configuration is the element carrying an `<accessLogs>`
/// child, whether it is the modify's `<loggingConfiguration>` or one `<item>` of the
/// describe's `<loggingConfigurationSet>`.
fn configurations(case: &str, body: &str) -> Vec<(String, Vec<(String, String)>)> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "accessLogs").is_some())
        .map(|node| {
            let access_logs = child(node, "accessLogs").expect("filtered on this child");
            (
                child_text(node, "verifiedAccessInstanceId").unwrap_or_default(),
                members(access_logs),
            )
        })
        .collect()
}

/// The one configuration a response carries, or a panic naming the case.
fn one_configuration(case: &str, body: &str) -> (String, Vec<(String, String)>) {
    let reported = configurations(case, body);
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected exactly one logging configuration, got {reported:?} in {body}"
    );
    reported.into_iter().next().expect("one configuration")
}

/// Assert that `reported` carries every `expected` pair and none of the `absent` paths.
fn assert_members(
    case: &str,
    reported: &[(String, String)],
    expected: &[(&str, &str)],
    absent: &[&str],
) {
    for (path, value) in expected {
        let actual = reported
            .iter()
            .find(|(reported_path, _)| reported_path == path)
            .map(|(_, reported_value)| reported_value.as_str());
        assert_eq!(
            actual,
            Some(*value),
            "case {case:?}: expected {path}={value:?}, got {actual:?} in {reported:?}"
        );
    }
    for path in absent {
        let actual = reported
            .iter()
            .find(|(reported_path, _)| reported_path == path);
        assert!(
            actual.is_none(),
            "case {case:?}: nothing set {path}, so it must not be reported, got {actual:?}"
        );
    }
}

/// The ids `DescribeVerifiedAccessInstanceLoggingConfigurations` reports for `params`, in the
/// order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the describe, got {status} with body {body}"
    );
    configurations(case, &body)
        .into_iter()
        .map(|(instance_id, _)| instance_id)
        .collect()
}

/// The configuration the describe reports for one instance.
async fn described_configuration(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
) -> Vec<(String, String)> {
    let (status, body) = describe(
        router,
        &[p("VerifiedAccessInstanceId.1", instance_id.to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the describe, got {status} with body {body}"
    );
    let (reported_id, reported) = one_configuration(case, &body);
    assert_eq!(
        reported_id, instance_id,
        "case {case:?}: expected the configuration of the instance the request named, got \
         {reported_id:?}"
    );
    reported
}

/// One configured instance a case has to read back: the case's name, the instance it
/// configured, the members its configuration must report and the members it must not report.
type Configured = (&'static str, String, Expected, Absent);

/// One row's expected `path -> value` pairs.
type Expected = Vec<(&'static str, &'static str)>;

/// One row's paths that must not be reported at all.
type Absent = Vec<&'static str>;

/// One filter table row: the case's name, the filters it asks with, and the instance ids the
/// report must carry in id order. The filter VALUES borrow the ids the case created, so the
/// row's lifetime is theirs.
type FilterRow<'a> = (&'static str, Vec<(&'static str, Vec<&'a str>)>, Vec<String>);

/// RULE 1: an instance nothing has configured carries the one unconfigured configuration —
/// every destination disabled, no trust data, no log schema and no delivery status — and
/// describing it writes nothing into the store.
#[tokio::test]
async fn an_unconfigured_instance_reports_every_destination_disabled() {
    let (router, engine) = emulator();
    let mut created = Vec::new();
    for index in 0..3 {
        created.push(seed_instance(&router, &format!("instance {index}")).await);
    }
    created.sort();

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "case \"unconfigured\": expected HTTP 200, got {status} with body {body}"
    );
    let reported = configurations("unconfigured", &body);
    assert_eq!(
        reported
            .iter()
            .map(|(instance_id, _)| instance_id.clone())
            .collect::<Vec<_>>(),
        created,
        "case \"unconfigured\": expected one configuration per stored instance, in id order, \
         got {body}"
    );
    for (instance_id, logs) in &reported {
        assert_members(
            &format!("unconfigured {instance_id}"),
            logs,
            &[
                ("s3/enabled", "false"),
                ("cloudWatchLogs/enabled", "false"),
                ("kinesisDataFirehose/enabled", "false"),
                ("includeTrustContext", "false"),
            ],
            &[
                "logVersion",
                "s3/bucketName",
                "s3/prefix",
                "s3/bucketOwner",
                "s3/deliveryStatus",
                "cloudWatchLogs/logGroup",
                "cloudWatchLogs/deliveryStatus",
                "kinesisDataFirehose/deliveryStream",
                "kinesisDataFirehose/deliveryStatus",
            ],
        );
    }

    // The read REPORTS that configuration rather than storing it, so the store holds no
    // configuration at all and a later caller sees what this one did.
    assert_eq!(
        stored_configurations(&engine),
        0,
        "case \"unconfigured\": a describe must store nothing, got {} configurations",
        stored_configurations(&engine)
    );
    let (status, repeat) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "case \"unconfigured repeat\": expected HTTP 200, got {status} with body {repeat}"
    );
    assert_eq!(
        configurations("unconfigured repeat", &repeat),
        reported,
        "case \"unconfigured repeat\": a second describe must report what the first did"
    );
}

/// RULE 2: every shape the options may take sets the configuration the modify answers with,
/// and the describe then reports exactly that configuration out of the STORE.
#[tokio::test]
async fn every_option_shape_sets_the_configuration_the_describe_reports() {
    // (case, the options the modify carries, the members the configuration must report, the
    // members it must NOT report at all).
    type Row = (
        &'static str,
        Params,
        Vec<(&'static str, &'static str)>,
        Vec<&'static str>,
    );
    let rows: Vec<Row> = vec![
        (
            "s3 with a bucket, a prefix and an owner",
            vec![
                p("AccessLogs.S3.Enabled", "true"),
                p("AccessLogs.S3.BucketName", "verified-access-records"),
                p("AccessLogs.S3.Prefix", "instance/logs"),
                p("AccessLogs.S3.BucketOwner", "123456789012"),
            ],
            vec![
                ("s3/enabled", "true"),
                ("s3/bucketName", "verified-access-records"),
                ("s3/prefix", "instance/logs"),
                ("s3/bucketOwner", "123456789012"),
                ("cloudWatchLogs/enabled", "false"),
                ("kinesisDataFirehose/enabled", "false"),
                ("includeTrustContext", "false"),
            ],
            vec!["logVersion", "s3/deliveryStatus", "cloudWatchLogs/logGroup"],
        ),
        (
            "cloudWatchLogs with the first published log version",
            vec![
                p("AccessLogs.CloudWatchLogs.Enabled", "true"),
                p("AccessLogs.CloudWatchLogs.LogGroup", "/aws/verified-access"),
                p("AccessLogs.LogVersion", "ocsf-0.1"),
            ],
            vec![
                ("cloudWatchLogs/enabled", "true"),
                ("cloudWatchLogs/logGroup", "/aws/verified-access"),
                ("logVersion", "ocsf-0.1"),
                ("s3/enabled", "false"),
                ("kinesisDataFirehose/enabled", "false"),
                ("includeTrustContext", "false"),
            ],
            vec!["s3/bucketName", "cloudWatchLogs/deliveryStatus"],
        ),
        (
            "kinesisDataFirehose with trust context and the later published log version",
            vec![
                p("AccessLogs.KinesisDataFirehose.Enabled", "true"),
                p(
                    "AccessLogs.KinesisDataFirehose.DeliveryStream",
                    "verified-access-stream",
                ),
                p("AccessLogs.IncludeTrustContext", "true"),
                p("AccessLogs.LogVersion", "ocsf-1.0.0-rc.2"),
            ],
            vec![
                ("kinesisDataFirehose/enabled", "true"),
                ("kinesisDataFirehose/deliveryStream", "verified-access-stream"),
                ("includeTrustContext", "true"),
                ("logVersion", "ocsf-1.0.0-rc.2"),
                ("s3/enabled", "false"),
                ("cloudWatchLogs/enabled", "false"),
            ],
            vec!["kinesisDataFirehose/deliveryStatus", "s3/bucketName"],
        ),
        (
            "every destination at once",
            vec![
                p("AccessLogs.S3.Enabled", "true"),
                p("AccessLogs.S3.BucketName", "every-destination"),
                p("AccessLogs.CloudWatchLogs.Enabled", "true"),
                p("AccessLogs.CloudWatchLogs.LogGroup", "/aws/every-destination"),
                p("AccessLogs.KinesisDataFirehose.Enabled", "true"),
                p(
                    "AccessLogs.KinesisDataFirehose.DeliveryStream",
                    "every-destination",
                ),
                p("AccessLogs.LogVersion", "ocsf-0.1"),
                p("AccessLogs.IncludeTrustContext", "true"),
            ],
            vec![
                ("s3/enabled", "true"),
                ("s3/bucketName", "every-destination"),
                ("cloudWatchLogs/enabled", "true"),
                ("cloudWatchLogs/logGroup", "/aws/every-destination"),
                ("kinesisDataFirehose/enabled", "true"),
                ("kinesisDataFirehose/deliveryStream", "every-destination"),
                ("logVersion", "ocsf-0.1"),
                ("includeTrustContext", "true"),
            ],
            vec![
                "s3/deliveryStatus",
                "cloudWatchLogs/deliveryStatus",
                "kinesisDataFirehose/deliveryStatus",
            ],
        ),
        (
            "every destination explicitly disabled",
            vec![
                p("AccessLogs.S3.Enabled", "false"),
                p("AccessLogs.CloudWatchLogs.Enabled", "false"),
                p("AccessLogs.KinesisDataFirehose.Enabled", "false"),
                p("AccessLogs.IncludeTrustContext", "false"),
            ],
            vec![
                ("s3/enabled", "false"),
                ("cloudWatchLogs/enabled", "false"),
                ("kinesisDataFirehose/enabled", "false"),
                ("includeTrustContext", "false"),
            ],
            vec!["logVersion", "s3/bucketName", "cloudWatchLogs/logGroup"],
        ),
        (
            "a destination named but not enabled",
            vec![
                p("AccessLogs.S3.Enabled", "false"),
                p("AccessLogs.S3.BucketName", "named-but-disabled"),
                p("AccessLogs.S3.Prefix", "held"),
            ],
            vec![
                ("s3/enabled", "false"),
                ("s3/bucketName", "named-but-disabled"),
                ("s3/prefix", "held"),
                ("cloudWatchLogs/enabled", "false"),
                ("kinesisDataFirehose/enabled", "false"),
            ],
            vec!["s3/bucketOwner", "logVersion"],
        ),
    ];

    // One emulator for every row: each row configures an instance of its own, so a reader
    // that reported any instance's configuration at all would not pass.
    let (router, engine) = emulator();
    let mut configured: Vec<Configured> = Vec::new();
    for (case, options, expected, absent) in rows {
        let instance_id = seed_instance(&router, case).await;
        let mut params = vec![p("VerifiedAccessInstanceId", instance_id.clone())];
        params.extend(options);
        let (status, body) = modify(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the modify, got {status} with body {body}"
        );
        let (reported_id, reported) = one_configuration(case, &body);
        assert_eq!(
            reported_id, instance_id,
            "case {case:?}: expected the configuration of the instance the request named, got \
             {reported_id:?}"
        );
        assert_members(case, &reported, &expected, &absent);
        configured.push((case, instance_id, expected, absent));
    }

    assert_eq!(
        stored_configurations(&engine),
        configured.len(),
        "the store must hold one configuration per configured instance, got {}",
        stored_configurations(&engine)
    );

    // Every configuration reads back out of the STORE through the describe, which is what
    // makes the modify's answer the stored record rather than an echo of its own request.
    for (case, instance_id, expected, absent) in &configured {
        let reported = described_configuration(&router, case, instance_id).await;
        assert_members(case, &reported, expected, absent);
    }
}

/// RULE 2's merge: an option the request does not carry keeps its stored value, while a
/// destination block it carries replaces that destination's whole configuration.
#[tokio::test]
async fn an_option_the_request_omits_keeps_its_stored_value() {
    let (router, _engine) = emulator();
    let instance_id = seed_instance(&router, "merge").await;
    let on = |params: Params| {
        let mut all = vec![p("VerifiedAccessInstanceId", instance_id.clone())];
        all.extend(params);
        all
    };

    // (case, the options this call carries, the members the configuration must then report,
    // the members it must NOT report) — each step reads the one before it.
    type Row = (
        &'static str,
        Params,
        Vec<(&'static str, &'static str)>,
        Vec<&'static str>,
    );
    let steps: Vec<Row> = vec![
        (
            "enable s3",
            vec![
                p("AccessLogs.S3.Enabled", "true"),
                p("AccessLogs.S3.BucketName", "merge-bucket"),
                p("AccessLogs.LogVersion", "ocsf-0.1"),
            ],
            vec![
                ("s3/enabled", "true"),
                ("s3/bucketName", "merge-bucket"),
                ("logVersion", "ocsf-0.1"),
                ("includeTrustContext", "false"),
            ],
            vec!["cloudWatchLogs/logGroup"],
        ),
        (
            "ask only for trust context",
            vec![p("AccessLogs.IncludeTrustContext", "true")],
            vec![
                ("includeTrustContext", "true"),
                // Carried by neither this request nor the one before it in this form, so both
                // stay exactly as the first call left them.
                ("s3/enabled", "true"),
                ("s3/bucketName", "merge-bucket"),
                ("logVersion", "ocsf-0.1"),
            ],
            vec!["cloudWatchLogs/logGroup"],
        ),
        (
            "add cloudWatchLogs",
            vec![
                p("AccessLogs.CloudWatchLogs.Enabled", "true"),
                p("AccessLogs.CloudWatchLogs.LogGroup", "/aws/merge"),
            ],
            vec![
                ("cloudWatchLogs/enabled", "true"),
                ("cloudWatchLogs/logGroup", "/aws/merge"),
                ("s3/enabled", "true"),
                ("s3/bucketName", "merge-bucket"),
                ("includeTrustContext", "true"),
                ("logVersion", "ocsf-0.1"),
            ],
            vec![],
        ),
        (
            "replace the s3 block with a disabled one",
            vec![p("AccessLogs.S3.Enabled", "false")],
            vec![
                ("s3/enabled", "false"),
                // The block the request carried is that destination's WHOLE configuration, so
                // the bucket it names nothing for is gone, while the untouched destination and
                // the untouched options stay.
                ("cloudWatchLogs/enabled", "true"),
                ("cloudWatchLogs/logGroup", "/aws/merge"),
                ("includeTrustContext", "true"),
                ("logVersion", "ocsf-0.1"),
            ],
            vec!["s3/bucketName"],
        ),
        (
            "move to the later published log version",
            vec![p("AccessLogs.LogVersion", "ocsf-1.0.0-rc.2")],
            vec![
                ("logVersion", "ocsf-1.0.0-rc.2"),
                ("s3/enabled", "false"),
                ("cloudWatchLogs/logGroup", "/aws/merge"),
                ("includeTrustContext", "true"),
            ],
            vec!["s3/bucketName"],
        ),
    ];

    for (case, options, expected, absent) in steps {
        let (status, body) = modify(&router, &on(options)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the modify, got {status} with body {body}"
        );
        let (_, answered) = one_configuration(case, &body);
        assert_members(case, &answered, &expected, &absent);
        // The same configuration out of the store, so the modify's answer is the record and
        // not a reading of its own request.
        let reported = described_configuration(&router, case, &instance_id).await;
        assert_members(case, &reported, &expected, &absent);
    }
}

/// RULE 3: the modify refuses the requests it cannot store, each with EC2's own code, and
/// accepts each published bound of the values it holds to one.
#[tokio::test]
async fn the_modify_refuses_what_it_cannot_store() {
    let (router, engine) = emulator();
    let instance_id = seed_instance(&router, "refusals").await;
    let on = |params: Params| {
        let mut all = vec![p("VerifiedAccessInstanceId", instance_id.clone())];
        all.extend(params);
        all
    };

    // (case, the parameters under test, the HTTP status, the error code — the empty string
    // for a request that must be accepted).
    let rows: Vec<(&str, Params, u16, &str)> = vec![
        (
            "the first published log version",
            on(vec![
                p("AccessLogs.LogVersion", "ocsf-0.1"),
                p("AccessLogs.S3.Enabled", "false"),
            ]),
            200,
            "",
        ),
        (
            "the later published log version",
            on(vec![p("AccessLogs.LogVersion", "ocsf-1.0.0-rc.2")]),
            200,
            "",
        ),
        (
            "a log version the model does not publish",
            on(vec![p("AccessLogs.LogVersion", "ocsf-2.0")]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a log version differing only in case",
            on(vec![p("AccessLogs.LogVersion", "OCSF-0.1")]),
            400,
            "InvalidParameterValue",
        ),
        (
            "an empty log version",
            on(vec![p("AccessLogs.LogVersion", "")]),
            400,
            "InvalidParameterValue",
        ),
        (
            "s3 enabled with no bucket",
            on(vec![p("AccessLogs.S3.Enabled", "true")]),
            400,
            "InvalidParameterValue",
        ),
        (
            "s3 enabled with a blank bucket",
            on(vec![
                p("AccessLogs.S3.Enabled", "true"),
                p("AccessLogs.S3.BucketName", "   "),
            ]),
            400,
            "InvalidParameterValue",
        ),
        (
            "cloudWatchLogs enabled with no log group",
            on(vec![p("AccessLogs.CloudWatchLogs.Enabled", "true")]),
            400,
            "InvalidParameterValue",
        ),
        (
            "kinesisDataFirehose enabled with no delivery stream",
            on(vec![p("AccessLogs.KinesisDataFirehose.Enabled", "true")]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a twelve-digit bucket owner",
            on(vec![
                p("AccessLogs.S3.Enabled", "true"),
                p("AccessLogs.S3.BucketName", "owned-bucket"),
                p("AccessLogs.S3.BucketOwner", "210987654321"),
            ]),
            200,
            "",
        ),
        (
            "a bucket owner one digit short",
            on(vec![
                p("AccessLogs.S3.Enabled", "true"),
                p("AccessLogs.S3.BucketName", "owned-bucket"),
                p("AccessLogs.S3.BucketOwner", "12345678901"),
            ]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a bucket owner that is not digits at all",
            on(vec![
                p("AccessLogs.S3.Enabled", "true"),
                p("AccessLogs.S3.BucketName", "owned-bucket"),
                p("AccessLogs.S3.BucketOwner", "account-one"),
            ]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a client token at the published bound",
            on(vec![
                p("AccessLogs.S3.Enabled", "false"),
                p("ClientToken", "t".repeat(MAX_CLIENT_TOKEN_CHARS)),
            ]),
            200,
            "",
        ),
        (
            "a client token one character past the bound",
            on(vec![
                p("AccessLogs.S3.Enabled", "false"),
                p("ClientToken", "t".repeat(MAX_CLIENT_TOKEN_CHARS + 1)),
            ]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a long instance id nothing minted",
            vec![
                p("VerifiedAccessInstanceId", "vai-0123456789abcdef0"),
                p("AccessLogs.S3.Enabled", "false"),
            ],
            400,
            "InvalidVerifiedAccessInstanceId.NotFound",
        ),
        (
            "a short instance id nothing minted",
            vec![
                p("VerifiedAccessInstanceId", "vai-01234567"),
                p("AccessLogs.S3.Enabled", "false"),
            ],
            400,
            "InvalidVerifiedAccessInstanceId.NotFound",
        ),
        (
            "an instance id of another resource kind",
            vec![
                p("VerifiedAccessInstanceId", "vpc-0123456789abcdef0"),
                p("AccessLogs.S3.Enabled", "false"),
            ],
            400,
            "InvalidVerifiedAccessInstanceId.NotFound",
        ),
    ];

    for (case, params, expected_status, expected_code) in rows {
        let (status, body) = modify(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = if status == 200 {
            String::new()
        } else {
            error_code(&body)
        };
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected the error code {expected_code:?}, got {code:?} in {body}"
        );
    }

    // An id nothing minted is named in the refusal, so a caller holding several sees which.
    let (_, body) = modify(
        &router,
        &[
            p("VerifiedAccessInstanceId", "vai-0123456789abcdef0"),
            p("AccessLogs.S3.Enabled", "false"),
        ],
    )
    .await;
    assert_eq!(
        error_message(&body),
        "The verified-access-instance ID 'vai-0123456789abcdef0' does not exist",
        "case \"unknown id wording\": expected EC2's own wording, got {body}"
    );

    // A log version the model does not publish is refused with the versions it does publish.
    let (_, body) = modify(&router, &on(vec![p("AccessLogs.LogVersion", "ocsf-2.0")])).await;
    assert_eq!(
        error_message(&body),
        "Invalid value 'ocsf-2.0' for logVersion. Valid values: ocsf-0.1 | ocsf-1.0.0-rc.2",
        "case \"unpublished log version wording\": expected the published versions, got {body}"
    );

    // A DryRun reports the outcome the request WOULD have had and writes nothing: the
    // configuration the accepted rows above left behind is the one the describe still reports.
    let before = described_configuration(&router, "before the dry run", &instance_id).await;
    let (status, body) = modify(
        &router,
        &on(vec![
            p("AccessLogs.KinesisDataFirehose.Enabled", "true"),
            p("AccessLogs.KinesisDataFirehose.DeliveryStream", "dry-run"),
            p("DryRun", "true"),
        ]),
    )
    .await;
    assert_eq!(
        status, 412,
        "case \"dry run\": expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case \"dry run\": expected DryRunOperation, got {body}"
    );
    assert_eq!(
        described_configuration(&router, "after the dry run", &instance_id).await,
        before,
        "case \"dry run\": a dry run must write nothing"
    );
    // A dry run whose request would have been REFUSED is refused with that reason instead.
    let (status, body) = modify(
        &router,
        &on(vec![
            p("AccessLogs.S3.Enabled", "true"),
            p("DryRun", "true"),
        ]),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"dry run of a refused request\": expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case \"dry run of a refused request\": expected the refusal's own reason, got {body}"
    );
    assert_eq!(
        stored_configurations(&engine),
        1,
        "only the one configured instance may hold a configuration, got {}",
        stored_configurations(&engine)
    );
}

/// RULE 4: one `ClientToken` names the one update it applied — a repeat applies nothing
/// further — while the same token carrying a different update is refused.
#[tokio::test]
async fn one_client_token_names_one_update() {
    let (router, _engine) = emulator();
    let instance_id = seed_instance(&router, "client token").await;
    let token = "logging-token-1";
    let first = vec![
        p("VerifiedAccessInstanceId", instance_id.clone()),
        p("AccessLogs.S3.Enabled", "true"),
        p("AccessLogs.S3.BucketName", "token-bucket"),
        p("ClientToken", token),
    ];
    let (status, body) = modify(&router, &first).await;
    assert_eq!(
        status, 200,
        "the first update must succeed, got {status} with body {body}"
    );

    // A repeat of the SAME request under the SAME token applies nothing further and reports
    // the configuration as it stands.
    let (status, body) = modify(&router, &first).await;
    assert_eq!(
        status, 200,
        "case \"replay\": expected HTTP 200, got {status} with body {body}"
    );
    let (_, replayed) = one_configuration("replay", &body);
    assert_members(
        "replay",
        &replayed,
        &[("s3/enabled", "true"), ("s3/bucketName", "token-bucket")],
        &[],
    );

    // (case, the differing request sent under the same token).
    let rows: Vec<(&str, Params)> = vec![
        (
            "another bucket",
            vec![
                p("VerifiedAccessInstanceId", instance_id.clone()),
                p("AccessLogs.S3.Enabled", "true"),
                p("AccessLogs.S3.BucketName", "another-bucket"),
                p("ClientToken", token),
            ],
        ),
        (
            "another destination",
            vec![
                p("VerifiedAccessInstanceId", instance_id.clone()),
                p("AccessLogs.CloudWatchLogs.Enabled", "true"),
                p("AccessLogs.CloudWatchLogs.LogGroup", "/aws/another"),
                p("ClientToken", token),
            ],
        ),
        (
            "an added option",
            vec![
                p("VerifiedAccessInstanceId", instance_id.clone()),
                p("AccessLogs.S3.Enabled", "true"),
                p("AccessLogs.S3.BucketName", "token-bucket"),
                p("AccessLogs.IncludeTrustContext", "true"),
                p("ClientToken", token),
            ],
        ),
    ];
    for (case, params) in rows {
        let (status, body) = modify(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for the same token carrying another update, got \
             {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "IdempotentParameterMismatch",
            "case {case:?}: expected IdempotentParameterMismatch, got {body}"
        );
        let reported = described_configuration(&router, case, &instance_id).await;
        assert_members(
            case,
            &reported,
            &[
                ("s3/enabled", "true"),
                ("s3/bucketName", "token-bucket"),
                ("cloudWatchLogs/enabled", "false"),
                ("includeTrustContext", "false"),
            ],
            &[],
        );
    }

    // A token the store has not seen applies an update of its own.
    let (status, body) = modify(
        &router,
        &[
            p("VerifiedAccessInstanceId", instance_id.clone()),
            p("AccessLogs.S3.Enabled", "true"),
            p("AccessLogs.S3.BucketName", "second-token-bucket"),
            p("ClientToken", "logging-token-2"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a fresh token\": expected HTTP 200, got {status} with body {body}"
    );
    let reported = described_configuration(&router, "a fresh token", &instance_id).await;
    assert_members(
        "a fresh token",
        &reported,
        &[("s3/bucketName", "second-token-bucket")],
        &[],
    );
}

/// RULE 5: the describe reports the store narrowed by `VerifiedAccessInstanceIds` and by
/// every filter it answers, with EC2's AND across filters, OR within one filter and `*`
/// wildcards; an id the account holds no instance for is refused, a filter name it does not
/// answer is refused, and a `DryRun` reports the outcome.
#[tokio::test]
async fn the_describe_narrows_by_id_and_by_every_answered_filter() {
    let (router, _engine) = emulator();

    // Three instances whose configurations differ along every attribute the filters select
    // on, and a fourth nothing has configured at all.
    let s3_only = seed_instance(&router, "s3 only").await;
    let logs_only = seed_instance(&router, "cloudWatchLogs only").await;
    let stream_only = seed_instance(&router, "kinesisDataFirehose only").await;
    let unconfigured = seed_instance(&router, "unconfigured").await;

    for (case, params) in [
        (
            "s3 only",
            vec![
                p("VerifiedAccessInstanceId", s3_only.clone()),
                p("AccessLogs.S3.Enabled", "true"),
                p("AccessLogs.S3.BucketName", "filter-bucket"),
                p("AccessLogs.S3.Prefix", "filter/prefix"),
                p("AccessLogs.S3.BucketOwner", "123456789012"),
                p("AccessLogs.LogVersion", "ocsf-0.1"),
            ],
        ),
        (
            "cloudWatchLogs only",
            vec![
                p("VerifiedAccessInstanceId", logs_only.clone()),
                p("AccessLogs.CloudWatchLogs.Enabled", "true"),
                p("AccessLogs.CloudWatchLogs.LogGroup", "/aws/filter-group"),
                p("AccessLogs.LogVersion", "ocsf-1.0.0-rc.2"),
                p("AccessLogs.IncludeTrustContext", "true"),
            ],
        ),
        (
            "kinesisDataFirehose only",
            vec![
                p("VerifiedAccessInstanceId", stream_only.clone()),
                p("AccessLogs.KinesisDataFirehose.Enabled", "true"),
                p(
                    "AccessLogs.KinesisDataFirehose.DeliveryStream",
                    "filter-stream",
                ),
                p("AccessLogs.LogVersion", "ocsf-1.0.0-rc.2"),
            ],
        ),
    ] {
        let (status, body) = modify(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the modify must succeed, got {status} with body {body}"
        );
    }

    let mut all_ids = vec![
        s3_only.clone(),
        logs_only.clone(),
        stream_only.clone(),
        unconfigured.clone(),
    ];
    all_ids.sort();

    // (case, the filters, the instance ids the report must carry, in id order).
    let mut expected_disabled = vec![logs_only.clone(), stream_only.clone(), unconfigured.clone()];
    expected_disabled.sort();
    let mut expected_later_version = vec![logs_only.clone(), stream_only.clone()];
    expected_later_version.sort();
    let rows: Vec<FilterRow> = vec![
        (
            "verified-access-instance-id",
            vec![("verified-access-instance-id", vec![s3_only.as_str()])],
            vec![s3_only.clone()],
        ),
        (
            "s3-enabled true",
            vec![("s3-enabled", vec!["true"])],
            vec![s3_only.clone()],
        ),
        (
            "s3-enabled false",
            vec![("s3-enabled", vec!["false"])],
            expected_disabled.clone(),
        ),
        (
            "s3-bucket-name",
            vec![("s3-bucket-name", vec!["filter-bucket"])],
            vec![s3_only.clone()],
        ),
        (
            "s3-bucket-name by wildcard",
            vec![("s3-bucket-name", vec!["filter-*"])],
            vec![s3_only.clone()],
        ),
        (
            "s3-prefix",
            vec![("s3-prefix", vec!["filter/prefix"])],
            vec![s3_only.clone()],
        ),
        (
            "s3-bucket-owner",
            vec![("s3-bucket-owner", vec!["123456789012"])],
            vec![s3_only.clone()],
        ),
        (
            "cloud-watch-logs-enabled true",
            vec![("cloud-watch-logs-enabled", vec!["true"])],
            vec![logs_only.clone()],
        ),
        (
            "cloud-watch-logs-log-group",
            vec![("cloud-watch-logs-log-group", vec!["/aws/filter-group"])],
            vec![logs_only.clone()],
        ),
        (
            "kinesis-data-firehose-enabled true",
            vec![("kinesis-data-firehose-enabled", vec!["true"])],
            vec![stream_only.clone()],
        ),
        (
            "kinesis-data-firehose-delivery-stream",
            vec![(
                "kinesis-data-firehose-delivery-stream",
                vec!["filter-stream"],
            )],
            vec![stream_only.clone()],
        ),
        (
            "include-trust-context true",
            vec![("include-trust-context", vec!["true"])],
            vec![logs_only.clone()],
        ),
        (
            "log-version",
            vec![("log-version", vec!["ocsf-0.1"])],
            vec![s3_only.clone()],
        ),
        (
            "log-version, OR within one filter",
            vec![("log-version", vec!["ocsf-0.1", "ocsf-1.0.0-rc.2"])],
            {
                let mut ids = vec![s3_only.clone(), logs_only.clone(), stream_only.clone()];
                ids.sort();
                ids
            },
        ),
        (
            "log-version selects nothing on an unconfigured instance",
            vec![("log-version", vec!["ocsf-1.0.0-rc.2"])],
            expected_later_version.clone(),
        ),
        (
            "AND across filters",
            vec![
                ("log-version", vec!["ocsf-1.0.0-rc.2"]),
                ("kinesis-data-firehose-enabled", vec!["true"]),
            ],
            vec![stream_only.clone()],
        ),
        (
            "AND across filters nothing satisfies",
            vec![
                ("s3-enabled", vec!["true"]),
                ("cloud-watch-logs-enabled", vec!["true"]),
            ],
            vec![],
        ),
        (
            "a bucket name no instance carries",
            vec![("s3-bucket-name", vec!["no-such-bucket"])],
            vec![],
        ),
    ];

    for (case, entries, expected) in rows {
        let asked: Vec<(&str, &[&str])> = entries
            .iter()
            .map(|(name, values)| (*name, values.as_slice()))
            .collect();
        let reported = described_ids(&router, case, &filters(&asked)).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // No narrowing at all reports every instance, configured or not.
    assert_eq!(
        described_ids(&router, "no narrowing", &[]).await,
        all_ids,
        "case \"no narrowing\": expected every stored instance"
    );

    // `VerifiedAccessInstanceIds` narrows to the instances it names, and only those.
    let mut two = vec![logs_only.clone(), unconfigured.clone()];
    two.sort();
    assert_eq!(
        described_ids(
            &router,
            "two ids",
            &[
                p("VerifiedAccessInstanceId.1", logs_only.clone()),
                p("VerifiedAccessInstanceId.2", unconfigured.clone()),
            ],
        )
        .await,
        two,
        "case \"two ids\": expected exactly the instances the request named"
    );

    // An id the account holds no instance for is an error, not an absent row.
    for (case, id) in [
        ("a long id nothing minted", "vai-0123456789abcdef0"),
        ("a short id nothing minted", "vai-01234567"),
        ("an id of another resource kind", "subnet-0123456789abcdef0"),
    ] {
        let (status, body) = describe(&router, &[p("VerifiedAccessInstanceId.1", id)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidVerifiedAccessInstanceId.NotFound",
            "case {case:?}: expected the not-found code, got {body}"
        );
    }

    // A filter name the operation does not answer is refused rather than read as "no
    // results".
    for (case, name) in [
        ("a filter of another describe", "fips-enabled"),
        ("a tag filter this report has no tags for", "tag-key"),
        ("a misspelling", "s3-enable"),
        ("a filter differing only in case", "S3-Enabled"),
    ] {
        let (status, body) = describe(&router, &filters(&[(name, &["true"][..])])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The filter '{name}' is invalid"),
            "case {case:?}: expected the describe's own filter wording, got {body}"
        );
    }

    // A DryRun reports the outcome the request would have had.
    let (status, body) = describe(&router, &[p("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "case \"dry run\": expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case \"dry run\": expected DryRunOperation, got {body}"
    );
}

/// RULE 5's paging: `MaxResults` and `NextToken` walk the collection exactly once, whatever
/// page size inside the modelled `range 1..10` the caller asks for; a page size outside that
/// range is refused before the handler runs, and a token this service did not mint is refused.
#[tokio::test]
async fn the_describe_pages_by_max_results_and_next_token() {
    let (router, _engine) = emulator();

    // More instances than the largest page the model admits, so more than one page is needed
    // however the caller asks.
    let mut created = Vec::new();
    for index in 0..12 {
        created.push(seed_instance(&router, &format!("page instance {index}")).await);
    }
    created.sort();

    // Each published bound of the modelled range, and a size between them.
    for page_size in [1usize, 4, 10] {
        let case = format!("MaxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        for _ in 0..created.len() + 1 {
            let mut params = vec![p("MaxResults", page_size.to_string())];
            if let Some(token) = next_token.as_deref() {
                params.push(p("NextToken", token));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = configurations(&case, &body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most MaxResults configurations, got {}",
                page.len()
            );
            walked.extend(page.into_iter().map(|(instance_id, _)| instance_id));
            next_token = element(&body, "nextToken");
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, created,
            "case {case:?}: paging must walk every instance once, in id order"
        );
    }

    // A request naming no page size is answered with the largest page the operation can be
    // asked for, and a token for what follows it.
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "case \"no MaxResults\": expected HTTP 200, got {status} with body {body}"
    );
    let page = configurations("no MaxResults", &body);
    assert_eq!(
        page.len(),
        10,
        "case \"no MaxResults\": expected the largest page the operation serves, got {}",
        page.len()
    );
    assert!(
        element(&body, "nextToken").is_some(),
        "case \"no MaxResults\": a page that does not hold everything carries a token, got \
         {body}"
    );

    // One past each published bound of the modelled `@range`: refused at the boundary, before
    // the handler runs.
    for max_results in ["0", "11"] {
        let case = format!("MaxResults={max_results}");
        let (status, body) = describe(&router, &[p("MaxResults", max_results)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a MaxResults outside the modelled range, got \
             {status} with body {body}"
        );
    }

    // A token this service did not issue is refused rather than read as a cursor.
    for (case, token) in [
        ("a token that is not base64", "not-a-token"),
        ("an empty token", ""),
        ("a token naming an instance", "vai-0123456789abcdef0"),
    ] {
        let (status, body) = describe(&router, &[p("NextToken", token)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {case:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}
