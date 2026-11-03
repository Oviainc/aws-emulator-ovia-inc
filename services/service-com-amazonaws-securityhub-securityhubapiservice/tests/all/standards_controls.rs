// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `DescribeStandardsControls` and
//! `UpdateStandardsControl`: the controls a standards subscription of the caller's hub
//! publishes, the status that subscription holds for each of them, and the refusals a
//! request outside the rule is answered with.
//!
//! Every case drives the real restJson1 wire path through `MultiServiceRouter`, so a
//! route, a status code or a serialized member that is wrong on the wire fails here.

use std::collections::BTreeSet;
use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every emulated caller presents.
const ACCOUNT_ID: &str = "000000000000";

/// The Region most cases sign for.
const REGION: &str = "us-east-1";

/// A second Region, so "a subscription belongs to one account AND Region" is
/// observable.
const OTHER_REGION: &str = "eu-west-1";

/// Longer than the `PENDING` -> `READY` transition's 500 ms due span, so a read after
/// this wait must see the transition materialised.
const PAST_READY_DUE: std::time::Duration = std::time::Duration::from_millis(900);

/// The `SeverityRating` values the model declares (`smithy_query(query="shape",
/// shape="SeverityRating")`), which is what every reported control's severity must be
/// one of.
const SEVERITY_RATINGS: [&str; 4] = ["LOW", "MEDIUM", "HIGH", "CRITICAL"];

/// One published standard, and the controls this emulator publishes for it.
///
/// Source: the AWS Security Hub user guide's "Security Hub standards reference" (the
/// `standards/<slug>/v/<version>` ARNs) and its controls reference (the `ControlId`
/// each standard addresses a control by). The two revisions that predate consolidated
/// control findings — CIS AWS Foundations Benchmark v1.2.0 and PCI DSS v3.2.1 —
/// address a control by their own prefixed requirement id (`CIS.`/`PCI.`), which is
/// the form the `StandardsControlArn` example in this service's `smithy-model/`
/// carries; every other standard addresses the security control by the control's own
/// id.
struct StandardRow {
    /// The `<slug>` and `<version>` of `standards/<slug>/v/<version>`.
    slug: &'static str,
    version: &'static str,
    /// The partition-wide `ruleset/` ARN form the CIS AWS Foundations Benchmark
    /// v1.2.0 standard kept, which carries neither Region nor account.
    partition_wide_ruleset: bool,
    /// One control id this standard must report, chosen so each row pins a different
    /// id form.
    expected_control_id: &'static str,
    /// Every control id this standard reports, in the published order — written out
    /// only for the rows where this suite pins the whole set.
    every_control_id: &'static [&'static str],
}

/// Every standard of the published catalog, so no enabled standard is left without
/// coverage: each one must report controls.
const STANDARDS: [StandardRow; 13] = [
    StandardRow {
        slug: "ai-security-best-practices",
        version: "1.0.0",
        partition_wide_ruleset: false,
        expected_control_id: "Bedrock.1",
        every_control_id: &["Bedrock.1", "SageMaker.1", "SageMaker.2", "SageMaker.3"],
    },
    StandardRow {
        slug: "aws-foundational-security-best-practices",
        version: "1.0.0",
        partition_wide_ruleset: false,
        expected_control_id: "CloudTrail.1",
        every_control_id: &[
            "ACM.1",
            "CloudTrail.1",
            "CloudTrail.2",
            "CloudWatch.1",
            "Config.1",
            "EC2.2",
            "GuardDuty.1",
            "IAM.4",
            "S3.1",
        ],
    },
    StandardRow {
        slug: "aws-resource-tagging-standard",
        version: "1.0.0",
        partition_wide_ruleset: false,
        expected_control_id: "EC2.52",
        every_control_id: &["EC2.52", "ECS.13"],
    },
    StandardRow {
        slug: "azure-foundational-security-best-practices",
        version: "1.0.0",
        partition_wide_ruleset: false,
        expected_control_id: "AzureStorage.1",
        every_control_id: &[],
    },
    StandardRow {
        slug: "cis-aws-foundations-benchmark",
        version: "1.2.0",
        partition_wide_ruleset: true,
        expected_control_id: "CIS.1.1",
        every_control_id: &[
            "CIS.1.1",
            "CIS.1.2",
            "CIS.1.3",
            "CIS.1.4",
            "CIS.1.12",
            "CIS.1.13",
            "CIS.2.1",
            "CIS.2.3",
            "CIS.2.5",
            "CIS.4.1",
            "CIS.4.2",
        ],
    },
    StandardRow {
        slug: "cis-aws-foundations-benchmark",
        version: "1.4.0",
        partition_wide_ruleset: false,
        expected_control_id: "CloudWatch.1",
        every_control_id: &[],
    },
    StandardRow {
        slug: "cis-aws-foundations-benchmark",
        version: "3.0.0",
        partition_wide_ruleset: false,
        expected_control_id: "KMS.4",
        every_control_id: &[],
    },
    StandardRow {
        slug: "cis-aws-foundations-benchmark",
        version: "5.0.0",
        partition_wide_ruleset: false,
        expected_control_id: "EC2.13",
        every_control_id: &[],
    },
    StandardRow {
        slug: "cis-azure-foundations-benchmark",
        version: "4.0.0",
        partition_wide_ruleset: false,
        expected_control_id: "AzureVirtualMachine.1",
        every_control_id: &[],
    },
    StandardRow {
        slug: "nist-800-171",
        version: "2.0.0",
        partition_wide_ruleset: false,
        expected_control_id: "Config.1",
        every_control_id: &[],
    },
    StandardRow {
        slug: "nist-800-53",
        version: "5.0.0",
        partition_wide_ruleset: false,
        expected_control_id: "ACM.1",
        every_control_id: &[],
    },
    StandardRow {
        slug: "pci-dss",
        version: "3.2.1",
        partition_wide_ruleset: false,
        expected_control_id: "PCI.AutoScaling.1",
        every_control_id: &[
            "PCI.AutoScaling.1",
            "PCI.CloudTrail.1",
            "PCI.CloudTrail.2",
            "PCI.IAM.1",
            "PCI.S3.1",
        ],
    },
    StandardRow {
        slug: "pci-dss",
        version: "4.0.1",
        partition_wide_ruleset: false,
        expected_control_id: "S3.5",
        every_control_id: &[],
    },
];

impl StandardRow {
    /// The ARN of the standard itself, which `BatchEnableStandards` names it by.
    fn standards_arn(&self, region: &str) -> String {
        if self.partition_wide_ruleset {
            format!(
                "arn:aws:securityhub:::ruleset/{}/v/{}",
                self.slug, self.version
            )
        } else {
            format!(
                "arn:aws:securityhub:{region}::standards/{}/v/{}",
                self.slug, self.version
            )
        }
    }

    /// The ARN of the caller's subscription to this standard.
    fn subscription_arn(&self, region: &str) -> String {
        format!(
            "arn:aws:securityhub:{region}:{ACCOUNT_ID}:subscription/{}/v/{}",
            self.slug, self.version
        )
    }

    /// The ARN of one control of the caller's subscription to this standard.
    fn control_arn(&self, region: &str, control_id: &str) -> String {
        format!(
            "arn:aws:securityhub:{region}:{ACCOUNT_ID}:control/{}/v/{}/{control_id}",
            self.slug, self.version
        )
    }

    fn case(&self) -> String {
        format!("{}/v/{}", self.slug, self.version)
    }
}

/// The `aws-foundational-security-best-practices` row: one of the two standards
/// `EnableSecurityHub` enables by default, and the one whose nine controls the paging
/// cases walk.
fn fsbp() -> &'static StandardRow {
    &STANDARDS[1]
}

/// One emulator with the Security Hub service mounted, driven over HTTP.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_securityhub_securityhubapiservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Dispatch one request signed for `region` and return `(status, parsed body)`.
    /// An empty body parses as `Value::Null`, which is what an operation with no
    /// output members returns.
    async fn call(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let serialized = body.map(|b| serde_json::to_vec(&b).expect("serializable request body"));
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{region}/securityhub/aws4_request, \
                     SignedHeaders=host, Signature=0000"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(serialized.unwrap_or_default()))
            .expect("valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let parsed = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    async fn enable_in(&self, region: &str, body: Value) -> (u16, Value) {
        self.call(region, "POST", "/accounts", Some(body)).await
    }

    async fn enable_standards_in(&self, region: &str, standards_arns: &[String]) -> (u16, Value) {
        let requests: Vec<Value> = standards_arns
            .iter()
            .map(|arn| json!({ "StandardsArn": arn }))
            .collect();
        self.call(
            region,
            "POST",
            "/standards/register",
            Some(json!({ "StandardsSubscriptionRequests": requests })),
        )
        .await
    }

    async fn describe_controls_in(
        &self,
        region: &str,
        subscription_arn: &str,
        query: &str,
    ) -> (u16, Value) {
        let path = if query.is_empty() {
            format!("/standards/controls/{subscription_arn}")
        } else {
            format!("/standards/controls/{subscription_arn}?{query}")
        };
        self.call(region, "GET", &path, None).await
    }

    async fn describe_controls(&self, subscription_arn: &str) -> (u16, Value) {
        self.describe_controls_in(REGION, subscription_arn, "").await
    }

    async fn update_control_in(
        &self,
        region: &str,
        control_arn: &str,
        body: Value,
    ) -> (u16, Value) {
        self.call(
            region,
            "PATCH",
            &format!("/standards/control/{control_arn}"),
            Some(body),
        )
        .await
    }

    async fn update_control(&self, control_arn: &str, body: Value) -> (u16, Value) {
        self.update_control_in(REGION, control_arn, body).await
    }

    /// A harness whose `REGION` hub is subscribed with the default standards, waited
    /// past the `PENDING` -> `READY` transition the enabling call owes.
    async fn enabled_and_ready() -> Self {
        let harness = Self::new();
        let (status, body) = harness.enable_in(REGION, json!({})).await;
        assert_eq!(
            status, 200,
            "EnableSecurityHub must succeed, got {status} with {body}"
        );
        tokio::time::sleep(PAST_READY_DUE).await;
        harness
    }

    /// A harness whose `REGION` hub has every published standard enabled and ready.
    async fn every_standard_ready() -> Self {
        let harness = Self::new();
        let (status, body) = harness.enable_in(REGION, json!({})).await;
        assert_eq!(
            status, 200,
            "EnableSecurityHub must succeed, got {status} with {body}"
        );
        // `StandardsSubscriptionRequests` is `len 1..25`, so the catalog fits in one
        // batch.
        let arns: Vec<String> = STANDARDS
            .iter()
            .map(|standard| standard.standards_arn(REGION))
            .collect();
        let (status, body) = harness.enable_standards_in(REGION, &arns).await;
        assert_eq!(
            status, 200,
            "BatchEnableStandards must succeed, got {status} with {body}"
        );
        tokio::time::sleep(PAST_READY_DUE).await;
        harness
    }
}

/// The `Controls` of a `DescribeStandardsControls` response, in the order the
/// response listed them.
fn returned_controls(body: &Value) -> Vec<Value> {
    body["Controls"]
        .as_array()
        .unwrap_or_else(|| panic!("Controls must be a list: {body}"))
        .clone()
}

/// The `ControlId`s of a `DescribeStandardsControls` response, in response order.
fn returned_control_ids(body: &Value) -> Vec<String> {
    returned_controls(body)
        .iter()
        .map(|control| {
            control["ControlId"]
                .as_str()
                .unwrap_or_else(|| panic!("every control carries a ControlId: {control}"))
                .to_string()
        })
        .collect()
}

/// One control of a response, by `ControlId`.
fn control_by_id(body: &Value, control_id: &str) -> Value {
    returned_controls(body)
        .into_iter()
        .find(|control| control["ControlId"].as_str() == Some(control_id))
        .unwrap_or_else(|| panic!("case {control_id:?}: expected the control, got {body}"))
}

/// The error code of a refusal body, whichever casing the envelope uses.
fn error_code(body: &Value) -> Option<&str> {
    body["Code"].as_str().or_else(|| body["code"].as_str())
}

/// The instant `ControlStatusUpdatedAt` carries, after asserting its WIRE FORM:
/// `StandardsControl$ControlStatusUpdatedAt` targets `Timestamp`, which the model marks
/// `@timestampFormat("date-time")`, so it travels as an RFC 3339 string and never as an
/// epoch number.
fn control_status_updated_at(case: &str, control: &Value) -> DateTime {
    let value = &control["ControlStatusUpdatedAt"];
    let text = value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: ControlStatusUpdatedAt is modelled @timestampFormat(\"date-time\"), \
             so it must be an RFC 3339 string, got {value:?}"
        )
    });
    let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: ControlStatusUpdatedAt must be a readable instant, got {text:?}: {error}")
    });
    assert!(
        parsed.secs() > 0,
        "case {case:?}: ControlStatusUpdatedAt must name an instant after the epoch, got {text:?}"
    );
    parsed
}

/// RULE: an enabled standard reports the controls it publishes, each one addressed by
/// `arn:aws:securityhub:<region>:<account>:control/<slug>/v/<version>/<ControlId>` and
/// described with the metadata the controls reference publishes for it.
#[tokio::test]
async fn every_enabled_standard_reports_the_controls_it_publishes() {
    let harness = Harness::every_standard_ready().await;

    for standard in &STANDARDS {
        let case = standard.case();
        let (status, body) = harness
            .describe_controls(&standard.subscription_arn(REGION))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStandardsControls must succeed, got {status} with {body}"
        );
        let controls = returned_controls(&body);
        assert!(
            !controls.is_empty(),
            "case {case:?}: a READY subscription reports the controls of its standard, got {body}"
        );
        assert!(
            body["NextToken"].is_null(),
            "case {case:?}: a complete result carries no NextToken, got {body}"
        );

        let ids = returned_control_ids(&body);
        assert!(
            ids.contains(&standard.expected_control_id.to_string()),
            "case {case:?}: expected the published control {:?} among {ids:?}",
            standard.expected_control_id
        );
        if !standard.every_control_id.is_empty() {
            assert_eq!(
                ids, standard.every_control_id,
                "case {case:?}: expected {:?}, got {ids:?}",
                standard.every_control_id
            );
        }
        assert_eq!(
            ids.iter().collect::<BTreeSet<_>>().len(),
            ids.len(),
            "case {case:?}: a control is reported once, got {ids:?}"
        );

        for control in &controls {
            let control_id = control["ControlId"].as_str().unwrap_or("");
            let expected_arn = standard.control_arn(REGION, control_id);
            assert_eq!(
                control["StandardsControlArn"].as_str(),
                Some(expected_arn.as_str()),
                "case {case:?}/{control_id}: expected ARN {expected_arn}, got {control}"
            );
            assert_eq!(
                control["ControlStatus"].as_str(),
                Some("ENABLED"),
                "case {case:?}/{control_id}: enabling a standard enables its controls, got {control}"
            );
            assert!(
                control["DisabledReason"].is_null(),
                "case {case:?}/{control_id}: an enabled control reports no reason, got {control}"
            );
            assert!(
                control["ControlStatusUpdatedAt"].is_string(),
                "case {case:?}/{control_id}: expected the instant the status was set, got {control}"
            );
            control_status_updated_at(&format!("{case}/{control_id}"), control);
            for member in ["Title", "Description", "RemediationUrl", "SeverityRating"] {
                let value = control[member].as_str().unwrap_or("");
                assert!(
                    !value.trim().is_empty(),
                    "case {case:?}/{control_id}: expected a non-empty {member}, got {control}"
                );
            }
            let severity = control["SeverityRating"].as_str().unwrap_or("");
            assert!(
                SEVERITY_RATINGS.contains(&severity),
                "case {case:?}/{control_id}: expected one of {SEVERITY_RATINGS:?}, got {severity:?}"
            );
            let remediation_url = control["RemediationUrl"].as_str().unwrap_or("");
            assert!(
                remediation_url.ends_with(&format!("/{control_id}/remediation")),
                "case {case:?}/{control_id}: the remediation link names the control, got {remediation_url:?}"
            );
        }
    }
}

/// RULE: reporting the controls changes nothing a later caller sees — two reads of the
/// same subscription answer the same controls, with the same status instants.
#[tokio::test]
async fn reading_the_controls_twice_answers_the_same_controls() {
    let harness = Harness::enabled_and_ready().await;
    let subscription = fsbp().subscription_arn(REGION);

    let (status, first) = harness.describe_controls(&subscription).await;
    assert_eq!(
        status, 200,
        "DescribeStandardsControls must succeed, got {status} with {first}"
    );
    let (status, second) = harness.describe_controls(&subscription).await;
    assert_eq!(
        status, 200,
        "the second read must succeed, got {status} with {second}"
    );
    assert_eq!(
        first, second,
        "a read must not change what a later caller sees: expected {first}, got {second}"
    );
}

/// RULE: a standard whose controls are not updatable yet reports an empty control
/// list — "this operation returns an empty list for standard subscriptions where
/// StandardsControlsUpdatable has value NOT_READY_FOR_UPDATES" — and reports them once
/// the `PENDING` -> `READY` transition the enabling call owes falls due.
#[tokio::test]
async fn a_subscription_whose_controls_are_not_updatable_reports_none() {
    let harness = Harness::new();
    let (status, body) = harness.enable_in(REGION, json!({})).await;
    assert_eq!(
        status, 200,
        "EnableSecurityHub must succeed, got {status} with {body}"
    );
    let subscription = fsbp().subscription_arn(REGION);

    let (status, body) = harness.describe_controls(&subscription).await;
    assert_eq!(
        status, 200,
        "a PENDING subscription is reported, not refused, got {status} with {body}"
    );
    assert_eq!(
        returned_controls(&body).len(),
        0,
        "a subscription whose controls are NOT_READY_FOR_UPDATES reports none, got {body}"
    );

    tokio::time::sleep(PAST_READY_DUE).await;
    let (status, body) = harness.describe_controls(&subscription).await;
    assert_eq!(
        status, 200,
        "DescribeStandardsControls must succeed, got {status} with {body}"
    );
    assert!(
        !returned_controls(&body).is_empty(),
        "a READY subscription reports its controls, got {body}"
    );
}

/// RULE: `UpdateStandardsControl` sets the status of the one control its ARN names,
/// and a later read reports that status, the reason a disable carried, and the instant
/// of the change — leaving every other control of the subscription as it was.
#[tokio::test]
async fn updating_a_control_round_trips_through_describe() {
    let harness = Harness::enabled_and_ready().await;
    let standard = fsbp();
    let subscription = standard.subscription_arn(REGION);

    // One row per control and transition, so the write is exercised on different
    // controls, with reasons of different length and shape, and in both directions.
    let cases: Vec<(&str, &str, Value, &str, Option<&str>)> = vec![
        (
            "disable with a short reason",
            "CloudTrail.1",
            json!({ "ControlStatus": "DISABLED", "DisabledReason": "Not applicable" }),
            "DISABLED",
            Some("Not applicable"),
        ),
        (
            "disable another control with a sentence",
            "S3.1",
            json!({
                "ControlStatus": "DISABLED",
                "DisabledReason": "Public access is governed by the organization's service control policies."
            }),
            "DISABLED",
            Some("Public access is governed by the organization's service control policies."),
        ),
        (
            "disable a control whose severity is CRITICAL",
            "IAM.4",
            json!({ "ControlStatus": "DISABLED", "DisabledReason": "Tracked outside Security Hub" }),
            "DISABLED",
            Some("Tracked outside Security Hub"),
        ),
        (
            "re-enable a disabled control, which drops the reason",
            "CloudTrail.1",
            json!({ "ControlStatus": "ENABLED" }),
            "ENABLED",
            None,
        ),
        (
            "re-enable a control that carries a reason in the request",
            "S3.1",
            json!({ "ControlStatus": "ENABLED", "DisabledReason": "ignored on an enable" }),
            "ENABLED",
            None,
        ),
        (
            "enable a control that was never disabled",
            "GuardDuty.1",
            json!({ "ControlStatus": "ENABLED" }),
            "ENABLED",
            None,
        ),
    ];

    for (case, control_id, request, expected_status, expected_reason) in cases {
        let control_arn = standard.control_arn(REGION, control_id);
        // The instant the control's status carried before this call, so the change
        // this call makes is observable as a later instant.
        let (_, before) = harness.describe_controls(&subscription).await;
        let was_updated_at = control_status_updated_at(case, &control_by_id(&before, control_id));
        let (status, body) = harness.update_control(&control_arn, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateStandardsControl must succeed, got {status} with {body}"
        );

        let (status, read_back) = harness.describe_controls(&subscription).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStandardsControls must succeed, got {status} with {read_back}"
        );
        let control = control_by_id(&read_back, control_id);
        assert_eq!(
            control["ControlStatus"].as_str(),
            Some(expected_status),
            "case {case:?}: expected ControlStatus {expected_status}, got {control}"
        );
        assert_eq!(
            control["DisabledReason"].as_str(),
            expected_reason,
            "case {case:?}: expected DisabledReason {expected_reason:?}, got {control}"
        );
        assert!(
            control["ControlStatusUpdatedAt"].is_string(),
            "case {case:?}: the change carries the instant it was made, got {control}"
        );
        let now_updated_at = control_status_updated_at(case, &control);
        assert!(
            now_updated_at >= was_updated_at,
            "case {case:?}: the status instant must move with the change, expected at or after \
             {was_updated_at:?}, got {now_updated_at:?}"
        );

        // A control the request did not name keeps the status it had.
        let untouched = control_by_id(&read_back, "Config.1");
        assert_eq!(
            untouched["ControlStatus"].as_str(),
            Some("ENABLED"),
            "case {case:?}: a control the call did not name is unchanged, got {untouched}"
        );
    }
}

/// RULE: a control's status belongs to the subscription it was set on — the same
/// control of another standard, and of another Region's subscription, is untouched.
#[tokio::test]
async fn a_control_status_belongs_to_one_subscription() {
    let harness = Harness::every_standard_ready().await;
    let (status, body) = harness.enable_in(OTHER_REGION, json!({})).await;
    assert_eq!(
        status, 200,
        "EnableSecurityHub in {OTHER_REGION} must succeed, got {status} with {body}"
    );
    tokio::time::sleep(PAST_READY_DUE).await;

    // `CloudTrail.1` is published by several standards, so disabling it on one says
    // nothing about the others.
    let fsbp = fsbp();
    let nist = &STANDARDS[10];
    let (status, body) = harness
        .update_control(
            &fsbp.control_arn(REGION, "CloudTrail.1"),
            json!({ "ControlStatus": "DISABLED", "DisabledReason": "Covered elsewhere" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "UpdateStandardsControl must succeed, got {status} with {body}"
    );

    let cases: Vec<(&str, &str, String, &str)> = vec![
        (
            "the subscription the disable was made on",
            REGION,
            fsbp.subscription_arn(REGION),
            "DISABLED",
        ),
        (
            "the same control of another standard",
            REGION,
            nist.subscription_arn(REGION),
            "ENABLED",
        ),
        (
            "the same control of another Region's subscription",
            OTHER_REGION,
            fsbp.subscription_arn(OTHER_REGION),
            "ENABLED",
        ),
    ];

    for (case, region, subscription, expected_status) in cases {
        let (status, body) = harness
            .describe_controls_in(region, &subscription, "")
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStandardsControls must succeed, got {status} with {body}"
        );
        let control = control_by_id(&body, "CloudTrail.1");
        assert_eq!(
            control["ControlStatus"].as_str(),
            Some(expected_status),
            "case {case:?}: expected ControlStatus {expected_status}, got {control}"
        );
    }
}

/// RULE: a disable must say why — `DisabledReason` "is required" when the status is
/// `DISABLED` — and the refusal names the control the request addressed.
#[tokio::test]
async fn disabling_a_control_without_a_reason_is_refused() {
    let harness = Harness::enabled_and_ready().await;
    let standard = fsbp();

    // One row per form a missing reason can take, on different controls, plus the
    // forms the modelled input refuses on its own (`DisabledReason` is a string that
    // must match `\S`).
    let cases: Vec<(&str, &str, Value, bool)> = vec![
        (
            "no DisabledReason member at all",
            "CloudTrail.1",
            json!({ "ControlStatus": "DISABLED" }),
            true,
        ),
        (
            "no DisabledReason member, on another control",
            "S3.1",
            json!({ "ControlStatus": "DISABLED" }),
            true,
        ),
        (
            "DisabledReason null",
            "IAM.4",
            json!({ "ControlStatus": "DISABLED", "DisabledReason": Value::Null }),
            false,
        ),
        (
            "DisabledReason blank",
            "GuardDuty.1",
            json!({ "ControlStatus": "DISABLED", "DisabledReason": "   " }),
            false,
        ),
    ];

    for (case, control_id, request, refused_by_this_operation) in cases {
        let control_arn = standard.control_arn(REGION, control_id);
        let (status, body) = harness.update_control(&control_arn, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        if refused_by_this_operation {
            assert_eq!(
                error_code(&body),
                Some("InvalidInputException"),
                "case {case:?}: expected InvalidInputException, got {body}"
            );
            let expected =
                format!("DisabledReason must be given when disabling control: {control_arn}");
            assert_eq!(
                body["Message"].as_str().or_else(|| body["message"].as_str()),
                Some(expected.as_str()),
                "case {case:?}: expected message {expected:?}, got {body}"
            );
        }

        // The refused call wrote nothing: the control is still enabled.
        let (_, read_back) = harness
            .describe_controls(&standard.subscription_arn(REGION))
            .await;
        let control = control_by_id(&read_back, control_id);
        assert_eq!(
            control["ControlStatus"].as_str(),
            Some("ENABLED"),
            "case {case:?}: a refused disable changes nothing, got {control}"
        );
    }
}

/// RULE: `MaxResults` bounds the page and `NextToken` resumes the same published
/// order, with no control repeated or skipped at any page size.
#[tokio::test]
async fn max_results_bounds_the_page_and_next_token_resumes_it() {
    let harness = Harness::enabled_and_ready().await;
    let subscription = fsbp().subscription_arn(REGION);
    let (_, body) = harness.describe_controls(&subscription).await;
    let all = returned_control_ids(&body);
    assert!(
        all.len() > 1,
        "the paging cases need more than one control: got {all:?}"
    );

    // One row per page size: below the result count, around it, above it, and the
    // published maximum.
    for page_size in [1_u64, 2, 4, all.len() as u64, all.len() as u64 + 1, 100] {
        let mut collected: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = format!("MaxResults={page_size}");
            if let Some(token) = next_token.clone() {
                query.push_str(&format!("&NextToken={token}"));
            }
            let (status, body) = harness
                .describe_controls_in(REGION, &subscription, &query)
                .await;
            assert_eq!(
                status, 200,
                "case MaxResults={page_size}: must succeed, got {status} with {body}"
            );
            let page = returned_control_ids(&body);
            assert!(
                page.len() as u64 <= page_size,
                "case MaxResults={page_size}: a page may not exceed the page size, got {page:?}"
            );
            collected.extend(page);
            pages += 1;
            next_token = body["NextToken"].as_str().map(str::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= all.len(),
                "case MaxResults={page_size}: paging must terminate, got {collected:?}"
            );
        }
        assert_eq!(
            collected, all,
            "case MaxResults={page_size}: expected {all:?} across pages, got {collected:?}"
        );
        let expected_pages = (all.len() as u64).div_ceil(page_size);
        assert_eq!(
            pages as u64, expected_pages,
            "case MaxResults={page_size}: expected {expected_pages} page(s), got {pages}"
        );
    }
}

/// RULE: a request outside the modelled input is refused 400 — a `MaxResults` beyond
/// the published range, a cursor this list never issued, and a `ControlStatus` the
/// model does not declare — while each published `MaxResults` bound is accepted.
#[tokio::test]
async fn requests_outside_the_modelled_input_are_refused() {
    let harness = Harness::enabled_and_ready().await;
    let standard = fsbp();
    let subscription = standard.subscription_arn(REGION);

    // `MaxResults` is `@range(min: 1, max: 100)`; `NextToken` is opaque, so only a
    // token this operation issued resumes a page.
    let describe_cases: Vec<(&str, String)> = vec![
        (
            "MaxResults below the published minimum",
            "MaxResults=0".to_string(),
        ),
        (
            "MaxResults above the published maximum",
            "MaxResults=101".to_string(),
        ),
        ("MaxResults negative", "MaxResults=-1".to_string()),
        (
            "a NextToken this operation never issued",
            "NextToken=not-a-cursor".to_string(),
        ),
        (
            "a NextToken another list issued",
            format!(
                "NextToken={}",
                hex::encode("securityhub:enabled-standards:1")
            ),
        ),
        (
            "a NextToken past the end of the result set",
            format!(
                "NextToken={}",
                hex::encode("securityhub:standards-controls:99")
            ),
        ),
    ];
    for (case, query) in describe_cases {
        let (status, body) = harness
            .describe_controls_in(REGION, &subscription, &query)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
    }

    // Each published `MaxResults` bound is accepted.
    for page_size in [1, 100] {
        let (status, body) = harness
            .describe_controls_in(REGION, &subscription, &format!("MaxResults={page_size}"))
            .await;
        assert_eq!(
            status, 200,
            "case MaxResults={page_size}: a published bound must be accepted, got {status} with {body}"
        );
    }

    // `ControlStatus` targets the `ControlStatus` enum (`ENABLED`, `DISABLED`).
    let control_arn = standard.control_arn(REGION, "CloudTrail.1");
    for status_value in ["enabled", "SUSPENDED", ""] {
        let (status, body) = harness
            .update_control(
                &control_arn,
                json!({ "ControlStatus": status_value, "DisabledReason": "a reason" }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case ControlStatus={status_value:?}: expected HTTP 400, got {status} with {body}"
        );
    }
}

/// RULE: an ARN that names no resource of the caller's hub is 404
/// `ResourceNotFoundException`, and a caller that is not subscribed at all is 401
/// `InvalidAccessException`.
#[tokio::test]
async fn an_arn_that_names_no_resource_of_the_hub_is_refused() {
    let harness = Harness::enabled_and_ready().await;
    let standard = fsbp();
    let not_enabled = &STANDARDS[12];

    // Each row is an ARN that resolves to no subscription of this hub: a standard the
    // account has not enabled, another Region's subscription, a subscription of
    // another account, and a value that is not one of this service's ARNs at all.
    let describe_cases: Vec<(&str, String)> = vec![
        (
            "a standard the account has not enabled",
            not_enabled.subscription_arn(REGION),
        ),
        (
            "another Region's subscription",
            standard.subscription_arn(OTHER_REGION),
        ),
        (
            "another account's subscription",
            standard
                .subscription_arn(REGION)
                .replace(ACCOUNT_ID, "999999999999"),
        ),
        (
            "the ARN of the standard rather than of the subscription",
            standard.standards_arn(REGION),
        ),
        ("not an ARN at all", "nope".to_string()),
    ];
    for (case, subscription_arn) in describe_cases {
        let (status, body) = harness.describe_controls(&subscription_arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        let expected = format!("Given Arn: {subscription_arn} does not exist");
        assert_eq!(
            body["Message"].as_str().or_else(|| body["message"].as_str()),
            Some(expected.as_str()),
            "case {case:?}: expected message {expected:?}, got {body}"
        );
    }

    // The same for a control ARN: a control id the standard does not publish, a
    // standard the account has not enabled, and another Region's control.
    let update_cases: Vec<(&str, String)> = vec![
        (
            "a control id the standard does not publish",
            standard.control_arn(REGION, "CloudTrail.999"),
        ),
        (
            "a control of a standard the account has not enabled",
            not_enabled.control_arn(REGION, "S3.5"),
        ),
        (
            "another Region's control",
            standard.control_arn(OTHER_REGION, "CloudTrail.1"),
        ),
        ("not an ARN at all", "nope".to_string()),
    ];
    for (case, control_arn) in update_cases {
        let (status, body) = harness
            .update_control(&control_arn, json!({ "ControlStatus": "ENABLED" }))
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }

    // An account that never subscribed has no hub to read or write.
    let fresh = Harness::new();
    let (status, body) = fresh
        .describe_controls(&standard.subscription_arn(REGION))
        .await;
    assert_eq!(
        status, 401,
        "an unsubscribed account must be refused, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        Some("InvalidAccessException"),
        "an unsubscribed account is refused with InvalidAccessException, got {body}"
    );
    let (status, body) = fresh
        .update_control(
            &standard.control_arn(REGION, "CloudTrail.1"),
            json!({ "ControlStatus": "ENABLED" }),
        )
        .await;
    assert_eq!(
        status, 401,
        "an unsubscribed account must be refused, got {status} with {body}"
    );
}

/// RULE: a control is addressable only while its standard's controls are updatable —
/// a call against a subscription that is still being enabled is refused 404, as
/// `UpdateStandardsControl` documents for `NOT_READY_FOR_UPDATES`.
#[tokio::test]
async fn updating_a_control_of_a_pending_subscription_is_refused() {
    let harness = Harness::new();
    let (status, body) = harness.enable_in(REGION, json!({})).await;
    assert_eq!(
        status, 200,
        "EnableSecurityHub must succeed, got {status} with {body}"
    );
    let standard = fsbp();
    let control_arn = standard.control_arn(REGION, "CloudTrail.1");

    let (status, body) = harness
        .update_control(
            &control_arn,
            json!({ "ControlStatus": "DISABLED", "DisabledReason": "too early" }),
        )
        .await;
    assert_eq!(
        status, 404,
        "a control of a NOT_READY_FOR_UPDATES subscription is not addressable, got {status} with {body}"
    );

    tokio::time::sleep(PAST_READY_DUE).await;
    let (status, body) = harness
        .update_control(
            &control_arn,
            json!({ "ControlStatus": "DISABLED", "DisabledReason": "now updatable" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "once the subscription is READY the control is addressable, got {status} with {body}"
    );
    let (_, read_back) = harness
        .describe_controls(&standard.subscription_arn(REGION))
        .await;
    let control = control_by_id(&read_back, "CloudTrail.1");
    assert_eq!(
        control["ControlStatus"].as_str(),
        Some("DISABLED"),
        "the accepted disable must persist, got {control}"
    );
}
