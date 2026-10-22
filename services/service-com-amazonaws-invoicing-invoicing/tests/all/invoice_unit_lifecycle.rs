// SPDX-License-Identifier: CC0-1.0

//! The invoice-unit lifecycle, driven end to end over this service's wire protocol.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router
//! uses: `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an awsJson1_0
//! body. Nothing reaches into the state struct, so what these tests see is exactly
//! what an SDK client sees.
//!
//! The tables below span each rule the slice implements rather than one recorded
//! request: a create/read round trip over differently shaped inputs, the filter
//! semantics each `Filters` member documents, pagination at several page sizes, the
//! in-place update of only the members a request carries, and the `AsOf` read of the
//! configuration that was in effect at an instant.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// The `@pattern` of `InvoiceUnitArnString`, the shape `CreateInvoiceUnit` returns:
/// `smithy_query(query="operation", operation="CreateInvoiceUnit")`.
const INVOICE_UNIT_ARN_PATTERN: &str =
    r"^arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$";

/// The `@pattern` of `TagrisArn`, the ARN `ListTagsForResource` accepts:
/// `smithy_query(query="operation", operation="ListTagsForResource")`.
const TAGRIS_ARN_PATTERN: &str = r"^arn:aws[-a-z0-9]*:(invoicing)::[0-9]{12}:[-a-zA-Z0-9/:_]+$";

/// Whether `arn` satisfies [`INVOICE_UNIT_ARN_PATTERN`], one segment per character
/// class of that `@pattern`. The resource segment itself admits `:`, so the split keeps
/// the remainder whole.
fn is_invoice_unit_arn(arn: &str) -> bool {
    let parts: Vec<&str> = arn.splitn(6, ':').collect();
    let lower_or_digit_or_dash =
        |text: &str| text.chars().all(|c| c == '-' || c.is_ascii_lowercase() || c.is_ascii_digit());
    parts.len() == 6
        // `arn:`
        && parts[0] == "arn"
        // `aws[-a-z0-9]*`
        && parts[1]
            .strip_prefix("aws")
            .is_some_and(lower_or_digit_or_dash)
        // `[a-z0-9]+`
        && !parts[2].is_empty()
        && parts[2]
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        // `[-a-z0-9]*`
        && lower_or_digit_or_dash(parts[3])
        // `[0-9]{12}`
        && parts[4].len() == 12
        && parts[4].chars().all(|c| c.is_ascii_digit())
        // `[-a-zA-Z0-9/:_]+`
        && !parts[5].is_empty()
        && parts[5]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '/' | ':' | '_'))
}

/// Whether `arn` satisfies [`TAGRIS_ARN_PATTERN`]: the invoice-unit ARN form, narrowed
/// to the `invoicing` service and the empty region that pattern spells out.
fn is_tagris_arn(arn: &str) -> bool {
    let parts: Vec<&str> = arn.splitn(6, ':').collect();
    is_invoice_unit_arn(arn) && parts[2] == "invoicing" && parts[3].is_empty()
}

/// A router carrying only this service, over one shared engine — the same assembly
/// the production router performs, minus the other services.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is the router's concern, not this slice's: drive the operations unsigned.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_invoicing_invoicing::EmulatedService::new(
        engine,
    ));
    router
}

/// One awsJson1_0 request, returning the HTTP status and the parsed body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", format!("Invoicing.{operation}"))
        .header("content-type", "application/x-amz-json-1.0")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The same call, asserting it succeeded.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} {parsed}"
    );
    parsed
}

/// The wire error code of a failure body (`__type` on the awsJson protocols).
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `InvoiceUnitArn` of a response that carries one.
fn arn_of(body: &Value) -> String {
    body.get("InvoiceUnitArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected an InvoiceUnitArn in {body}"))
        .to_string()
}

/// One `CreateInvoiceUnit` request shape, as a table row.
#[derive(Debug, Clone)]
struct CreateCase {
    name: &'static str,
    invoice_receiver: &'static str,
    description: Option<&'static str>,
    tax_inheritance_disabled: bool,
    linked_accounts: Option<&'static [&'static str]>,
    bill_source_accounts: Option<&'static [&'static str]>,
    tags: &'static [(&'static str, &'static str)],
}

impl CreateCase {
    fn request(&self, client_token: &str) -> Value {
        let mut rule = serde_json::Map::new();
        if let Some(accounts) = self.linked_accounts {
            rule.insert("LinkedAccounts".to_string(), json!(accounts));
        }
        if let Some(accounts) = self.bill_source_accounts {
            rule.insert("BillSourceAccounts".to_string(), json!(accounts));
        }
        let mut request = json!({
            "Name": self.name,
            "InvoiceReceiver": self.invoice_receiver,
            "TaxInheritanceDisabled": self.tax_inheritance_disabled,
            "Rule": Value::Object(rule),
            "ClientToken": client_token,
        });
        if let Some(description) = self.description {
            request["Description"] = json!(description);
        }
        if !self.tags.is_empty() {
            request["ResourceTags"] = Value::Array(
                self.tags
                    .iter()
                    .map(|(key, value)| json!({"Key": key, "Value": value}))
                    .collect(),
            );
        }
        request
    }
}

/// Rows differing along what the create/read round trip generalises over: the name's
/// character classes, whether `Description` is carried at all, both values of
/// `TaxInheritanceDisabled`, each shape of `InvoiceUnitRule` (linked only, bill
/// sources only, both, neither), and tagged vs untagged.
const CREATE_CASES: &[CreateCase] = &[
    CreateCase {
        name: "BootstrapUnit",
        invoice_receiver: "123456789012",
        description: Some("first slice"),
        tax_inheritance_disabled: false,
        linked_accounts: Some(&["210987654321"]),
        bill_source_accounts: None,
        tags: &[("env", "test")],
    },
    CreateCase {
        name: "unit with spaces and 1234",
        invoice_receiver: "210987654321",
        description: None,
        tax_inheritance_disabled: true,
        linked_accounts: Some(&["111111111111", "222222222222"]),
        bill_source_accounts: Some(&["333333333333"]),
        tags: &[("owner", "finance"), ("env", "prod")],
    },
    CreateCase {
        name: "Ünïcödé_Einheit-3",
        invoice_receiver: "310987654321",
        description: Some(""),
        tax_inheritance_disabled: false,
        linked_accounts: None,
        bill_source_accounts: Some(&["444444444444", "555555555555"]),
        tags: &[],
    },
    CreateCase {
        name: "x",
        invoice_receiver: "410987654321",
        description: Some("no rule accounts at all"),
        tax_inheritance_disabled: true,
        linked_accounts: Some(&[]),
        bill_source_accounts: None,
        tags: &[("only", "")],
    },
];

/// A create mints an ARN of the modelled form, stores the unit, and a following get
/// reports back exactly what was stored — for every shape of input in the table.
#[tokio::test]
async fn create_then_get_reports_what_was_created() {
    let router = emulator();

    for (index, case) in CREATE_CASES.iter().enumerate() {
        let created = ok(
            &router,
            "CreateInvoiceUnit",
            &case.request(&format!("token-{index}")),
        )
        .await;
        let arn = arn_of(&created);
        assert!(
            is_invoice_unit_arn(&arn),
            "case {case:?}: InvoiceUnitArn must match {INVOICE_UNIT_ARN_PATTERN}, got {arn:?}"
        );

        let got = ok(
            &router,
            "GetInvoiceUnit",
            &json!({"InvoiceUnitArn": arn.clone()}),
        )
        .await;
        assert_eq!(
            got.get("InvoiceUnitArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: get must report the ARN it was asked for, got {got}"
        );
        assert_eq!(
            got.get("Name").and_then(Value::as_str),
            Some(case.name),
            "case {case:?}: stored Name, got {got}"
        );
        assert_eq!(
            got.get("InvoiceReceiver").and_then(Value::as_str),
            Some(case.invoice_receiver),
            "case {case:?}: stored InvoiceReceiver, got {got}"
        );
        assert_eq!(
            got.get("Description").and_then(Value::as_str),
            case.description,
            "case {case:?}: stored Description, got {got}"
        );
        assert_eq!(
            got.get("TaxInheritanceDisabled").and_then(Value::as_bool),
            Some(case.tax_inheritance_disabled),
            "case {case:?}: stored TaxInheritanceDisabled, got {got}"
        );
        assert_eq!(
            got.get("Rule")
                .and_then(|rule| rule.get("LinkedAccounts"))
                .cloned(),
            case.linked_accounts.map(|accounts| json!(accounts)),
            "case {case:?}: stored Rule.LinkedAccounts, got {got}"
        );
        assert_eq!(
            got.get("Rule")
                .and_then(|rule| rule.get("BillSourceAccounts"))
                .cloned(),
            case.bill_source_accounts.map(|accounts| json!(accounts)),
            "case {case:?}: stored Rule.BillSourceAccounts, got {got}"
        );
        assert!(
            got.get("LastModified").is_some_and(Value::is_number),
            "case {case:?}: LastModified must be an epoch-seconds timestamp, got {got}"
        );
    }

    // Each row created its own unit, and nothing else.
    let listed = ok(&router, "ListInvoiceUnits", &json!({})).await;
    let units = listed["InvoiceUnits"].as_array().expect("a unit list");
    assert_eq!(
        units.len(),
        CREATE_CASES.len(),
        "every row must have created exactly one unit, got {listed}"
    );
}

/// `CreateInvoiceUnit` is `@idempotent`: a repeat carrying the same `ClientToken`
/// returns the first ARN and creates nothing further, while a fresh token creates a
/// new unit.
#[tokio::test]
async fn create_replays_the_same_client_token() {
    let router = emulator();
    let case = &CREATE_CASES[0];

    let first = arn_of(&ok(&router, "CreateInvoiceUnit", &case.request("tok-1")).await);
    let replay = arn_of(&ok(&router, "CreateInvoiceUnit", &case.request("tok-1")).await);
    assert_eq!(
        replay, first,
        "case replay of tok-1: the same ClientToken must return the first ARN, got {replay:?}"
    );

    // A replay must not have created a second unit, under any name.
    let listed = ok(&router, "ListInvoiceUnits", &json!({})).await;
    let units = listed["InvoiceUnits"].as_array().expect("a unit list");
    assert_eq!(
        units.len(),
        1,
        "case replay of tok-1: state must hold one unit, got {listed}"
    );

    // A different request under a different token is a different unit.
    let other = arn_of(&ok(&router, "CreateInvoiceUnit", &CREATE_CASES[1].request("tok-2")).await);
    assert_ne!(
        other, first,
        "case tok-2: a fresh ClientToken must mint a new ARN, got {other:?}"
    );
}

/// `Name` is documented as unique per account, so a second unit under a name already
/// in use is refused with HTTP 400 `ValidationException` rather than created.
#[tokio::test]
async fn a_duplicate_name_is_refused() {
    let router = emulator();
    let case = &CREATE_CASES[0];
    ok(&router, "CreateInvoiceUnit", &case.request("tok-1")).await;

    // Same name, different everything else and a fresh token.
    let duplicate = CreateCase {
        invoice_receiver: "999999999999",
        description: Some("a different unit under a used name"),
        ..case.clone()
    };
    let (status, body) = call(&router, "CreateInvoiceUnit", &duplicate.request("tok-2")).await;
    assert_eq!(
        status, 400,
        "case duplicate name: expected HTTP 400, got {status} {body}"
    );
    assert!(
        error_code(&body).contains("ValidationException"),
        "case duplicate name: expected a ValidationException, got {body}"
    );

    let listed = ok(&router, "ListInvoiceUnits", &json!({})).await;
    assert_eq!(
        listed["InvoiceUnits"].as_array().map(Vec::len),
        Some(1),
        "case duplicate name: the refused create must not have stored a unit, got {listed}"
    );
}

/// Create the whole table and return each row's ARN, in table order.
async fn create_all(router: &MultiServiceRouter) -> Vec<String> {
    let mut arns = Vec::new();
    for (index, case) in CREATE_CASES.iter().enumerate() {
        let created = ok(
            router,
            "CreateInvoiceUnit",
            &case.request(&format!("token-{index}")),
        )
        .await;
        arns.push(arn_of(&created));
    }
    arns
}

/// The names `ListInvoiceUnits` reported, in the order it reported them.
fn listed_names(body: &Value) -> Vec<String> {
    body["InvoiceUnits"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|unit| {
            unit.get("Name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Values within one filter are OR-ed and filters of different types are AND-ed, as
/// each `Filters` member documents; `Accounts` matches the invoice receiver OR a
/// linked account of the rule.
#[tokio::test]
async fn list_honours_every_filter() {
    let router = emulator();
    create_all(&router).await;

    // (filters, the names that must come back)
    let cases: Vec<(Value, Vec<&str>)> = vec![
        (json!({}), CREATE_CASES.iter().map(|c| c.name).collect()),
        (
            json!({"Names": ["BootstrapUnit"]}),
            vec!["BootstrapUnit"],
        ),
        (
            json!({"Names": ["BootstrapUnit", "x"]}),
            vec!["BootstrapUnit", "x"],
        ),
        (json!({"Names": ["nope"]}), vec![]),
        // An empty value list constrains nothing, the same as an absent filter.
        (
            json!({"Names": []}),
            CREATE_CASES.iter().map(|c| c.name).collect(),
        ),
        (
            json!({"InvoiceReceivers": ["310987654321"]}),
            vec!["Ünïcödé_Einheit-3"],
        ),
        (
            json!({"InvoiceReceivers": ["123456789012", "410987654321"]}),
            vec!["BootstrapUnit", "x"],
        ),
        // `Accounts` matches the receiver...
        (
            json!({"Accounts": ["210987654321"]}),
            vec!["BootstrapUnit", "unit with spaces and 1234"],
        ),
        // ...or a linked account of the rule.
        (
            json!({"Accounts": ["222222222222"]}),
            vec!["unit with spaces and 1234"],
        ),
        (
            json!({"BillSourceAccounts": ["333333333333", "555555555555"]}),
            vec!["unit with spaces and 1234", "Ünïcödé_Einheit-3"],
        ),
        // Two filter types AND-ed: both must hold.
        (
            json!({"Names": ["BootstrapUnit"], "InvoiceReceivers": ["123456789012"]}),
            vec!["BootstrapUnit"],
        ),
        (
            json!({"Names": ["BootstrapUnit"], "InvoiceReceivers": ["410987654321"]}),
            vec![],
        ),
    ];

    for (filters, expected) in cases {
        let listed = ok(&router, "ListInvoiceUnits", &json!({"Filters": filters})).await;
        let mut got = listed_names(&listed);
        got.sort();
        let mut want: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        want.sort();
        assert_eq!(
            got, want,
            "case Filters={filters}: expected {want:?}, got {got:?}"
        );
    }
}

/// `MaxResults` is a page size and `NextToken` resumes where the page stopped: at
/// every page size the pages concatenate to the full set, in order, with no
/// duplicates, and only the last page omits `NextToken`.
#[tokio::test]
async fn list_pages_at_every_page_size() {
    let router = emulator();
    create_all(&router).await;
    let all = listed_names(&ok(&router, "ListInvoiceUnits", &json!({})).await);
    assert_eq!(
        all.len(),
        CREATE_CASES.len(),
        "the unpaged list must hold every unit, got {all:?}"
    );

    // One page size per row: below, at, and above the number of units, plus the
    // modelled maximum of 500.
    for page_size in [1_usize, 2, 3, 4, 5, 500] {
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"MaxResults": page_size});
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let page = ok(&router, "ListInvoiceUnits", &request).await;
            let names = listed_names(&page);
            assert!(
                names.len() <= page_size,
                "case MaxResults={page_size}: a page must hold at most {page_size}, got {names:?}"
            );
            walked.extend(names);
            next_token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= CREATE_CASES.len() + 1,
                "case MaxResults={page_size}: pagination did not terminate after {pages} pages"
            );
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, all,
            "case MaxResults={page_size}: the pages must concatenate to the full list in order, got {walked:?}"
        );
        let expected_pages = all.len().div_ceil(page_size).max(1);
        assert_eq!(
            pages, expected_pages,
            "case MaxResults={page_size}: expected {expected_pages} pages, got {pages}"
        );
    }
}

/// A `NextToken` this service did not mint is refused with the modelled
/// `ValidationException` rather than silently serving the first page.
#[tokio::test]
async fn an_unrecognised_next_token_is_refused() {
    let router = emulator();
    create_all(&router).await;

    for token in ["not-a-token", "6e6f742d612d746f6b656e", "00"] {
        let (status, body) = call(
            &router,
            "ListInvoiceUnits",
            &json!({"NextToken": token}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case NextToken={token:?}: expected HTTP 400, got {status} {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case NextToken={token:?}: expected a ValidationException, got {body}"
        );
    }
}

/// `UpdateInvoiceUnit` replaces only the members the request carries, leaves the rest
/// as they were, keeps `Name`/`InvoiceReceiver` immutable, and records a newer
/// `LastModified` — which the next get reports.
#[tokio::test]
async fn update_changes_only_the_members_the_request_carries() {
    let router = emulator();

    // (the update request, the members it must have changed)
    let cases: Vec<(Value, Value)> = vec![
        (
            json!({"Description": "updated"}),
            json!({"Description": "updated"}),
        ),
        (
            json!({"TaxInheritanceDisabled": true}),
            json!({"TaxInheritanceDisabled": true}),
        ),
        (
            json!({"Rule": {"LinkedAccounts": ["210987654321", "310987654321"]}}),
            json!({"Rule": {"LinkedAccounts": ["210987654321", "310987654321"]}}),
        ),
        (
            json!({
                "Description": "everything at once",
                "TaxInheritanceDisabled": true,
                "Rule": {"BillSourceAccounts": ["666666666666"]},
            }),
            json!({
                "Description": "everything at once",
                "TaxInheritanceDisabled": true,
                "Rule": {"BillSourceAccounts": ["666666666666"]},
            }),
        ),
    ];

    for (index, (update, changed)) in cases.iter().enumerate() {
        let router = &router;
        let case = CreateCase {
            name: CREATE_CASES[0].name,
            ..CREATE_CASES[0].clone()
        };
        // Each row works on its own unit, so the rows do not interfere; the name is
        // unique per account, so delete the previous one first.
        let created = ok(router, "CreateInvoiceUnit", &case.request(&format!("c-{index}"))).await;
        let arn = arn_of(&created);
        let before = ok(router, "GetInvoiceUnit", &json!({"InvoiceUnitArn": arn})).await;

        // The clock must advance between the two writes for LastModified to move.
        tokio::time::sleep(Duration::from_millis(5)).await;

        let mut request = update.clone();
        request["InvoiceUnitArn"] = json!(arn);
        request["ClientToken"] = json!(format!("u-{index}"));
        let updated = ok(router, "UpdateInvoiceUnit", &request).await;
        assert_eq!(
            updated.get("InvoiceUnitArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {update}: update must return the ARN it changed, got {updated}"
        );

        let after = ok(router, "GetInvoiceUnit", &json!({"InvoiceUnitArn": arn})).await;
        for (member, expected) in changed.as_object().expect("an object of changed members") {
            assert_eq!(
                after.get(member),
                Some(expected),
                "case {update}: {member} must be the updated value, got {after}"
            );
        }
        for member in ["Name", "InvoiceReceiver"] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {update}: {member} is immutable, got {after}"
            );
        }
        for member in ["Description", "TaxInheritanceDisabled", "Rule"] {
            if changed.get(member).is_none() {
                assert_eq!(
                    after.get(member),
                    before.get(member),
                    "case {update}: {member} was not in the request and must be unchanged, got {after}"
                );
            }
        }
        let last_modified = |body: &Value| {
            body.get("LastModified")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("expected a LastModified in {body}"))
        };
        assert!(
            last_modified(&after) > last_modified(&before),
            "case {update}: LastModified must move forward, got {} then {}",
            last_modified(&before),
            last_modified(&after)
        );

        // Leave state empty for the next row.
        ok(
            router,
            "DeleteInvoiceUnit",
            &json!({"InvoiceUnitArn": arn, "ClientToken": format!("d-{index}")}),
        )
        .await;
    }
}

/// An ARN no create minted is a `ResourceNotFoundException` (HTTP 404) from every
/// reader and writer of an invoice unit — never a 501 and never an empty 200.
#[tokio::test]
async fn an_unknown_arn_is_not_found() {
    let router = emulator();
    create_all(&router).await;
    let unknown = "arn:aws:invoicing::000000000000:invoice-unit/ffffffffffff";

    let cases: Vec<(&str, Value)> = vec![
        ("GetInvoiceUnit", json!({"InvoiceUnitArn": unknown})),
        (
            "UpdateInvoiceUnit",
            json!({"InvoiceUnitArn": unknown, "Description": "nope"}),
        ),
        ("DeleteInvoiceUnit", json!({"InvoiceUnitArn": unknown})),
        ("ListTagsForResource", json!({"ResourceArn": unknown})),
    ];
    for (operation, request) in cases {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 404,
            "case {operation} on an unknown ARN: expected HTTP 404, got {status} {body}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {operation} on an unknown ARN: expected a ResourceNotFoundException, got {body}"
        );
    }
}

/// `DeleteInvoiceUnit` removes the unit from state: the next get is a 404, the list no
/// longer reports it, its tags are gone, and the delete is `@idempotent` under its
/// `ClientToken`.
#[tokio::test]
async fn delete_removes_the_unit_from_state() {
    let router = emulator();
    let arns = create_all(&router).await;
    let doomed = arns[1].clone();
    let survivor = arns[0].clone();

    let deleted = ok(
        &router,
        "DeleteInvoiceUnit",
        &json!({"InvoiceUnitArn": doomed, "ClientToken": "del-1"}),
    )
    .await;
    assert_eq!(
        deleted.get("InvoiceUnitArn").and_then(Value::as_str),
        Some(doomed.as_str()),
        "case delete: the response must name the ARN it removed, got {deleted}"
    );

    let (status, body) = call(&router, "GetInvoiceUnit", &json!({"InvoiceUnitArn": doomed})).await;
    assert_eq!(
        status, 404,
        "case get after delete: expected HTTP 404, got {status} {body}"
    );

    let listed = listed_names(&ok(&router, "ListInvoiceUnits", &json!({})).await);
    assert!(
        !listed.contains(&CREATE_CASES[1].name.to_string()),
        "case list after delete: the deleted unit must be gone, got {listed:?}"
    );
    assert_eq!(
        listed.len(),
        CREATE_CASES.len() - 1,
        "case list after delete: every other unit must remain, got {listed:?}"
    );

    // The tags went with it; the survivor's are untouched.
    let (status, body) = call(
        &router,
        "ListTagsForResource",
        &json!({"ResourceArn": doomed}),
    )
    .await;
    assert_eq!(
        status, 404,
        "case tags after delete: expected HTTP 404, got {status} {body}"
    );
    let survivor_tags = ok(
        &router,
        "ListTagsForResource",
        &json!({"ResourceArn": survivor}),
    )
    .await;
    assert_eq!(
        survivor_tags["ResourceTags"],
        json!([{"Key": "env", "Value": "test"}]),
        "case tags of a surviving unit: got {survivor_tags}"
    );

    // `@idempotent`: the same token replays the first delete rather than 404-ing.
    let replay = ok(
        &router,
        "DeleteInvoiceUnit",
        &json!({"InvoiceUnitArn": doomed, "ClientToken": "del-1"}),
    )
    .await;
    assert_eq!(
        replay.get("InvoiceUnitArn").and_then(Value::as_str),
        Some(doomed.as_str()),
        "case delete replay: the same ClientToken must replay the first result, got {replay}"
    );
    // ...and a fresh token on a unit that is gone is a 404.
    let (status, body) = call(
        &router,
        "DeleteInvoiceUnit",
        &json!({"InvoiceUnitArn": doomed, "ClientToken": "del-2"}),
    )
    .await;
    assert_eq!(
        status, 404,
        "case delete under a fresh token: expected HTTP 404, got {status} {body}"
    );
}

/// `ListTagsForResource` reports the `ResourceTags` the create persisted, for every
/// tag shape in the table — and an empty list for a unit created without any.
#[tokio::test]
async fn list_tags_reports_the_tags_that_were_persisted() {
    let router = emulator();
    let arns = create_all(&router).await;

    for (case, arn) in CREATE_CASES.iter().zip(&arns) {
        assert!(
            is_tagris_arn(arn),
            "case {case:?}: a minted ARN must be acceptable to ListTagsForResource \
             ({TAGRIS_ARN_PATTERN}), got {arn:?}"
        );
        let listed = ok(
            &router,
            "ListTagsForResource",
            &json!({"ResourceArn": arn}),
        )
        .await;
        let mut got: Vec<(String, String)> = listed["ResourceTags"]
            .as_array()
            .expect("a tag list")
            .iter()
            .map(|tag| {
                (
                    tag["Key"].as_str().unwrap_or_default().to_string(),
                    tag["Value"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        got.sort();
        let mut want: Vec<(String, String)> = case
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        want.sort();
        assert_eq!(
            got, want,
            "case {case:?}: expected the persisted tags {want:?}, got {got:?}"
        );
    }
}

/// `AsOf` reads the configuration that was in effect at that instant: before the
/// update for an instant between the two writes, and nothing at all for an instant
/// before the unit existed.
#[tokio::test]
async fn as_of_reads_the_configuration_in_effect_then() {
    let router = emulator();
    let before_creation = epoch_seconds_now();
    tokio::time::sleep(Duration::from_millis(5)).await;

    let arn = arn_of(&ok(&router, "CreateInvoiceUnit", &CREATE_CASES[0].request("tok-1")).await);
    let created = ok(&router, "GetInvoiceUnit", &json!({"InvoiceUnitArn": arn})).await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let between = epoch_seconds_now();
    tokio::time::sleep(Duration::from_millis(5)).await;

    ok(
        &router,
        "UpdateInvoiceUnit",
        &json!({
            "InvoiceUnitArn": arn,
            "Description": "after the update",
            "ClientToken": "upd-1",
        }),
    )
    .await;

    let now = ok(&router, "GetInvoiceUnit", &json!({"InvoiceUnitArn": arn})).await;
    assert_eq!(
        now.get("Description").and_then(Value::as_str),
        Some("after the update"),
        "case AsOf absent: the current configuration is reported, got {now}"
    );

    let earlier = ok(
        &router,
        "GetInvoiceUnit",
        &json!({"InvoiceUnitArn": arn, "AsOf": between}),
    )
    .await;
    assert_eq!(
        earlier.get("Description"),
        created.get("Description"),
        "case AsOf between the writes: the configuration in effect then is reported, got {earlier}"
    );

    let (status, body) = call(
        &router,
        "GetInvoiceUnit",
        &json!({"InvoiceUnitArn": arn, "AsOf": before_creation}),
    )
    .await;
    assert_eq!(
        status, 404,
        "case AsOf before creation: the unit did not exist then, got {status} {body}"
    );

    let listed = listed_names(
        &ok(
            &router,
            "ListInvoiceUnits",
            &json!({"AsOf": before_creation}),
        )
        .await,
    );
    assert!(
        listed.is_empty(),
        "case list AsOf before creation: nothing existed then, got {listed:?}"
    );
    let listed_now = listed_names(&ok(&router, "ListInvoiceUnits", &json!({"AsOf": between})).await);
    assert_eq!(
        listed_now,
        vec![CREATE_CASES[0].name.to_string()],
        "case list AsOf between the writes: the unit existed then, got {listed_now:?}"
    );
}

/// Now, as the epoch-seconds number an awsJson1_0 timestamp is carried as.
fn epoch_seconds_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// A `ClientToken` identifies a repeat of ONE request, so the same token used against a
/// different invoice unit is a different request: its write must be applied, not
/// discarded, and the response must name the unit the caller asked about.
#[tokio::test]
async fn one_client_token_against_another_unit_is_not_a_replay() {
    let router = emulator();
    let first = arn_of(&ok(&router, "CreateInvoiceUnit", &CREATE_CASES[0].request("c-1")).await);
    let second = arn_of(&ok(&router, "CreateInvoiceUnit", &CREATE_CASES[1].request("c-2")).await);
    let shared_token = "shared-token";

    // (operation, the unit it targets, the description it writes)
    for (target, description) in [(&first, "written to the first"), (&second, "written to the second")] {
        let updated = ok(
            &router,
            "UpdateInvoiceUnit",
            &json!({
                "InvoiceUnitArn": target,
                "Description": description,
                "ClientToken": shared_token,
            }),
        )
        .await;
        assert_eq!(
            updated.get("InvoiceUnitArn").and_then(Value::as_str),
            Some(target.as_str()),
            "case update {target} under a shared token: the response must name that unit, got {updated}"
        );
    }
    for (target, description) in [(&first, "written to the first"), (&second, "written to the second")] {
        let got = ok(
            &router,
            "GetInvoiceUnit",
            &json!({"InvoiceUnitArn": target}),
        )
        .await;
        assert_eq!(
            got.get("Description").and_then(Value::as_str),
            Some(description),
            "case {target} after a shared-token update: the write must not have been discarded, got {got}"
        );
    }

    // The same holds for delete: a token already used on another unit must not make this
    // delete a no-op that leaves the unit in state.
    ok(
        &router,
        "DeleteInvoiceUnit",
        &json!({"InvoiceUnitArn": first, "ClientToken": shared_token}),
    )
    .await;
    let deleted_second = ok(
        &router,
        "DeleteInvoiceUnit",
        &json!({"InvoiceUnitArn": second, "ClientToken": shared_token}),
    )
    .await;
    assert_eq!(
        deleted_second.get("InvoiceUnitArn").and_then(Value::as_str),
        Some(second.as_str()),
        "case delete under a shared token: the response must name the unit deleted, got {deleted_second}"
    );
    let listed = listed_names(&ok(&router, "ListInvoiceUnits", &json!({})).await);
    assert!(
        listed.is_empty(),
        "case delete under a shared token: both units must be gone, got {listed:?}"
    );
}

/// `CreateInvoiceUnit` refuses a create once the account holds the maximum number of
/// invoice units, with the model's own `maxInvoiceUnitsExceeded` reason, rather than
/// growing the registry without limit.
#[tokio::test]
async fn creating_past_the_maximum_is_refused() {
    let router = emulator();
    // The maximum the service enforces, read from the constant that enforces it.
    let maximum = service_com_amazonaws_invoicing_invoicing::state::MAX_INVOICE_UNITS;

    for index in 0..maximum {
        let created = ok(
            &router,
            "CreateInvoiceUnit",
            &json!({
                "Name": format!("unit-{index}"),
                "InvoiceReceiver": "123456789012",
                "TaxInheritanceDisabled": false,
                "Rule": {"LinkedAccounts": ["210987654321"]},
                "ClientToken": format!("bulk-{index}"),
            }),
        )
        .await;
        assert!(
            created.get("InvoiceUnitArn").is_some(),
            "case unit-{index}: a create below the maximum must succeed, got {created}"
        );
    }

    let (status, body) = call(
        &router,
        "CreateInvoiceUnit",
        &json!({
            "Name": "one-too-many",
            "InvoiceReceiver": "123456789012",
            "TaxInheritanceDisabled": false,
            "Rule": {"LinkedAccounts": ["210987654321"]},
            "ClientToken": "one-too-many",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case one past the maximum of {maximum}: expected HTTP 400, got {status} {body}"
    );
    assert!(
        error_code(&body).contains("ValidationException"),
        "case one past the maximum of {maximum}: expected a ValidationException, got {body}"
    );

    // The refused create stored nothing.
    let page = ok(
        &router,
        "ListInvoiceUnits",
        &json!({"Filters": {"Names": ["one-too-many"]}}),
    )
    .await;
    assert_eq!(
        page["InvoiceUnits"].as_array().map(Vec::len),
        Some(0),
        "case one past the maximum of {maximum}: the refused create must not be in state, got {page}"
    );
}

/// The configuration history one unit keeps for `AsOf` is bounded, so a caller that
/// updates the same unit over and over cannot grow it without limit — and every read
/// stays consistent across that eviction: the current configuration is the last one
/// written, an `AsOf` inside the retained window reads a stored configuration, and an
/// `AsOf` before the unit existed is still a 404.
#[tokio::test]
async fn repeated_updates_keep_reads_consistent_within_the_bounded_history() {
    let router = emulator();
    let before_creation = epoch_seconds_now();
    tokio::time::sleep(Duration::from_millis(5)).await;

    let arn = arn_of(&ok(&router, "CreateInvoiceUnit", &CREATE_CASES[0].request("tok-1")).await);
    let just_created = epoch_seconds_now();

    // More updates than any bounded history can retain in full.
    let updates = 40;
    for revision in 0..updates {
        ok(
            &router,
            "UpdateInvoiceUnit",
            &json!({
                "InvoiceUnitArn": arn,
                "Description": format!("revision {revision}"),
                "ClientToken": format!("upd-{revision}"),
            }),
        )
        .await;
    }

    let current = ok(&router, "GetInvoiceUnit", &json!({"InvoiceUnitArn": arn})).await;
    assert_eq!(
        current.get("Description").and_then(Value::as_str),
        Some(format!("revision {}", updates - 1).as_str()),
        "case after {updates} updates: the current configuration is the last one written, got {current}"
    );

    // An instant the unit existed at still reads a configuration it actually held.
    let retained = ok(
        &router,
        "GetInvoiceUnit",
        &json!({"InvoiceUnitArn": arn, "AsOf": just_created}),
    )
    .await;
    let description = retained
        .get("Description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let was_held = description == CREATE_CASES[0].description.unwrap_or_default()
        || (0..updates).any(|revision| description == format!("revision {revision}"));
    assert!(
        was_held,
        "case AsOf just after creation: must read a configuration the unit held, got {description:?}"
    );

    // And the bound did not resurrect the unit before it existed.
    let (status, body) = call(
        &router,
        "GetInvoiceUnit",
        &json!({"InvoiceUnitArn": arn, "AsOf": before_creation}),
    )
    .await;
    assert_eq!(
        status, 404,
        "case AsOf before creation after {updates} updates: expected HTTP 404, got {status} {body}"
    );

    let listed = listed_names(&ok(&router, "ListInvoiceUnits", &json!({})).await);
    assert_eq!(
        listed,
        vec![CREATE_CASES[0].name.to_string()],
        "case after {updates} updates: still exactly one unit, got {listed:?}"
    );
}
