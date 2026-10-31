// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateImageUsageReport`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//! ec2Query is selected by the `ec2` signing name in the request's SigV4 credential scope, so
//! every call here carries one.
//!
//! Four rules are under test:
//!
//! 1. The create STORES ONE REPORT over the image, resource types and accounts the request
//!    names, and `DescribeImageUsageReports` reads back exactly that: every member the
//!    modelled `ImageUsageReport` carries is computed from the request and the clock, with the
//!    documented scope default filled in. The rows differ along what that generalises over —
//!    which resource types, how many, which scope options and values at each published bound,
//!    how many accounts, and how many tags.
//! 2. A created report is `pending` — "the report is being created and not available to view"
//!    (`ImageUsageReport$State`) — and the clock owes it `available` one
//!    [`ec2::state::TRANSITION_DELAY`] later, with its deletion owed 30 days out
//!    (`ImageUsageReport$ExpirationTime`).
//! 3. A `ClientToken` makes the create IDEMPOTENT: the same token answers the report it first
//!    created and stores nothing more, the same token with a different request is refused, and
//!    a different token creates a second report.
//! 4. A member a request may get wrong is REFUSED with EC2's own code and nothing is stored:
//!    an `ImageId` outside the AMI id form, one the account can see no image for (in each of
//!    the two id forms, which draw different codes), a resource type outside the two
//!    documented words, a scope option on a type that admits none, an option value outside the
//!    published range, an account id that is not one, a tag specification naming another
//!    resource type, and `DryRun`.

use std::time::Duration;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DEFAULT_IMAGE_USAGE_VERSION_DEPTH,
    IDEMPOTENT_PARAMETER_MISMATCH, IMAGE_USAGE_REPORT_ID_PREFIX, IMAGE_USAGE_REPORT_RETENTION,
    IMAGE_USAGE_REPORT_STATE_AVAILABLE, IMAGE_USAGE_REPORT_STATE_PENDING,
    IMAGE_USAGE_RESOURCE_TYPE_NAMES, IMAGE_USAGE_VERSION_DEPTH_OPTION,
    IMAGE_USAGE_VERSION_DEPTH_RANGE, ImageSpec, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, element, elements, error_code, error_message, indexed};
use super::wire::{child, child_text, document, set_items};

/// The Region every call here is signed for. A usage report carries no Region member of its
/// own, so the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// The two resource types a report may be created over: "Valid values: `ec2:Instance` |
/// `ec2:LaunchTemplate`" (`ImageUsageResourceTypeRequest$ResourceType`, read with
/// `smithy_query(query="shape",shape="ImageUsageResourceTypeRequest")`). Named here from that
/// member's documentation, the same source
/// [`IMAGE_USAGE_RESOURCE_TYPE_NAMES`] is named from — and
/// [`the_documented_catalog_is_what_the_service_accepts`] pins the two against each other.
const TYPE_INSTANCE: &str = "ec2:Instance";
const TYPE_LAUNCH_TEMPLATE: &str = "ec2:LaunchTemplate";

/// An account that is not the one this emulator answers for, as a 12-digit account id.
const OTHER_ACCOUNT: &str = "123456789012";

/// Account ids a report may be narrowed to: 12 decimal digits, the form
/// `ec2::state::aws_account_id_is_well_formed` accepts.
const ACCOUNT_A: &str = "111122223333";
const ACCOUNT_B: &str = "444455556666";

/// Long enough past [`TRANSITION_DELAY`] that a transition the clock owes is due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// One `CreateImageUsageReport` call carrying `params`, as its HTTP status and XML body.
async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "CreateImageUsageReport", params).await
}

/// One `DescribeImageUsageReports` call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeImageUsageReports", params).await
}

/// One `DeleteImageUsageReport` call, as its HTTP status and XML body.
async fn delete(router: &MultiServiceRouter, report_id: &str) -> (u16, String) {
    super::ec2_query::call(
        router,
        REGION,
        "DeleteImageUsageReport",
        &[param("ReportId", report_id)],
    )
    .await
}

/// One scope option of a resource type, as a table row carries it: the option name and the
/// values asked for.
type OptionRow<'a> = (&'a str, Vec<&'a str>);

/// One resource type of a request, as a table row carries it: the type and its scope options.
type TypeRow<'a> = (&'a str, Vec<OptionRow<'a>>);

/// The form fields that ask for `types`, as ec2Query flattens the `ResourceTypes` list (its
/// `xmlName` is `ResourceType`, each entry's options `ResourceTypeOption` and each option's
/// values `OptionValue` —
/// `smithy_query(query="operation",operation="CreateImageUsageReport")`).
fn resource_type_params(types: &[TypeRow<'_>]) -> Vec<(String, String)> {
    let mut params = Vec::new();
    for (index, (name, options)) in types.iter().enumerate() {
        let position = index + 1;
        params.push(param(&format!("ResourceType.{position}.ResourceType"), name));
        for (option_index, (option_name, values)) in options.iter().enumerate() {
            let option_position = option_index + 1;
            params.push(param(
                &format!(
                    "ResourceType.{position}.ResourceTypeOption.{option_position}.OptionName"
                ),
                option_name,
            ));
            for (value_index, value) in values.iter().enumerate() {
                params.push(param(
                    &format!(
                        "ResourceType.{position}.ResourceTypeOption.{option_position}\
                         .OptionValue.{}",
                        value_index + 1
                    ),
                    value,
                ));
            }
        }
    }
    params
}

/// The form fields that ask for `tags` on the report, as one `TagSpecification` naming the
/// report's own resource type. No tags asks for no specification at all.
fn tag_params(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    if tags.is_empty() {
        return Vec::new();
    }
    let mut params = vec![param("TagSpecification.1.ResourceType", "image-usage-report")];
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        params.push(param(&format!("TagSpecification.1.Tag.{position}.Key"), key));
        params.push(param(
            &format!("TagSpecification.1.Tag.{position}.Value"),
            value,
        ));
    }
    params
}

/// The whole parameter list of one create.
fn create_params(
    image_id: &str,
    types: &[TypeRow<'_>],
    accounts: &[&str],
    tags: &[(&str, &str)],
) -> Vec<(String, String)> {
    let mut params = vec![param("ImageId", image_id)];
    params.extend(resource_type_params(types));
    params.extend(indexed("AccountId", accounts));
    params.extend(tag_params(tags));
    params
}

/// The report id a successful create answers, having pinned the id form it was minted in.
async fn created_report_id(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> String {
    let (status, body) = create(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let report_id = element(&body, "reportId").unwrap_or_default();
    assert!(
        super::wire::is_long_id(IMAGE_USAGE_REPORT_ID_PREFIX, &report_id),
        "case {case:?}: expected a report id of the form {IMAGE_USAGE_REPORT_ID_PREFIX}-<17 hex \
         digits>, got {report_id:?} in {body}"
    );
    report_id
}

/// The XML of the one report `report_id` names, read back through the describe.
async fn describe_one(router: &MultiServiceRouter, case: &str, report_id: &str) -> String {
    let (status, body) = describe(router, &indexed("ReportId", &[report_id])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// Every image usage report id the store holds, in store order.
fn stored_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .image_usage_reports
        .keys()
        .cloned()
        .collect()
}

/// How many caller-chosen bytes the store is charged for right now.
fn stored_bytes(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().stored_bytes()
}

/// The AMIs every case here reports on: one the account owns, one another account published,
/// and one another account keeps private (which this account cannot see at all).
struct SeededImages {
    own: String,
    public_of_another_account: String,
    private_of_another_account: String,
}

/// Write those three AMIs into the store the way a register operation would.
fn seed_images(engine: &engine::InMemoryEngine) -> SeededImages {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    SeededImages {
        own: guard.insert_image(ImageSpec {
            name: "usage-report-own".to_string(),
            ..ImageSpec::default()
        }),
        public_of_another_account: guard.insert_image(ImageSpec {
            name: "usage-report-published".to_string(),
            owner_id: OTHER_ACCOUNT.to_string(),
            public: true,
            ..ImageSpec::default()
        }),
        private_of_another_account: guard.insert_image(ImageSpec {
            name: "usage-report-private-elsewhere".to_string(),
            owner_id: OTHER_ACCOUNT.to_string(),
            public: false,
            ..ImageSpec::default()
        }),
    }
}

/// The reported `resourceTypeSet` of the one report a describe body carries: each resource
/// type with its `version-depth` option values, in reported order.
fn reported_resource_types(case: &str, body: &str) -> Vec<(String, Vec<String>)> {
    let parsed = document(case, body);
    let report = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "reportId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no report (no <reportId>) in {body}"));
    set_items(report, "resourceTypeSet")
        .into_iter()
        .map(|entry| {
            let name = child_text(entry, "resourceType").unwrap_or_default();
            let values = set_items(entry, "resourceTypeOptionSet")
                .into_iter()
                .flat_map(|option| {
                    let option_name = child_text(option, "optionName").unwrap_or_default();
                    assert_eq!(
                        option_name, IMAGE_USAGE_VERSION_DEPTH_OPTION,
                        "case {case:?}: the only option a report carries is \
                         {IMAGE_USAGE_VERSION_DEPTH_OPTION:?}, got {option_name:?} in {body}"
                    );
                    set_items(option, "optionValueSet")
                        .into_iter()
                        .map(|value| value.text().unwrap_or_default().to_string())
                        .collect::<Vec<String>>()
                })
                .collect();
            (name, values)
        })
        .collect()
}

/// The account ids the one report a describe body carries reports, in reported order.
fn reported_account_ids(case: &str, body: &str) -> Vec<String> {
    let parsed = document(case, body);
    let report = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "reportId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no report (no <reportId>) in {body}"));
    set_items(report, "accountIdSet")
        .into_iter()
        .map(|item| item.text().unwrap_or_default().to_string())
        .collect()
}

/// The tags the one report a describe body carries reports, as key/value pairs.
fn reported_tags(case: &str, body: &str) -> Vec<(String, String)> {
    let parsed = document(case, body);
    let report = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "reportId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no report (no <reportId>) in {body}"));
    set_items(report, "tagSet")
        .into_iter()
        .map(|item| {
            (
                child_text(item, "key").unwrap_or_default(),
                child_text(item, "value").unwrap_or_default(),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The create computes the report, and the describe reads it back
// ---------------------------------------------------------------------------

/// The two words the handler accepts are the two the member documents, so the catalog under
/// test here and the one the service enforces cannot drift apart.
#[test]
fn the_documented_catalog_is_what_the_service_accepts() {
    assert_eq!(
        IMAGE_USAGE_RESOURCE_TYPE_NAMES,
        [TYPE_INSTANCE, TYPE_LAUNCH_TEMPLATE],
        "the resource type catalog must be the two words the member documents, got \
         {IMAGE_USAGE_RESOURCE_TYPE_NAMES:?}"
    );
}

/// One row of the create table.
struct CreateCase<'a> {
    case: &'a str,
    /// The resource types asked for, with their scope options.
    types: Vec<TypeRow<'a>>,
    /// The accounts asked for.
    accounts: Vec<&'a str>,
    /// The tags asked for.
    tags: Vec<(&'a str, &'a str)>,
    /// The resource types the report must report, each with its option values.
    reported: Vec<(&'a str, Vec<&'a str>)>,
    /// The accounts the report must report.
    reported_accounts: Vec<&'a str>,
}

/// Every member of a created report is computed from the request and the clock, and
/// `DescribeImageUsageReports` reads back exactly what was stored — including the documented
/// `version-depth` default a request that names no option gets.
///
/// Each row differs along what the rule generalises over: which resource types and how many,
/// whether a launch template carries a scope option, the option value at each published bound,
/// how many accounts (none being "all accounts") and how many tags.
#[tokio::test]
async fn a_created_report_is_computed_from_the_request_and_read_back() {
    let (router, engine) = super::ec2_query::emulator();
    let images = seed_images(&engine);
    let (depth_min, depth_max) = IMAGE_USAGE_VERSION_DEPTH_RANGE;
    let default_depth = DEFAULT_IMAGE_USAGE_VERSION_DEPTH.to_string();
    let min_depth = depth_min.to_string();
    let max_depth = depth_max.to_string();

    let cases: Vec<CreateCase<'_>> = vec![
        CreateCase {
            case: "instances only, every account, no tags",
            types: vec![(TYPE_INSTANCE, Vec::new())],
            accounts: Vec::new(),
            tags: Vec::new(),
            reported: vec![(TYPE_INSTANCE, Vec::new())],
            reported_accounts: Vec::new(),
        },
        CreateCase {
            case: "launch templates with no option take the documented default",
            types: vec![(TYPE_LAUNCH_TEMPLATE, Vec::new())],
            accounts: vec![ACCOUNT_A],
            tags: Vec::new(),
            reported: vec![(TYPE_LAUNCH_TEMPLATE, vec![default_depth.as_str()])],
            reported_accounts: vec![ACCOUNT_A],
        },
        CreateCase {
            case: "launch templates at the lowest version depth",
            types: vec![(
                TYPE_LAUNCH_TEMPLATE,
                vec![(IMAGE_USAGE_VERSION_DEPTH_OPTION, vec![min_depth.as_str()])],
            )],
            accounts: vec![ACCOUNT_A, ACCOUNT_B],
            tags: vec![("Owner", "TeamA")],
            reported: vec![(TYPE_LAUNCH_TEMPLATE, vec![min_depth.as_str()])],
            reported_accounts: vec![ACCOUNT_A, ACCOUNT_B],
        },
        CreateCase {
            case: "launch templates at the highest version depth",
            types: vec![(
                TYPE_LAUNCH_TEMPLATE,
                vec![(IMAGE_USAGE_VERSION_DEPTH_OPTION, vec![max_depth.as_str()])],
            )],
            accounts: vec![OTHER_ACCOUNT],
            tags: vec![("Owner", "TeamB"), ("Tier", "web")],
            reported: vec![(TYPE_LAUNCH_TEMPLATE, vec![max_depth.as_str()])],
            reported_accounts: vec![OTHER_ACCOUNT],
        },
        CreateCase {
            case: "both resource types, the launch template carrying an option",
            types: vec![
                (TYPE_INSTANCE, Vec::new()),
                (
                    TYPE_LAUNCH_TEMPLATE,
                    vec![(IMAGE_USAGE_VERSION_DEPTH_OPTION, vec!["7"])],
                ),
            ],
            accounts: vec![ACCOUNT_B],
            tags: vec![("Name", "both")],
            reported: vec![
                (TYPE_INSTANCE, Vec::new()),
                (TYPE_LAUNCH_TEMPLATE, vec!["7"]),
            ],
            reported_accounts: vec![ACCOUNT_B],
        },
        CreateCase {
            case: "one account named twice names it once",
            types: vec![(TYPE_INSTANCE, Vec::new())],
            accounts: vec![ACCOUNT_A, ACCOUNT_A],
            tags: Vec::new(),
            reported: vec![(TYPE_INSTANCE, Vec::new())],
            reported_accounts: vec![ACCOUNT_A],
        },
        CreateCase {
            case: "a report on an AMI another account published",
            types: vec![(TYPE_LAUNCH_TEMPLATE, Vec::new())],
            accounts: Vec::new(),
            tags: vec![("Owner", "TeamA"), ("Tier", "db"), ("Name", "published")],
            reported: vec![(TYPE_LAUNCH_TEMPLATE, vec![default_depth.as_str()])],
            reported_accounts: Vec::new(),
        },
    ];

    for (position, row) in cases.iter().enumerate() {
        let case = row.case;
        let image_id = if position + 1 == cases.len() {
            images.public_of_another_account.as_str()
        } else {
            images.own.as_str()
        };
        let params = create_params(image_id, &row.types, &row.accounts, &row.tags);
        let report_id = created_report_id(&router, case, &params).await;
        let body = describe_one(&router, case, &report_id).await;

        assert_eq!(
            element(&body, "imageId").as_deref(),
            Some(image_id),
            "case {case:?}: expected the report to name the image it was created for, got {body}"
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(IMAGE_USAGE_REPORT_STATE_PENDING),
            "case {case:?}: a report EC2 has just created is \
             {IMAGE_USAGE_REPORT_STATE_PENDING:?}, got {body}"
        );
        assert!(
            elements(&body, "stateReason").is_empty(),
            "case {case:?}: a report that is not in error carries no state reason, got {body}"
        );

        let reported = reported_resource_types(case, &body);
        let expected: Vec<(String, Vec<String>)> = row
            .reported
            .iter()
            .map(|(name, values)| {
                (
                    (*name).to_string(),
                    values.iter().map(|value| (*value).to_string()).collect(),
                )
            })
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the resource types {expected:?}, got {reported:?}"
        );

        let accounts = reported_account_ids(case, &body);
        let expected_accounts: Vec<String> = row
            .reported_accounts
            .iter()
            .map(|account| (*account).to_string())
            .collect();
        assert_eq!(
            accounts, expected_accounts,
            "case {case:?}: expected the accounts {expected_accounts:?}, got {accounts:?}"
        );

        let mut tags = reported_tags(case, &body);
        tags.sort();
        let mut expected_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        expected_tags.sort();
        assert_eq!(
            tags, expected_tags,
            "case {case:?}: expected the tags {expected_tags:?}, got {tags:?}"
        );
    }

    assert_eq!(
        stored_ids(&engine).len(),
        cases.len(),
        "every create stored one report of its own"
    );
}

/// A created report is `pending` and the clock owes it `available` one [`TRANSITION_DELAY`]
/// later, whether or not anybody asks again — and its `ExpirationTime` is its creation instant
/// plus the 30 days `ImageUsageReport$ExpirationTime` documents.
#[tokio::test]
async fn the_clock_owes_a_created_report_available_and_a_deletion_30_days_out() {
    let (router, engine) = super::ec2_query::emulator();
    let images = seed_images(&engine);

    let params = create_params(
        &images.own,
        &[(TYPE_INSTANCE, Vec::new())],
        &[ACCOUNT_A],
        &[("Owner", "TeamA")],
    );
    let report_id = created_report_id(&router, "the clock", &params).await;

    let body = describe_one(&router, "straight after the create", &report_id).await;
    assert_eq!(
        element(&body, "state").as_deref(),
        Some(IMAGE_USAGE_REPORT_STATE_PENDING),
        "straight after the create: expected {IMAGE_USAGE_REPORT_STATE_PENDING:?}, got {body}"
    );

    let created_at = element(&body, "creationTime")
        .and_then(|at| as_timestamp(&at))
        .unwrap_or_else(|| panic!("the report must carry a creation instant, got {body}"));
    let expires_at = element(&body, "expirationTime")
        .and_then(|at| as_timestamp(&at))
        .unwrap_or_else(|| panic!("the report must carry an expiration instant, got {body}"));
    let retention = IMAGE_USAGE_REPORT_RETENTION.as_secs_f64();
    assert!(
        (expires_at - created_at - retention).abs() < 1.0,
        "the deletion is due {retention} seconds after the create, got {} ({body})",
        expires_at - created_at
    );

    tokio::time::sleep(past_the_transition()).await;
    let body = describe_one(&router, "past the transition", &report_id).await;
    assert_eq!(
        element(&body, "state").as_deref(),
        Some(IMAGE_USAGE_REPORT_STATE_AVAILABLE),
        "past the transition: expected {IMAGE_USAGE_REPORT_STATE_AVAILABLE:?}, got {body}"
    );

    // A report created after that move is `pending` of its own: the state is a fact of each
    // report rather than of the store.
    let later = created_report_id(&router, "a later create", &params).await;
    let body = describe_one(&router, "a later create", &later).await;
    assert_eq!(
        element(&body, "state").as_deref(),
        Some(IMAGE_USAGE_REPORT_STATE_PENDING),
        "a later create: expected {IMAGE_USAGE_REPORT_STATE_PENDING:?}, got {body}"
    );
    assert_ne!(
        later, report_id,
        "two creates without a client token mint two reports"
    );
}

/// A `ClientToken` makes the create idempotent: the same token answers the report it first
/// created and stores nothing more, the same token with a different request is refused with
/// `IdempotentParameterMismatch`, and a different token creates a second report. Deleting a
/// report gives the store's byte budget back what the create charged.
#[tokio::test]
async fn a_client_token_makes_the_create_idempotent() {
    let (router, engine) = super::ec2_query::emulator();
    let images = seed_images(&engine);
    let before = stored_bytes(&engine);

    let base = create_params(
        &images.own,
        &[(TYPE_INSTANCE, Vec::new())],
        &[ACCOUNT_A],
        &[("Owner", "TeamA")],
    );
    let mut first = base.clone();
    first.push(param("ClientToken", "usage-report-token"));

    let report_id = created_report_id(&router, "the first create", &first).await;
    let replayed = created_report_id(&router, "the same token again", &first).await;
    assert_eq!(
        replayed, report_id,
        "the same token answers the report it first created"
    );
    assert_eq!(
        stored_ids(&engine),
        vec![report_id.clone()],
        "a replay creates nothing"
    );

    // `(case, the request beside the token)`: each row changes exactly one member of what the
    // token was first given with.
    let mismatches: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "another image",
            create_params(
                &images.public_of_another_account,
                &[(TYPE_INSTANCE, Vec::new())],
                &[ACCOUNT_A],
                &[("Owner", "TeamA")],
            ),
        ),
        (
            "another resource type",
            create_params(
                &images.own,
                &[(TYPE_LAUNCH_TEMPLATE, Vec::new())],
                &[ACCOUNT_A],
                &[("Owner", "TeamA")],
            ),
        ),
        (
            "another account",
            create_params(
                &images.own,
                &[(TYPE_INSTANCE, Vec::new())],
                &[ACCOUNT_B],
                &[("Owner", "TeamA")],
            ),
        ),
        (
            "another tag value",
            create_params(
                &images.own,
                &[(TYPE_INSTANCE, Vec::new())],
                &[ACCOUNT_A],
                &[("Owner", "TeamB")],
            ),
        ),
    ];
    for (case, request) in mismatches {
        let mut params = request;
        params.push(param("ClientToken", "usage-report-token"));
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            IDEMPOTENT_PARAMETER_MISMATCH,
            "case {case:?}: expected {IDEMPOTENT_PARAMETER_MISMATCH}, got {body}"
        );
        assert_eq!(
            stored_ids(&engine),
            vec![report_id.clone()],
            "case {case:?}: a refused replay stores nothing"
        );
    }

    let mut second = base.clone();
    second.push(param("ClientToken", "another-usage-report-token"));
    let other = created_report_id(&router, "another token", &second).await;
    assert_ne!(
        other, report_id,
        "another token creates a report of its own"
    );

    for report_id in [report_id, other] {
        let (status, body) = delete(&router, &report_id).await;
        assert_eq!(
            status, 200,
            "deleting {report_id}: expected HTTP 200, got {status} with body {body}"
        );
    }
    assert!(
        stored_ids(&engine).is_empty(),
        "the deletes removed every report"
    );
    assert_eq!(
        stored_bytes(&engine),
        before,
        "the bytes a created report and its token were charged are given back when it is \
         removed"
    );
}

// ---------------------------------------------------------------------------
// What the create refuses
// ---------------------------------------------------------------------------

/// One row of the refusal table: the case, the parameters the create carries, and the error
/// code due.
type RefusalCase = (String, Vec<(String, String)>, &'static str);

/// One row of the dry-run table: the case, the parameters beside `DryRun`, and the HTTP status
/// and error code due.
type DryRunCase<'a> = (&'a str, Vec<(String, String)>, u16, &'a str);

/// Every member a request may get wrong is refused with EC2's own code, before anything is
/// stored.
///
/// Each row differs along what the rule generalises over: which member is wrong, and in which
/// way — an id outside the form, an id inside the form naming nothing the account can see, a
/// word outside the documented catalog, a value outside the published range (on each bound and
/// one past it), and members that cannot hold together.
#[tokio::test]
async fn a_request_outside_the_documented_contract_is_refused_and_stores_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let images = seed_images(&engine);
    let (depth_min, depth_max) = IMAGE_USAGE_VERSION_DEPTH_RANGE;
    let below_min = (depth_min - 1).to_string();
    let above_max = (depth_max + 1).to_string();

    // `(case, the parameters, the code due)`.
    let mut cases: Vec<RefusalCase> = vec![
        (
            "an image id that is not an AMI id at all".to_string(),
            create_params("not-an-ami", &[(TYPE_INSTANCE, Vec::new())], &[], &[]),
            "InvalidAMIID.Malformed",
        ),
        (
            "an instance id where an AMI id belongs".to_string(),
            create_params(
                "i-0123456789abcdef0",
                &[(TYPE_INSTANCE, Vec::new())],
                &[],
                &[],
            ),
            "InvalidAMIID.Malformed",
        ),
        (
            "an AMI id with one hex digit too many".to_string(),
            create_params(
                "ami-0123456789abcdef01",
                &[(TYPE_INSTANCE, Vec::new())],
                &[],
                &[],
            ),
            "InvalidAMIID.Malformed",
        ),
        (
            "a short-form AMI id the account has no image for".to_string(),
            create_params("ami-00000000", &[(TYPE_INSTANCE, Vec::new())], &[], &[]),
            "InvalidAMIID.NotFound",
        ),
        (
            // The LONG form is EC2's obfuscated id, so one that names no image the account
            // can see is not an id it was ever issued: it is refused as malformed before any
            // lookup, where a legacy short-form id is simply absent.
            "a long-form AMI id the account has no image for".to_string(),
            create_params(
                "ami-0ffffffffffffffff",
                &[(TYPE_INSTANCE, Vec::new())],
                &[],
                &[],
            ),
            "InvalidAMIID.Malformed",
        ),
        (
            // Stored under a minted long-form id, so the same reading applies: this account
            // cannot see the image, so the id is not one it was issued.
            "an AMI another account keeps private".to_string(),
            create_params(
                &images.private_of_another_account,
                &[(TYPE_INSTANCE, Vec::new())],
                &[],
                &[],
            ),
            "InvalidAMIID.Malformed",
        ),
        (
            // `ResourceTypes` is a modelled `@required` member
            // (`smithy_query(query="operation",operation="CreateImageUsageReport")`), so a
            // request that names none is refused by the boundary validator — with its own
            // code for a required member the request omitted — before this handler runs.
            "no resource type at all".to_string(),
            create_params(&images.own, &[], &[], &[]),
            "MalformedInput",
        ),
        (
            "a resource type outside the documented catalog".to_string(),
            create_params(&images.own, &[("ec2:Volume", Vec::new())], &[], &[]),
            "InvalidParameterValue",
        ),
        (
            "a resource type in the wrong case".to_string(),
            create_params(&images.own, &[("ec2:instance", Vec::new())], &[], &[]),
            "InvalidParameterValue",
        ),
        (
            "one resource type named twice".to_string(),
            create_params(
                &images.own,
                &[(TYPE_INSTANCE, Vec::new()), (TYPE_INSTANCE, Vec::new())],
                &[],
                &[],
            ),
            "InvalidParameterValue",
        ),
        (
            "a scope option on a resource type that admits none".to_string(),
            create_params(
                &images.own,
                &[(
                    TYPE_INSTANCE,
                    vec![(IMAGE_USAGE_VERSION_DEPTH_OPTION, vec!["5"])],
                )],
                &[],
                &[],
            ),
            "InvalidParameterCombination",
        ),
        (
            "a scope option outside the documented catalog".to_string(),
            create_params(
                &images.own,
                &[(TYPE_LAUNCH_TEMPLATE, vec![("version-count", vec!["5"])])],
                &[],
                &[],
            ),
            "InvalidParameterValue",
        ),
        (
            format!("a version depth below the published minimum of {depth_min}"),
            create_params(
                &images.own,
                &[(
                    TYPE_LAUNCH_TEMPLATE,
                    vec![(IMAGE_USAGE_VERSION_DEPTH_OPTION, vec![&below_min])],
                )],
                &[],
                &[],
            ),
            "InvalidParameterValue",
        ),
        (
            format!("a version depth above the published maximum of {depth_max}"),
            create_params(
                &images.own,
                &[(
                    TYPE_LAUNCH_TEMPLATE,
                    vec![(IMAGE_USAGE_VERSION_DEPTH_OPTION, vec![&above_max])],
                )],
                &[],
                &[],
            ),
            "InvalidParameterValue",
        ),
        (
            "a version depth that is not an integer".to_string(),
            create_params(
                &images.own,
                &[(
                    TYPE_LAUNCH_TEMPLATE,
                    vec![(IMAGE_USAGE_VERSION_DEPTH_OPTION, vec!["twenty"])],
                )],
                &[],
                &[],
            ),
            "InvalidParameterValue",
        ),
        (
            // "A value for the specified option" is ONE value, so several is refused rather
            // than one of them being chosen.
            "a scope option carrying two values".to_string(),
            create_params(
                &images.own,
                &[(
                    TYPE_LAUNCH_TEMPLATE,
                    vec![(IMAGE_USAGE_VERSION_DEPTH_OPTION, vec!["5", "7"])],
                )],
                &[],
                &[],
            ),
            "InvalidParameterValue",
        ),
        (
            // One option named twice would silently discard the first.
            "the same scope option named twice".to_string(),
            create_params(
                &images.own,
                &[(
                    TYPE_LAUNCH_TEMPLATE,
                    vec![
                        (IMAGE_USAGE_VERSION_DEPTH_OPTION, vec!["5"]),
                        (IMAGE_USAGE_VERSION_DEPTH_OPTION, vec!["7"]),
                    ],
                )],
                &[],
                &[],
            ),
            "InvalidParameterValue",
        ),
        (
            "an account id of the wrong length".to_string(),
            create_params(
                &images.own,
                &[(TYPE_INSTANCE, Vec::new())],
                &["12345"],
                &[],
            ),
            "InvalidUserID.Malformed",
        ),
        (
            "an account id that is not digits".to_string(),
            create_params(
                &images.own,
                &[(TYPE_INSTANCE, Vec::new())],
                &["abcdefghijkl"],
                &[],
            ),
            "InvalidUserID.Malformed",
        ),
    ];

    // A tag specification naming any resource type other than the report itself: "the
    // ResourceType must be set to image-usage-report; any other value will cause the report
    // creation to fail" (`$TagSpecifications`).
    let mut mistyped_tags = create_params(&images.own, &[(TYPE_INSTANCE, Vec::new())], &[], &[]);
    mistyped_tags.push(param("TagSpecification.1.ResourceType", "image"));
    mistyped_tags.push(param("TagSpecification.1.Tag.1.Key", "Owner"));
    mistyped_tags.push(param("TagSpecification.1.Tag.1.Value", "TeamA"));
    cases.push((
        "a tag specification naming another resource type".to_string(),
        mistyped_tags,
        "InvalidParameterValue",
    ));

    for (case, params, code) in cases {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {body}"
        );
        assert!(
            stored_ids(&engine).is_empty(),
            "case {case:?}: a refused create stores nothing, got {:?}",
            stored_ids(&engine)
        );
    }
}

/// An AMI id the account can see no image for is refused with EC2's own sentence for the code
/// that id form draws, whose one variable part is the id the request sent: the legacy SHORT
/// form is absent (`InvalidAMIID.NotFound`), while a value in the obfuscated LONG form that
/// names no visible image is not an id this account was issued (`InvalidAMIID.Malformed`).
///
/// The rows differ along what that generalises over: the id form, and whether the image is
/// held by another account or by nobody at all.
#[tokio::test]
async fn an_unresolvable_image_is_refused_with_ec2s_own_sentence() {
    let (router, engine) = super::ec2_query::emulator();
    let images = seed_images(&engine);

    // `(case, the id, the code due, the sentence due)`.
    let cases: Vec<(&str, String, &str, String)> = vec![
        (
            "a short-form id nothing is stored under",
            "ami-00000000".to_string(),
            "InvalidAMIID.NotFound",
            "The image id '[ami-00000000]' does not exist".to_string(),
        ),
        (
            "another short-form id nothing is stored under",
            "ami-0fffffff".to_string(),
            "InvalidAMIID.NotFound",
            "The image id '[ami-0fffffff]' does not exist".to_string(),
        ),
        (
            "a long-form id nothing is stored under",
            "ami-0ffffffffffffffff".to_string(),
            "InvalidAMIID.Malformed",
            "The image ID 'ami-0ffffffffffffffff' is malformed".to_string(),
        ),
        (
            "a long-form id another account keeps private",
            images.private_of_another_account.clone(),
            "InvalidAMIID.Malformed",
            format!(
                "The image ID '{}' is malformed",
                images.private_of_another_account
            ),
        ),
    ];

    for (case, image_id, code, sentence) in cases {
        let params = create_params(&image_id, &[(TYPE_INSTANCE, Vec::new())], &[], &[]);
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            sentence,
            "case {case:?}: expected EC2's own sentence, got {body}"
        );
    }
}

/// `DryRun` — "checks whether you have the required permissions for the action, without
/// actually making the request" — answers whether the request WOULD have succeeded and stores
/// nothing either way: `DryRunOperation` when it would, and the reason it would not when it
/// would not.
#[tokio::test]
async fn a_dry_run_create_stores_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let images = seed_images(&engine);

    // `(case, the parameters beside DryRun, the status due, the code due)`.
    let cases: Vec<DryRunCase<'_>> = vec![
        (
            "a request that would have succeeded",
            create_params(
                &images.own,
                &[(TYPE_INSTANCE, Vec::new())],
                &[ACCOUNT_A],
                &[("Owner", "TeamA")],
            ),
            412,
            "DryRunOperation",
        ),
        (
            "a request naming an image the account cannot see",
            create_params("ami-00000000", &[(TYPE_INSTANCE, Vec::new())], &[], &[]),
            400,
            "InvalidAMIID.NotFound",
        ),
        (
            // Refused by the boundary validator for the modelled `@required` member, ahead of
            // the flag, so a dry run of it is refused for that reason too.
            "a request naming no resource type",
            create_params(&images.own, &[], &[], &[]),
            400,
            "MalformedInput",
        ),
    ];

    for (case, params, status_due, code) in cases {
        let mut params = params;
        params.push(param("DryRun", "true"));
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, status_due,
            "case {case:?}: expected HTTP {status_due}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {body}"
        );
        assert!(
            stored_ids(&engine).is_empty(),
            "case {case:?}: a dry run stores nothing, got {:?}",
            stored_ids(&engine)
        );
    }
}
