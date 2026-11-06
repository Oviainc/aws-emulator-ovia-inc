// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's option group slice: `CreateOptionGroup` and
//! `DescribeOptionGroups`.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter`
//! exactly as the production router mounts it (through `db_instances::emulator`, the
//! one mounting helper this test target has), and each request is a real awsQuery form
//! POST carrying the SigV4 credential scope the router keys on — `rds`, the signing
//! name RDS shares with DocumentDB and Neptune. The assertions read the XML the
//! emulator answers with, so a handler that computed nothing could not pass them.
//!
//! The rule under test: `DescribeOptionGroups` reports the option groups THIS
//! service's state holds for the region the request was signed for, and a request
//! naming a group that state does not hold is answered with the modelled
//! `OptionGroupNotFoundFault` — HTTP 404, `text/xml`, an `<ErrorResponse>` envelope —
//! never a router-level refusal and never an empty list.
//!
//! The tables below span the classes those rules generalise over: the engines
//! `CreateOptionGroupMessage$EngineName` publishes (`db2-ae` … `sqlserver-web`), the
//! shapes `OptionGroupName`'s constraints admit and refuse including its published
//! length bound and one past it, both documented `MaxRecords` bounds and one past
//! each, the three selection combinations the input members forbid, and the regional
//! scope one name resolves in.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    self, OPTION_GROUP_ENGINE_NAMES, OPTION_GROUP_QUOTA_PER_REGION,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{emulator, optional_text, text, texts};

/// The region every call below is signed for, unless it names another.
const REGION: &str = "us-west-2";

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// The content type an awsQuery service answers with — the one a botocore client
/// parses as XML. The router's own `AmbiguousService` refusal was
/// `application/x-amz-json-1.1`, which is how this suite tells a real answer from a
/// routing refusal.
const XML_CONTENT_TYPE: &str = "text/xml";

/// The query parameters of one call, owned so a table row can hold them.
type Params = Vec<(&'static str, String)>;

/// One row of the selection table: the case, the parameters of the describe, and the
/// option group names its answer must report, in order.
type SelectionRow = (&'static str, Params, Vec<&'static str>);

/// One row of the refusal table: the case, the action, its parameters, and the HTTP
/// status and wire error code the request must be refused with.
type RefusalRow = (&'static str, &'static str, Params, u16, &'static str);

/// One answer from the emulator: its HTTP status, its `content-type` and its body.
struct Answer {
    status: u16,
    content_type: String,
    body: String,
}

/// One awsQuery call against `region`.
///
/// Returns the `content-type` beside the status and the body because this issue is
/// partly ABOUT the content type: the refusal it reports was JSON on a wire that
/// speaks XML.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[(&str, String)],
) -> Answer {
    let mut body = format!("Action={action}&Version=2014-10-31");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        // Percent-escaped exactly as the shared harness escapes a form value, so a row
        // can send a value whose own alphabet overlaps the body's syntax — an opaque
        // pagination token is base64, so `+`, `/` and `=` are in it.
        for byte in value.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    body.push(char::from(byte));
                }
                other => body.push_str(&format!("%{other:02X}")),
            }
        }
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("https://rds.amazonaws.com/")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{region}/rds/aws4_request, \
                 SignedHeaders=host, Signature=unverified"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .map(|value| String::from_utf8_lossy(value.as_bytes()).to_string())
        .unwrap_or_default();
    let body = String::from_utf8(response.body().to_vec()).expect("a UTF-8 response body");
    Answer {
        status,
        content_type,
        body,
    }
}

/// One awsQuery call against [`REGION`].
async fn call(router: &MultiServiceRouter, action: &str, params: &[(&str, String)]) -> Answer {
    call_in(router, REGION, action, params).await
}

/// Assert a 200 answered as XML, and hand back the body.
fn expect_ok(case: &str, answer: Answer) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with body {}",
        answer.status, answer.body
    );
    assert!(
        answer.content_type.starts_with(XML_CONTENT_TYPE),
        "case {case:?}: expected a {XML_CONTENT_TYPE} answer, got {:?} with body {}",
        answer.content_type,
        answer.body
    );
    answer.body
}

/// Assert the modelled failure: its HTTP status, its XML content type, its
/// `<ErrorResponse>` envelope and its wire error `<Code>`. Hands back the body.
fn expect_error(case: &str, answer: Answer, expected_status: u16, expected_code: &str) -> String {
    assert_eq!(
        answer.status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {} with body {}",
        answer.status, answer.body
    );
    assert!(
        answer.content_type.starts_with(XML_CONTENT_TYPE),
        "case {case:?}: expected a {XML_CONTENT_TYPE} answer, got {:?} with body {}",
        answer.content_type,
        answer.body
    );
    assert!(
        answer.body.contains("<ErrorResponse"),
        "case {case:?}: expected an <ErrorResponse> envelope, got {}",
        answer.body
    );
    let code = text(&answer.body, "Code");
    assert_eq!(
        code, expected_code,
        "case {case:?}: expected error code {expected_code}, got {code:?} in {}",
        answer.body
    );
    answer.body
}

/// `CreateOptionGroup` with its four required members, in `region`.
async fn create_group_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    engine_name: &str,
    major_engine_version: &str,
    description: &str,
) -> Answer {
    let params = [
        ("OptionGroupName", name.to_string()),
        ("EngineName", engine_name.to_string()),
        ("MajorEngineVersion", major_engine_version.to_string()),
        ("OptionGroupDescription", description.to_string()),
    ];
    call_in(router, region, "CreateOptionGroup", &params).await
}

/// `CreateOptionGroup` in [`REGION`].
async fn create_group(
    router: &MultiServiceRouter,
    name: &str,
    engine_name: &str,
    major_engine_version: &str,
    description: &str,
) -> Answer {
    create_group_in(
        router,
        REGION,
        name,
        engine_name,
        major_engine_version,
        description,
    )
    .await
}

/// `DescribeOptionGroups` with the given parameters, in `region`.
async fn describe_in(
    router: &MultiServiceRouter,
    region: &str,
    params: &[(&str, String)],
) -> Answer {
    call_in(router, region, "DescribeOptionGroups", params).await
}

/// `DescribeOptionGroups` with the given parameters, in [`REGION`].
async fn describe(router: &MultiServiceRouter, params: &[(&str, String)]) -> Answer {
    describe_in(router, REGION, params).await
}

/// The `<OptionGroupName>` of every group a describe answer reports, in the order it
/// reported them.
fn reported_names(xml: &str) -> Vec<String> {
    texts(xml, "OptionGroupName")
}

/// The groups a describe answer reports that the ACCOUNT created, in the order it reported
/// them: every reported group except the ones Amazon RDS provides itself.
///
/// RDS creates a default option group per engine and major engine version
/// (`default:<engine>-<major version>`), so a describe reports those beside an account's
/// own groups; the assertions below are about the groups a request created, and this is
/// what separates them. The name form is the one `CreateOptionGroup` can never produce: its
/// `OptionGroupName` admits letters, numbers and hyphens only.
fn account_owned_names(xml: &str) -> Vec<String> {
    reported_names(xml)
        .into_iter()
        .filter(|name| !name.starts_with("default:"))
        .collect()
}

/// One row of the create/describe round trip: the option group's four members.
struct GroupCase {
    case: &'static str,
    name: &'static str,
    engine_name: &'static str,
    major_engine_version: &'static str,
    description: &'static str,
}

/// The option groups the round trip creates and reads back.
///
/// The rows differ along what the rule generalises over: the engine (five entries of
/// the published `EngineName` list, four of which this issue never cited, including
/// the Db2 and SQL Server families), the major engine version's form (`8.0`, `16`,
/// `19`, `15.00`, `11.5`), and the name's shape (plain, hyphenated, digit-bearing).
/// The published 255-character name bound, and one character past it, are covered by
/// the not-found and refusal tables below, which are where a length that long belongs.
const GROUP_CASES: &[GroupCase] = &[
    GroupCase {
        case: "mysql group, dotted major version",
        name: "optiongroup-mysql",
        engine_name: "mysql",
        major_engine_version: "8.0",
        description: "mysql-reporting-options",
    },
    GroupCase {
        case: "postgres group, bare major version",
        name: "pg16",
        engine_name: "postgres",
        major_engine_version: "16",
        description: "postgres-options",
    },
    GroupCase {
        case: "oracle group, hyphenated name",
        name: "oracle-se2-19-options",
        engine_name: "oracle-se2",
        major_engine_version: "19",
        description: "oracle-se2-options",
    },
    GroupCase {
        case: "sqlserver group, four-part major version",
        name: "sqlserver-ex-1500",
        engine_name: "sqlserver-ex",
        major_engine_version: "15.00",
        description: "sqlserver-express-options",
    },
    GroupCase {
        case: "db2 group",
        name: "db2ae-options",
        engine_name: "db2-ae",
        major_engine_version: "11.5",
        description: "db2-advanced-edition-options",
    },
];

#[tokio::test]
async fn create_persists_every_member_and_describe_reads_it_back() {
    let router = emulator();

    for row in GROUP_CASES {
        let created = expect_ok(
            row.case,
            create_group(
                &router,
                row.name,
                row.engine_name,
                row.major_engine_version,
                row.description,
            )
            .await,
        );
        // The create's own answer, and then the group read back through
        // `DescribeOptionGroups`: the round trip is what proves the record was
        // PERSISTED rather than echoed.
        let described = expect_ok(
            row.case,
            describe(&router, &[("OptionGroupName", row.name.to_string())]).await,
        );

        for (label, xml) in [("CreateOptionGroup", &created), ("DescribeOptionGroups", &described)]
        {
            let case = format!("{} via {label}", row.case);
            let expected_arn = format!("arn:aws:rds:{REGION}:{ACCOUNT}:og:{}", row.name);
            for (tag, expected) in [
                ("OptionGroupName", row.name),
                ("EngineName", row.engine_name),
                ("MajorEngineVersion", row.major_engine_version),
                ("OptionGroupDescription", row.description),
                ("OptionGroupArn", expected_arn.as_str()),
            ] {
                let actual = text(xml, tag);
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected <{tag}> {expected:?}, got {actual:?} in {xml}"
                );
            }
            // A group nothing has added options to reports an EMPTY option list, not a
            // fabricated one.
            let options = optional_text(xml, "Options").unwrap_or_default();
            assert!(
                options.trim().is_empty(),
                "case {case:?}: expected an empty <Options> list, got {options:?} in {xml}"
            );
        }
    }

    // With no name named, the describe reports every group the region holds, in name
    // order: the ones created above, plus the default group RDS provides for each engine
    // and major engine version it publishes. Nothing else is invented.
    let listed = expect_ok("list every group", describe(&router, &[("MaxRecords", "100".to_string())]).await);
    let mut expected: Vec<&str> = GROUP_CASES.iter().map(|row| row.name).collect();
    expected.sort_unstable();
    let reported = account_owned_names(&listed);
    assert_eq!(
        reported, expected,
        "case \"list every group\": expected {expected:?}, got {reported:?} in {listed}"
    );
    // The AWS-managed catalog is there too, spanning more than one engine, each group
    // reporting the members `OptionGroup` publishes for it.
    let defaults: Vec<String> = reported_names(&listed)
        .into_iter()
        .filter(|name| name.starts_with("default:"))
        .collect();
    assert!(
        defaults.len() > 1,
        "case \"list every group\": expected the account's default option groups, got \
         {defaults:?} in {listed}"
    );
}

#[tokio::test]
async fn a_name_no_option_group_has_is_refused_as_option_group_not_found() {
    let router = emulator();
    expect_ok(
        "seed a group",
        create_group(&router, "optiongroup-mysql", "mysql", "8.0", "mysql-options").await,
    );

    // Names no stored group has, differing along what the rule generalises over: a
    // name nothing resembles, one that differs from the stored group only in case,
    // one that is a prefix of it, and one at the published length bound. Each must be
    // the modelled 404 — the recorded step of this issue is the first row's shape.
    let missing = [
        ("a name nothing resembles", "no-such-option-group".to_string()),
        ("the stored name in another case", "OptionGroup-MySQL".to_string()),
        ("a prefix of the stored name", "optiongroup".to_string()),
        ("a name at the 255-character bound", "a".repeat(255)),
    ];

    for (case, name) in missing {
        let body = expect_error(
            case,
            describe(&router, &[("OptionGroupName", name.clone())]).await,
            404,
            "OptionGroupNotFoundFault",
        );
        // The message is built from the request, so it names the group that was asked
        // for and no other.
        let message = text(&body, "Message");
        assert!(
            message.contains(&name),
            "case {case:?}: expected the message to name {name:?}, got {message:?}"
        );
        // The refusal this issue reports carried no `<ErrorResponse>` at all, and a
        // `__type` instead of a `<Code>`; neither may come back.
        assert!(
            !body.contains("__type") && !body.contains("AmbiguousService"),
            "case {case:?}: a router refusal reached the caller: {body}"
        );
    }

    // The group that DOES exist still reads back after every refusal: a failed lookup
    // changed nothing.
    let described = expect_ok(
        "the stored group still reads back",
        describe(&router, &[("OptionGroupName", "optiongroup-mysql".to_string())]).await,
    );
    let reported = reported_names(&described);
    assert_eq!(
        reported,
        vec!["optiongroup-mysql".to_string()],
        "case \"the stored group still reads back\": got {reported:?} in {described}"
    );
}

#[tokio::test]
async fn describe_selects_by_engine_name_and_major_engine_version() {
    let router = emulator();
    for row in GROUP_CASES {
        expect_ok(
            row.case,
            create_group(
                &router,
                row.name,
                row.engine_name,
                row.major_engine_version,
                row.description,
            )
            .await,
        );
    }
    // A second mysql group on another major version, so the EngineName filter and the
    // EngineName+MajorEngineVersion pair select different sets.
    expect_ok(
        "second mysql group",
        create_group(&router, "optiongroup-mysql57", "mysql", "5.7", "mysql-legacy-options").await,
    );

    // (case, parameters, the names the answer must report, in order). The groups RDS
    // provides itself are reported beside them and are filtered out by
    // `account_owned_names`, so each row states what the ACCOUNT's groups do under that
    // selection.
    let rows: Vec<SelectionRow> = vec![
        (
            "every group",
            vec![("MaxRecords", "100".to_string())],
            vec![
                "db2ae-options",
                "optiongroup-mysql",
                "optiongroup-mysql57",
                "oracle-se2-19-options",
                "pg16",
                "sqlserver-ex-1500",
            ],
        ),
        (
            "EngineName selects one engine's groups",
            vec![
                ("EngineName", "mysql".to_string()),
                ("MaxRecords", "100".to_string()),
            ],
            vec!["optiongroup-mysql", "optiongroup-mysql57"],
        ),
        (
            "EngineName and MajorEngineVersion select one group",
            vec![
                ("EngineName", "mysql".to_string()),
                ("MajorEngineVersion", "8.0".to_string()),
            ],
            vec!["optiongroup-mysql"],
        ),
        (
            "an engine with one group",
            vec![("EngineName", "sqlserver-ex".to_string())],
            vec!["sqlserver-ex-1500"],
        ),
        (
            // A published engine no group was created for selects none of the account's
            // own — an empty list, not a 404: the 404 belongs to a NAME, not to a filter.
            "a published engine with no groups",
            vec![("EngineName", "mariadb".to_string())],
            vec![],
        ),
        (
            "a major version no group of that engine has",
            vec![
                ("EngineName", "postgres".to_string()),
                ("MajorEngineVersion", "9.6".to_string()),
            ],
            vec![],
        ),
    ];

    for (case, params, expected) in rows {
        let body = expect_ok(case, describe(&router, &params).await);
        let reported = account_owned_names(&body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?} in {body}"
        );
        // A filter selects the AWS-managed groups by the same members: every default group
        // the answer reports is one of the named engine and major version.
        for (member, value) in &params {
            if *member == "MaxRecords" {
                continue;
            }
            let selected = texts(&body, if *member == "EngineName" { "EngineName" } else { "MajorEngineVersion" });
            assert!(
                selected.iter().all(|reported| reported == value),
                "case {case:?}: every reported group must carry <{member}> {value:?}, got \
                 {selected:?} in {body}"
            );
        }
    }
}

#[tokio::test]
async fn the_input_members_are_refused_outside_what_they_document() {
    let router = emulator();
    expect_ok(
        "seed a group",
        create_group(&router, "optiongroup-mysql", "mysql", "8.0", "mysql-options").await,
    );

    // (case, action, parameters, expected status, expected code). Every row is a value
    // or combination the member's own documentation excludes; the 200 rows are the
    // published bounds themselves, so the refusals are pinned AT the boundary.
    let rows: Vec<RefusalRow> = vec![
        (
            // "OptionGroupName ... Can't be supplied together with EngineName or
            // MajorEngineVersion."
            "a name together with an engine",
            "DescribeOptionGroups",
            vec![
                ("OptionGroupName", "optiongroup-mysql".to_string()),
                ("EngineName", "mysql".to_string()),
            ],
            400,
            "ValidationException",
        ),
        (
            "a name together with a major version",
            "DescribeOptionGroups",
            vec![
                ("OptionGroupName", "optiongroup-mysql".to_string()),
                ("MajorEngineVersion", "8.0".to_string()),
            ],
            400,
            "ValidationException",
        ),
        (
            // "MajorEngineVersion ... If specified, then EngineName must also be
            // specified."
            "a major version without an engine",
            "DescribeOptionGroups",
            vec![("MajorEngineVersion", "8.0".to_string())],
            400,
            "ValidationException",
        ),
        (
            // "Filters ... This parameter isn't currently supported."
            "a filter, which the member says is unsupported",
            "DescribeOptionGroups",
            vec![
                ("Filters.Filter.1.Name", "engine".to_string()),
                ("Filters.Filter.1.Values.Value.1", "mysql".to_string()),
            ],
            400,
            "ValidationException",
        ),
        (
            // "MaxRecords ... Constraints: Minimum 20, maximum 100." A value outside the
            // documented window is one the parameter does not accept.
            "MaxRecords one below the documented minimum",
            "DescribeOptionGroups",
            vec![("MaxRecords", "19".to_string())],
            400,
            "InvalidParameterValue",
        ),
        (
            "MaxRecords one above the documented maximum",
            "DescribeOptionGroups",
            vec![("MaxRecords", "101".to_string())],
            400,
            "InvalidParameterValue",
        ),
        (
            // The EngineName value list, on a describe and on a create.
            "an engine outside the published value list, on a describe",
            "DescribeOptionGroups",
            vec![("EngineName", "not-an-engine".to_string())],
            400,
            "InvalidParameterValue",
        ),
        (
            "an engine outside the published value list, on a create",
            "CreateOptionGroup",
            vec![
                ("OptionGroupName", "optiongroup-unknown-engine".to_string()),
                ("EngineName", "not-an-engine".to_string()),
                ("MajorEngineVersion", "1.0".to_string()),
                ("OptionGroupDescription", "unknown-engine".to_string()),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            // "OptionGroupName ... Must be 1 to 255 letters, numbers, or hyphens.
            // First character must be a letter. Can't end with a hyphen or contain
            // two consecutive hyphens."
            "a name one character past the documented length bound",
            "CreateOptionGroup",
            vec![
                ("OptionGroupName", "a".repeat(256)),
                ("EngineName", "mysql".to_string()),
                ("MajorEngineVersion", "8.0".to_string()),
                ("OptionGroupDescription", "too-long".to_string()),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a name that does not start with a letter",
            "CreateOptionGroup",
            vec![
                ("OptionGroupName", "1-optiongroup".to_string()),
                ("EngineName", "mysql".to_string()),
                ("MajorEngineVersion", "8.0".to_string()),
                ("OptionGroupDescription", "bad-first-character".to_string()),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a name ending with a hyphen",
            "CreateOptionGroup",
            vec![
                ("OptionGroupName", "optiongroup-".to_string()),
                ("EngineName", "mysql".to_string()),
                ("MajorEngineVersion", "8.0".to_string()),
                ("OptionGroupDescription", "trailing-hyphen".to_string()),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a name with two consecutive hyphens",
            "CreateOptionGroup",
            vec![
                ("OptionGroupName", "option--group".to_string()),
                ("EngineName", "mysql".to_string()),
                ("MajorEngineVersion", "8.0".to_string()),
                ("OptionGroupDescription", "double-hyphen".to_string()),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            // "The option group you are trying to create already exists."
            "a name already taken in this region",
            "CreateOptionGroup",
            vec![
                ("OptionGroupName", "optiongroup-mysql".to_string()),
                ("EngineName", "mysql".to_string()),
                ("MajorEngineVersion", "8.0".to_string()),
                ("OptionGroupDescription", "duplicate".to_string()),
            ],
            400,
            "OptionGroupAlreadyExistsFault",
        ),
    ];

    for (case, action, params, expected_status, expected_code) in rows {
        expect_error(
            case,
            call(&router, action, &params).await,
            expected_status,
            expected_code,
        );
    }

    // Each published `MaxRecords` bound itself is served, which is what makes the two
    // refusals above boundary assertions rather than blanket ones. The account's one group
    // is reported beside the default groups RDS provides, so the page is bounded by what
    // the request asked for and carries a marker exactly while groups are left.
    for bound in ["20", "100"] {
        let case = format!("MaxRecords at the documented bound {bound}");
        let body = expect_ok(
            &case,
            describe(&router, &[("MaxRecords", bound.to_string())]).await,
        );
        let page: usize = bound.parse().expect("the bound is a number");
        let reported = reported_names(&body);
        assert!(
            reported.len() <= page,
            "case {case:?}: expected at most {page} groups, got {reported:?} in {body}"
        );
        assert_eq!(
            optional_text(&body, "Marker").is_some(),
            reported.len() == page,
            "case {case:?}: a full page must carry a continuation marker and a last page \
             must not, got {reported:?} in {body}"
        );
    }

    // And every engine the model's own value list publishes is accepted by the
    // describe filter — the whole catalog, not just the rows above.
    for engine_name in OPTION_GROUP_ENGINE_NAMES {
        let case = format!("published engine {engine_name} is accepted");
        let body = expect_ok(
            &case,
            describe(
                &router,
                &[
                    ("EngineName", engine_name.to_string()),
                    ("MaxRecords", "100".to_string()),
                ],
            )
            .await,
        );
        let expected: Vec<String> = if engine_name == "mysql" {
            vec!["optiongroup-mysql".to_string()]
        } else {
            Vec::new()
        };
        let reported = account_owned_names(&body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?} in {body}"
        );
    }
}

#[tokio::test]
async fn one_name_is_a_different_option_group_in_each_region() {
    let router = emulator();
    // The same name, created in two regions with different members: RDS is regional,
    // so each region's record is its own.
    expect_ok(
        "create in us-west-2",
        create_group_in(
            &router,
            REGION,
            "shared-optiongroup",
            "mysql",
            "8.0",
            "west-options",
        )
        .await,
    );
    expect_ok(
        "create in eu-central-1",
        create_group_in(
            &router,
            "eu-central-1",
            "shared-optiongroup",
            "postgres",
            "16",
            "central-options",
        )
        .await,
    );

    // (region, the engine that region's group carries).
    for (region, expected_engine) in [("us-west-2", "mysql"), ("eu-central-1", "postgres")] {
        let case = format!("the group named in {region}");
        let body = expect_ok(
            &case,
            describe_in(
                &router,
                region,
                &[("OptionGroupName", "shared-optiongroup".to_string())],
            )
            .await,
        );
        let engine = text(&body, "EngineName");
        assert_eq!(
            engine, expected_engine,
            "case {case:?}: expected <EngineName> {expected_engine:?}, got {engine:?} in {body}"
        );
        let arn = text(&body, "OptionGroupArn");
        let expected_arn = format!("arn:aws:rds:{region}:{ACCOUNT}:og:shared-optiongroup");
        assert_eq!(
            arn, expected_arn,
            "case {case:?}: expected <OptionGroupArn> {expected_arn:?}, got {arn:?} in {body}"
        );
    }

    // A region that holds no group of that name answers the modelled 404, not another
    // region's record.
    expect_error(
        "a third region holds no group of that name",
        describe_in(
            &router,
            "ap-southeast-4",
            &[("OptionGroupName", "shared-optiongroup".to_string())],
        )
        .await,
        404,
        "OptionGroupNotFoundFault",
    );
}

#[tokio::test]
async fn describe_resumes_from_the_marker() {
    let router = emulator();
    // Three groups of the account's own, read back beside the default groups RDS provides:
    // `MaxRecords` cannot go below its documented minimum of 20, so the paging is driven by
    // the emulator's own page walk over an ordered range, and the account's groups sort
    // after every `default:` name.
    let names = ["pagegroup-a", "pagegroup-b", "pagegroup-c"];
    for name in names {
        expect_ok(
            name,
            create_group(&router, name, "mysql", "8.0", "paging-options").await,
        );
    }

    let first = expect_ok(
        "the first page",
        describe(&router, &[("MaxRecords", "20".to_string())]).await,
    );
    let reported = reported_names(&first);
    assert_eq!(
        reported.len(),
        20,
        "case \"the first page\": a page of MaxRecords=20 over more than 20 groups is \
         exactly 20, got {reported:?} in {first}"
    );
    assert!(
        optional_text(&first, "Marker").is_some(),
        "case \"the first page\": a full page must carry a Marker: {first}"
    );

    // Resuming AT the token for a name reports that name and the ones after it: the
    // marker is the opaque token for the key the next page starts at.
    let resumed = expect_ok(
        "resume from the second name",
        describe(
            &router,
            &[("Marker", state::encode_marker(names[1]))],
        )
        .await,
    );
    let reported = reported_names(&resumed);
    assert_eq!(
        reported,
        vec![names[1].to_string(), names[2].to_string()],
        "case \"resume from the second name\": got {reported:?} in {resumed}"
    );
    assert!(
        optional_text(&resumed, "Marker").is_none(),
        "case \"resume from the second name\": a last page must carry no Marker: {resumed}"
    );
}

#[tokio::test]
async fn the_region_quota_refuses_an_option_group_past_it() {
    let router = emulator();
    // The documented allowance ("You can create up to 20 option groups"), enforced per
    // region by this store: the last group inside it is created, the next one is the
    // modelled quota fault.
    for index in 0..OPTION_GROUP_QUOTA_PER_REGION {
        let name = format!("quotagroup-{index:03}");
        expect_ok(
            &name,
            create_group(&router, &name, "mysql", "8.0", "quota-options").await,
        );
    }
    let case = "one option group past the quota";
    expect_error(
        case,
        create_group(&router, "quotagroup-over", "mysql", "8.0", "quota-options").await,
        400,
        "OptionGroupQuotaExceededFault",
    );
    // The refused group was not stored.
    expect_error(
        "the refused group is not in the store",
        describe(&router, &[("OptionGroupName", "quotagroup-over".to_string())]).await,
        404,
        "OptionGroupNotFoundFault",
    );
    // …and another region still has its own full allowance, so the quota is the
    // region's and not the store's.
    expect_ok(
        "another region still has room",
        create_group_in(
            &router,
            "eu-central-1",
            "quotagroup-over",
            "mysql",
            "8.0",
            "quota-options",
        )
        .await,
    );
}
