// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `BatchEnableStandards`: every `StandardsArn` of the
//! published standards catalog subscribes the caller's account/Region to that
//! standard, an ARN the catalog does not carry is refused with
//! `InvalidInputException`, and a standard already enabled stays one subscription.
//!
//! Every case drives the real restJson1 wire path through `MultiServiceRouter`, so a
//! route, a status code or a serialized member that is wrong on the wire fails here.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every emulated caller presents.
const ACCOUNT_ID: &str = "000000000000";

/// The Region most cases sign for.
const REGION: &str = "us-east-1";

/// A second Region, so "one subscription per account AND Region" is observable.
const OTHER_REGION: &str = "eu-west-1";

/// Longer than the `PENDING` -> `READY` transition's 500 ms due span, so a read after
/// this wait must see the transition materialised.
const PAST_READY_DUE: std::time::Duration = std::time::Duration::from_millis(900);

/// One standard of the catalog, as a case drives it: the `<slug>` and `<version>` its
/// ARNs carry, the `StandardsProvider` whose resources it evaluates, and whether the
/// standard keeps the partition-wide `ruleset/` ARN form.
struct CatalogEntry {
    slug: &'static str,
    version: &'static str,
    provider: &'static str,
    partition_wide_ruleset: bool,
}

/// The standards this emulator publishes, from `crate::state::STANDARDS_CATALOG` (the
/// AWS Security Hub user guide's "Security Hub standards reference"), which is also
/// what `DescribeStandards` reports. Every entry is driven below, so a standard a
/// caller can read out of the catalog is a standard it can enable — including the two
/// the issue never cited and the one partition-wide `ruleset/` ARN.
const CATALOG: [CatalogEntry; 13] = [
    CatalogEntry {
        slug: "ai-security-best-practices",
        version: "1.0.0",
        provider: "AWS",
        partition_wide_ruleset: false,
    },
    CatalogEntry {
        slug: "aws-foundational-security-best-practices",
        version: "1.0.0",
        provider: "AWS",
        partition_wide_ruleset: false,
    },
    CatalogEntry {
        slug: "aws-resource-tagging-standard",
        version: "1.0.0",
        provider: "AWS",
        partition_wide_ruleset: false,
    },
    CatalogEntry {
        slug: "azure-foundational-security-best-practices",
        version: "1.0.0",
        provider: "Azure",
        partition_wide_ruleset: false,
    },
    CatalogEntry {
        slug: "cis-aws-foundations-benchmark",
        version: "1.2.0",
        provider: "AWS",
        partition_wide_ruleset: true,
    },
    CatalogEntry {
        slug: "cis-aws-foundations-benchmark",
        version: "1.4.0",
        provider: "AWS",
        partition_wide_ruleset: false,
    },
    CatalogEntry {
        slug: "cis-aws-foundations-benchmark",
        version: "3.0.0",
        provider: "AWS",
        partition_wide_ruleset: false,
    },
    CatalogEntry {
        slug: "cis-aws-foundations-benchmark",
        version: "5.0.0",
        provider: "AWS",
        partition_wide_ruleset: false,
    },
    CatalogEntry {
        slug: "cis-azure-foundations-benchmark",
        version: "4.0.0",
        provider: "Azure",
        partition_wide_ruleset: false,
    },
    CatalogEntry {
        slug: "nist-800-171",
        version: "2.0.0",
        provider: "AWS",
        partition_wide_ruleset: false,
    },
    CatalogEntry {
        slug: "nist-800-53",
        version: "5.0.0",
        provider: "AWS",
        partition_wide_ruleset: false,
    },
    CatalogEntry {
        slug: "pci-dss",
        version: "3.2.1",
        provider: "AWS",
        partition_wide_ruleset: false,
    },
    CatalogEntry {
        slug: "pci-dss",
        version: "4.0.1",
        provider: "AWS",
        partition_wide_ruleset: false,
    },
];

impl CatalogEntry {
    /// The ARN of the standard itself: a standard's ARN carries the Region but no
    /// account, except the one partition-wide ruleset, which carries neither.
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

    /// The ARN of this account's subscription to the standard in `region`.
    fn subscription_arn(&self, region: &str) -> String {
        format!(
            "arn:aws:securityhub:{region}:{ACCOUNT_ID}:subscription/{}/v/{}",
            self.slug, self.version
        )
    }

    /// How a case names this standard in the response's assertion text.
    fn case(&self) -> String {
        format!("{}/v/{}", self.slug, self.version)
    }
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

    async fn enable_hub_in(&self, region: &str, body: Value) -> (u16, Value) {
        self.call(region, "POST", "/accounts", Some(body)).await
    }

    async fn batch_enable_standards_in(&self, region: &str, body: Value) -> (u16, Value) {
        self.call(region, "POST", "/standards/register", Some(body))
            .await
    }

    async fn batch_enable_standards(&self, body: Value) -> (u16, Value) {
        self.batch_enable_standards_in(REGION, body).await
    }

    async fn get_enabled_standards_in(&self, region: &str, body: Value) -> (u16, Value) {
        self.call(region, "POST", "/standards/get", Some(body)).await
    }

    async fn get_enabled_standards(&self, body: Value) -> (u16, Value) {
        self.get_enabled_standards_in(REGION, body).await
    }

    /// A harness whose `REGION` hub is subscribed with NO standard enabled, so every
    /// subscription a case observes is one `BatchEnableStandards` created.
    async fn subscribed_without_standards() -> Self {
        let harness = Self::new();
        let (status, body) = harness
            .enable_hub_in(REGION, json!({ "EnableDefaultStandards": false }))
            .await;
        assert_eq!(
            status, 200,
            "EnableSecurityHub must succeed, got {status} with {body}"
        );
        harness
    }
}

/// A `BatchEnableStandards` request enabling each ARN in `standards_arns`.
fn register_request(standards_arns: &[String]) -> Value {
    json!({
        "StandardsSubscriptionRequests": standards_arns
            .iter()
            .map(|arn| json!({ "StandardsArn": arn }))
            .collect::<Vec<Value>>(),
    })
}

/// The `StandardsSubscriptionArn`s of a response, in the order it listed them.
fn returned_subscription_arns(body: &Value) -> Vec<String> {
    body["StandardsSubscriptions"]
        .as_array()
        .unwrap_or_else(|| panic!("StandardsSubscriptions must be a list: {body}"))
        .iter()
        .map(|subscription| {
            subscription["StandardsSubscriptionArn"]
                .as_str()
                .unwrap_or_else(|| panic!("every subscription carries an ARN: {body}"))
                .to_string()
        })
        .collect()
}

/// The error code of a refusal, whichever casing the wire shape carries.
fn error_code(body: &Value) -> Option<&str> {
    body["Code"].as_str().or_else(|| body["code"].as_str())
}

/// Every standard the catalog publishes can be enabled, and the subscription the call
/// creates reports that standard, its provider and the `PENDING` status the enabling
/// call owes — then shows up in `GetEnabledStandards`, `READY`, once the transition
/// falls due.
#[tokio::test]
async fn enabling_a_published_standard_creates_its_subscription() {
    let harness = Harness::subscribed_without_standards().await;

    // One row per published standard: the rows differ in slug, version, evaluated
    // cloud provider and ARN form.
    for entry in &CATALOG {
        let case = entry.case();
        let standards_arn = entry.standards_arn(REGION);
        let (status, body) = harness
            .batch_enable_standards(register_request(std::slice::from_ref(&standards_arn)))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchEnableStandards must succeed, got {status} with {body}"
        );
        let subscriptions = body["StandardsSubscriptions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {body}"));
        assert_eq!(
            subscriptions.len(),
            1,
            "case {case:?}: one requested standard yields one subscription, got {body}"
        );
        let subscription = &subscriptions[0];
        assert_eq!(
            subscription["StandardsArn"].as_str(),
            Some(standards_arn.as_str()),
            "case {case:?}: expected StandardsArn {standards_arn}, got {subscription}"
        );
        let expected_subscription_arn = entry.subscription_arn(REGION);
        assert_eq!(
            subscription["StandardsSubscriptionArn"].as_str(),
            Some(expected_subscription_arn.as_str()),
            "case {case:?}: expected {expected_subscription_arn}, got {subscription}"
        );
        assert_eq!(
            subscription["StandardsStatus"].as_str(),
            Some("PENDING"),
            "case {case:?}: a just-enabled standard is PENDING, got {subscription}"
        );
        assert_eq!(
            subscription["StandardsControlsUpdatable"].as_str(),
            Some("NOT_READY_FOR_UPDATES"),
            "case {case:?}: a PENDING standard's controls are not updatable, got {subscription}"
        );
        assert_eq!(
            subscription["Provider"].as_str(),
            Some(entry.provider),
            "case {case:?}: expected Provider {}, got {subscription}",
            entry.provider
        );
        assert_eq!(
            subscription["StandardsInput"],
            json!({}),
            "case {case:?}: a standard enabled with no StandardsInput reports none, got {subscription}"
        );
    }

    // Read the created subscriptions back through the implemented reader: each one
    // persisted, and the transition the enabling call owes has fallen due.
    tokio::time::sleep(PAST_READY_DUE).await;
    let (status, body) = harness.get_enabled_standards(json!({})).await;
    assert_eq!(
        status, 200,
        "GetEnabledStandards must succeed, got {status} with {body}"
    );
    let returned: BTreeSet<String> = returned_subscription_arns(&body).into_iter().collect();
    let expected: BTreeSet<String> = CATALOG
        .iter()
        .map(|entry| entry.subscription_arn(REGION))
        .collect();
    assert_eq!(
        returned, expected,
        "every enabled standard must persist: expected {expected:?}, got {returned:?}"
    );
    for subscription in body["StandardsSubscriptions"]
        .as_array()
        .unwrap_or_else(|| panic!("StandardsSubscriptions must be a list: {body}"))
    {
        let case = subscription["StandardsSubscriptionArn"]
            .as_str()
            .unwrap_or("");
        assert_eq!(
            subscription["StandardsStatus"].as_str(),
            Some("READY"),
            "case {case:?}: the due transition must be materialised, got {subscription}"
        );
    }
}

/// One call enables every standard it names, whatever the batch's size: the response
/// carries one subscription per distinct standard requested, and a batch naming a
/// standard twice still yields one subscription for it.
#[tokio::test]
async fn one_call_enables_every_standard_it_names() {
    // Row per batch size: one request, a pair, the whole published catalog, and the
    // modelled maximum of 25 entries (`StandardsSubscriptionRequests`, len 1..25)
    // reached by repeating catalog ARNs.
    let cases: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        (
            "a single request",
            vec![CATALOG[11].standards_arn(REGION)],
            vec![CATALOG[11].subscription_arn(REGION)],
        ),
        (
            "two requests",
            vec![
                CATALOG[0].standards_arn(REGION),
                CATALOG[4].standards_arn(REGION),
            ],
            vec![
                CATALOG[0].subscription_arn(REGION),
                CATALOG[4].subscription_arn(REGION),
            ],
        ),
        (
            "the whole published catalog in one batch",
            CATALOG
                .iter()
                .map(|entry| entry.standards_arn(REGION))
                .collect(),
            CATALOG
                .iter()
                .map(|entry| entry.subscription_arn(REGION))
                .collect(),
        ),
        (
            "the modelled maximum of 25 requests, with repeats",
            (0..25)
                .map(|index| CATALOG[index % 3].standards_arn(REGION))
                .collect(),
            (0..3)
                .map(|index| CATALOG[index].subscription_arn(REGION))
                .collect(),
        ),
        (
            "one standard named twice in the same batch",
            vec![
                CATALOG[9].standards_arn(REGION),
                CATALOG[9].standards_arn(REGION),
            ],
            vec![CATALOG[9].subscription_arn(REGION)],
        ),
    ];

    for (case, requested, expected) in cases {
        let harness = Harness::subscribed_without_standards().await;
        let (status, body) = harness
            .batch_enable_standards(register_request(&requested))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchEnableStandards must succeed, got {status} with {body}"
        );
        let returned = returned_subscription_arns(&body);
        assert_eq!(
            returned.iter().collect::<BTreeSet<_>>(),
            expected.iter().collect::<BTreeSet<_>>(),
            "case {case:?}: expected {expected:?}, got {returned:?}"
        );

        let (status, read_back) = harness.get_enabled_standards(json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetEnabledStandards must succeed, got {status} with {read_back}"
        );
        let persisted = returned_subscription_arns(&read_back);
        assert_eq!(
            persisted.iter().collect::<BTreeSet<_>>(),
            expected.iter().collect::<BTreeSet<_>>(),
            "case {case:?}: expected {expected:?} to persist, got {persisted:?}"
        );
    }
}

/// A `StandardsArn` no published standard carries is refused with HTTP 400
/// `InvalidInputException`, and the refusal holds whether or not the account is
/// subscribed, because the request is judged before any hub state is read.
#[tokio::test]
async fn an_arn_outside_the_published_catalog_is_refused() {
    let unknown_version = format!(
        "arn:aws:securityhub:{REGION}::standards/pci-dss/v/9.9.9"
    );
    let cases: Vec<(&str, String)> = vec![
        ("not an ARN at all", "not-a-standard".to_string()),
        (
            "a slug no published standard has",
            format!("arn:aws:securityhub:{REGION}::standards/not-a-standard/v/1.0.0"),
        ),
        ("a version no published standard has", unknown_version),
        (
            "a published standard's ARN for another Region",
            CATALOG[1].standards_arn(OTHER_REGION),
        ),
        (
            "a regional standard written in the partition-wide ruleset form",
            "arn:aws:securityhub:::ruleset/pci-dss/v/3.2.1".to_string(),
        ),
        (
            "the partition-wide ruleset written in the regional standards form",
            format!(
                "arn:aws:securityhub:{REGION}::standards/cis-aws-foundations-benchmark/v/1.2.0"
            ),
        ),
        (
            "the subscription ARN instead of the standard ARN",
            CATALOG[1].subscription_arn(REGION),
        ),
        (
            "a standard ARN stamped with an account id",
            format!(
                "arn:aws:securityhub:{REGION}:{ACCOUNT_ID}:standards/pci-dss/v/3.2.1"
            ),
        ),
    ];

    for (case, standards_arn) in cases {
        for subscribed in [false, true] {
            let harness = if subscribed {
                Harness::subscribed_without_standards().await
            } else {
                Harness::new()
            };
            let (status, body) = harness
                .batch_enable_standards(register_request(std::slice::from_ref(&standards_arn)))
                .await;
            assert_eq!(
                status, 400,
                "case {case:?} (subscribed={subscribed}): expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                Some("InvalidInputException"),
                "case {case:?} (subscribed={subscribed}): expected InvalidInputException, got {body}"
            );
        }
    }
}

/// A batch is all-or-nothing: one unsupported `StandardsArn` refuses the call, and
/// the standards named beside it are not enabled.
#[tokio::test]
async fn a_batch_with_one_unsupported_arn_enables_nothing() {
    let harness = Harness::subscribed_without_standards().await;
    let bad = format!("arn:aws:securityhub:{REGION}::standards/not-a-standard/v/1.0.0");

    // Rows differ in where the unsupported ARN sits in the batch.
    let cases: Vec<(&str, Vec<String>)> = vec![
        (
            "unsupported ARN first",
            vec![bad.clone(), CATALOG[12].standards_arn(REGION)],
        ),
        (
            "unsupported ARN last",
            vec![CATALOG[12].standards_arn(REGION), bad.clone()],
        ),
        (
            "unsupported ARN between two supported ones",
            vec![
                CATALOG[0].standards_arn(REGION),
                bad.clone(),
                CATALOG[12].standards_arn(REGION),
            ],
        ),
    ];

    for (case, requested) in cases {
        let (status, body) = harness
            .batch_enable_standards(register_request(&requested))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        let (status, read_back) = harness.get_enabled_standards(json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetEnabledStandards must succeed, got {status} with {read_back}"
        );
        let persisted = returned_subscription_arns(&read_back);
        assert!(
            persisted.is_empty(),
            "case {case:?}: a refused batch must enable nothing, got {persisted:?}"
        );
    }
}

/// `StandardsInput` is the input the subscription was created with: it round-trips
/// through `GetEnabledStandards` whatever its size.
#[tokio::test]
async fn standards_input_round_trips_with_the_subscription() {
    let harness = Harness::subscribed_without_standards().await;

    // Rows differ in how many parameters the standard is enabled with.
    let cases: Vec<(&str, usize, Value)> = vec![
        ("no parameters", 0, json!({})),
        ("one parameter", 1, json!({ "requiredTagKeys": "Owner" })),
        (
            "three parameters",
            2,
            json!({ "a": "1", "b": "2", "c": "3" }),
        ),
    ];

    for (case, index, standards_input) in cases {
        let entry = &CATALOG[index];
        let standards_arn = entry.standards_arn(REGION);
        let request = json!({
            "StandardsSubscriptionRequests": [{
                "StandardsArn": standards_arn,
                "StandardsInput": standards_input,
            }],
        });
        let (status, body) = harness.batch_enable_standards(request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchEnableStandards must succeed, got {status} with {body}"
        );
        assert_eq!(
            body["StandardsSubscriptions"][0]["StandardsInput"], standards_input,
            "case {case:?}: expected StandardsInput {standards_input}, got {body}"
        );

        let subscription_arn = entry.subscription_arn(REGION);
        let (status, read_back) = harness
            .get_enabled_standards(json!({ "StandardsSubscriptionArns": [subscription_arn] }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetEnabledStandards must succeed, got {status} with {read_back}"
        );
        assert_eq!(
            read_back["StandardsSubscriptions"][0]["StandardsInput"], standards_input,
            "case {case:?}: expected StandardsInput {standards_input} to persist, got {read_back}"
        );
    }
}

/// A standard a hub already subscribes to is the same resource, not a second one: a
/// repeated call reports the subscription as it stands, without restarting its
/// transition or replacing the input it was enabled with.
#[tokio::test]
async fn enabling_an_already_enabled_standard_keeps_one_subscription() {
    let harness = Harness::subscribed_without_standards().await;
    let entry = &CATALOG[11];
    let standards_arn = entry.standards_arn(REGION);
    let subscription_arn = entry.subscription_arn(REGION);
    let first_input = json!({ "first": "value" });

    let (status, body) = harness
        .batch_enable_standards(json!({
            "StandardsSubscriptionRequests": [{
                "StandardsArn": standards_arn,
                "StandardsInput": first_input,
            }],
        }))
        .await;
    assert_eq!(
        status, 200,
        "the first enable must succeed, got {status} with {body}"
    );
    tokio::time::sleep(PAST_READY_DUE).await;

    // Rows: the same standard re-enabled with other input, then alone, then beside a
    // standard that is new to the hub.
    let cases: Vec<(&str, Value)> = vec![
        (
            "re-enabled with different input",
            json!({
                "StandardsSubscriptionRequests": [{
                    "StandardsArn": standards_arn,
                    "StandardsInput": { "second": "value" },
                }],
            }),
        ),
        (
            "re-enabled with no input",
            register_request(std::slice::from_ref(&standards_arn)),
        ),
        (
            "re-enabled beside a standard new to the hub",
            register_request(&[standards_arn.clone(), CATALOG[2].standards_arn(REGION)]),
        ),
    ];

    for (case, request) in cases {
        let (status, body) = harness.batch_enable_standards(request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchEnableStandards must succeed, got {status} with {body}"
        );
        let subscription = body["StandardsSubscriptions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {body}"))
            .iter()
            .find(|candidate| {
                candidate["StandardsSubscriptionArn"].as_str() == Some(subscription_arn.as_str())
            })
            .unwrap_or_else(|| panic!("case {case:?}: expected {subscription_arn}, got {body}"));
        assert_eq!(
            subscription["StandardsStatus"].as_str(),
            Some("READY"),
            "case {case:?}: an enabled standard's status is not restarted, got {subscription}"
        );
        assert_eq!(
            subscription["StandardsInput"], first_input,
            "case {case:?}: the subscription keeps the input it was enabled with, got {subscription}"
        );

        let (status, read_back) = harness
            .get_enabled_standards(json!({ "StandardsSubscriptionArns": [subscription_arn] }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetEnabledStandards must succeed, got {status} with {read_back}"
        );
        let persisted = returned_subscription_arns(&read_back);
        assert_eq!(
            persisted,
            vec![subscription_arn.clone()],
            "case {case:?}: one standard stays one subscription, got {persisted:?}"
        );
        assert_eq!(
            read_back["StandardsSubscriptions"][0]["StandardsInput"], first_input,
            "case {case:?}: the stored input must survive the repeat, got {read_back}"
        );
    }
}

/// Enabling a standard needs a hub: an account that has not subscribed to Security
/// Hub CSPM in the request's Region is refused, and a subscription belongs to the
/// Region it was enabled in.
#[tokio::test]
async fn standards_are_scoped_to_the_subscribed_region() {
    let harness = Harness::new();
    let standards_arn = CATALOG[12].standards_arn(REGION);

    for region in [REGION, OTHER_REGION] {
        let (status, body) = harness
            .batch_enable_standards_in(region, register_request(&[CATALOG[12].standards_arn(region)]))
            .await;
        assert_eq!(
            status, 401,
            "case {region:?}: an unsubscribed account must be refused, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            Some("InvalidAccessException"),
            "case {region:?}: expected InvalidAccessException, got {body}"
        );
    }

    let (status, body) = harness
        .enable_hub_in(REGION, json!({ "EnableDefaultStandards": false }))
        .await;
    assert_eq!(
        status, 200,
        "EnableSecurityHub must succeed, got {status} with {body}"
    );
    let (status, body) = harness
        .batch_enable_standards(register_request(&[standards_arn]))
        .await;
    assert_eq!(
        status, 200,
        "BatchEnableStandards must succeed, got {status} with {body}"
    );

    let (status, body) = harness.get_enabled_standards_in(OTHER_REGION, json!({})).await;
    assert_eq!(
        status, 401,
        "the other Region is still unsubscribed, got {status} with {body}"
    );
    let (status, body) = harness.get_enabled_standards(json!({})).await;
    assert_eq!(
        status, 200,
        "GetEnabledStandards must succeed, got {status} with {body}"
    );
    let persisted = returned_subscription_arns(&body);
    assert_eq!(
        persisted,
        vec![CATALOG[12].subscription_arn(REGION)],
        "the standard belongs to the Region it was enabled in, got {persisted:?}"
    );
}

/// A request outside the modelled input is refused with HTTP 400, at each published
/// bound of `StandardsSubscriptionRequests` and one past it.
#[tokio::test]
async fn requests_outside_the_modelled_input_are_refused() {
    let harness = Harness::subscribed_without_standards().await;
    let supported = CATALOG[1].standards_arn(REGION);

    // `StandardsSubscriptionRequests` is `@length(min: 1, max: 25)` and
    // `StandardsSubscriptionRequest$StandardsArn` is required with `@pattern("\\S")`.
    let cases: Vec<(&str, Value)> = vec![
        (
            "no StandardsSubscriptionRequests member",
            json!({}),
        ),
        (
            "an empty batch, one below the published minimum",
            json!({ "StandardsSubscriptionRequests": [] }),
        ),
        (
            "26 requests, one above the published maximum",
            register_request(
                &(0..26)
                    .map(|index| CATALOG[index % CATALOG.len()].standards_arn(REGION))
                    .collect::<Vec<String>>(),
            ),
        ),
        (
            "a request with no StandardsArn",
            json!({ "StandardsSubscriptionRequests": [{}] }),
        ),
        (
            "a blank StandardsArn",
            json!({ "StandardsSubscriptionRequests": [{ "StandardsArn": " " }] }),
        ),
    ];

    for (case, request) in cases {
        let (status, body) = harness.batch_enable_standards(request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
    }

    // Each published batch bound is accepted: one request, and twenty-five.
    let bounds: Vec<(&str, Vec<String>)> = vec![
        ("the published minimum batch", vec![supported.clone()]),
        (
            "the published maximum batch",
            (0..25)
                .map(|index| CATALOG[index % CATALOG.len()].standards_arn(REGION))
                .collect(),
        ),
    ];
    for (case, requested) in bounds {
        let (status, body) = harness
            .batch_enable_standards(register_request(&requested))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a published bound must be accepted, got {status} with {body}"
        );
    }
}
