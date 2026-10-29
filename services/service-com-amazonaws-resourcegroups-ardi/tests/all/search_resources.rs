// SPDX-License-Identifier: CC0-1.0

//! `SearchResources`, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the lifecycle suite's `Emulator`), so these
//! exercise the whole restJson1 path: routing, boundary validation, the handler, and
//! serialization.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * a search runs the request's query over the resources the service records in the
//!   region the request addressed — the groups stored there and the memberships
//!   recorded in them — and reports each match's ARN and derived resource type, in
//!   ARN order, once per resource;
//! * a query's `ResourceTypeFilters` limits matches to the types it names, and names
//!   no limit when it is absent or holds `AWS::AllSupported`;
//! * a query's `TagFilters` narrow across keys and widen across one key's values, so
//!   a match carries every key and, for each, one of its values (or any value, for a
//!   filter that names none);
//! * a `CLOUDFORMATION_STACK_1_0` query names a stack this service holds none of, so
//!   it is answered with the modelled `CLOUDFORMATION_STACK_NOT_EXISTING` query
//!   error and no resource;
//! * a page carries at most `MaxResults` matches and resumes from the `NextToken` of
//!   the page before, and a request whose query or token is not valid is
//!   `BadRequestException` (400);
//! * searching REPORTS: the groups, their tags and their memberships are unchanged
//!   afterwards.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::group_tags::stored_tags;
use super::resource_group_lifecycle::{error_code, is_modelled_token, static_group, Emulator};
use super::ungroup_resources::sorted;

/// The URI the model binds this operation to
/// (`smithy_query(query="operation",operation="SearchResources")` answers
/// `POST /resources/search`).
const SEARCH: &str = "/resources/search";

/// Resource ARNs recorded as static memberships, spanning the forms the modelled
/// `ResourceArn` admits and the types this emulator does and does not have a
/// CloudFormation-style name for (`AWS::EC2::Instance`, `AWS::S3::Bucket`,
/// `AWS::SQS::Queue`, and a vault whose type the emulator names not at all).
const INSTANCE: &str = "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0";
const BUCKET: &str = "arn:aws:s3:::search-probe-bucket";
const QUEUE: &str = "arn:aws:sqs:us-east-1:000000000000:search-probe-queue";
const VAULT: &str = "arn:aws:glacier:us-east-1:000000000000:vaults/records";

/// The resource type a resource group's own ARN names.
///
/// Source: the CloudFormation resource-type reference, which is the vocabulary
/// `ResourceIdentifier$ResourceType` is documented in
/// (`smithy_query(query="shape",shape="ResourceIdentifier")`: "such as
/// `AWS::EC2::Instance`"), and the type this emulator derives from a
/// `resource-groups:group` ARN.
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

/// The fixture emulator: the groups and memberships every search below runs over,
/// with each group's stored ARN read back from its create.
struct Fixtures {
    emulator: Emulator,
    /// Each fixture group's name mapped to the ARN `CreateGroup` reported for it.
    group_arns: BTreeMap<&'static str, String>,
    /// The ARN of the group created in the OTHER region, which no search of
    /// `us-east-1` may reach.
    west_group_arn: String,
}

impl Fixtures {
    /// Create the fixture groups: three tagged groups a tag filter selects among,
    /// one group holding every static membership, one more holding a membership the
    /// first already holds (so one resource is recorded twice), and one group in
    /// another region.
    async fn new() -> Self {
        let emulator = Emulator::new();
        let mut group_arns: BTreeMap<&'static str, String> = BTreeMap::new();
        let tagged: [(&'static str, Value); 3] = [
            ("search-alpha", json!({ "Stage": "Test", "Team": "Platform" })),
            ("search-beta", json!({ "Stage": "Deploy", "Team": "Platform" })),
            ("search-gamma", json!({ "Stage": "Archived", "Version": "4" })),
        ];
        for (name, tags) in tagged {
            let created = emulator
                .ok("/groups", &json!({ "Name": name, "Tags": tags }))
                .await;
            group_arns.insert(name, group_arn(&created));
        }
        for (name, members) in [
            ("search-members", vec![INSTANCE, BUCKET, QUEUE, VAULT]),
            ("search-shared", vec![INSTANCE]),
        ] {
            let created = emulator.ok("/groups", &static_group(name)).await;
            group_arns.insert(name, group_arn(&created));
            emulator
                .ok(
                    "/group-resources",
                    &json!({ "Group": name, "ResourceArns": members }),
                )
                .await;
        }
        // Another region's group, tagged like `search-alpha`: a search of one region
        // reaches no other region's resources.
        let west = emulator
            .ok_in(
                "us-west-2",
                "/groups",
                &json!({ "Name": "search-west", "Tags": { "Stage": "Test" } }),
            )
            .await;
        Self {
            emulator,
            group_arns,
            west_group_arn: group_arn(&west),
        }
    }

    /// The stored ARN of fixture group `name`.
    fn arn(&self, name: &str) -> String {
        self.group_arns
            .get(name)
            .unwrap_or_else(|| panic!("the fixtures must hold a group named {name:?}"))
            .clone()
    }

    /// Every resource a search of `us-east-1` can match: the five fixture groups and
    /// the four recorded memberships, in ARN order.
    fn every_resource(&self) -> Vec<String> {
        let mut arns: Vec<String> = self.group_arns.values().cloned().collect();
        arns.extend([INSTANCE, BUCKET, QUEUE, VAULT].map(str::to_string));
        sorted(arns)
    }

    /// Search `us-east-1` with `query`, requiring the modelled 200, and return the
    /// ARNs it reported in the order it reported them.
    async fn search(&self, case: &str, query: &Value) -> Vec<String> {
        let page = self
            .emulator
            .ok(SEARCH, &json!({ "ResourceQuery": query }))
            .await;
        arns_of(case, &page)
    }
}

/// The ARN `CreateGroup` reported for the group it stored.
fn group_arn(created: &Value) -> String {
    created["Group"]["GroupArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateGroup must answer a GroupArn, got {created}"))
        .to_string()
}

/// The `ResourceIdentifiers` of a page as (ARN, reported type) pairs, in the order
/// the page holds them.
fn identifiers_of(case: &str, page: &Value) -> Vec<(String, Option<String>)> {
    page["ResourceIdentifiers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected ResourceIdentifiers, got {page}"))
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

/// A tag filter narrows across keys and widens across one key's values: a resource
/// is reported only when it carries every filter's key and, for each, one of that
/// filter's values — any value, for a filter that names none.
#[tokio::test]
async fn tag_filters_select_the_resources_carrying_every_key_and_one_of_its_values() {
    let fixtures = Fixtures::new().await;
    let alpha = fixtures.arn("search-alpha");
    let beta = fixtures.arn("search-beta");
    let gamma = fixtures.arn("search-gamma");
    // (the TagFilters document, the resources it must select)
    let cases: Vec<(Value, Vec<String>)> = vec![
        // One key, one value.
        (json!([{ "Key": "Stage", "Values": ["Test"] }]), vec![alpha.clone()]),
        // One key, several values: the matches are the union.
        (
            json!([{ "Key": "Stage", "Values": ["Test", "Deploy"] }]),
            sorted([alpha.clone(), beta.clone()]),
        ),
        // Several keys narrow: a resource must carry all of them.
        (
            json!([
                { "Key": "Stage", "Values": ["Test"] },
                { "Key": "Team", "Values": ["Platform"] }
            ]),
            vec![alpha.clone()],
        ),
        (
            json!([
                { "Key": "Stage", "Values": ["Archived"] },
                { "Key": "Version", "Values": ["4"] }
            ]),
            vec![gamma.clone()],
        ),
        // Every key, but no value of one of them matches: nothing.
        (
            json!([
                { "Key": "Stage", "Values": ["Archived"] },
                { "Key": "Version", "Values": ["1", "2"] }
            ]),
            Vec::new(),
        ),
        // A key one resource does not carry at all: nothing.
        (
            json!([
                { "Key": "Stage", "Values": ["Archived"] },
                { "Key": "Team", "Values": ["Platform"] }
            ]),
            Vec::new(),
        ),
        // A filter that names no value is satisfied by carrying the key.
        (
            json!([{ "Key": "Team" }]),
            sorted([alpha.clone(), beta.clone()]),
        ),
        (
            json!([{ "Key": "Stage", "Values": [] }]),
            sorted([alpha.clone(), beta.clone(), gamma.clone()]),
        ),
        // Tag keys and values are case-sensitive.
        (json!([{ "Key": "Stage", "Values": ["test"] }]), Vec::new()),
        (json!([{ "Key": "stage", "Values": ["Test"] }]), Vec::new()),
        // A key nothing carries.
        (json!([{ "Key": "Nothing", "Values": ["Test"] }]), Vec::new()),
    ];
    for (filters, expected) in cases {
        let case = filters.to_string();
        let answered = fixtures
            .search(&case, &tag_query(&json!({ "TagFilters": filters })))
            .await;
        assert_eq!(
            answered, expected,
            "case {case}: expected {expected:?}, got {answered:?}",
        );
    }
}

/// `ResourceTypeFilters` limits a search to the types it names, and limits it to
/// nothing when it is absent or names every supported type.
#[tokio::test]
async fn resource_type_filters_limit_the_search_to_the_types_they_name() {
    let fixtures = Fixtures::new().await;
    let groups = sorted(fixtures.group_arns.values().cloned());
    // (the whole query document, the resources it must select)
    let cases: Vec<(Value, Vec<String>)> = vec![
        // A group's own ARN names a resource of the group type.
        (json!({ "ResourceTypeFilters": [GROUP_TYPE] }), groups.clone()),
        (
            json!({ "ResourceTypeFilters": ["AWS::EC2::Instance"] }),
            vec![INSTANCE.to_string()],
        ),
        // Several types select the union, still in ARN order.
        (
            json!({ "ResourceTypeFilters": ["AWS::EC2::Instance", "AWS::S3::Bucket"] }),
            sorted([INSTANCE, BUCKET]),
        ),
        (
            json!({ "ResourceTypeFilters": ["AWS::SQS::Queue"] }),
            vec![QUEUE.to_string()],
        ),
        // `AWS::AllSupported` is no limit at all, and neither is an absent element
        // nor an empty one.
        (
            json!({ "ResourceTypeFilters": ["AWS::AllSupported"] }),
            fixtures.every_resource(),
        ),
        (json!({}), fixtures.every_resource()),
        (
            json!({ "ResourceTypeFilters": [] }),
            fixtures.every_resource(),
        ),
        // A limit beside a type it does not name still admits the named one.
        (
            json!({ "ResourceTypeFilters": ["AWS::AllSupported", "AWS::EC2::Instance"] }),
            fixtures.every_resource(),
        ),
        // A supported type nothing recorded is of: nothing.
        (
            json!({ "ResourceTypeFilters": ["AWS::Lambda::Function"] }),
            Vec::new(),
        ),
        // Type and tag filters narrow together.
        (
            json!({
                "ResourceTypeFilters": [GROUP_TYPE],
                "TagFilters": [{ "Key": "Stage", "Values": ["Deploy"] }]
            }),
            vec![fixtures.arn("search-beta")],
        ),
        (
            json!({
                "ResourceTypeFilters": ["AWS::EC2::Instance"],
                "TagFilters": [{ "Key": "Stage", "Values": ["Deploy"] }]
            }),
            Vec::new(),
        ),
    ];
    for (document, expected) in cases {
        let case = document.to_string();
        let answered = fixtures.search(&case, &tag_query(&document)).await;
        assert_eq!(
            answered, expected,
            "case {case}: expected {expected:?}, got {answered:?}",
        );
    }
}

/// Every match reports the resource type derived from its ARN, and no type at all
/// for an ARN this emulator has no type name for. A resource recorded by two groups
/// is reported once.
#[tokio::test]
async fn a_match_reports_the_type_derived_from_its_arn_once_per_resource() {
    let fixtures = Fixtures::new().await;
    let page = fixtures
        .emulator
        .ok(
            SEARCH,
            &json!({ "ResourceQuery": tag_query(&json!({ "ResourceTypeFilters": ["AWS::AllSupported"] })) }),
        )
        .await;
    let reported = identifiers_of("every resource", &page);
    let expected: Vec<(String, Option<String>)> = {
        let mut rows: Vec<(String, Option<String>)> = fixtures
            .group_arns
            .values()
            .map(|arn| (arn.clone(), Some(GROUP_TYPE.to_string())))
            .collect();
        rows.push((INSTANCE.to_string(), Some("AWS::EC2::Instance".to_string())));
        rows.push((BUCKET.to_string(), Some("AWS::S3::Bucket".to_string())));
        rows.push((QUEUE.to_string(), Some("AWS::SQS::Queue".to_string())));
        // The emulator has no CloudFormation-style name for this ARN's type, so the
        // optional member is absent rather than guessed.
        rows.push((VAULT.to_string(), None));
        rows.sort();
        rows
    };
    assert_eq!(
        reported, expected,
        "expected every recorded resource once, with the type derived from its ARN, got {reported:?}",
    );
}

/// A search pages through its matches exactly once, in ARN order, whatever the page
/// size, and reports a `NextToken` of the modelled form while more remain.
#[tokio::test]
async fn a_search_pages_through_its_matches_once_in_arn_order() {
    let fixtures = Fixtures::new().await;
    let expected = fixtures.every_resource();
    let query = tag_query(&json!({ "ResourceTypeFilters": ["AWS::AllSupported"] }));
    for page_size in [1usize, 2, 4, 50] {
        let case = format!("page_size={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            // `MaxResults` and `NextToken` are members of the request PAYLOAD: the
            // model binds no member of this operation to the query string
            // (`smithy_query(query="operation",operation="SearchResources")`).
            let request = match &token {
                Some(t) => json!({
                    "ResourceQuery": query,
                    "MaxResults": page_size,
                    "NextToken": t
                }),
                None => json!({ "ResourceQuery": query, "MaxResults": page_size }),
            };
            let page = fixtures.emulator.ok(SEARCH, &request).await;
            let arns = arns_of(&case, &page);
            assert!(
                arns.len() <= page_size,
                "case {case}: a page carried {} items, got {page}",
                arns.len(),
            );
            seen.extend(arns);
            token = page["NextToken"].as_str().map(str::to_string);
            if let Some(t) = &token {
                assert!(
                    is_modelled_token(t),
                    "case {case}: NextToken {t:?} is not the modelled form",
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
            "case {case}: expected every match once in ARN order, got {seen:?}",
        );
    }
}

/// A search runs over the region the request addressed and reaches no other
/// region's resources.
#[tokio::test]
async fn a_search_is_scoped_to_the_region_the_request_addressed() {
    let fixtures = Fixtures::new().await;
    let query = tag_query(&json!({ "TagFilters": [{ "Key": "Stage", "Values": ["Test"] }] }));
    // (region, the resources a search there must report)
    let cases: Vec<(&str, Vec<String>)> = vec![
        ("us-east-1", vec![fixtures.arn("search-alpha")]),
        ("us-west-2", vec![fixtures.west_group_arn.clone()]),
        // A region nothing was ever written through holds no resource at all.
        ("eu-west-1", Vec::new()),
    ];
    for (region, expected) in cases {
        let page = fixtures
            .emulator
            .ok_in(region, SEARCH, &json!({ "ResourceQuery": query }))
            .await;
        let answered = arns_of(region, &page);
        assert_eq!(
            answered, expected,
            "case {region}: expected {expected:?}, got {answered:?}",
        );
    }
}

/// A `NextToken` is scoped to the query and the region that issued it: a token from
/// another query, another region, or no query at all is `BadRequestException` (400).
#[tokio::test]
async fn a_token_from_another_search_is_bad_request() {
    let fixtures = Fixtures::new().await;
    let query = tag_query(&json!({ "ResourceTypeFilters": ["AWS::AllSupported"] }));
    let first = fixtures
        .emulator
        .ok(SEARCH, &json!({ "ResourceQuery": query, "MaxResults": 1 }))
        .await;
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a one-item page of nine matches must carry a NextToken, got {first}"))
        .to_string();
    // The token the first page issued resumes THAT query in THAT region.
    let resumed = fixtures
        .emulator
        .ok(
            SEARCH,
            &json!({ "ResourceQuery": query, "MaxResults": 50, "NextToken": token }),
        )
        .await;
    let answered = arns_of("the issuing query", &resumed);
    assert_eq!(
        answered,
        fixtures.every_resource()[1..].to_vec(),
        "expected the matches after the first, got {answered:?}",
    );
    // (case, the query the token is replayed against, the region it is replayed in)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "another query",
            tag_query(&json!({ "ResourceTypeFilters": [GROUP_TYPE] })),
            "us-east-1",
        ),
        ("another region", query.clone(), "us-west-2"),
    ];
    for (case, replayed, region) in cases {
        let (status, body) = fixtures
            .emulator
            .call_in(
                region,
                SEARCH,
                &json!({ "ResourceQuery": replayed, "NextToken": token }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "BadRequestException"),
            "case {case}: expected 400 BadRequestException, got {status} {body}",
        );
    }
    // A base64 string no search issued is no resume point either.
    for forged in ["bm90LWEtdG9rZW4=", "AAAA"] {
        let (status, body) = fixtures
            .emulator
            .call(
                SEARCH,
                &json!({ "ResourceQuery": query, "NextToken": forged }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "BadRequestException"),
            "case {forged:?}: expected 400 BadRequestException, got {status} {body}",
        );
    }
}

/// A CloudFormation-stack query names a stack, and this service records no stack at
/// all, so it is answered with the modelled `CLOUDFORMATION_STACK_NOT_EXISTING`
/// query error, no resource and no token — not with a refusal.
#[tokio::test]
async fn a_stack_query_reports_the_modelled_stack_query_error() {
    let fixtures = Fixtures::new().await;
    // Stack identifiers spanning the forms the documented element carries: a stack
    // ARN, one in another region, and a bare stack name.
    for stack in [
        "arn:aws:cloudformation:us-east-1:000000000000:stack/demo/0",
        "arn:aws:cloudformation:eu-west-1:000000000000:stack/other/1",
        "demo",
    ] {
        let page = fixtures
            .emulator
            .ok(
                SEARCH,
                &json!({
                    "ResourceQuery": stack_query(&json!({ "StackIdentifier": stack }))
                }),
            )
            .await;
        let answered = arns_of(stack, &page);
        assert!(
            answered.is_empty(),
            "case {stack:?}: expected no resource for a stack this service holds none of, got {answered:?}",
        );
        assert!(
            page["NextToken"].is_null(),
            "case {stack:?}: expected no NextToken, got {page}",
        );
        let errors = page["QueryErrors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {stack:?}: expected QueryErrors, got {page}"));
        assert_eq!(
            errors.len(),
            1,
            "case {stack:?}: expected one QueryError, got {page}",
        );
        let code = errors[0]["ErrorCode"].as_str().unwrap_or("<none>");
        assert_eq!(
            code, "CLOUDFORMATION_STACK_NOT_EXISTING",
            "case {stack:?}: expected the modelled stack-not-existing code, got {page}",
        );
        assert!(
            QUERY_ERROR_CODES.contains(&code),
            "case {stack:?}: {code} is not a code the model declares",
        );
        let message = errors[0]["Message"].as_str().unwrap_or("<none>");
        assert!(
            message.contains(stack),
            "case {stack:?}: expected the message to name the stack, got {message:?}",
        );
    }
}

/// A request whose query document the service cannot run is `BadRequestException`
/// (400) — never an empty success and never a 501.
#[tokio::test]
async fn a_query_the_service_cannot_run_is_bad_request() {
    let fixtures = Fixtures::new().await;
    // (case, the whole ResourceQuery)
    let cases: Vec<(&str, Value)> = vec![
        // A `Type` the modelled `QueryType` does not declare.
        (
            "an undeclared query type",
            json!({ "Type": "TAG_FILTERS_2_0", "Query": "{}" }),
        ),
        // A `Query` that is not a JSON object defines no query.
        (
            "a query that is not JSON",
            json!({ "Type": "TAG_FILTERS_1_0", "Query": "not json" }),
        ),
        (
            "a query that is a JSON array",
            json!({ "Type": "TAG_FILTERS_1_0", "Query": "[]" }),
        ),
        // `TagFilters` that is not the documented collection.
        (
            "TagFilters that is not an array",
            tag_query(&json!({ "TagFilters": { "Key": "Stage" } })),
        ),
        (
            "a TagFilters entry naming no key",
            tag_query(&json!({ "TagFilters": [{ "Values": ["Test"] }] })),
        ),
        (
            "a TagFilters entry whose key is not a string",
            tag_query(&json!({ "TagFilters": [{ "Key": 7, "Values": ["Test"] }] })),
        ),
        (
            "TagFilters values that are not an array",
            tag_query(&json!({ "TagFilters": [{ "Key": "Stage", "Values": "Test" }] })),
        ),
        (
            "a TagFilters value that is not a string",
            tag_query(&json!({ "TagFilters": [{ "Key": "Stage", "Values": [7] }] })),
        ),
        // A stack query naming no stack matches the members of no stack.
        ("a stack query naming no stack", stack_query(&json!({}))),
        (
            "a stack query whose stack is not a string",
            stack_query(&json!({ "StackIdentifier": 7 })),
        ),
    ];
    for (case, query) in cases {
        let (status, body) = fixtures
            .emulator
            .call(SEARCH, &json!({ "ResourceQuery": query }))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "BadRequestException"),
            "case {case}: expected 400 BadRequestException, got {status} {body}",
        );
    }
}

/// `MaxResults` is served at each modelled bound and refused past it.
///
/// Source: `smithy_query(query="operation",operation="SearchResources")`, whose
/// `MaxResults` is `range 1..50`.
const MAX_RESULTS_BOUNDS: [i64; 2] = [1, 50];

#[tokio::test]
async fn max_results_is_served_inside_the_modelled_range_and_refused_outside_it() {
    let fixtures = Fixtures::new().await;
    let query = tag_query(&json!({ "ResourceTypeFilters": ["AWS::AllSupported"] }));
    let every = fixtures.every_resource();
    for bound in MAX_RESULTS_BOUNDS {
        let page = fixtures
            .emulator
            .ok(
                SEARCH,
                &json!({ "ResourceQuery": query, "MaxResults": bound }),
            )
            .await;
        let case = format!("MaxResults={bound}");
        let answered = arns_of(&case, &page);
        let wanted = usize::try_from(bound).expect("a positive bound").min(every.len());
        assert_eq!(
            answered,
            every[..wanted].to_vec(),
            "case {case}: expected the first {wanted} matches, got {answered:?}",
        );
    }
    // One past each published bound.
    for outside in [MAX_RESULTS_BOUNDS[0] - 1, MAX_RESULTS_BOUNDS[1] + 1] {
        let (status, body) = fixtures
            .emulator
            .call(
                SEARCH,
                &json!({ "ResourceQuery": query, "MaxResults": outside }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "BadRequestException"),
            "case MaxResults={outside}: expected 400 BadRequestException, got {status} {body}",
        );
    }
}

/// Searching REPORTS: the groups, their tags and their memberships are the same
/// afterwards, so a later caller sees exactly what it saw before.
#[tokio::test]
async fn searching_changes_nothing_a_later_caller_sees() {
    let fixtures = Fixtures::new().await;
    let before = fixtures.every_resource();
    let queries = [
        tag_query(&json!({ "ResourceTypeFilters": ["AWS::AllSupported"] })),
        tag_query(&json!({ "TagFilters": [{ "Key": "Stage", "Values": ["Test"] }] })),
        stack_query(&json!({ "StackIdentifier": "demo" })),
    ];
    for query in &queries {
        for _ in 0..3 {
            fixtures
                .emulator
                .ok(SEARCH, &json!({ "ResourceQuery": query }))
                .await;
        }
    }
    // The searchable resources are unchanged, read back through the search itself…
    let after = fixtures
        .search(
            "after searching",
            &tag_query(&json!({ "ResourceTypeFilters": ["AWS::AllSupported"] })),
        )
        .await;
    assert_eq!(
        after, before,
        "expected the recorded resources to survive the searches, got {after:?}",
    );
    // …through the group's own membership listing…
    let listed = fixtures
        .emulator
        .ok(
            "/list-group-resources",
            &json!({ "Group": "search-members" }),
        )
        .await;
    let members = sorted(arns_of("membership listing", &listed));
    assert_eq!(
        members,
        sorted([INSTANCE, BUCKET, QUEUE, VAULT]),
        "expected the memberships to survive the searches, got {members:?}",
    );
    // …and through the tags the groups still carry.
    for (name, expected) in [
        (
            "search-alpha",
            vec![("Stage", "Test"), ("Team", "Platform")],
        ),
        ("search-members", Vec::new()),
    ] {
        let carried = stored_tags(&fixtures.emulator, "us-east-1", name, &fixtures.arn(name)).await;
        let want: std::collections::BTreeMap<String, String> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            carried, want,
            "case {name}: expected the stored tags to survive the searches, got {carried:?}",
        );
    }
}
