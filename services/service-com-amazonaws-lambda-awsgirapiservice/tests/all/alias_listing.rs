// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListAliases`: "Returns a list of aliases for a Lambda Event
//! Function."
//!
//! The rule under test: `ListAliases` reports, for the one function its `FunctionName`
//! resolves to, every alias that function holds — ordered by alias name, each entry the
//! stored `AliasConfiguration` that `GetAlias` reports for the same alias — narrowed to
//! the aliases that invoke `FunctionVersion` when the request names one, cut into pages
//! of at most `MaxItems` entries with a `NextMarker` naming the last alias of a
//! truncated page, and it refuses a function or a function version it does not hold and
//! a marker that is no alias name at all.
//!
//! Every suite drives real HTTP requests through the router, so the assertions are about
//! the wire behaviour (status, body) rather than about any handler's internals. Each
//! listed entry is checked against what `GetAlias` reports for the same alias, so
//! nothing here trusts the listing alone. The rows differ along what the rule
//! generalises over: how many aliases the function holds (none, one, several), the alias
//! names (the modelled length bounds, both documented separators, mixed case, digits),
//! the version each alias invokes (unpublished and published), the page size and the
//! marker the page continues from.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `lambda` in `us-east-1`. Nothing in it is verified: this
/// harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names, which every minted ARN carries.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The unpublished version label, as the model's `Version` pattern
/// (`^(\$LATEST|[0-9]+)$`) spells it.
const VERSION_LATEST: &str = "$LATEST";

/// The bounds the model puts on `Alias`, the alias name shape (`@length(min: 1, max:
/// 128)`), from `smithy_query(query="shape", shape="AliasConfiguration")`.
const ALIAS_MAX_LEN: usize = 128;

/// The bounds the model puts on `MaxListItems`, this operation's `MaxItems`
/// (`@range(min: 1, max: 10000)`), from `smithy_query(query="operation",
/// operation="ListAliases")`.
const MAX_ITEMS_MIN: i64 = 1;
const MAX_ITEMS_MAX: i64 = 10_000;

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| self.text())
    }
}

/// One service mounted on its own engine, reached exactly the way the production router
/// reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // Pinned off so the suite does not depend on how the emulator process was started;
    // these tests assert Lambda's behaviour, not the emulator's SigV4 check.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// The unqualified ARN a function of `name` carries.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// `ListAliases` on the function `reference` names, with the given query string.
async fn list_aliases(router: &MultiServiceRouter, reference: &str, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{FUNCTIONS_URI}/{reference}/aliases{query}"),
        &Value::Null,
    )
    .await
}

/// `GetAlias` on the function `reference` names.
async fn get_alias(router: &MultiServiceRouter, reference: &str, alias: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{FUNCTIONS_URI}/{reference}/aliases/{alias}"),
        &Value::Null,
    )
    .await
}

/// Create a function and publish `versions` versions of it, numbered from 1.
///
/// The first comes from `CreateFunction`'s own `Publish` parameter; each further one
/// from a `PublishVersion` that overrides the description, because "Lambda doesn't
/// publish a version if the function's configuration and code haven't changed since the
/// last version".
async fn create_function_with_versions(router: &MultiServiceRouter, name: &str, versions: u32) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Publish": versions >= 1,
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {name}: CreateFunction must succeed, got {} {}",
        created.status,
        created.text()
    );
    for version in 2..=versions {
        let published = call(
            router,
            "POST",
            &format!("{FUNCTIONS_URI}/{name}/versions"),
            &json!({ "Description": format!("snapshot {version}") }),
        )
        .await;
        assert_eq!(
            published.status,
            201,
            "case {name}: PublishVersion {version} must succeed, got {} {}",
            published.status,
            published.text()
        );
    }
}

/// `CreateAlias` of `alias` on `function`, invoking `version`, asserting it was stored.
async fn create_alias(router: &MultiServiceRouter, function: &str, alias: &str, version: &str) {
    let mut body = json!({ "Name": alias, "FunctionVersion": version });
    // Half the aliases carry a description, so an entry that drops or invents one shows
    // up when the listing is compared with `GetAlias`.
    if alias.len().is_multiple_of(2) {
        body["Description"] = json!(format!("alias {alias} of {function}"));
    }
    let created = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/aliases"),
        &body,
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {function}/{alias}: CreateAlias must succeed, got {} {}",
        created.status,
        created.text()
    );
}

/// The `Aliases` array of a successful listing.
fn aliases_of(case: &str, answer: &Answer) -> Vec<Value> {
    assert_eq!(
        answer.status,
        200,
        "case {case}: expected 200 from ListAliases, got {} {}",
        answer.status,
        answer.text()
    );
    let body = answer.json();
    body["Aliases"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an Aliases array, got {body:?}"))
        .clone()
}

/// The `Name` of each entry of a listing page.
fn names_of(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| entry["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// One row of the listing table: a function, how many versions it has, and the aliases
/// to create on it as `(name, version)` pairs in creation order.
struct ListingCase {
    case: &'static str,
    function: &'static str,
    versions: u32,
    aliases: Vec<(String, &'static str)>,
}

/// The rows. They differ along how many aliases the function holds, the alias names
/// (both documented separators, mixed case, digits, and each modelled length bound) and
/// the version each alias invokes.
fn listing_cases() -> Vec<ListingCase> {
    vec![
        // A function nobody has aliased: an empty list, not a refusal.
        ListingCase {
            case: "no aliases at all",
            function: "aliasless",
            versions: 1,
            aliases: Vec::new(),
        },
        ListingCase {
            case: "one alias to the unpublished version",
            function: "one-alias",
            versions: 1,
            aliases: vec![("dev".to_string(), VERSION_LATEST)],
        },
        // Created in the reverse of the order they must be reported in, so a listing
        // that reports insertion order fails here.
        ListingCase {
            case: "two aliases created out of name order",
            function: "two-aliases",
            versions: 1,
            aliases: vec![
                ("zulu".to_string(), "1"),
                ("alpha".to_string(), VERSION_LATEST),
            ],
        },
        // Every form the `Alias` pattern (`^(?!^[0-9]+$)([a-zA-Z0-9-_]+)$`) admits:
        // both separators, mixed case, a digit-bearing name, and the shortest and
        // longest names the model's `@length(min: 1, max: 128)` allows.
        ListingCase {
            case: "aliases spanning the modelled name forms",
            function: "many-aliases",
            versions: 3,
            aliases: vec![
                ("x".to_string(), VERSION_LATEST),
                ("blue-green".to_string(), "1"),
                ("Prod".to_string(), "2"),
                ("stage_2".to_string(), "3"),
                ("v2beta".to_string(), "2"),
                ("A".repeat(ALIAS_MAX_LEN), "1"),
                ("canary".to_string(), VERSION_LATEST),
            ],
        },
    ]
}

/// `ListAliases` reports every alias of the named function, in alias-name order, each
/// entry exactly what `GetAlias` reports for the same alias.
#[tokio::test]
async fn list_aliases_reports_every_stored_alias_in_name_order() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for row in listing_cases() {
        create_function_with_versions(&router, row.function, row.versions).await;
        for (alias, version) in &row.aliases {
            create_alias(&router, row.function, alias, version).await;
        }

        let mut expected: Vec<String> = row
            .aliases
            .iter()
            .map(|(alias, _)| alias.clone())
            .collect();
        expected.sort();

        let answer = list_aliases(&router, row.function, "").await;
        let entries = aliases_of(row.case, &answer);
        assert_eq!(
            names_of(&entries),
            expected,
            "case {}: expected every alias once in name order, got {:?}",
            row.case,
            names_of(&entries)
        );
        assert!(
            answer.json()["NextMarker"].is_null(),
            "case {}: a listing that reported every alias is the last page, so it carries \
             no NextMarker, got {:?}",
            row.case,
            answer.json()["NextMarker"]
        );

        for entry in &entries {
            let alias = entry["Name"].as_str().unwrap_or_default().to_string();
            let invoked = row
                .aliases
                .iter()
                .find(|(name, _)| *name == alias)
                .map(|(_, version)| *version)
                .unwrap_or_default();
            assert_eq!(
                entry["FunctionVersion"],
                json!(invoked),
                "case {} alias {alias}: expected the version the alias invokes, got {:?}",
                row.case,
                entry["FunctionVersion"]
            );
            assert_eq!(
                entry["AliasArn"],
                json!(format!("{}:{alias}", function_arn(row.function))),
                "case {} alias {alias}: expected the function's ARN qualified by the alias \
                 name, got {:?}",
                row.case,
                entry["AliasArn"]
            );

            // The listed entry is the stored configuration, so every member of it is the
            // one `GetAlias` reports for the same alias.
            let fetched = get_alias(&router, row.function, &alias).await;
            assert_eq!(
                fetched.status,
                200,
                "case {} alias {alias}: expected 200 from GetAlias, got {} {}",
                row.case,
                fetched.status,
                fetched.text()
            );
            let configuration = fetched.json();
            for member in [
                "AliasArn",
                "Name",
                "FunctionVersion",
                "Description",
                "RoutingConfig",
                "RevisionId",
            ] {
                assert_eq!(
                    entry[member], configuration[member],
                    "case {} alias {alias}: the listed {member} must be what GetAlias \
                     reports ({:?}), got {:?}",
                    row.case, configuration[member], entry[member]
                );
            }
        }
    }
}

/// "Specify a function version to only list aliases that invoke that version": the
/// listing is narrowed to the aliases invoking it, and a version the function holds but
/// nothing invokes reports an empty list.
#[tokio::test]
async fn list_aliases_reports_only_the_aliases_that_invoke_the_named_version() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let function = "filtered";
    create_function_with_versions(&router, function, 3).await;
    let aliases = [
        ("latest-a", VERSION_LATEST),
        ("one-a", "1"),
        ("two-a", "2"),
        ("one-b", "1"),
        ("latest-b", VERSION_LATEST),
    ];
    for (alias, version) in aliases {
        create_alias(&router, function, alias, version).await;
    }

    // One row per version the function holds, including the published version no alias
    // invokes, plus the unfiltered listing the rows are a subset of.
    let rows: [(&str, Vec<&str>); 5] = [
        (VERSION_LATEST, vec!["latest-a", "latest-b"]),
        ("1", vec!["one-a", "one-b"]),
        ("2", vec!["two-a"]),
        ("3", Vec::new()),
        ("", vec!["latest-a", "latest-b", "one-a", "one-b", "two-a"]),
    ];
    for (version, expected) in rows {
        let query = if version.is_empty() {
            String::new()
        } else {
            format!("?FunctionVersion={version}")
        };
        let case = format!("FunctionVersion={version:?}");
        let answer = list_aliases(&router, function, &query).await;
        let entries = aliases_of(&case, &answer);
        assert_eq!(
            names_of(&entries),
            expected,
            "case {case}: expected only the aliases invoking that version, in name order, \
             got {:?}",
            names_of(&entries)
        );
    }

    // The filter and the page size compose: a filtered listing is paged over the
    // filtered entries, and its marker names the last entry the page reported.
    let answer = list_aliases(&router, function, "?FunctionVersion=1&MaxItems=1").await;
    let entries = aliases_of("filtered first page", &answer);
    assert_eq!(
        names_of(&entries),
        vec!["one-a".to_string()],
        "case filtered first page: expected one entry, got {:?}",
        names_of(&entries)
    );
    assert_eq!(
        answer.json()["NextMarker"],
        json!("one-a"),
        "case filtered first page: expected a marker naming the last entry of the page, got \
         {:?}",
        answer.json()["NextMarker"]
    );
    let answer = list_aliases(
        &router,
        function,
        "?FunctionVersion=1&MaxItems=1&Marker=one-a",
    )
    .await;
    let entries = aliases_of("filtered second page", &answer);
    assert_eq!(
        names_of(&entries),
        vec!["one-b".to_string()],
        "case filtered second page: expected the next filtered entry, got {:?}",
        names_of(&entries)
    );
    assert!(
        answer.json()["NextMarker"].is_null(),
        "case filtered second page: the last page carries no NextMarker, got {:?}",
        answer.json()["NextMarker"]
    );
}

/// A paged walk reports every alias of the function exactly once, in name order,
/// whatever the page size — and a marker continues after the alias it names.
#[tokio::test]
async fn list_aliases_pages_over_every_alias() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let function = "paged";
    create_function_with_versions(&router, function, 2).await;
    // Created out of order, and each name differing from the others in length and in the
    // separator it carries.
    let created = [
        "release-2", "beta", "Gamma", "a", "prod_blue", "prod_green", "zz-last", "mid_1",
    ];
    for (index, alias) in created.iter().enumerate() {
        let version = if index % 2 == 0 { "1" } else { VERSION_LATEST };
        create_alias(&router, function, alias, version).await;
    }
    let mut expected: Vec<String> = created.iter().map(|name| (*name).to_string()).collect();
    expected.sort();

    // Page sizes that divide the entry count, that do not, that are the whole count, and
    // that are far past it (up to the modelled maximum).
    for page_size in [1_usize, 2, 3, 8, expected.len() + 1, 10_000] {
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= expected.len() + 2,
                "case page size {page_size}: paging did not terminate after {pages} pages"
            );
            let query = match &marker {
                Some(marker) => format!("?MaxItems={page_size}&Marker={marker}"),
                None => format!("?MaxItems={page_size}"),
            };
            let answer = list_aliases(&router, function, &query).await;
            let page = aliases_of(&format!("page size {page_size}"), &answer);
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: expected at most {page_size} entries, got {}",
                page.len()
            );
            for name in names_of(&page) {
                assert!(
                    !seen.contains(&name),
                    "case page size {page_size}: alias {name} was reported twice"
                );
                seen.push(name);
            }
            match answer.json()["NextMarker"].as_str() {
                Some(next) => marker = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case page size {page_size}: expected every alias once in name order, got {seen:?}"
        );
    }

    // A marker continues AFTER the alias it names, whichever alias that is.
    for start_after in &expected {
        let answer = list_aliases(&router, function, &format!("?Marker={start_after}")).await;
        let names = names_of(&aliases_of(&format!("marker {start_after}"), &answer));
        let tail: Vec<String> = expected
            .iter()
            .skip_while(|name| *name != start_after)
            .skip(1)
            .cloned()
            .collect();
        assert_eq!(
            names, tail,
            "case marker {start_after}: expected the aliases after it, got {names:?}"
        );
    }

    // A marker naming an alias name the function does not hold still names a place in
    // the order, so the page is the aliases ordered after it.
    let answer = list_aliases(&router, function, "?Marker=mz").await;
    let names = names_of(&aliases_of("marker between two aliases", &answer));
    let tail: Vec<String> = expected
        .iter()
        .filter(|name| name.as_str() > "mz")
        .cloned()
        .collect();
    assert_eq!(
        names, tail,
        "case marker between two aliases: expected the aliases ordered after it, got {names:?}"
    );
}

/// A function holding more aliases than the old emulator ceiling allowed: `MaxItems`
/// alone limits a page, so a caller that names none is answered with every alias, and a
/// caller that names one walks the rest through the marker.
///
/// `ListAliases`'s `MaxItems` documents only "Limit the number of aliases returned" —
/// unlike `ListFunctions`'s and `ListVersionsByFunction`'s, which each document
/// "returns a maximum of 50 items in each response, even if you set the number higher" —
/// so nothing caps an unasked-for page below the aliases the function holds. The count
/// is above one hundred because AWS accepts more than that many aliases on one function.
#[tokio::test]
async fn list_aliases_reports_every_alias_of_a_function_that_holds_more_than_a_hundred() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let function = "crowded";
    create_function_with_versions(&router, function, 1).await;
    let total = 150_usize;
    let mut expected: Vec<String> = (0..total).map(|index| format!("a{index:04}")).collect();
    for alias in &expected {
        create_alias(&router, function, alias, "1").await;
    }
    expected.sort();

    let answer = list_aliases(&router, function, "").await;
    let names = names_of(&aliases_of("unlimited page", &answer));
    assert_eq!(
        names, expected,
        "case unlimited page: expected all {total} aliases in name order, got {} entries",
        names.len()
    );
    assert!(
        answer.json()["NextMarker"].is_null(),
        "case unlimited page: a listing that reported every alias carries no NextMarker, \
         got {:?}",
        answer.json()["NextMarker"]
    );

    // The same aliases, walked in pages of 40: every one of them, once, in order.
    let page_size = 40_usize;
    let mut seen: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let query = match &marker {
            Some(marker) => format!("?MaxItems={page_size}&Marker={marker}"),
            None => format!("?MaxItems={page_size}"),
        };
        let answer = list_aliases(&router, function, &query).await;
        let page = aliases_of("paged over a crowded function", &answer);
        assert!(
            page.len() <= page_size,
            "case paged over a crowded function: expected at most {page_size} entries, got {}",
            page.len()
        );
        seen.extend(names_of(&page));
        match answer.json()["NextMarker"].as_str() {
            Some(next) => marker = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(
        seen, expected,
        "case paged over a crowded function: expected every alias once in name order, got \
         {} entries",
        seen.len()
    );
}

/// One row of the refusal table.
struct RefusalCase {
    case: &'static str,
    /// The function reference in the URI.
    reference: &'static str,
    /// The query string of the request.
    query: &'static str,
    status: u16,
    error_code: &'static str,
}

/// A request naming no function, no such version of one, or a page the operation cannot
/// address is refused with the modelled error rather than answered with an empty page.
#[tokio::test]
async fn list_aliases_refuses_a_request_it_cannot_serve() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function_with_versions(&router, "refusals", 1).await;
    create_alias(&router, "refusals", "live", "1").await;

    let cases = [
        // "The resource specified in the request does not exist."
        RefusalCase {
            case: "unknown function",
            reference: "absent",
            query: "",
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "unknown function by arn",
            reference: "arn:aws:lambda:us-east-1:000000000000:function:absent",
            query: "",
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        // The aliases listed are the aliases of a function, so a `FunctionVersion` the
        // function does not hold names no version to filter on.
        RefusalCase {
            case: "FunctionVersion the function never published",
            reference: "refusals",
            query: "?FunctionVersion=9",
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        // An alias belongs to the function, not to one of its versions: a qualified
        // `FunctionName` names no function whose aliases to list.
        RefusalCase {
            case: "FunctionName qualified by a version",
            reference: "refusals:1",
            query: "",
            status: 400,
            error_code: "InvalidParameterValueException",
        },
        RefusalCase {
            case: "FunctionName qualified by an alias",
            reference: "refusals:live",
            query: "",
            status: 400,
            error_code: "InvalidParameterValueException",
        },
        // The marker is the name of the last alias a previous page reported, so a value
        // that is no alias name at all names no page.
        RefusalCase {
            case: "marker is not an alias name",
            reference: "refusals",
            query: "?Marker=not%20an%20alias",
            status: 400,
            error_code: "InvalidParameterValueException",
        },
        RefusalCase {
            case: "marker is a version number",
            reference: "refusals",
            query: "?Marker=1",
            status: 400,
            error_code: "InvalidParameterValueException",
        },
    ];

    for case in cases {
        let answer = list_aliases(&router, case.reference, case.query).await;
        assert_eq!(
            answer.status, case.status,
            "case {}: expected {}, got {} {}",
            case.case,
            case.status,
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains(case.error_code),
            "case {}: expected the {} error code, got {}",
            case.case,
            case.error_code,
            answer.error_code()
        );
    }

    // `MaxItems` is modelled `@range(min: 1, max: 10000)`: a page size outside it is
    // refused rather than paged over, and each published bound is accepted.
    for max_items in [0, MAX_ITEMS_MIN - 2, MAX_ITEMS_MAX + 1] {
        let answer = list_aliases(&router, "refusals", &format!("?MaxItems={max_items}")).await;
        assert_eq!(
            answer.status,
            400,
            "case MaxItems {max_items}: expected 400, got {} {}",
            answer.status,
            answer.text()
        );
    }
    for max_items in [MAX_ITEMS_MIN, MAX_ITEMS_MAX] {
        let answer = list_aliases(&router, "refusals", &format!("?MaxItems={max_items}")).await;
        let names = names_of(&aliases_of(&format!("MaxItems {max_items}"), &answer));
        assert_eq!(
            names,
            vec!["live".to_string()],
            "case MaxItems {max_items}: expected the function's one alias, got {names:?}"
        );
    }

    // `FunctionVersion` is typed `VersionWithLatestPublished`, which also admits
    // `$LATEST.PUBLISHED`; `CreateAlias` mints no alias to it, so this operation does
    // not model what filtering on it selects and keeps the scaffold's 501 for that one
    // value rather than inventing an answer.
    let answer = list_aliases(&router, "refusals", "?FunctionVersion=$LATEST.PUBLISHED").await;
    assert_eq!(
        answer.status,
        501,
        "case FunctionVersion=$LATEST.PUBLISHED: expected 501, got {} {}",
        answer.status,
        answer.text()
    );
}
