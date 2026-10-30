// SPDX-License-Identifier: CC0-1.0

//! `ListGroupResources` over a group whose membership comes from its resource
//! query, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the lifecycle suite's `Emulator`), so these
//! exercise the whole restJson1 path: routing, boundary validation, the handler,
//! and serialization.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * a group's membership comes from its DEFINITION — a group created with a
//!   `ResourceQuery` lists the resources that query selects NOW, and a group
//!   created without one lists the memberships recorded against it;
//! * a `TAG_FILTERS_1_0` query selects the recorded resources whose type its
//!   `ResourceTypeFilters` admits (every type, for `AWS::AllSupported` or for no
//!   such element) and whose tags satisfy every one of its `TagFilters`, where the
//!   keys narrow and one key's values widen;
//! * a `CLOUDFORMATION_STACK_1_0` query names a stack this service records none of,
//!   so it is answered with the modelled `CLOUDFORMATION_STACK_NOT_EXISTING` query
//!   error and no resource;
//! * the membership is what the group's CURRENT query selects, so updating the
//!   query moves it, and a `resource-type` filter narrows it;
//! * a query runs over the group's own region and reaches no other region's
//!   resources;
//! * the listing pages through every match exactly once whatever the page size;
//! * listing REPORTS: the groups, their queries, their tags and their memberships
//!   are unchanged afterwards.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::resource_group_lifecycle::{
    error_code, is_modelled_token, static_group, Emulator, ACCOUNT,
};
use super::ungroup_resources::sorted;

/// The URI the model binds this operation to
/// (`smithy_query(query="operation",operation="ListGroupResources")` answers
/// `POST /list-group-resources`).
const LIST: &str = "/list-group-resources";

/// The region every fixture below lives in, unless a case says otherwise.
const HOME: &str = "us-east-1";

/// The fixture groups: three carrying tags a tag query selects among, and one
/// configured to hold the static memberships.
const ALPHA: &str = "member-alpha";
const BETA: &str = "member-beta";
const GAMMA: &str = "member-gamma";
const STATIC: &str = "member-static";

/// Resource ARNs recorded as static memberships, spanning the forms the modelled
/// `ResourceArn` admits and the types this emulator does and does not have a
/// CloudFormation-style name for.
const INSTANCE: &str = "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0";
const BUCKET: &str = "arn:aws:s3:::query-probe-bucket";
const QUEUE: &str = "arn:aws:sqs:us-east-1:000000000000:query-probe-queue";
const VAULT: &str = "arn:aws:glacier:us-east-1:000000000000:vaults/records";

/// The resource type a resource group's own ARN names.
///
/// Source: the CloudFormation resource-type reference, which is the vocabulary
/// `ResourceIdentifier$ResourceType` is documented in
/// (`smithy_query(query="shape",shape="ResourceIdentifier")`: "such as
/// `AWS::EC2::Instance`").
const GROUP_TYPE: &str = "AWS::ResourceGroups::Group";

/// Every value the modelled `QueryErrorCode` enum declares.
///
/// Source: `smithy_query(query="shape",shape="QueryErrorCode")`, which answers
/// exactly these four.
const QUERY_ERROR_CODES: [&str; 4] = [
    "CLOUDFORMATION_STACK_INACTIVE",
    "CLOUDFORMATION_STACK_NOT_EXISTING",
    "CLOUDFORMATION_STACK_UNASSUMABLE_ROLE",
    "RESOURCE_TYPE_NOT_SUPPORTED",
];

/// The name of the group each case defines with the query under test.
const PROBE: &str = "query-probe";

/// A `ResourceQuery` of the tag-filter type carrying `document` as its `Query`.
///
/// The model carries the query document as a JSON STRING inside the structure
/// (`smithy_query(query="shape",shape="ResourceQuery")`), which is what the
/// `to_string` below writes.
fn tag_query(document: &Value) -> Value {
    json!({ "Type": "TAG_FILTERS_1_0", "Query": document.to_string() })
}

/// A `ResourceQuery` of the CloudFormation-stack type carrying `document`.
fn stack_query(document: &Value) -> Value {
    json!({ "Type": "CLOUDFORMATION_STACK_1_0", "Query": document.to_string() })
}

/// The fixture emulator: the tagged groups and the static memberships every query
/// below runs over, with each group's stored ARN read back from its create.
struct Fixtures {
    emulator: Emulator,
    /// Each group's name mapped to the ARN `CreateGroup` reported for it.
    group_arns: BTreeMap<String, String>,
}

impl Fixtures {
    /// Create the fixture groups of [`HOME`]: three tagged groups a tag filter
    /// selects among, and one configured group holding every static membership.
    async fn new() -> Self {
        let mut fixtures = Self {
            emulator: Emulator::new(),
            group_arns: BTreeMap::new(),
        };
        for (name, tags) in [
            (ALPHA, json!({ "Stage": "Test", "Team": "Platform" })),
            (BETA, json!({ "Stage": "Deploy", "Team": "Platform" })),
            (GAMMA, json!({ "Stage": "Archived", "Version": "4" })),
        ] {
            fixtures
                .create(HOME, &json!({ "Name": name, "Tags": tags }))
                .await;
        }
        fixtures.create(HOME, &static_group(STATIC)).await;
        fixtures
            .emulator
            .ok(
                "/group-resources",
                &json!({ "Group": STATIC, "ResourceArns": [INSTANCE, BUCKET, QUEUE, VAULT] }),
            )
            .await;
        fixtures
    }

    /// Create the group `request` describes in `region` and record the ARN the
    /// create reported for it.
    async fn create(&mut self, region: &str, request: &Value) -> String {
        let created = self.emulator.ok_in(region, "/groups", request).await;
        let arn = created["Group"]["GroupArn"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateGroup must answer a GroupArn, got {created}"))
            .to_string();
        let name = created["Group"]["Name"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateGroup must answer a Name, got {created}"))
            .to_string();
        self.group_arns.insert(name, arn.clone());
        arn
    }

    /// Create [`PROBE`] in [`HOME`] defined by `query`, and return its own ARN.
    async fn probe(&mut self, query: &Value) -> String {
        self.create(HOME, &json!({ "Name": PROBE, "ResourceQuery": query }))
            .await
    }

    /// The stored ARN of the fixture group `name`.
    fn arn(&self, name: &str) -> String {
        self.group_arns
            .get(name)
            .unwrap_or_else(|| panic!("the fixtures must hold a group named {name:?}"))
            .clone()
    }

    /// Every resource a query of [`HOME`] can select: each group stored there and
    /// each recorded membership, in ARN order.
    fn every_resource(&self) -> Vec<String> {
        let mut arns: Vec<String> = self.group_arns.values().cloned().collect();
        arns.extend([INSTANCE, BUCKET, QUEUE, VAULT].map(str::to_string));
        sorted(arns)
    }

    /// List the membership of `group` in [`HOME`], requiring the modelled 200.
    async fn list(&self, group: &str) -> Value {
        self.emulator.ok(LIST, &json!({ "Group": group })).await
    }

    /// The ARNs the membership listing of `group` reports, in the order it reports
    /// them.
    async fn listed(&self, case: &str, group: &str) -> Vec<String> {
        let page = self.list(group).await;
        arns_of(case, &page)
    }
}

/// The `ResourceIdentifiers` of a page as (ARN, reported type) pairs, in the order
/// the page holds them — asserting as it reads that `Resources` reports the same
/// identifiers, since the two members are the same membership.
fn identifiers_of(case: &str, page: &Value) -> Vec<(String, Option<String>)> {
    let identifiers = page["ResourceIdentifiers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected ResourceIdentifiers, got {page}"));
    let from_items: Vec<Value> = page["Resources"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected Resources, got {page}"))
        .iter()
        .map(|item| item["Identifier"].clone())
        .collect();
    assert_eq!(
        identifiers, &from_items,
        "case {case:?}: expected ResourceIdentifiers to mirror Resources, got {page}",
    );
    identifiers
        .iter()
        .map(|identifier| {
            let arn = identifier["ResourceArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a ResourceArn, got {identifier}"))
                .to_string();
            (arn, identifier["ResourceType"].as_str().map(str::to_string))
        })
        .collect()
}

/// The ARNs of a page, in the order the page holds them.
fn arns_of(case: &str, page: &Value) -> Vec<String> {
    identifiers_of(case, page)
        .into_iter()
        .map(|(arn, _)| arn)
        .collect()
}

/// The `QueryErrors` of a page as (ErrorCode, Message) pairs.
fn query_errors_of(case: &str, page: &Value) -> Vec<(Option<String>, Option<String>)> {
    page["QueryErrors"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected QueryErrors, got {page}"))
        .iter()
        .map(|error| {
            (
                error["ErrorCode"].as_str().map(str::to_string),
                error["Message"].as_str().map(str::to_string),
            )
        })
        .collect()
}

/// A group defined by a tag query lists the resources that query selects: the
/// recorded resources whose type its `ResourceTypeFilters` admits and whose tags
/// satisfy every one of its `TagFilters`.
///
/// Each row is a whole query document, and the rows differ along what the rule
/// generalises over: how many tag keys the query names, how many values each key
/// carries, a key with no values at all, the case of a key and of a value, which
/// resource types the query admits, how many it names, and the two spellings that
/// name no type limit at all.
#[tokio::test]
async fn a_query_group_lists_the_resources_its_query_selects() {
    // Each case gets its own fixtures, so the group the case itself defines is the
    // only query group among the resources the query runs over.
    for index in 0..QUERY_CASES {
        let mut fixtures = Fixtures::new().await;
        let document = query_document(index);
        // The probe group is itself a resource of its region, so a query that
        // admits the group type selects it too: the expectations below are read
        // from the store AFTER the probe is created.
        fixtures.probe(&tag_query(&document)).await;
        let expected = query_expectation(index, &fixtures);
        let case = document.to_string();
        let answered = fixtures.listed(&case, PROBE).await;
        assert_eq!(
            answered, expected,
            "case {case}: expected {expected:?}, got {answered:?}",
        );
    }
}

/// How many rows the query table holds.
const QUERY_CASES: usize = 17;

/// The `Query` document of row `index` of the query table.
fn query_document(index: usize) -> Value {
    match index {
        // One key, one value.
        0 => json!({ "TagFilters": [{ "Key": "Stage", "Values": ["Test"] }] }),
        // One key, several values: the matches are the union.
        1 => json!({ "TagFilters": [{ "Key": "Stage", "Values": ["Test", "Deploy"] }] }),
        // Several keys narrow: a resource must carry all of them.
        2 => json!({
            "TagFilters": [
                { "Key": "Stage", "Values": ["Test"] },
                { "Key": "Team", "Values": ["Platform"] }
            ]
        }),
        // Every key, but no value of one of them matches.
        3 => json!({
            "TagFilters": [
                { "Key": "Stage", "Values": ["Archived"] },
                { "Key": "Version", "Values": ["1", "2"] }
            ]
        }),
        // A filter that names no value is satisfied by carrying the key.
        4 => json!({ "TagFilters": [{ "Key": "Team" }] }),
        5 => json!({ "TagFilters": [{ "Key": "Stage", "Values": [] }] }),
        // Tag keys and values are case-sensitive.
        6 => json!({ "TagFilters": [{ "Key": "Stage", "Values": ["test"] }] }),
        7 => json!({ "TagFilters": [{ "Key": "stage", "Values": ["Test"] }] }),
        // A key nothing carries.
        8 => json!({ "TagFilters": [{ "Key": "Nothing", "Values": ["Test"] }] }),
        // `ResourceTypeFilters` limits the selection to the types it names.
        9 => json!({ "ResourceTypeFilters": ["AWS::EC2::Instance"] }),
        10 => json!({ "ResourceTypeFilters": ["AWS::EC2::Instance", "AWS::S3::Bucket"] }),
        11 => json!({ "ResourceTypeFilters": ["AWS::SQS::Queue"] }),
        // A supported type nothing recorded is of.
        12 => json!({ "ResourceTypeFilters": ["AWS::Lambda::Function"] }),
        // Type and tag filters narrow together: a group's own ARN names a resource
        // of the group type, and a group is the only kind of resource this service
        // records tags for.
        13 => json!({
            "ResourceTypeFilters": [GROUP_TYPE],
            "TagFilters": [{ "Key": "Stage", "Values": ["Archived"] }]
        }),
        14 => json!({ "ResourceTypeFilters": [GROUP_TYPE] }),
        // `AWS::AllSupported` is no limit at all, and neither is an absent element:
        // with no tag filter either, every recorded resource is selected.
        15 => json!({ "ResourceTypeFilters": ["AWS::AllSupported"], "TagFilters": [] }),
        16 => json!({}),
        other => panic!("the query table has no row {other}"),
    }
}

/// The ARNs row `index` of the query table must select, read out of the fixtures
/// the row runs against.
fn query_expectation(index: usize, fixtures: &Fixtures) -> Vec<String> {
    let alpha = fixtures.arn(ALPHA);
    let beta = fixtures.arn(BETA);
    let gamma = fixtures.arn(GAMMA);
    match index {
        0 => vec![alpha],
        1 => sorted([alpha, beta]),
        2 => vec![alpha],
        3 => Vec::new(),
        4 => sorted([alpha, beta]),
        5 => sorted([alpha, beta, gamma]),
        6..=8 => Vec::new(),
        9 => vec![INSTANCE.to_string()],
        10 => sorted([INSTANCE, BUCKET]),
        11 => vec![QUEUE.to_string()],
        12 => Vec::new(),
        13 => vec![gamma],
        14 => sorted(fixtures.group_arns.values().cloned()),
        15 | 16 => fixtures.every_resource(),
        other => panic!("the query table has no row {other}"),
    }
}

/// A query-derived item carries the resource type derived from its ARN — absent for
/// an ARN form this emulator has no type name for — and no `Status`: the modelled
/// `PENDING` means an addition that has not finished, and a query selects now.
#[tokio::test]
async fn query_derived_items_carry_the_derived_type_and_no_status() {
    let mut fixtures = Fixtures::new().await;
    fixtures.probe(&tag_query(&json!({}))).await;
    let page = fixtures.list(PROBE).await;
    let reported: BTreeMap<String, Option<String>> = identifiers_of("derived types", &page)
        .into_iter()
        .collect();
    // (the ARN, the type this emulator names for it)
    let expected: Vec<(String, Option<&str>)> = vec![
        (fixtures.arn(ALPHA), Some(GROUP_TYPE)),
        (fixtures.arn(STATIC), Some(GROUP_TYPE)),
        (INSTANCE.to_string(), Some("AWS::EC2::Instance")),
        (BUCKET.to_string(), Some("AWS::S3::Bucket")),
        (QUEUE.to_string(), Some("AWS::SQS::Queue")),
        // A vault ARN: a resource this emulator has no type name for.
        (VAULT.to_string(), None),
    ];
    for (arn, want) in expected {
        let answered = reported
            .get(&arn)
            .unwrap_or_else(|| panic!("case {arn}: not reported at all in {page}"));
        assert_eq!(
            answered.as_deref(),
            want,
            "case {arn}: expected ResourceType {want:?}, got {answered:?}",
        );
    }
    for item in page["Resources"].as_array().expect("Resources is a list") {
        assert!(
            item.get("Status").is_none_or(Value::is_null),
            "case {item}: expected no Status on a query-derived member, got {item}",
        );
    }
}

/// A `CLOUDFORMATION_STACK_1_0` query names one stack, and this service records no
/// CloudFormation stack, so the listing reports the modelled
/// `CLOUDFORMATION_STACK_NOT_EXISTING` query error and no resource — over the
/// identifier forms the model documents (a stack ARN, and a stack name).
#[tokio::test]
async fn a_stack_query_group_reports_the_stack_query_error_and_no_resource() {
    for stack in [
        "arn:aws:cloudformation:us-east-1:000000000000:stack/demo/0",
        "demo",
    ] {
        let mut fixtures = Fixtures::new().await;
        fixtures
            .probe(&stack_query(&json!({ "StackIdentifier": stack })))
            .await;
        let page = fixtures.list(PROBE).await;
        assert_eq!(
            arns_of(stack, &page),
            Vec::<String>::new(),
            "case {stack:?}: expected no resource, got {page}",
        );
        assert!(
            page["NextToken"].is_null(),
            "case {stack:?}: expected no NextToken, got {page}",
        );
        let errors = query_errors_of(stack, &page);
        assert_eq!(
            errors.len(),
            1,
            "case {stack:?}: expected one QueryError, got {page}",
        );
        let (code, message) = &errors[0];
        assert_eq!(
            code.as_deref(),
            Some("CLOUDFORMATION_STACK_NOT_EXISTING"),
            "case {stack:?}: expected the modelled code, got {code:?}",
        );
        assert!(
            code.as_deref().is_some_and(|c| QUERY_ERROR_CODES.contains(&c)),
            "case {stack:?}: expected a modelled QueryErrorCode, got {code:?}",
        );
        let message = message
            .as_deref()
            .unwrap_or_else(|| panic!("case {stack:?}: expected a Message, got {page}"));
        assert!(
            message.contains(stack),
            "case {stack:?}: expected the message to name the stack, got {message:?}",
        );
    }
}

/// A group created without a resource query lists the memberships recorded against
/// it — and raises no query error, because it runs no query.
#[tokio::test]
async fn a_group_with_no_query_lists_its_recorded_memberships() {
    let fixtures = Fixtures::new().await;
    // (label, the group, the ARNs it must list)
    let cases: Vec<(&str, String, Vec<String>)> = vec![
        (
            "a configured group holding memberships",
            STATIC.to_string(),
            sorted([INSTANCE, BUCKET, QUEUE, VAULT]),
        ),
        (
            "a group with neither a query nor a membership",
            ALPHA.to_string(),
            Vec::new(),
        ),
    ];
    for (label, group, expected) in cases {
        let page = fixtures.list(&group).await;
        assert_eq!(
            arns_of(label, &page),
            expected,
            "case {label:?}: expected {expected:?}, got {page}",
        );
        assert_eq!(
            query_errors_of(label, &page),
            Vec::new(),
            "case {label:?}: a group with no query raises no query error, got {page}",
        );
    }
    // A removal moves the recorded membership, and the listing follows it.
    fixtures
        .emulator
        .ok(
            "/ungroup-resources",
            &json!({ "Group": STATIC, "ResourceArns": [QUEUE] }),
        )
        .await;
    let after = fixtures.listed("after a removal", STATIC).await;
    assert_eq!(
        after,
        sorted([INSTANCE, BUCKET, VAULT]),
        "expected the removal to leave the rest of the membership, got {after:?}",
    );
}

/// The membership is what the group's query says NOW: updating the query moves it,
/// in both directions and across both query types.
#[tokio::test]
async fn membership_follows_the_group_s_current_query() {
    let mut fixtures = Fixtures::new().await;
    fixtures
        .probe(&tag_query(&json!({
            "TagFilters": [{ "Key": "Stage", "Values": ["Test"] }]
        })))
        .await;
    let alpha = fixtures.arn(ALPHA);
    let beta = fixtures.arn(BETA);
    let gamma = fixtures.arn(GAMMA);

    // (label, the query to store, the ARNs the listing must then report)
    let steps: Vec<(&str, Option<Value>, Vec<String>)> = vec![
        ("as created", None, vec![alpha.clone()]),
        (
            "after widening the values",
            Some(tag_query(&json!({
                "TagFilters": [{ "Key": "Stage", "Values": ["Test", "Deploy"] }]
            }))),
            sorted([alpha.clone(), beta.clone()]),
        ),
        (
            "after narrowing to another key",
            Some(tag_query(&json!({
                "TagFilters": [{ "Key": "Version", "Values": ["4"] }]
            }))),
            vec![gamma.clone()],
        ),
        (
            "after limiting to a resource type",
            Some(tag_query(&json!({
                "ResourceTypeFilters": ["AWS::S3::Bucket"]
            }))),
            vec![BUCKET.to_string()],
        ),
        (
            "after replacing it with a stack query",
            Some(stack_query(&json!({ "StackIdentifier": "demo" }))),
            Vec::new(),
        ),
        (
            "after going back to a tag query",
            Some(tag_query(&json!({
                "TagFilters": [{ "Key": "Stage", "Values": ["Archived"] }]
            }))),
            vec![gamma.clone()],
        ),
    ];
    for (label, update, expected) in steps {
        if let Some(query) = update {
            fixtures
                .emulator
                .ok(
                    "/update-group-query",
                    &json!({ "Group": PROBE, "ResourceQuery": query }),
                )
                .await;
        }
        let answered = fixtures.listed(label, PROBE).await;
        assert_eq!(
            answered, expected,
            "case {label:?}: expected {expected:?}, got {answered:?}",
        );
    }
}

/// A `resource-type` filter narrows the query-derived membership to the types it
/// names, exactly as it narrows a recorded one.
#[tokio::test]
async fn a_resource_type_filter_narrows_a_query_derived_membership() {
    let mut fixtures = Fixtures::new().await;
    fixtures
        .probe(&tag_query(&json!({ "ResourceTypeFilters": ["AWS::AllSupported"] })))
        .await;
    let groups = sorted(fixtures.group_arns.values().cloned());
    // (the filter values, the ARNs the filter must leave)
    let cases: Vec<(Vec<&str>, Vec<String>)> = vec![
        (vec!["AWS::EC2::Instance"], vec![INSTANCE.to_string()]),
        (vec!["AWS::S3::Bucket"], vec![BUCKET.to_string()]),
        (
            vec!["AWS::EC2::Instance", "AWS::S3::Bucket"],
            sorted([INSTANCE, BUCKET]),
        ),
        (vec![GROUP_TYPE], groups),
        // A type nothing selected is of.
        (vec!["AWS::EC2::Volume"], Vec::new()),
    ];
    for (values, expected) in cases {
        let case = format!("{values:?}");
        let page = fixtures
            .emulator
            .ok(
                LIST,
                &json!({
                    "Group": PROBE,
                    "Filters": [{ "Name": "resource-type", "Values": values }]
                }),
            )
            .await;
        let answered = arns_of(&case, &page);
        assert_eq!(
            answered, expected,
            "case {case}: expected {expected:?}, got {answered:?}",
        );
    }
}

/// A query runs over the resources of the group's OWN region: the same query in two
/// regions selects each region's resources and nothing of the other's.
#[tokio::test]
async fn a_query_runs_over_the_group_s_own_region() {
    let away = "us-west-2";
    let mut fixtures = Fixtures::new().await;
    let west = fixtures
        .create(
            away,
            &json!({ "Name": "member-west", "Tags": { "Stage": "Test" } }),
        )
        .await;
    let home_probe = fixtures
        .probe(&tag_query(&json!({
            "TagFilters": [{ "Key": "Stage", "Values": ["Test"] }]
        })))
        .await;
    fixtures
        .emulator
        .ok_in(
            away,
            "/groups",
            &json!({
                "Name": PROBE,
                "ResourceQuery": tag_query(&json!({
                    "TagFilters": [{ "Key": "Stage", "Values": ["Test"] }]
                }))
            }),
        )
        .await;

    let at_home = fixtures.listed("the home region", PROBE).await;
    assert_eq!(
        at_home,
        vec![fixtures.arn(ALPHA)],
        "case the home region: expected only the home region's tagged group, got \
         {at_home:?} (the probe is {home_probe})",
    );
    let page = fixtures
        .emulator
        .ok_in(away, LIST, &json!({ "Group": PROBE }))
        .await;
    assert_eq!(
        arns_of("the other region", &page),
        vec![west],
        "case the other region: expected only that region's tagged group, got {page}",
    );
}

/// The listing pages through every match exactly once, in ARN order, whatever the
/// page size — and a token belongs to the listing that issued it.
#[tokio::test]
async fn a_query_group_pages_through_every_match_exactly_once() {
    let mut fixtures = Fixtures::new().await;
    fixtures.probe(&tag_query(&json!({}))).await;
    let expected = fixtures.every_resource();

    for page_size in [1usize, 2, 3, 8, 50] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = json!({ "Group": PROBE, "MaxResults": page_size });
            if let Some(carried) = &token {
                body["NextToken"] = json!(carried);
            }
            let page = fixtures.emulator.ok(LIST, &body).await;
            let case = format!("page_size={page_size}");
            let page_arns = arns_of(&case, &page);
            assert!(
                page_arns.len() <= page_size,
                "case {case}: a page carried {} items, got {page}",
                page_arns.len(),
            );
            seen.extend(page_arns);
            token = page["NextToken"].as_str().map(str::to_string);
            if let Some(carried) = &token {
                assert!(
                    is_modelled_token(carried),
                    "case {case}: NextToken {carried:?} is not the modelled form",
                );
            }
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case}: paging did not terminate, saw {seen:?}",
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case page_size={page_size}: expected every match once in ARN order, got {seen:?}",
        );
    }

    // A token this listing never issued is refused rather than resuming somewhere.
    let (status, body) = fixtures
        .emulator
        .call(
            LIST,
            &json!({ "Group": PROBE, "NextToken": "bm90LWEtdG9rZW4h" }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "BadRequestException"),
        "case a forged token: expected 400 BadRequestException, got {status} {body}",
    );
}

/// Listing REPORTS: running a group's query changes nothing a later caller sees —
/// not the membership it answers, not the stored query, not the groups, and not the
/// memberships recorded in them.
#[tokio::test]
async fn listing_a_query_group_changes_nothing_a_later_caller_sees() {
    let mut fixtures = Fixtures::new().await;
    fixtures
        .probe(&tag_query(&json!({ "ResourceTypeFilters": ["AWS::AllSupported"] })))
        .await;
    let before = fixtures.listed("before", PROBE).await;
    let stored_query = fixtures
        .emulator
        .ok("/get-group-query", &json!({ "Group": PROBE }))
        .await;
    let groups_before = fixtures.emulator.ok("/groups-list", &json!({})).await;

    for _ in 0..3 {
        fixtures.list(PROBE).await;
        fixtures.list(STATIC).await;
    }

    let after = fixtures.listed("after", PROBE).await;
    assert_eq!(
        after, before,
        "expected the query-derived membership to survive the listings, got {after:?}",
    );
    assert_eq!(
        fixtures
            .emulator
            .ok("/get-group-query", &json!({ "Group": PROBE }))
            .await,
        stored_query,
        "expected the stored query to survive the listings",
    );
    assert_eq!(
        fixtures.emulator.ok("/groups-list", &json!({})).await,
        groups_before,
        "expected the groups to survive the listings",
    );
    let members = fixtures.listed("the recorded membership", STATIC).await;
    assert_eq!(
        members,
        sorted([INSTANCE, BUCKET, QUEUE, VAULT]),
        "expected the recorded memberships to survive the listings, got {members:?}",
    );
}

/// The fixtures are self-consistent: every fixture group's ARN is the one this
/// service mints for its name in its region, so the expectations above name the
/// resources the store really holds.
#[tokio::test]
async fn the_fixture_group_arns_are_the_minted_ones() {
    let fixtures = Fixtures::new().await;
    for name in [ALPHA, BETA, GAMMA, STATIC] {
        let expected = format!("arn:aws:resource-groups:{HOME}:{ACCOUNT}:group/{name}");
        let answered = fixtures.arn(name);
        assert_eq!(
            answered, expected,
            "case {name}: expected the group ARN {expected}, got {answered}",
        );
    }
}
