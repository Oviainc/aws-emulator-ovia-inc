// SPDX-License-Identifier: CC0-1.0

//! What the catalog may claim as an IDENTITY — the `QuotaCode` of a quota and the
//! `ServiceName` of a service — over the wire.
//!
//! Both are identifiers AWS owns, and neither is derivable from the model, so the
//! rules asserted here are about what this emulator is allowed to publish rather
//! than about one recorded value:
//!
//! * `L-` followed by eight hex characters is the shape of a code AWS MINTS. A code
//!   of that shape is what a client reads out of AWS's published catalog and looks a
//!   quota up by, so every one this catalog publishes must be a code it cites from
//!   AWS; a quota whose AWS code the catalog cannot cite is published under a
//!   descriptive code built from the quota's own published name;
//! * `ServiceInfo$ServiceName` is the service's display name, which is a different
//!   string from its `ServiceCode` — a service listed under its own code as its name
//!   renders a name that never appears in AWS;
//! * a code in AWS's namespace that this catalog does not cite resolves nothing, so
//!   a client holding a real AWS quota code never reads a quota back from one this
//!   emulator made up.
//!
//! Each suite drives a TABLE of inputs through `ListServices`,
//! `ListAWSDefaultServiceQuotas` and `GetAWSDefaultServiceQuota`, and the whole
//! catalog where the rule is an invariant over it. The harness is the one
//! `quota_workflow.rs` owns.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_servicequotas_servicequotasv20190624::state::descriptive_quota_code;

use crate::quota_workflow::{client, emulator, walk_pages, Client};

/// The `(ServiceCode, QuotaCode)` pairs this catalog CITES from AWS's own published
/// Service Quotas data — the only quotas allowed to carry a code in AWS's minted
/// `L-` namespace.
///
/// SOURCE: the `.code("L-…")` rows of `src/state.rs::seed_quota_catalog` (each one
/// carries AWS's code for that quota). A new row that claims an `L-` code and is not
/// listed here fails `only_a_cited_quota_code_sits_in_the_aws_minted_namespace`,
/// which is the point: a code in that namespace has to be attributable.
const CITED_AWS_QUOTA_CODES: &[(&str, &str)] = &[
    ("access-analyzer", "L-1E51937C"),
    ("access-analyzer", "L-96F55078"),
    ("account-access", "L-42A1D874"),
    ("acm", "L-DD60D132"),
    ("acm-pca", "L-01B8608D"),
    ("aco-automation", "L-1501440B"),
    ("agent-registry", "L-7F384C37"),
    ("aidevops", "L-4FF187E6"),
    ("aiops", "L-EF31E0C4"),
    ("airflow", "L-52DCDA32"),
    ("airflow", "L-67D41C6B"),
    ("dynamodb", "L-F98FE922"),
    ("ec2", "L-0263D0A3"),
    ("ec2", "L-1216C47A"),
    ("ec2", "L-34B43A08"),
    ("lambda", "L-2ACBD22F"),
    ("lambda", "L-72E0D058"),
    ("lambda", "L-B2AA0F47"),
    ("lambda", "L-B82A30EA"),
    ("lambda", "L-B99A9384"),
    ("lambda", "L-F55076EC"),
    ("s3", "L-DC2B2D3D"),
    ("vpc", "L-045E6E9C"),
    ("vpc", "L-085A6257"),
    ("vpc", "L-A4707A72"),
    ("vpc", "L-F678F1CE"),
];

/// The `(ServiceCode, QuotaCode)` pairs this catalog cites from a SERVICE'S OWN API
/// rather than from AWS's minted namespace: `b2bi` reports its profile limit under
/// `ProfilesPerAccount` in its own `ServiceQuotaExceededException`, so a client that
/// reads that refusal finds the quota here under the code the refusal named.
const CITED_SERVICE_QUOTA_CODES: &[(&str, &str)] = &[("b2bi", "ProfilesPerAccount")];

/// The service codes AWS's inventory lists for which this catalog carries no display
/// name it can cite, and which therefore report their code (see the tail of
/// `src/state.rs::SEED_SERVICE_CATALOG`). Every OTHER code reports a display name.
const CODES_WITHOUT_A_CITED_DISPLAY_NAME: &[&str] = &[
    "airflow-serverless",
    "aws-mcp",
    "axis",
    "bedrock-websearch",
    "cloudwatchpredictions",
    "eks-mcp",
    "elemental-inference",
    "health-agent",
    "interconnect",
    "networkinsights",
    "s3files",
    "securityagent",
];

/// Whether `code` has the shape of a code AWS MINTS: `L-` and eight upper-case hex
/// characters, the form every `QuotaCode` in AWS's published catalog has.
fn is_aws_minted_shape(code: &str) -> bool {
    code.strip_prefix("L-").is_some_and(|rest| {
        rest.len() == 8
            && rest
                .chars()
                .all(|char| char.is_ascii_digit() || ('A'..='F').contains(&char))
    })
}

/// The descriptive code a quota name yields: its words, each capitalised, joined,
/// with a `Quota` prefix where that would not start with a letter and truncated to
/// the modelled maximum. The rule
/// `src/state.rs::descriptive_quota_code` implements, restated in full here so the
/// test fails if the implementation drifts from it.
fn descriptive_code(quota_name: &str) -> String {
    let mut code = String::new();
    for word in quota_name.split(|char: char| !char.is_ascii_alphanumeric()) {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            code.push(first.to_ascii_uppercase());
            code.extend(chars);
        }
    }
    if !code.starts_with(|char: char| char.is_ascii_alphabetic()) {
        code.insert_str(0, "Quota");
    }
    if code.len() < 2 {
        code.push_str("Quota");
    }
    code.truncate(128);
    code
}

/// The rule itself, over names that reach every branch of it — including the two no
/// catalog row trips today, so the restated rule above is complete rather than
/// agreeing with the implementation by luck.
#[test]
fn a_descriptive_code_is_the_quota_name_as_one_modelled_identifier() {
    // Rows: a plain name, punctuation and digits inside words, a name that starts
    // with a digit, a name with a single letter in it, and a name far longer than
    // the modelled 128-character maximum.
    for name in [
        "Workgroups per Region",
        "Alternate domain names (CNAMEs) per distribution",
        "Running On-Demand Standard (A, C, D, H, I, M, R, T, Z) instances",
        "concurrent-evaluations-per-agent-space",
        "5xx responses per minute",
        "A",
        "!!",
        &"Extremely long quota name that AWS would never publish ".repeat(8),
    ] {
        let code = descriptive_quota_code(name);
        assert_eq!(
            code,
            descriptive_code(name),
            "case {name:?}: expected the descriptive code of the name, got {code:?}"
        );
        assert!(
            (2..=128).contains(&code.len())
                && code.starts_with(|char: char| char.is_ascii_alphabetic())
                && code
                    .chars()
                    .all(|char| char.is_ascii_alphanumeric() || char == '-'),
            "case {name:?}: every descriptive code satisfies the modelled QuotaCode \
             pattern, got {code:?}"
        );
    }
}

/// Every `(ServiceCode, ServiceName)` pair `ListServices` reports.
async fn inventory(client: &Client) -> Vec<Value> {
    walk_pages(client, "ListServices", &json!({}), "Services", None).await
}

/// Every AWS default quota of `service_code`.
async fn defaults(client: &Client, service_code: &str) -> Vec<Value> {
    walk_pages(
        client,
        "ListAWSDefaultServiceQuotas",
        &json!({"ServiceCode": service_code}),
        "Quotas",
        None,
    )
    .await
}

/// The whole catalog, as `ServiceCode -> its default quotas`.
async fn whole_catalog(client: &Client) -> BTreeMap<String, Vec<Value>> {
    let mut catalog = BTreeMap::new();
    for service in inventory(client).await {
        let code = service["ServiceCode"]
            .as_str()
            .unwrap_or_else(|| panic!("case ListServices: a service without a code: {service}"))
            .to_string();
        let quotas = defaults(client, &code).await;
        catalog.insert(code, quotas);
    }
    catalog
}

/// A code in AWS's minted namespace is published only where this catalog cites it;
/// every other quota is published under the descriptive code of its own name, and
/// every published code satisfies the modelled `QuotaCode` pattern
/// (`smithy_query(query="shape", shape="ServiceQuota")`: length 1..128, pattern
/// `^[a-zA-Z][a-zA-Z0-9-]{1,128}$`).
#[tokio::test]
async fn only_a_cited_quota_code_sits_in_the_aws_minted_namespace() {
    let engine = emulator();
    let client = client(&engine, "us-west-2");

    let cited: BTreeSet<(&str, &str)> = CITED_AWS_QUOTA_CODES.iter().copied().collect();
    let catalog = whole_catalog(&client).await;
    let mut minted = 0_usize;
    let mut descriptive = 0_usize;

    for (service_code, quotas) in &catalog {
        for quota in quotas {
            let code = quota["QuotaCode"]
                .as_str()
                .unwrap_or_else(|| panic!("case {service_code}: a quota without a code: {quota}"));
            let name = quota["QuotaName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {service_code}: a quota without a name: {quota}"));
            let case = format!("{service_code}/{code}");

            // The modelled constraints, for every row.
            assert!(
                (2..=128).contains(&code.len())
                    && code.starts_with(|char: char| char.is_ascii_alphabetic())
                    && code
                        .chars()
                        .all(|char| char.is_ascii_alphanumeric() || char == '-'),
                "case {case}: expected a code satisfying the modelled QuotaCode pattern, got \
                 {code:?}"
            );

            if CITED_SERVICE_QUOTA_CODES.contains(&(service_code.as_str(), code)) {
                // A code the service's own API publishes: cited, so neither minted
                // nor descriptive.
            } else if is_aws_minted_shape(code) {
                assert!(
                    cited.contains(&(service_code.as_str(), code)),
                    "case {case}: a code in AWS's minted L- namespace must be one this catalog \
                     cites from AWS, got the unattributable {code:?} for {name:?}"
                );
                minted += 1;
            } else {
                assert_eq!(
                    code,
                    descriptive_quota_code(name),
                    "case {case}: a quota whose AWS code this catalog does not cite is published \
                     under its name's descriptive code, expected {:?}, got {code:?}",
                    descriptive_quota_code(name)
                );
                descriptive += 1;
            }
        }
    }

    // Both branches of the rule are exercised by the table above.
    assert!(
        minted >= CITED_AWS_QUOTA_CODES.len() && descriptive > 100,
        "case catalog: expected both cited and descriptive codes to be published, got \
         {minted} cited and {descriptive} descriptive"
    );

    // The pairs this catalog cites are published, each under the service that cites
    // it — a cited code left unpublished would make this suite vacuous.
    for (service_code, quota_code) in CITED_AWS_QUOTA_CODES {
        let published = catalog
            .get(*service_code)
            .unwrap_or_else(|| panic!("case {service_code}: expected the service to be listed"));
        assert!(
            published
                .iter()
                .any(|quota| quota["QuotaCode"] == json!(quota_code)),
            "case {service_code}/{quota_code}: expected the cited quota to be published, got {} \
             quotas",
            published.len()
        );
    }
}

/// The published rows of the services whose default sets this suite pins, each read
/// back through `GetAWSDefaultServiceQuota`.
///
/// SOURCE: AWS's published default-quota data for each `ServiceCode` (what
/// `aws service-quotas list-aws-default-service-quotas --service-code <code>`
/// reports). Rows cover a countable quota, a not-adjustable one, a unit other than
/// `None`, and a rate quota with a `Period`.
#[tokio::test]
async fn a_cited_default_quota_reads_back_with_the_values_aws_publishes() {
    let engine = emulator();
    let client = client(&engine, "us-west-2");

    struct Row {
        service_code: &'static str,
        quota_code: &'static str,
        quota_name: &'static str,
        value: f64,
        unit: &'static str,
        adjustable: bool,
        period: Option<(i32, &'static str)>,
    }

    let rows = [
        Row {
            service_code: "access-analyzer",
            quota_code: "L-96F55078",
            quota_name: "Policy generation CloudTrail time range",
            value: 90.0,
            unit: "None",
            adjustable: false,
            period: None,
        },
        Row {
            service_code: "access-analyzer",
            quota_code: "L-1E51937C",
            quota_name: "Archive rules per analyzer",
            value: 100.0,
            unit: "None",
            adjustable: true,
            period: None,
        },
        Row {
            service_code: "account-access",
            quota_code: "L-42A1D874",
            quota_name: "Group entitlements per role",
            value: 20.0,
            unit: "None",
            adjustable: true,
            period: None,
        },
        Row {
            service_code: "agent-registry",
            quota_code: "L-7F384C37",
            quota_name: "InvokeRegistryMcp requests per second",
            value: 10.0,
            unit: "Count/Second",
            adjustable: true,
            period: Some((1, "SECOND")),
        },
        Row {
            service_code: "aidevops",
            quota_code: "L-4FF187E6",
            quota_name: "concurrent-evaluations-per-agent-space",
            value: 1.0,
            unit: "Count",
            adjustable: false,
            period: None,
        },
        Row {
            service_code: "airflow",
            quota_code: "L-52DCDA32",
            quota_name: "Web servers per environment",
            value: 5.0,
            unit: "None",
            adjustable: true,
            period: None,
        },
        Row {
            service_code: "acm-pca",
            quota_code: "L-01B8608D",
            quota_name: "Rate of CreateCertificateAuthority requests",
            value: 1.0,
            unit: "None",
            adjustable: false,
            period: Some((1, "SECOND")),
        },
        // The quota whose usage AWS publishes as a RESOURCE count: its period is the
        // window that metric is published over, which the row does not state
        // separately.
        Row {
            service_code: "aiops",
            quota_code: "L-EF31E0C4",
            quota_name: "Concurrent active investigations",
            value: 2.0,
            unit: "None",
            adjustable: true,
            period: Some((5, "MINUTE")),
        },
        // A quota the issue behind this suite never cited, carried by the catalog
        // from AWS's published VPC data.
        Row {
            service_code: "vpc",
            quota_code: "L-085A6257",
            quota_name: "IPv6 CIDR blocks per VPC",
            value: 1.0,
            unit: "None",
            adjustable: false,
            period: None,
        },
    ];

    for row in &rows {
        let case = format!("{}/{}", row.service_code, row.quota_code);
        let pair = json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code});
        let quota = client.ok("GetAWSDefaultServiceQuota", pair.clone()).await["Quota"].clone();

        assert_eq!(
            quota["QuotaName"],
            json!(row.quota_name),
            "case {case}: expected the published quota name, got {quota}"
        );
        assert_eq!(
            quota["Value"],
            json!(row.value),
            "case {case}: expected the published default value, got {quota}"
        );
        assert_eq!(
            quota["Unit"],
            json!(row.unit),
            "case {case}: expected the published unit, got {quota}"
        );
        assert_eq!(
            quota["Adjustable"],
            json!(row.adjustable),
            "case {case}: expected the published adjustability, got {quota}"
        );
        match row.period {
            Some((value, unit)) => assert_eq!(
                quota["Period"],
                json!({"PeriodValue": value, "PeriodUnit": unit}),
                "case {case}: a rate quota reports the window AWS publishes it over, got {quota}"
            ),
            None => assert_eq!(
                quota["Period"],
                Value::Null,
                "case {case}: a quota AWS publishes no rate window for reports none, got {quota}"
            ),
        }

        // The listing reports the same row the single read does.
        let listed = defaults(&client, row.service_code).await;
        assert!(
            listed.contains(&quota),
            "case {case}: the listed entry must be the one the single read returns, got {listed:?}"
        );
    }
}

/// A `ServiceName` is a display name, never the `ServiceCode` echoed back — for
/// every service the inventory lists except the codes this catalog cannot cite a
/// name for.
#[tokio::test]
async fn a_listed_service_reports_a_display_name_not_its_own_code() {
    let engine = emulator();
    let client = client(&engine, "eu-west-1");

    let listed = inventory(&client).await;
    let names: BTreeMap<String, String> = listed
        .iter()
        .map(|service| {
            (
                service["ServiceCode"].as_str().unwrap_or_default().to_string(),
                service["ServiceName"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();

    let echoing: BTreeSet<&str> = names
        .iter()
        .filter(|(code, name)| code == name)
        .map(|(code, _)| code.as_str())
        .collect();
    let expected: BTreeSet<&str> = CODES_WITHOUT_A_CITED_DISPLAY_NAME.iter().copied().collect();
    assert_eq!(
        echoing, expected,
        "case ListServices: only a code with no display name this catalog can cite may report \
         its own code as its name"
    );

    // The display names, as a table, with whether the catalog carries published
    // default quotas for the code. The last three rows are codes the issue behind
    // this suite never cited.
    for (code, name, carries_quotas) in [
        ("access-analyzer", "Access Analyzer", true),
        ("account-access", "Account access manager", true),
        ("aco-automation", "AWS Compute Optimizer Automation", true),
        ("agent-registry", "AWS Agent Registry", true),
        ("aidevops", "AWS DevOps Agent", true),
        ("aiops", "Amazon AI Operations", true),
        (
            "app-integrations",
            "Amazon Connect Application Integrations",
            false,
        ),
        ("aps", "Amazon Managed Prometheus", false),
        ("robomaker", "AWS RoboMaker", false),
        ("security-ir", "AWS Security Incident Response", false),
        ("ssm-sap", "AWS Systems Manager for SAP", false),
    ] {
        assert_eq!(
            names.get(code).map(String::as_str),
            Some(name),
            "case {code}: expected the display name AWS lists the code under, got {:?}",
            names.get(code)
        );
        // And a code the inventory lists resolves its defaults, so a client that
        // discovered it here can read its quotas: the rows the catalog carries
        // quotas for come back with entries, and a code whose published defaults it
        // does not carry comes back with the empty list its own code earns it.
        let quotas = client
            .ok(
                "ListAWSDefaultServiceQuotas",
                json!({"ServiceCode": code}),
            )
            .await;
        let listed = quotas["Quotas"]
            .as_array()
            .unwrap_or_else(|| panic!("case {code}: expected a Quotas list, got {quotas}"));
        assert_eq!(
            !listed.is_empty(),
            carries_quotas,
            "case {code}: expected the catalog to carry {}quotas for this code, got {} entries",
            if carries_quotas { "" } else { "no " },
            listed.len()
        );
    }
}

/// A lookup by a code in AWS's minted namespace that this catalog does not cite
/// resolves NOTHING — which is what keeps a client that holds a real AWS quota code
/// from resolving it against a quota this emulator invented — while the descriptive
/// code a quota IS published under resolves that quota.
#[tokio::test]
async fn a_minted_code_this_catalog_does_not_cite_resolves_nothing() {
    let engine = emulator();
    let client = client(&engine, "ap-northeast-1");

    // Rows: AWS-minted codes this catalog does not cite — one AWS publishes for the
    // service asked for, one it publishes for a DIFFERENT service, and one that is
    // merely well formed. A code of this shape used to be synthesized from the quota
    // name, which is how a lookup like the first row used to succeed.
    for (service_code, quota_code, why) in [
        ("airflow", "L-CFEFCF95", "a code no quota of this service is published under"),
        ("access-analyzer", "L-5EFCE71D", "an AWS code this catalog does not carry"),
        ("monitoring", "L-1216C47A", "an EC2 code under CloudWatch"),
        ("athena", "L-00000000", "a well-formed code nothing publishes"),
    ] {
        let case = format!("{service_code}/{quota_code}");
        for operation in ["GetAWSDefaultServiceQuota", "GetServiceQuota"] {
            let (status, error) = client
                .refused(
                    operation,
                    json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
                )
                .await;
            assert_eq!(
                (status, error.as_str()),
                (400, "NoSuchResourceException"),
                "case {case} ({why}): {operation} must refuse a quota it does not publish"
            );
        }
    }

    // Rows: the descriptive code of a published quota, for services whose rows carry
    // no AWS code — each resolves the quota whose name it was built from.
    for (service_code, quota_code, quota_name) in [
        ("athena", "WorkgroupsPerRegion", "Workgroups per Region"),
        ("monitoring", "AlarmsPerRegion", "Alarms per Region"),
        ("vpc", "SubnetsPerVPC", "Subnets per VPC"),
        // The one row whose code the SERVICE's own refusal publishes.
        ("b2bi", "ProfilesPerAccount", "Customer profiles per account"),
    ] {
        let case = format!("{service_code}/{quota_code}");
        let quota = client
            .ok(
                "GetAWSDefaultServiceQuota",
                json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
            )
            .await["Quota"]
            .clone();
        assert_eq!(
            quota["QuotaName"],
            json!(quota_name),
            "case {case}: the code must resolve the quota it names, got {quota}"
        );
    }
}
