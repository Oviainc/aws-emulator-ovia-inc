// SPDX-License-Identifier: CC0-1.0

//! `DescribeApplication` end to end over the `awsJson1_1` wire — the operation the stub
//! issue reported as an unimplemented scaffold.
//!
//! The sibling suites assert what a CREATE and an UPDATE leave behind, reading the result
//! back through this operation. What this suite owns are `DescribeApplication`'s OWN
//! rules, each asserted over a TABLE of inputs rather than over a single example:
//!
//! 1. it REPORTS the stored application, so the body it answers carries exactly the
//!    members the model declares for its output that the record holds a value for — and
//!    nothing else the create was given (its `Tags`, its `ClientToken`) and nothing
//!    recomputed from the request;
//! 2. it is a READ: describing an application any number of times, from any Region,
//!    answers the identical body and leaves every stored record exactly as it was, so a
//!    later caller sees what it would have seen had nobody described anything;
//! 3. an `ApplicationArn` no create minted is refused with the modelled
//!    `ResourceNotFoundException` naming the ARN asked for — never an empty 200 and never
//!    the scaffold's HTTP 501 — whichever spelling the model's `ApplicationArn` pattern
//!    admits;
//! 4. an application belongs to its instance, so once the removal a `DeleteInstance` owes
//!    falls due the application is gone with it and its ARN earns rule 3's refusal, while
//!    before that instant it still reports exactly what it reported before.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns. Where a test has
//! to be on the far side of a due instant it moves the STORED instant rather than
//! sleeping — the transition itself is still applied by the service's own
//! `materialize_due`, exactly as in production.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    assert_refused, call, call_in_region, hold_deletion, router, router_over, seeded_instance_arn,
    with_store, TEST_REGION,
};

/// The one published provider an application can be CREATED under.
///
/// SOURCE: `state::BuiltInApplicationProvider::accepts_customer_managed_application` over
/// `state::BUILT_IN_APPLICATION_PROVIDERS`, the catalogue `ListApplicationProviders`
/// publishes over the wire.
const CUSTOM_PROVIDER_ARN: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// Every member the model declares for this operation's OUTPUT, and no other.
///
/// SOURCE: `smithy_query(query="operation",operation="DescribeApplication",
/// model_path="services/service-com-amazonaws-ssoadmin-swbexternalservice/smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json")`
/// → `output`.
const MODELLED_OUTPUT_MEMBERS: [&str; 11] = [
    "ApplicationAccount",
    "ApplicationArn",
    "ApplicationProviderArn",
    "CreatedDate",
    "CreatedFrom",
    "Description",
    "IdentityStoreArn",
    "InstanceArn",
    "Name",
    "PortalOptions",
    "Status",
];

/// The output members a describe must ALWAYS report: the ones every stored record holds a
/// value for. `Description` is the one member of [`MODELLED_OUTPUT_MEMBERS`] a create can
/// leave unstored, so it is the only one that may be absent.
const ALWAYS_REPORTED_MEMBERS: [&str; 10] = [
    "ApplicationAccount",
    "ApplicationArn",
    "ApplicationProviderArn",
    "CreatedDate",
    "CreatedFrom",
    "IdentityStoreArn",
    "InstanceArn",
    "Name",
    "PortalOptions",
    "Status",
];

/// Regions a read is signed for. A describe answers from the RECORD, so the Region of the
/// read is one of the things rule 2 generalises over; the spellings are all inside
/// `RegionName`'s `^([a-z]+-){2,3}\d$` (two, three and four segments).
const READ_REGIONS: [&str; 4] = [TEST_REGION, "eu-west-2", "ap-southeast-2", "us-gov-east-1"];

/// One `CreateApplication` request: a row of the table.
#[derive(Debug, Clone)]
struct Row {
    label: &'static str,
    name: String,
    description: Option<String>,
    status: Option<&'static str>,
    portal_options: Option<Value>,
    /// `Tags` and `ClientToken` are members of the CREATE that this operation's output
    /// does not model, so a row carrying them pins rule 1's "and nothing else" half.
    tags: Vec<(&'static str, &'static str)>,
    client_token: Option<&'static str>,
    /// The Region the create is signed for — what `CreatedFrom` must report.
    region: &'static str,
}

impl Row {
    /// The request body for this row, under `instance_arn`.
    fn request(&self, instance_arn: &str) -> Value {
        let mut body = json!({
            "InstanceArn": instance_arn,
            "ApplicationProviderArn": CUSTOM_PROVIDER_ARN,
            "Name": self.name,
        });
        if let Some(description) = &self.description {
            body["Description"] = json!(description);
        }
        if let Some(status) = self.status {
            body["Status"] = json!(status);
        }
        if let Some(portal_options) = &self.portal_options {
            body["PortalOptions"] = portal_options.clone();
        }
        if !self.tags.is_empty() {
            body["Tags"] = Value::Array(
                self.tags
                    .iter()
                    .map(|(key, value)| json!({"Key": key, "Value": value}))
                    .collect(),
            );
        }
        if let Some(client_token) = self.client_token {
            body["ClientToken"] = json!(client_token);
        }
        body
    }

    /// The `Status` a describe must report: the one stored, or the one an application the
    /// caller did not disable carries.
    fn expected_status(&self) -> &str {
        self.status.unwrap_or("ENABLED")
    }

    /// The `PortalOptions` a describe must report: the ones stored, with the model's own
    /// default for `Visibility` (`smithy_query(query="shape",shape="PortalOptions")` →
    /// `Visibility` `default: "ENABLED"`) standing in where the create sent none.
    fn expected_portal_options(&self) -> Value {
        let mut options = self.portal_options.clone().unwrap_or_else(|| json!({}));
        if options.get("Visibility").is_none() {
            options["Visibility"] = json!("ENABLED");
        }
        options
    }
}

/// The rows the rules are asserted over.
///
/// They differ along every member of a stored application a describe has to report back:
/// the length of `Name` at both published bounds (`len 1..100`, `^[\S\s]*$`, so spaces and
/// punctuation are admitted), whether `Description` was stored at all and its length at
/// both bounds (`len 1..128`), each `ApplicationStatus` (`ENABLED | DISABLED`), the shape
/// of `PortalOptions` (absent, `Visibility` only, each `SignInOrigin`), the Region the
/// create ran in, and whether the create carried members this output does not model
/// (`Tags`, `ClientToken`). Every constraint is from
/// `smithy_query(query="operation",operation="CreateApplication")` and
/// `smithy_query(query="shape",shape="PortalOptions,SignInOptions,ApplicationStatus")`.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "only the required members, from the harness Region",
            // `Name` at its lower length bound.
            name: "a".to_string(),
            description: None,
            status: None,
            portal_options: None,
            tags: Vec::new(),
            client_token: None,
            region: TEST_REGION,
        },
        Row {
            label: "every member of the create, including ones this output does not model",
            // `Name` at its upper length bound (100), spanning spaces and punctuation.
            name: format!("Data lake (prod) - {}", "x".repeat(81)),
            // `Description` at its upper length bound (128).
            description: Some("d".repeat(128)),
            status: Some("ENABLED"),
            portal_options: Some(json!({
                "SignInOptions": {
                    "Origin": "APPLICATION",
                    "ApplicationUrl": "https://lake.example.com/sso/start?tenant=one",
                },
                "Visibility": "ENABLED",
            })),
            tags: vec![("owner", "platform"), ("Env", "staging")],
            client_token: Some("f81d4fae-7dec-11d0-a765-00a0c91e6bf6"),
            region: "eu-west-2",
        },
        Row {
            label: "a DISABLED application hidden from the portal, created elsewhere",
            name: "disabled-app".to_string(),
            // `Description` at its lower length bound (1).
            description: Some("d".to_string()),
            status: Some("DISABLED"),
            portal_options: Some(json!({"Visibility": "DISABLED"})),
            tags: vec![("team", "identity")],
            client_token: None,
            region: "ap-southeast-2",
        },
        Row {
            label: "the IDENTITY_CENTER sign-in origin, from a four-segment Region",
            name: "identity center origin".to_string(),
            description: None,
            status: None,
            portal_options: Some(json!({"SignInOptions": {"Origin": "IDENTITY_CENTER"}})),
            tags: Vec::new(),
            client_token: Some("describe-rules-token-1"),
            region: "us-gov-east-1",
        },
    ]
}

/// `DescribeApplication` for `arn`, signed for `region`.
async fn describe_in(router: &MultiServiceRouter, region: &str, arn: &str) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "DescribeApplication",
        &json!({"ApplicationArn": arn}),
    )
    .await
}

/// The same describe, which must answer 200, and its body.
async fn describe_ok(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, body) = describe_in(router, TEST_REGION, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeApplication of {arn} must answer 200, got {status} {body}"
    );
    body
}

/// Create one application that must succeed, and return its `ApplicationArn`.
async fn create_ok(router: &MultiServiceRouter, instance_arn: &str, row: &Row) -> String {
    let (status, body) = call_in_region(
        router,
        row.region,
        "CreateApplication",
        &row.request(instance_arn),
    )
    .await;
    assert_eq!(
        status,
        200,
        "case {:?}: CreateApplication must answer 200, got {status} {body}",
        row.label
    );
    body["ApplicationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: no ApplicationArn in {body}", row.label))
        .to_string()
}

/// The identity store of `instance_arn`, read the way a client reads it.
async fn identity_store_arn(router: &MultiServiceRouter, instance_arn: &str) -> String {
    let (status, body) = call(
        router,
        "DescribeInstance",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeInstance of {instance_arn} expected 200, got {status} {body}"
    );
    body["IdentityStoreArn"]
        .as_str()
        .unwrap_or_else(|| panic!("DescribeInstance reported no IdentityStoreArn: {body}"))
        .to_string()
}

/// `GetApplicationAssignmentConfiguration` for `arn`, which must answer 200: the other
/// reporting operation that reads the SAME stored record, so a describe that had consumed
/// or moved anything would show in its answer.
async fn assignment_configuration(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> Value {
    let (status, body) = call(
        router,
        "GetApplicationAssignmentConfiguration",
        &json!({"ApplicationArn": arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the assignment configuration of {arn} must read, got {status} {body}"
    );
    body
}

/// Every stored application, rendered so a test can assert that a call changed NOTHING
/// about any of them — including the members this operation's output does not model.
fn stored_applications(engine: &Arc<engine::InMemoryEngine>) -> String {
    with_store(engine, |store| format!("{:?}", store.applications))
}

/// RULE 1 — a describe reports the stored record: exactly the modelled output members the
/// record holds a value for, each carrying what the create stored.
#[tokio::test]
async fn describe_reports_the_stored_record_and_only_the_modelled_members() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let identity_store = identity_store_arn(&router, &instance_arn).await;

    for row in rows() {
        let case = row.label.to_string();
        let arn = create_ok(&router, &instance_arn, &row).await;
        let described = describe_ok(&router, &case, &arn).await;

        let members = described
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: describe must answer an object, got {described}"));
        for member in members.keys() {
            assert!(
                MODELLED_OUTPUT_MEMBERS.contains(&member.as_str()),
                "case {case:?}: {member:?} is not a modelled output member of \
                 DescribeApplication, got {described}"
            );
        }
        for member in ALWAYS_REPORTED_MEMBERS {
            assert!(
                members.contains_key(member),
                "case {case:?}: every stored application reports {member}, got {described}"
            );
        }
        assert_eq!(
            members.contains_key("Description"),
            row.description.is_some(),
            "case {case:?}: Description is reported exactly when one was stored, got \
             {described}"
        );

        assert_eq!(
            described["ApplicationArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: describe must report the ARN it was asked about, got {described}"
        );
        assert_eq!(
            described["InstanceArn"].as_str(),
            Some(instance_arn.as_str()),
            "case {case:?}: describe must report the owning instance, got {described}"
        );
        assert_eq!(
            described["IdentityStoreArn"].as_str(),
            Some(identity_store.as_str()),
            "case {case:?}: describe must report that instance's identity store, got {described}"
        );
        assert_eq!(
            described["ApplicationAccount"].as_str(),
            arn.split(':').nth(4),
            "case {case:?}: ApplicationAccount must be the account of the ARN, got {described}"
        );
        assert_eq!(
            described["ApplicationProviderArn"].as_str(),
            Some(CUSTOM_PROVIDER_ARN),
            "case {case:?}: describe must report the provider stored, got {described}"
        );
        assert_eq!(
            described["Name"].as_str(),
            Some(row.name.as_str()),
            "case {case:?}: describe must report the stored Name, got {described}"
        );
        assert_eq!(
            described["Description"].as_str(),
            row.description.as_deref(),
            "case {case:?}: describe must report the stored Description, got {described}"
        );
        assert_eq!(
            described["Status"].as_str(),
            Some(row.expected_status()),
            "case {case:?}: describe must report the stored Status, got {described}"
        );
        assert_eq!(
            described["PortalOptions"],
            row.expected_portal_options(),
            "case {case:?}: describe must report the stored PortalOptions, got {described}"
        );
        assert_eq!(
            described["CreatedFrom"].as_str(),
            Some(row.region),
            "case {case:?}: CreatedFrom must be the Region of the create, got {described}"
        );
        assert!(
            described["CreatedDate"].as_f64().is_some_and(|date| date > 0.0),
            "case {case:?}: describe must report when the application was created, got \
             {described}"
        );
    }
}

/// RULE 2 — a describe is a READ: repeating it, from any Region, answers the identical
/// body and changes nothing any later caller sees.
#[tokio::test]
async fn describing_an_application_changes_nothing_a_later_caller_sees() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let instance_arn = seeded_instance_arn(&router).await;

    // Every row stored, so "nothing changed" is a claim about a store holding several
    // applications rather than about one. Each row carries the body its first describe
    // answered and the assignment configuration it read BEFORE the rounds below, so both
    // "nothing changed" claims are against what was observed, not against a literal.
    let mut stored: Vec<(String, String, Value, Value)> = Vec::new();
    for row in rows() {
        let case = row.label.to_string();
        let arn = create_ok(&router, &instance_arn, &row).await;
        let first = describe_ok(&router, &case, &arn).await;
        let configuration = assignment_configuration(&router, &case, &arn).await;
        stored.push((case, arn, first, configuration));
    }
    let before = stored_applications(&engine);

    // Read each application twice from each Region: the answer is a function of the
    // record, so neither the repeat nor the Region of the read may move it.
    for round in 1..=2u32 {
        for region in READ_REGIONS {
            for (case, arn, first, _) in &stored {
                let (status, again) = describe_in(&router, region, arn).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: round {round} from {region} must answer 200, got {status} \
                     {again}"
                );
                assert_eq!(
                    &again, first,
                    "case {case:?}: round {round} from {region} must answer the identical body, \
                     got {again}"
                );
            }
        }
    }

    let after = stored_applications(&engine);
    assert_eq!(
        after, before,
        "a describe writes nothing: the stored applications must be untouched, got {after}"
    );

    // And the reads left every OTHER reporting operation of an application answering as
    // it did: the assignment configuration an application carries is read off the same
    // record, so a describe that had consumed or moved anything would show here.
    for (case, arn, _, configuration_before) in &stored {
        let configuration_after = assignment_configuration(&router, case, arn).await;
        assert_eq!(
            &configuration_after, configuration_before,
            "case {case:?}: no describe may have altered the assignment configuration, got \
             {configuration_after}"
        );
    }
}

/// RULE 3 — an `ApplicationArn` nothing minted is refused `ResourceNotFoundException`
/// naming that ARN, whichever spelling the model's pattern admits.
#[tokio::test]
async fn an_application_arn_nothing_minted_is_refused_not_found() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let instance_arn = seeded_instance_arn(&router).await;
    let held = create_ok(&router, &instance_arn, &rows()[0]).await;
    let before = stored_applications(&engine);

    // The rows span the `ApplicationArn` pattern
    // `^arn:aws(-[a-z]{1,5}){0,3}:sso::\d{12}:application/(sso)?ins-[a-zA-Z0-9-.]{16}/apl-[a-zA-Z0-9]{16}$`:
    // the `ssoins-` and bare `ins-` instance prefixes, the full `[a-zA-Z0-9-.]` instance
    // id class, the `[a-zA-Z0-9]` application id class in both cases, and the partition
    // suffixes the pattern allows.
    let cases: [(&str, &str); 6] = [
        (
            "a pattern-valid ssoins-/apl- ARN nothing created",
            "arn:aws:sso::123456789012:application/ssoins-0000000000000000/apl-0000000000000000",
        ),
        (
            "the bare ins- instance spelling",
            "arn:aws:sso::123456789012:application/ins-abcdefghij012345/apl-abcdefghij012345",
        ),
        (
            "an instance id using the full [a-zA-Z0-9-.] class",
            "arn:aws:sso::123456789012:application/ssoins-Ab.9-cDeF0123456/apl-ZYXWVUTSRQ987654",
        ),
        (
            "another account's twelve digits",
            "arn:aws:sso::210987654321:application/ssoins-1111111111111111/apl-1111111111111111",
        ),
        (
            "a GovCloud partition ARN",
            "arn:aws-us-gov:sso::123456789012:application/ssoins-2222222222222222/apl-2222222222222222",
        ),
        (
            "a China partition ARN",
            "arn:aws-cn:sso::123456789012:application/ssoins-3333333333333333/apl-3333333333333333",
        ),
    ];

    for (case, arn) in cases {
        assert_ne!(
            arn, held,
            "case {case:?}: the row must name a DIFFERENT ARN than the stored one"
        );
        let (status, body) = describe_in(&router, TEST_REGION, arn).await;
        // 400 carrying the modelled code, so neither an empty 200 nor the scaffold's 501.
        assert_refused(
            case,
            "DescribeApplication",
            "ResourceNotFoundException",
            status,
            &body,
        );
        let message = body["Message"]
            .as_str()
            .or_else(|| body["message"].as_str())
            .unwrap_or_default();
        assert!(
            message.contains(arn),
            "case {case:?}: the refusal must name the ARN asked for, got {message:?}"
        );
        assert!(
            body["ApplicationArn"].is_null(),
            "case {case:?}: a refusal reports no application, got {body}"
        );
    }

    // None of those refusals touched the store, and the application that IS held still
    // reports.
    let after = stored_applications(&engine);
    assert_eq!(
        after, before,
        "a refused describe writes nothing, the store now holds {after}"
    );
    let described = describe_ok(&router, "the application that is held", &held).await;
    assert_eq!(
        described["ApplicationArn"].as_str(),
        Some(held.as_str()),
        "the stored application must still report itself, got {described}"
    );
}

/// RULE 4 — an application goes with its instance: it still reports while the removal is
/// pending, and once that removal falls due its ARN earns rule 3's refusal.
#[tokio::test]
async fn an_application_reports_while_its_instances_removal_is_pending_and_is_gone_once_due() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let instance_arn = seeded_instance_arn(&router).await;

    // Several applications, so the rule is shown over the whole set the instance holds.
    let mut stored: Vec<(String, String, Value)> = Vec::new();
    for row in rows() {
        let case = row.label.to_string();
        let arn = create_ok(&router, &instance_arn, &row).await;
        let body = describe_ok(&router, &case, &arn).await;
        stored.push((case, arn, body));
    }

    let (status, deleted) = call(
        &router,
        "DeleteInstance",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteInstance expected 200, got {status} {deleted}"
    );

    // Before the removal falls due the instance still holds its applications, and each
    // reports exactly what it reported before the delete.
    hold_deletion(&engine, SystemTime::now() + Duration::from_secs(3_600));
    for (case, arn, before) in &stored {
        let (status, pending) = describe_in(&router, TEST_REGION, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an application of an instance being deleted still reports, got \
             {status} {pending}"
        );
        assert_eq!(
            &pending, before,
            "case {case:?}: a pending removal changes nothing it reports, got {pending}"
        );
    }

    // Once it is due, every application the instance held is gone.
    hold_deletion(&engine, SystemTime::now() - Duration::from_secs(1));
    for (case, arn, _) in &stored {
        let (status, gone) = describe_in(&router, TEST_REGION, arn).await;
        assert_refused(
            case,
            "DescribeApplication of an application of a removed instance",
            "ResourceNotFoundException",
            status,
            &gone,
        );
        let message = gone["Message"]
            .as_str()
            .or_else(|| gone["message"].as_str())
            .unwrap_or_default();
        assert!(
            message.contains(arn.as_str()),
            "case {case:?}: the refusal must name the ARN asked for, got {message:?}"
        );
    }
    assert_eq!(
        with_store(&engine, |store| store.applications.len()),
        0,
        "a removed instance holds no application"
    );
}
