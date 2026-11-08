// SPDX-License-Identifier: CC0-1.0

//! The error envelope every Directory Service refusal carries, driven end to end over
//! HTTP.
//!
//! Every case goes through the same front door a client uses — `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body — and
//! builds any state it needs through `CreateDirectory` / `CreateAlias`. Nothing reaches
//! into the state struct. The harness (router, call helpers, quotas) is the sibling
//! suites', reached as `crate::directory_lifecycle::<item>`.
//!
//! Four rules are under test, each over a SET of inputs rather than one recorded call:
//!
//! * every refusal this service answers with one of its MODELLED error shapes reports
//!   the id of the call in that shape's `RequestId` member — a UUID, per the member's own
//!   `@pattern` — and repeats it at the end of `Message` as ` : RequestId: <id>`; the id
//!   is per call, so two refusals never share one. Asserted over a TABLE spanning 17
//!   operations and 8 of the modelled error shapes.
//! * the tag API (`AddTagsToResource` / `ListTagsForResource` / `RemoveTagsFromResource`)
//!   answers a `ResourceId` that names no taggable resource with the tag-family
//!   diagnostic, which quotes NO id, while the directory-lookup family quotes the id it
//!   could not resolve. Asserted over the three tag operations x several absent ids.
//! * `RemoveRegion`, whose input carries a `DirectoryId` and nothing else, builds its
//!   not-found diagnostic from that id alone — the same text whatever Region the call was
//!   signed for. Asserted over several absent ids x several signing Regions, and over a
//!   directory that exists in another Region only.
//! * a member that does not satisfy its modelled constraint is refused BEFORE the
//!   operation resolves anything, with the framework's validation envelope (HTTP 400,
//!   `__type` `ValidationException`, a lowercase `message` naming the member and the
//!   constraint) rather than with one of the operation's own resolution errors. Asserted
//!   over the whole `AliasName` grammar: values that satisfy its `@pattern`, values that
//!   break each clause of it, and values outside its `@length`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{
    call, call_in_region, create_directory, minimal_create, ok, service, string, PAST_ONE_SPAN,
    TAG_QUOTA,
};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// Ids of the modelled `DirectoryId` / `ResourceId` shape (`^d-[0-9a-f]{10}$`) that name
/// no stored directory. Several, because the diagnostic is built from the request: the
/// rule is asserted on the id the request carried, not on one recorded id.
const ABSENT_DIRECTORY_IDS: [&str; 3] = ["d-0000000000", "d-1234567890", "d-0dead0beef"];

/// Regions a request can be signed for. The sigv4 credential scope is the only place a
/// request carries one, and a request-derived diagnostic must not vary with it.
const SIGNING_REGIONS: [&str; 3] = ["us-east-1", "us-west-2", "eu-central-1"];

/// The diagnostic for a page token this service did not issue: it names the input member
/// whose value was rejected, and is this service's recorded text rather than the
/// `InvalidNextTokenException` shape's documentation prose. The rule it belongs to is
/// `crate::pagination_token_refusal`'s; here it is one row of the envelope table.
const INVALID_NEXT_TOKEN: &str = "Invalid value passed for the NextToken parameter.";

/// The tag family's diagnostic for a `ResourceId` that names no taggable resource. It
/// quotes no id — unlike [`directory_not_found`] — and the recorded refusal carries the
/// call's request id TWICE after it: this family's diagnostic already ends with the
/// ` : RequestId: <id>` suffix before the envelope appends its own.
const INVALID_RESOURCE_ID: &str = "Invalid resource ID.";

/// The whole tag-family diagnostic for one call: [`INVALID_RESOURCE_ID`] with the
/// family's own request-id suffix, which the envelope's suffix then follows.
fn invalid_resource_id(request_id: &str) -> String {
    format!("{INVALID_RESOURCE_ID} : RequestId: {request_id}")
}

/// The three tag operations, with a body naming `resource_id`, so the tag-family rule is
/// asserted on each of them rather than on the one the recording covers.
fn tag_calls(resource_id: &str) -> [(&'static str, Value); 3] {
    [
        (
            "AddTagsToResource",
            json!({ "ResourceId": resource_id, "Tags": [{ "Key": "k", "Value": "v" }] }),
        ),
        (
            "ListTagsForResource",
            json!({ "ResourceId": resource_id }),
        ),
        (
            "RemoveTagsFromResource",
            json!({ "ResourceId": resource_id, "TagKeys": ["k"] }),
        ),
    ]
}

/// The directory-lookup family's diagnostic for an id that names no stored directory: it
/// QUOTES the id the request carried and names nothing else.
fn directory_not_found(directory_id: &str) -> String {
    format!("Directory: '{directory_id}' does not exist.")
}

/// One refusal to assert: which call to make, and what the envelope must say.
struct Refusal {
    /// What the row is, named in every assertion of it.
    case: String,
    operation: &'static str,
    body: Value,
    /// The modelled error shape the `__type` must name.
    error: &'static str,
    /// The diagnostic, WITHOUT the request-id suffix the rule appends — built from the
    /// request id the refusal reports, for the one family whose own diagnostic names it.
    diagnostic: Diagnostic,
}

/// What a refusal's diagnostic is, before the envelope's ` : RequestId: <id>` suffix.
enum Diagnostic {
    /// A fixed text, the same whatever id the call is answered with.
    Fixed(String),
    /// A text built from the request id the refusal reports (the tag family).
    WithRequestId(fn(&str) -> String),
}

impl Diagnostic {
    /// The diagnostic of a refusal answered with `request_id`.
    fn resolve(&self, request_id: &str) -> String {
        match self {
            Self::Fixed(text) => text.clone(),
            Self::WithRequestId(build) => build(request_id),
        }
    }
}

/// The modelled `RequestId` pattern
/// (`^([A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12})$`,
/// from `smithy_query(query="shape",shape="EntityDoesNotExistException")`), as an
/// assertion.
fn assert_uuid_shaped(request_id: &str, case: &str) {
    let groups: Vec<&str> = request_id.split('-').collect();
    let widths = [8_usize, 4, 4, 4, 12];
    assert!(
        groups.len() == widths.len()
            && groups
                .iter()
                .zip(widths)
                .all(|(group, width)| group.len() == width
                    && group.bytes().all(|b| b.is_ascii_hexdigit())),
        "case {case:?}: expected a RequestId matching the modelled UUID pattern, got \
         {request_id:?}"
    );
}

/// Assert the whole envelope of one refusal and hand back the request id it reported.
async fn assert_refusal(router: &MultiServiceRouter, refusal: &Refusal) -> String {
    let case = &refusal.case;
    let (status, body) = call(router, refusal.operation, refusal.body.clone()).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {} to answer 400, got {status} with {body}",
        refusal.operation
    );
    let wire_type = body["__type"].as_str().unwrap_or_default();
    assert!(
        wire_type.contains(refusal.error),
        "case {case:?}: expected __type to name {}, got {body}",
        refusal.error
    );
    let request_id = body["RequestId"].as_str().unwrap_or_else(|| {
        panic!("case {case:?}: expected the modelled RequestId member, got {body}")
    });
    assert_uuid_shaped(request_id, case);
    assert_eq!(
        body["Message"],
        json!(format!(
            "{diagnostic} : RequestId: {request_id}",
            diagnostic = refusal.diagnostic.resolve(request_id)
        )),
        "case {case:?}: expected the diagnostic followed by the reported request id, got \
         {body}"
    );
    request_id.to_string()
}

/// Every refusal row, against a router already holding `directory_id` (a stored
/// directory) and `taken_alias` (an alias another directory already holds).
fn refusal_table(directory_id: &str, taken_alias: &str) -> Vec<Refusal> {
    let mut rows = Vec::new();

    // The directory-lookup family, over three absent ids and every implemented
    // operation that resolves a directory: the diagnostic quotes the id the request
    // carried, and the error shape is the one that operation models for it.
    for absent in ABSENT_DIRECTORY_IDS {
        let lookups: [(&'static str, Value, &'static str); 10] = [
            (
                "DeleteDirectory",
                json!({ "DirectoryId": absent }),
                "EntityDoesNotExistException",
            ),
            (
                "CreateSnapshot",
                json!({ "DirectoryId": absent }),
                "EntityDoesNotExistException",
            ),
            (
                "GetSnapshotLimits",
                json!({ "DirectoryId": absent }),
                "EntityDoesNotExistException",
            ),
            (
                "DescribeEventTopics",
                json!({ "DirectoryId": absent }),
                "EntityDoesNotExistException",
            ),
            (
                "ListIpRoutes",
                json!({ "DirectoryId": absent }),
                "EntityDoesNotExistException",
            ),
            (
                "AddIpRoutes",
                json!({ "DirectoryId": absent, "IpRoutes": [{ "CidrIp": "10.0.0.0/24" }] }),
                "EntityDoesNotExistException",
            ),
            (
                "CreateAlias",
                json!({ "DirectoryId": absent, "Alias": "wellformedalias" }),
                "EntityDoesNotExistException",
            ),
            (
                "RemoveRegion",
                json!({ "DirectoryId": absent }),
                "DirectoryDoesNotExistException",
            ),
            (
                "DescribeLDAPSSettings",
                json!({ "DirectoryId": absent }),
                "DirectoryDoesNotExistException",
            ),
            (
                "EnableDirectoryDataAccess",
                json!({ "DirectoryId": absent }),
                "DirectoryDoesNotExistException",
            ),
        ];
        for (operation, body, error) in lookups {
            rows.push(Refusal {
                case: format!("{operation} on absent {absent}"),
                operation,
                body,
                error,
                diagnostic: Diagnostic::Fixed(directory_not_found(absent)),
            });
        }
        // The two reads that answer the same lookup with their own error shape, one of
        // which takes the id as a filter rather than as the subject.
        rows.push(Refusal {
            case: format!("DescribeClientAuthenticationSettings on absent {absent}"),
            operation: "DescribeClientAuthenticationSettings",
            body: json!({ "DirectoryId": absent }),
            error: "DirectoryDoesNotExistException",
            diagnostic: Diagnostic::Fixed(directory_not_found(absent)),
        });
        rows.push(Refusal {
            case: format!("ListADAssessments filtered by absent {absent}"),
            operation: "ListADAssessments",
            body: json!({ "DirectoryId": absent }),
            error: "DirectoryDoesNotExistException",
            diagnostic: Diagnostic::Fixed(directory_not_found(absent)),
        });
        rows.push(Refusal {
            case: format!("DescribeDirectoryDataAccess on absent {absent}"),
            operation: "DescribeDirectoryDataAccess",
            body: json!({ "DirectoryId": absent }),
            error: "DirectoryDoesNotExistException",
            diagnostic: Diagnostic::Fixed(directory_not_found(absent)),
        });
        // The two reads whose `DirectoryId` is a FILTER over another listing: those
        // report the VALUE as an invalid directory id instead, in the singular or the
        // plural spelling of the member it arrived in.
        rows.push(Refusal {
            case: format!("DescribeDirectories filtered by absent {absent}"),
            operation: "DescribeDirectories",
            body: json!({ "DirectoryIds": [absent] }),
            error: "EntityDoesNotExistException",
            diagnostic: Diagnostic::Fixed(format!("Invalid directory ID(s): {absent}.")),
        });
        rows.push(Refusal {
            case: format!("DescribeSnapshots filtered by absent {absent}"),
            operation: "DescribeSnapshots",
            body: json!({ "DirectoryId": absent }),
            error: "EntityDoesNotExistException",
            diagnostic: Diagnostic::Fixed(format!("Invalid directory ID: {absent}.")),
        });
        // The tag family, whose diagnostic is its own.
        for (operation, body) in tag_calls(absent) {
            rows.push(Refusal {
                case: format!("{operation} on absent {absent}"),
                operation,
                body,
                error: "EntityDoesNotExistException",
                diagnostic: Diagnostic::WithRequestId(invalid_resource_id),
            });
        }
    }

    // A refused page token, on the two paginated listings whose model answers it with
    // different shapes.
    rows.push(Refusal {
        case: "DescribeDirectories with a token this service did not issue".to_string(),
        operation: "DescribeDirectories",
        body: json!({ "NextToken": "not-a-token-this-service-issued" }),
        error: "InvalidNextTokenException",
        diagnostic: Diagnostic::Fixed(INVALID_NEXT_TOKEN.to_string()),
    });
    rows.push(Refusal {
        case: "ListTagsForResource with a token this service did not issue".to_string(),
        operation: "ListTagsForResource",
        body: json!({ "ResourceId": directory_id, "NextToken": "not-a-token" }),
        error: "InvalidNextTokenException",
        diagnostic: Diagnostic::Fixed(INVALID_NEXT_TOKEN.to_string()),
    });
    rows.push(Refusal {
        case: "DescribeClientAuthenticationSettings with a foreign token".to_string(),
        operation: "DescribeClientAuthenticationSettings",
        body: json!({ "DirectoryId": directory_id, "NextToken": "not-a-token" }),
        error: "InvalidParameterException",
        diagnostic: Diagnostic::Fixed(INVALID_NEXT_TOKEN.to_string()),
    });

    // An id of another kind that resolves to nothing: a snapshot — reported as the filter
    // VALUE the listing cannot be narrowed by — and an assessment, with the lookup
    // family's own diagnostic.
    rows.push(Refusal {
        case: "DescribeSnapshots filtered by an absent snapshot".to_string(),
        operation: "DescribeSnapshots",
        body: json!({ "SnapshotIds": ["s-0000000000"] }),
        error: "InvalidParameterException",
        diagnostic: Diagnostic::Fixed("Invalid snapshot ID: s-0000000000.".to_string()),
    });
    rows.push(Refusal {
        case: "DescribeADAssessment on an absent assessment".to_string(),
        operation: "DescribeADAssessment",
        body: json!({ "AssessmentId": "da-000000000000000000" }),
        error: "EntityDoesNotExistException",
        diagnostic: Diagnostic::Fixed(
            "Assessment Id da-000000000000000000 does not exist.".to_string(),
        ),
    });

    // Refusals that are not lookups at all: an alias already taken, and the two tag
    // bounds. Each is a different modelled error shape, and every one of them carries
    // the same envelope.
    rows.push(Refusal {
        case: "CreateAlias with an alias another directory holds".to_string(),
        operation: "CreateAlias",
        body: json!({ "DirectoryId": directory_id, "Alias": taken_alias }),
        error: "EntityAlreadyExistsException",
        diagnostic: Diagnostic::Fixed(format!("Alias {taken_alias} is already in use")),
    });
    let over_quota = TAG_QUOTA + 1;
    let too_many_tags: Vec<Value> = (0..over_quota)
        .map(|index| json!({ "Key": format!("key{index}"), "Value": "v" }))
        .collect();
    let too_many_keys: Vec<Value> = (0..over_quota)
        .map(|index| json!(format!("key{index}")))
        .collect();
    rows.push(Refusal {
        case: format!("AddTagsToResource with {over_quota} tags"),
        operation: "AddTagsToResource",
        body: json!({ "ResourceId": directory_id, "Tags": too_many_tags }),
        error: "TagLimitExceededException",
        diagnostic: Diagnostic::Fixed(format!(
            "A resource may carry at most {TAG_QUOTA} tags; the request would leave \
             {over_quota}"
        )),
    });
    rows.push(Refusal {
        case: format!("RemoveTagsFromResource with {over_quota} keys"),
        operation: "RemoveTagsFromResource",
        body: json!({ "ResourceId": directory_id, "TagKeys": too_many_keys }),
        error: "InvalidParameterException",
        diagnostic: Diagnostic::Fixed(format!(
            "TagKeys may name at most {TAG_QUOTA} keys, got {over_quota}"
        )),
    });

    rows
}

/// A router holding one stored directory and one alias already taken by another, with
/// both returned: the state the refusal table's non-lookup rows need.
async fn populated() -> (MultiServiceRouter, String, String) {
    let router = service();
    let case = "the refusal table's state";
    let subject = create_directory(&router, minimal_create("subject.example.com"), case).await;
    let holder = create_directory(&router, minimal_create("holder.example.com"), case).await;
    let taken_alias = "takenalias";
    // An alias is assigned only to a directory that has reached its serving stage, so the
    // holder is waited past the transition its create owes before it takes the alias. The
    // `CreateAlias` row of the table below then refuses on the alias being taken, which is
    // what it is a row about.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    ok(
        &router,
        "CreateAlias",
        json!({ "DirectoryId": &holder, "Alias": taken_alias }),
        case,
    )
    .await;
    (router, subject, taken_alias.to_string())
}

#[tokio::test]
async fn every_modelled_refusal_reports_its_request_id_in_the_member_and_the_message() {
    let (router, directory_id, taken_alias) = populated().await;
    let rows = refusal_table(&directory_id, &taken_alias);
    assert!(
        rows.len() > 40,
        "expected the table to span the service's refusal paths, got {} rows",
        rows.len()
    );

    let mut reported = Vec::new();
    for refusal in &rows {
        reported.push(assert_refusal(&router, refusal).await);
    }

    // The id identifies ONE call, so no two refusals report the same one.
    let mut unique = reported.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        reported.len(),
        "expected every refusal to report its own request id, got {reported:?}"
    );
}

#[tokio::test]
async fn repeating_one_refused_call_reports_a_fresh_request_id_each_time() {
    let router = service();
    let body = json!({ "DirectoryId": ABSENT_DIRECTORY_IDS[0] });

    let mut seen = Vec::new();
    for attempt in 1..=4 {
        let case = format!("attempt {attempt}");
        let (status, answer) = call(&router, "DeleteDirectory", body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the repeat to stay refused, got {status} with {answer}"
        );
        let request_id = string(&answer, "RequestId", &case);
        assert_uuid_shaped(&request_id, &case);
        assert!(
            !seen.contains(&request_id),
            "case {case:?}: expected a request id not yet reported, got {request_id} after \
             {seen:?}"
        );
        seen.push(request_id);
    }
}

#[tokio::test]
async fn the_tag_family_and_the_directory_family_word_a_missing_id_differently() {
    let router = service();

    for absent in ABSENT_DIRECTORY_IDS {
        // The tag family: the id is not a taggable resource id, and the diagnostic quotes
        // nothing.
        for (operation, body) in tag_calls(absent) {
            let case = format!("{operation} on absent {absent}");
            let (status, answer) = call(&router, operation, body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} with {answer}"
            );
            let message = string(&answer, "Message", &case);
            let request_id = string(&answer, "RequestId", &case);
            assert_eq!(
                message,
                format!(
                    "{diagnostic} : RequestId: {request_id}",
                    diagnostic = invalid_resource_id(&request_id)
                ),
                "case {case:?}: expected the tag-family diagnostic {INVALID_RESOURCE_ID:?}, \
                 got {message:?}"
            );
            assert!(
                !message.contains(absent),
                "case {case:?}: expected the tag-family diagnostic to quote no id, got \
                 {message:?}"
            );
        }

        // The directory-lookup family: the same absent id, quoted.
        for operation in ["DeleteDirectory", "GetSnapshotLimits", "CreateSnapshot"] {
            let case = format!("{operation} on absent {absent}");
            let (status, answer) =
                call(&router, operation, json!({ "DirectoryId": absent })).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} with {answer}"
            );
            let message = string(&answer, "Message", &case);
            assert!(
                message.starts_with(&directory_not_found(absent)),
                "case {case:?}: expected the directory-lookup diagnostic quoting {absent}, \
                 got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn remove_region_words_a_missing_directory_from_the_request_alone() {
    let router = service();

    // The rows differ along what the rule generalises over: the id the request carried
    // and the Region the call was signed for. The diagnostic is built from the request,
    // which carries a `DirectoryId` and nothing else, so it must not vary with the
    // Region or name one.
    for absent in ABSENT_DIRECTORY_IDS {
        for region in SIGNING_REGIONS {
            let case = format!("RemoveRegion on absent {absent} signed for {region}");
            let (status, answer) = call_in_region(
                &router,
                "RemoveRegion",
                json!({ "DirectoryId": absent }),
                region,
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} with {answer}"
            );
            let message = string(&answer, "Message", &case);
            let request_id = string(&answer, "RequestId", &case);
            assert_eq!(
                message,
                format!(
                    "{expected} : RequestId: {request_id}",
                    expected = directory_not_found(absent)
                ),
                "case {case:?}: expected the request-derived diagnostic, got {message:?}"
            );
            assert!(
                !message.contains(region),
                "case {case:?}: expected no Region qualifier in the diagnostic, got \
                 {message:?}"
            );
        }
    }

    // And the same text for a directory that EXISTS but holds no replica entry for the
    // Region the call was signed for: that is also "the id you passed names no directory I
    // can act on here", and it is still worded from the request alone. The directory is an
    // Enterprise Managed Microsoft AD, the only kind that replicates across Regions at all,
    // so this branch is reached rather than the edition gate ahead of it.
    let created = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("single-region.example.com"),
        "a directory in its primary Region only",
    )
    .await;
    for region in SIGNING_REGIONS.into_iter().filter(|r| *r != "us-east-1") {
        let case = format!("RemoveRegion on {created} signed for {region}");
        let (status, answer) = call_in_region(
            &router,
            "RemoveRegion",
            json!({ "DirectoryId": &created }),
            region,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {answer}"
        );
        let message = string(&answer, "Message", &case);
        let request_id = string(&answer, "RequestId", &case);
        assert_eq!(
            message,
            format!(
                "{expected} : RequestId: {request_id}",
                expected = directory_not_found(&created)
            ),
            "case {case:?}: expected the request-derived diagnostic, got {message:?}"
        );
    }
}

/// The modelled `AliasName` `@pattern`
/// (`smithy_query(query="shape",shape="AliasName")`:
/// `^(?!D-|d-)([\da-zA-Z]+)([-]*[\da-zA-Z])*$`) as a refusal states it: the expression
/// without the whole-value end anchor, which is how a recorded Directory Service
/// validation clause quotes it.
const STATED_ALIAS_PATTERN: &str = r"^(?!D-|d-)([\da-zA-Z]+)([-]*[\da-zA-Z])*";

/// The `AliasName` grammar as a table: a value, and whether it satisfies the modelled
/// `@pattern`. The rows break each clause of the expression in turn — the negative
/// lookahead (an alias spelled like a directory id), the alphanumeric first and last
/// character, the admitted character set — and satisfy it in each way it can be
/// satisfied: one character, digits, mixed case, single and repeated inner hyphens.
const ALIAS_GRAMMAR: [(&str, bool); 16] = [
    ("alias", true),
    ("A", true),
    ("9", true),
    ("Alias123", true),
    ("valid-alias", true),
    ("a--b", true),
    ("de-notadirectory", true),
    ("Dir-0123456789", true),
    ("SIM2REAL_Invalid_Alias", false),
    ("d-0123456789", false),
    ("D-0123456789", false),
    ("-leading", false),
    ("trailing-", false),
    ("has space", false),
    ("has.dot", false),
    ("under_score", false),
];

#[tokio::test]
async fn an_alias_outside_its_modelled_pattern_is_a_validation_failure() {
    for (index, (alias, satisfies)) in ALIAS_GRAMMAR.into_iter().enumerate() {
        let case = format!("alias {alias:?}");
        // A fresh Region per row: a directory holds at most one alias, so each row must
        // be the first alias of its own directory, and the rows outnumber the Simple AD
        // Region quota.
        let router = service();
        let directory_id = create_directory(
            &router,
            minimal_create(&format!("grammar{index}.example.com")),
            &case,
        )
        .await;
        // An alias is assigned only to a directory that has reached its serving stage, so
        // the row waits for the transition the create owes before the alias is asked for.
        tokio::time::sleep(PAST_ONE_SPAN).await;
        let (status, answer) = call(
            &router,
            "CreateAlias",
            json!({ "DirectoryId": &directory_id, "Alias": alias }),
        )
        .await;

        if satisfies {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the alias to be accepted, got {status} with {answer}"
            );
            assert_eq!(
                answer["Alias"],
                json!(alias),
                "case {case:?}: expected the stored alias to be reported back, got {answer}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected the alias to be refused, got {status} with {answer}"
        );
        assert_eq!(
            answer["__type"],
            json!("ValidationException"),
            "case {case:?}: expected the framework validation envelope, got {answer}"
        );
        assert_eq!(
            answer["message"],
            json!(format!(
                "1 validation error detected: Value '{alias}' at 'alias' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: \
                 {STATED_ALIAS_PATTERN}"
            )),
            "case {case:?}: expected the clause to name the member and the constraint, got \
             {answer}"
        );
        // The validation envelope is the FRAMEWORK's, not one of this service's modelled
        // error shapes, so it carries neither the uppercase `Message` nor a `RequestId`.
        assert!(
            answer["Message"].is_null() && answer["RequestId"].is_null(),
            "case {case:?}: expected no modelled error members on the validation envelope, \
             got {answer}"
        );
    }
}

#[tokio::test]
async fn a_malformed_alias_is_refused_before_the_directory_is_resolved() {
    let router = service();

    // Neither a directory that is absent nor one that already holds an alias changes the
    // answer: the member constraint is checked first, so the refusal is the validation
    // failure and never the operation's own resolution error.
    let holder = create_directory(&router, minimal_create("ordering.example.com"), "ordering").await;
    // The alias the holder row needs is assigned once the directory serves, which is the
    // stage `CreateAlias` admits.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    ok(
        &router,
        "CreateAlias",
        json!({ "DirectoryId": &holder, "Alias": "orderingalias" }),
        "ordering",
    )
    .await;

    let subjects = [
        ("an absent directory", ABSENT_DIRECTORY_IDS[0].to_string()),
        ("a directory that already has an alias", holder.clone()),
    ];
    for (subject, directory_id) in subjects {
        for alias in ["SIM2REAL_Invalid_Alias", "d-0123456789", "has space"] {
            let case = format!("{alias:?} against {subject}");
            let (status, answer) = call(
                &router,
                "CreateAlias",
                json!({ "DirectoryId": &directory_id, "Alias": alias }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} with {answer}"
            );
            assert_eq!(
                answer["__type"],
                json!("ValidationException"),
                "case {case:?}: expected the validation envelope rather than a resolution \
                 error, got {answer}"
            );
            assert!(
                answer["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("at 'alias'")),
                "case {case:?}: expected the clause to name the member, got {answer}"
            );
        }
    }
}

#[tokio::test]
async fn an_alias_outside_its_modelled_length_is_a_validation_failure() {
    let router = service();
    let directory_id = create_directory(&router, minimal_create("length.example.com"), "length").await;

    // `AliasName` carries `@length` 1..62, which the boundary validator enforces: the
    // bounds themselves, and one past each.
    let longest = "a".repeat(62);
    let rows: [(&str, String, bool); 4] = [
        ("the minimum", "a".to_string(), true),
        ("the maximum", longest.clone(), true),
        ("one below the minimum", String::new(), false),
        ("one past the maximum", "a".repeat(63), false),
    ];

    for (case, alias, satisfies) in rows {
        // A fresh directory for each accepted row, since an alias is a directory's only
        // one; the refused rows store nothing, so they can share one.
        let subject = if satisfies {
            let created = create_directory(
                &router,
                minimal_create(&format!("len{}.example.com", alias.len())),
                case,
            )
            .await;
            // An alias is assigned only to a directory that has reached its serving stage.
            tokio::time::sleep(PAST_ONE_SPAN).await;
            created
        } else {
            directory_id.clone()
        };
        let (status, answer) = call(
            &router,
            "CreateAlias",
            json!({ "DirectoryId": &subject, "Alias": &alias }),
        )
        .await;

        if satisfies {
            assert_eq!(
                status, 200,
                "case {case:?}: expected a {}-character alias to be accepted, got {status} \
                 with {answer}",
                alias.len()
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected a {}-character alias to be refused, got {status} with \
             {answer}",
            alias.len()
        );
        assert_eq!(
            answer["__type"],
            json!("ValidationException"),
            "case {case:?}: expected the validation envelope, got {answer}"
        );
        let message = answer["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("at 'alias'") && message.contains("length"),
            "case {case:?}: expected the clause to name the member and the length \
             constraint, got {answer}"
        );
    }
}
