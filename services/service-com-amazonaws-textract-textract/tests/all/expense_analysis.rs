// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `AnalyzeExpense` over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: Textract.AnalyzeExpense`).
//!
//! Each test asserts a RULE over a TABLE of documents rather than one recorded shape:
//!
//! * one `ExpenseDocument` per page of the text really read out of the document, in page
//!   order, `ExpenseIndex` counting from 1;
//! * every labelled line outside a table is a `SummaryField` whose `LabelDetection` and
//!   `ValueDetection` are the two halves of that line and whose `Type` is the normalized
//!   field the label names, or `OTHER`;
//! * every table is a `LineItemGroup`, its rows are `LineItems` carrying the row in full
//!   plus one field per populated cell, typed by the table's header row when it has one;
//! * a monetary value's `Currency.Code` is the supported code it names, by code or by
//!   symbol, and nothing when it names none;
//! * the `Blocks` of the expense documents are exactly what `DetectDocumentText` reports
//!   for the same document, so both read paths describe the same text;
//! * a document whose text cannot be read is REFUSED with the modelled error for why.

use serde_json::{json, Value};
use service_com_amazonaws_textract_textract::state;

use crate::document_extraction::{
    assert_block_graph, blocks_of, call, error_code, harness, text_document,
};

fn docs_of(body: &Value) -> Vec<Value> {
    body.get("ExpenseDocuments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn list_of(owner: &Value, member: &str) -> Vec<Value> {
    owner
        .get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn str_at(owner: &Value, path: &[&str]) -> String {
    let mut cursor = owner;
    for step in path {
        match cursor.get(step) {
            Some(next) => cursor = next,
            None => return String::new(),
        }
    }
    cursor.as_str().unwrap_or_default().to_string()
}

/// `(Type, LabelDetection.Text, ValueDetection.Text)` as a case writes it.
type ExpectedField = (&'static str, &'static str, &'static str);

/// Per line item group, per row, the fields a case expects after the row's own field.
type ExpectedGroups = Vec<Vec<Vec<ExpectedField>>>;

/// A page-count case: name, document text, and per page the expected
/// `(ExpenseIndex, summary field count, line item group count)`.
type PageCase = (&'static str, &'static str, Vec<(u64, usize, usize)>);

/// `(Type, LabelDetection.Text, ValueDetection.Text)` of one `ExpenseField`.
fn triple(field: &Value) -> (String, String, String) {
    (
        str_at(field, &["Type", "Text"]),
        str_at(field, &["LabelDetection", "Text"]),
        str_at(field, &["ValueDetection", "Text"]),
    )
}

fn triples(fields: &[Value]) -> Vec<(String, String, String)> {
    fields.iter().map(triple).collect()
}

fn currency_code(field: &Value) -> String {
    str_at(field, &["Currency", "Code"])
}

/// Every invariant one detected label or value holds.
fn assert_detection(case: &str, what: &str, detection: Option<&Value>) {
    let detection = detection
        .unwrap_or_else(|| panic!("case {case:?}: {what} is missing from the expense field"));
    let text = detection.get("Text").and_then(Value::as_str).unwrap_or("");
    assert!(
        !text.trim().is_empty(),
        "case {case:?}: {what} must carry the text it detected, got {detection}"
    );
    let geometry = detection
        .get("Geometry")
        .unwrap_or_else(|| panic!("case {case:?}: {what} has no Geometry, got {detection}"));
    let bbox = geometry
        .get("BoundingBox")
        .unwrap_or_else(|| panic!("case {case:?}: {what} has no BoundingBox, got {geometry}"));
    for member in ["Left", "Top", "Width", "Height"] {
        let value = bbox.get(member).and_then(Value::as_f64).unwrap_or(-1.0);
        assert!(
            (0.0..=1.0).contains(&value),
            "case {case:?}: {what} BoundingBox.{member} is {value}, outside 0..=1"
        );
    }
    let polygon = list_of(geometry, "Polygon");
    assert_eq!(
        polygon.len(),
        4,
        "case {case:?}: {what} Polygon must trace its bounding box, got {} points",
        polygon.len()
    );
    let confidence = detection
        .get("Confidence")
        .and_then(Value::as_f64)
        .unwrap_or(-1.0);
    // A recognised page reports the confidence MEASURED over the characters the detection
    // was read from, so what holds for every document is `Percent`'s range and that
    // something was measured at all.
    assert!(
        (0.0..=100.0).contains(&confidence) && confidence > 0.0,
        "case {case:?}: {what} must report the confidence it was recognised with, inside \
         Percent's 0..=100, got {confidence}"
    );
}

/// Every invariant one `ExpenseField` holds, whatever the document.
fn assert_field(case: &str, what: &str, field: &Value, page_number: u64) {
    let kind = str_at(field, &["Type", "Text"]);
    assert!(
        !kind.is_empty()
            && kind
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'),
        "case {case:?}: {what} Type must be a normalized field name, got {kind:?}"
    );
    let kind_confidence = field
        .get("Type")
        .and_then(|t| t.get("Confidence"))
        .and_then(Value::as_f64)
        .unwrap_or(-1.0);
    assert!(
        (0.0..=100.0).contains(&kind_confidence) && kind_confidence > 0.0,
        "case {case:?}: {what} Type must carry the confidence its text was recognised \
         with, inside Percent's 0..=100, got {field}"
    );
    assert_eq!(
        field.get("PageNumber").and_then(Value::as_u64),
        Some(page_number),
        "case {case:?}: {what} must report the page it was detected on ({page_number}), got {:?}",
        field.get("PageNumber")
    );
    assert_detection(case, &format!("{what} ValueDetection"), field.get("ValueDetection"));
    if field.get("LabelDetection").is_some() {
        assert_detection(case, &format!("{what} LabelDetection"), field.get("LabelDetection"));
    }
    let code = currency_code(field);
    if !code.is_empty() {
        assert!(
            // The codes `ExpenseCurrency$Code` documents.
            state::EXPENSE_CURRENCY_CODES.contains(&code.as_str()),
            "case {case:?}: {what} Currency.Code {code:?} is not a supported currency code"
        );
    }
}

/// Every invariant the whole answer holds, whatever the document.
fn assert_expense_invariants(case: &str, body: &Value) {
    let pages = body["DocumentMetadata"]["Pages"].as_u64().unwrap_or_default();
    let documents = docs_of(body);
    assert_eq!(
        documents.len() as u64,
        pages,
        "case {case:?}: expected one expense document per page ({pages}), got {}",
        documents.len()
    );
    for (index, document) in documents.iter().enumerate() {
        let expected_index = index as u64 + 1;
        assert_eq!(
            document.get("ExpenseIndex").and_then(Value::as_u64),
            Some(expected_index),
            "case {case:?}: expense documents must be indexed from 1 in page order, got {:?} at position {index}",
            document.get("ExpenseIndex")
        );
        for (position, field) in list_of(document, "SummaryFields").iter().enumerate() {
            assert_field(
                case,
                &format!("summary field {position} of expense document {expected_index}"),
                field,
                expected_index,
            );
        }
        let groups = list_of(document, "LineItemGroups");
        assert!(
            !groups.is_empty(),
            "case {case:?}: an expense document must carry at least the first line item \
             group, got {document}"
        );
        for (group_position, group) in groups.iter().enumerate() {
            assert_eq!(
                group.get("LineItemGroupIndex").and_then(Value::as_u64),
                Some(group_position as u64 + 1),
                "case {case:?}: line item groups must be indexed from 1 in page order, got {:?}",
                group.get("LineItemGroupIndex")
            );
            let items = list_of(group, "LineItems");
            assert!(
                group.get("LineItems").is_some(),
                "case {case:?}: a line item group must carry its LineItems list even when the \
                 document holds no line items, got {group}"
            );
            // Only the always-present first group may be empty, and only when it is the
            // one group there is: a group that came from a table on the page must hold
            // the rows of that table.
            assert!(
                !items.is_empty() || groups.len() == 1,
                "case {case:?}: group {group_position} came from a table, so it must hold at \
                 least one line item, got {group}"
            );
            for (row, item) in items.iter().enumerate() {
                let fields = list_of(item, "LineItemExpenseFields");
                assert_eq!(
                    fields.first().map(|field| str_at(field, &["Type", "Text"])),
                    Some(state::EXPENSE_ROW_FIELD_TYPE.to_string()),
                    "case {case:?}: row {row} must report the row in full first, got {item}"
                );
                for (position, field) in fields.iter().enumerate() {
                    assert_field(
                        case,
                        &format!("line item field {position} of row {row}"),
                        field,
                        expected_index,
                    );
                }
            }
        }
    }
}

/// One document of the summary-field table.
struct SummaryCase {
    name: &'static str,
    text: &'static str,
    /// `(Type, Label, Value)` of every summary field, in page-then-reading order.
    expected: Vec<(&'static str, &'static str, &'static str)>,
}

#[tokio::test]
async fn analyze_expense_reports_a_summary_field_for_every_labelled_line() {
    let router = harness();

    let cases = vec![
        SummaryCase {
            name: "a receipt's header fields",
            text: "Vendor: Acme Supplies\nDate: 2026-10-21\nTotal: 42.50",
            expected: vec![
                ("VENDOR_NAME", "Vendor", "Acme Supplies"),
                ("INVOICE_RECEIPT_DATE", "Date", "2026-10-21"),
                ("TOTAL", "Total", "42.50"),
            ],
        },
        SummaryCase {
            name: "labels that normalize onto the same field",
            text: "Invoice Number: INV-1024\nSub Total: 40.00\nSales Tax: 2.50\nBalance Due: 42.50",
            expected: vec![
                ("INVOICE_RECEIPT_ID", "Invoice Number", "INV-1024"),
                ("SUBTOTAL", "Sub Total", "40.00"),
                ("TAX", "Sales Tax", "2.50"),
                ("AMOUNT_DUE", "Balance Due", "42.50"),
            ],
        },
        SummaryCase {
            // Catalog entries the issue never cited, including the receiver and vendor
            // contact fields.
            name: "the rest of the catalog",
            text: "Ship To: 100 Main St\nBill To: Jane Roe\nPhone: 555-0100\nWebsite: acme.example\nPayment Terms: Net 30\nPurchase Order: PO-77\nTip: 5.00\nPrevious Balance: 1.00\nTax ID: 12-3456789\nShipping: 7.00",
            expected: vec![
                ("RECEIVER_ADDRESS", "Ship To", "100 Main St"),
                ("RECEIVER_NAME", "Bill To", "Jane Roe"),
                ("VENDOR_PHONE", "Phone", "555-0100"),
                ("VENDOR_URL", "Website", "acme.example"),
                ("PAYMENT_TERMS", "Payment Terms", "Net 30"),
                ("PO_NUMBER", "Purchase Order", "PO-77"),
                ("GRATUITY", "Tip", "5.00"),
                ("PRIOR_BALANCE", "Previous Balance", "1.00"),
                ("TAX_PAYER_ID", "Tax ID", "12-3456789"),
                ("SHIPPING_HANDLING_CHARGE", "Shipping", "7.00"),
            ],
        },
        SummaryCase {
            name: "a label the service recognises nothing in",
            text: "Table Number: 7\nServer: Alex",
            expected: vec![
                ("OTHER", "Table Number", "7"),
                ("OTHER", "Server", "Alex"),
            ],
        },
        SummaryCase {
            name: "a value that itself holds a colon",
            text: "Date: 2026-10-21 12:30",
            expected: vec![("INVOICE_RECEIPT_DATE", "Date", "2026-10-21 12:30")],
        },
        SummaryCase {
            name: "lines that label nothing",
            text: "Thank you for your business\nCome again",
            expected: vec![],
        },
        SummaryCase {
            name: "a colon with nothing on one side",
            text: "Total:\n: 42.50\nVendor: Acme",
            expected: vec![("VENDOR_NAME", "Vendor", "Acme")],
        },
    ];

    for case in &cases {
        let request = json!({ "Document": text_document(case.text) });
        let (status, body) = call(&router, "AnalyzeExpense", &request).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with {body}",
            case.name
        );
        assert_expense_invariants(case.name, &body);

        let documents = docs_of(&body);
        let fields: Vec<Value> = documents
            .iter()
            .flat_map(|document| list_of(document, "SummaryFields"))
            .collect();
        let expected: Vec<(String, String, String)> = case
            .expected
            .iter()
            .map(|(kind, label, value)| {
                (kind.to_string(), label.to_string(), value.to_string())
            })
            .collect();
        assert_eq!(
            triples(&fields),
            expected,
            "case {:?}: summary fields must be the document's own labelled lines",
            case.name
        );

        // The same document answers the same way: the result comes from stored text.
        let (repeat_status, repeat) = call(&router, "AnalyzeExpense", &request).await;
        assert_eq!(
            repeat_status, 200,
            "case {:?}: the repeat call must also succeed, got {repeat_status}",
            case.name
        );
        assert_eq!(
            docs_of(&repeat),
            documents,
            "case {:?}: the same document must answer with the same expense documents",
            case.name
        );
    }
}

#[tokio::test]
async fn analyze_expense_reports_one_expense_document_per_page() {
    let router = harness();

    // (case, text, expected (ExpenseIndex, summary field count, line item group count)).
    // A page with no table still carries the first line item group, with no line items.
    // Every row is ONE page: a raster is one page, and the multi-page document formats
    // (PDF, a TIFF of several frames) are not ones this emulator decodes, so the rows
    // differ along what the page CARRIES rather than along how many pages there are.
    let cases: Vec<PageCase> = vec![
        ("a labelled line", "Total: 1.00", vec![(1, 1, 1)]),
        (
            "a table and nothing else",
            "Item Code\tQty\nWidget Mk2\t2",
            vec![(1, 0, 1)],
        ),
        (
            "labelled lines and a table",
            "Vendor: Acme\nItem Code\tQty\nWidget Mk2\t2",
            vec![(1, 1, 1)],
        ),
        ("neither", "Thank you", vec![(1, 0, 1)]),
    ];

    for (name, text, expected) in cases {
        let (status, body) = call(
            &router,
            "AnalyzeExpense",
            &json!({ "Document": text_document(text) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200, got {status} with {body}"
        );
        assert_expense_invariants(name, &body);
        let seen: Vec<(u64, usize, usize)> = docs_of(&body)
            .iter()
            .map(|document| {
                (
                    document
                        .get("ExpenseIndex")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                    list_of(document, "SummaryFields").len(),
                    list_of(document, "LineItemGroups").len(),
                )
            })
            .collect();
        assert_eq!(
            seen, expected,
            "case {name:?}: expected (ExpenseIndex, summary fields, line item groups) {expected:?}, got {seen:?}"
        );
    }
}

/// One document of the line-item table.
struct LineItemCase {
    name: &'static str,
    text: &'static str,
    /// Per group, per row, the `(Type, Label, Value)` of every field after the row's own
    /// `EXPENSE_ROW` field.
    expected: ExpectedGroups,
    /// Per group, per row, the text of the row's `EXPENSE_ROW` field.
    expected_rows: Vec<Vec<&'static str>>,
}

#[tokio::test]
async fn analyze_expense_reports_a_line_item_for_every_table_row() {
    let router = harness();

    // What makes a gap a COLUMN break rather than a word break is that it is distinctly
    // wider than the line's own word gaps, so every row below carries a cell of more
    // than one word: a row of single-word cells, drawn on a page, is a row of words.
    let cases = vec![
        LineItemCase {
            name: "a table with a header row",
            text: "Item Code\tQty\tUnit Price\tAmount\n\
                   Widget Mk2\t2\t5.00\t10.00\n\
                   Gizmo Mk1\t1\t3.50\t3.50",
            expected: vec![vec![
                vec![
                    ("PRODUCT_CODE", "Item Code", "Widget Mk2"),
                    ("QUANTITY", "Qty", "2"),
                    ("UNIT_PRICE", "Unit Price", "5.00"),
                    ("PRICE", "Amount", "10.00"),
                ],
                vec![
                    ("PRODUCT_CODE", "Item Code", "Gizmo Mk1"),
                    ("QUANTITY", "Qty", "1"),
                    ("UNIT_PRICE", "Unit Price", "3.50"),
                    ("PRICE", "Amount", "3.50"),
                ],
            ]],
            expected_rows: vec![vec!["Widget Mk2 2 5.00 10.00", "Gizmo Mk1 1 3.50 3.50"]],
        },
        LineItemCase {
            name: "header aliases the issue did not cite",
            text: "SKU\tDescription\tUnit Price\nA-1\tBolt Mk2\t0.25",
            expected: vec![vec![vec![
                ("PRODUCT_CODE", "SKU", "A-1"),
                ("ITEM", "Description", "Bolt Mk2"),
                ("UNIT_PRICE", "Unit Price", "0.25"),
            ]]],
            expected_rows: vec![vec!["A-1 Bolt Mk2 0.25"]],
        },
        LineItemCase {
            name: "a table with no header row",
            text: "Widget Mk2\t10.00\nGizmo Mk1\t3.50",
            expected: vec![vec![
                vec![("OTHER", "", "Widget Mk2"), ("OTHER", "", "10.00")],
                vec![("OTHER", "", "Gizmo Mk1"), ("OTHER", "", "3.50")],
            ]],
            expected_rows: vec![vec!["Widget Mk2 10.00", "Gizmo Mk1 3.50"]],
        },
        LineItemCase {
            name: "two tables on one page",
            text: "Item Code\tQty\nWidget Mk2\t2\nTotal: 10.00\nItem Code\tQty\nGizmo Mk1\t4",
            expected: vec![
                vec![vec![
                    ("PRODUCT_CODE", "Item Code", "Widget Mk2"),
                    ("QUANTITY", "Qty", "2"),
                ]],
                vec![vec![
                    ("PRODUCT_CODE", "Item Code", "Gizmo Mk1"),
                    ("QUANTITY", "Qty", "4"),
                ]],
            ],
            expected_rows: vec![vec!["Widget Mk2 2"], vec!["Gizmo Mk1 4"]],
        },
        LineItemCase {
            name: "a one-row table is data, not a header",
            text: "Item Code\tQty",
            expected: vec![vec![vec![
                ("OTHER", "", "Item Code"),
                ("OTHER", "", "Qty"),
            ]]],
            expected_rows: vec![vec!["Item Code Qty"]],
        },
        LineItemCase {
            name: "no table at all",
            text: "Total: 10.00",
            // The first line item group is always reported, with no line items in it.
            expected: vec![vec![]],
            expected_rows: vec![vec![]],
        },
    ];

    for case in &cases {
        let (status, body) = call(
            &router,
            "AnalyzeExpense",
            &json!({ "Document": text_document(case.text) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with {body}",
            case.name
        );
        assert_expense_invariants(case.name, &body);

        let groups: Vec<Value> = docs_of(&body)
            .iter()
            .flat_map(|document| list_of(document, "LineItemGroups"))
            .collect();
        assert_eq!(
            groups.len(),
            case.expected.len(),
            "case {:?}: expected {} line item group(s), got {groups:?}",
            case.name,
            case.expected.len()
        );
        for (group_index, group) in groups.iter().enumerate() {
            let rows = list_of(group, "LineItems");
            // The always-present first group is EMPTY only for a page that carries no
            // table at all; a group that came from a table holds that table's rows.
            // Asserted against the INPUT of every row of this table, so the rule is
            // pinned over documents with none, one and several tables rather than over
            // the one recorded shape.
            let page_has_table = case.text.contains('\t');
            assert_eq!(
                rows.is_empty(),
                !page_has_table,
                "case {:?}: group {group_index} must be empty exactly when the page \
                 carries no table (page_has_table={page_has_table}), got {rows:?}",
                case.name
            );
            assert_eq!(
                rows.len(),
                case.expected[group_index].len(),
                "case {:?}: group {group_index} expected {} row(s), got {rows:?}",
                case.name,
                case.expected[group_index].len()
            );
            for (row_index, row) in rows.iter().enumerate() {
                let fields = list_of(row, "LineItemExpenseFields");
                assert_eq!(
                    triple(&fields[0]).2,
                    case.expected_rows[group_index][row_index],
                    "case {:?}: group {group_index} row {row_index} must report the whole row",
                    case.name
                );
                let expected: Vec<(String, String, String)> = case.expected[group_index]
                    [row_index]
                    .iter()
                    .map(|(kind, label, value)| {
                        (kind.to_string(), label.to_string(), value.to_string())
                    })
                    .collect();
                assert_eq!(
                    triples(&fields[1..]),
                    expected,
                    "case {:?}: group {group_index} row {row_index} cells must be the row's own",
                    case.name
                );
            }
        }
    }
}

#[tokio::test]
async fn analyze_expense_reports_the_currency_a_value_names() {
    let router = harness();

    // Every code `ExpenseCurrency$Code` documents, named explicitly in the value.
    let mut cases: Vec<(String, String, String)> = state::EXPENSE_CURRENCY_CODES
        .iter()
        .map(|code| {
            (
                format!("the explicit code {code}"),
                format!("{code} 12.00"),
                (*code).to_string(),
            )
        })
        .collect();
    // A symbol instead of a code; a qualified dollar sign names its own currency. Only
    // the ASCII symbols are exercised over the wire: a page is read off its pixels, and
    // the recogniser holds templates for printable ASCII, so `€`, `£`, `₹` and `¥` are
    // characters no document this service reads can carry.
    for (name, value, expected) in [
        ("a bare dollar sign", "$42.50", "USD"),
        ("a Canadian dollar sign", "CA$42.50", "CAD"),
        ("a Canadian dollar sign, short", "C$42.50", "CAD"),
        ("an Australian dollar sign", "A$9.99", "AUD"),
        ("a Hong Kong dollar sign", "HK$1,000", "HKD"),
        ("a Brazilian real sign", "R$10,00", "BZR"),
        ("an explicit code beside a symbol", "$1200 CNY", "CNY"),
        ("a number with no currency at all", "42.50", ""),
        ("a currency word with no number", "ten dollars", ""),
        ("no number and no currency", "Net 30", ""),
    ] {
        cases.push((name.to_string(), value.to_string(), expected.to_string()));
    }

    for (name, value, expected) in &cases {
        let text = format!("Total: {value}");
        let (status, body) = call(
            &router,
            "AnalyzeExpense",
            &json!({ "Document": text_document(&text) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200, got {status} with {body}"
        );
        assert_expense_invariants(name, &body);
        let fields = list_of(&docs_of(&body)[0], "SummaryFields");
        assert_eq!(
            fields.len(),
            1,
            "case {name:?}: expected one summary field, got {fields:?}"
        );
        assert_eq!(
            currency_code(&fields[0]),
            *expected,
            "case {name:?}: value {value:?} must report Currency.Code {expected:?}, got {}",
            fields[0]
        );
        if !expected.is_empty() {
            let confidence = fields[0]
                .get("Currency")
                .and_then(|c| c.get("Confidence"))
                .and_then(Value::as_f64)
                .unwrap_or(-1.0);
            assert!(
                (0.0..=100.0).contains(&confidence) && confidence > 0.0,
                "case {name:?}: a reported currency must carry the confidence its value was \
                 recognised with, got {}",
                fields[0]
            );
        }
    }
}

#[tokio::test]
async fn analyze_expense_blocks_are_the_same_text_detect_document_text_reports() {
    let router = harness();

    let texts = [
        "Vendor: Acme Supplies\nTotal: $42.50",
        "Item Code\tQty\tAmount\nWidget Mk2\t2\t10.00",
        "Just some text",
    ];

    for text in texts {
        let request = json!({ "Document": text_document(text) });
        let (status, expense) = call(&router, "AnalyzeExpense", &request).await;
        assert_eq!(
            status, 200,
            "case {text:?}: expected HTTP 200, got {status} with {expense}"
        );
        let (detect_status, detected) = call(&router, "DetectDocumentText", &request).await;
        assert_eq!(
            detect_status, 200,
            "case {text:?}: DetectDocumentText must also succeed, got {detect_status}"
        );
        assert_eq!(
            expense["DocumentMetadata"], detected["DocumentMetadata"],
            "case {text:?}: both read paths must report the same DocumentMetadata"
        );

        // Page by page, the expense documents carry exactly the text-detection blocks.
        let joined: Vec<Value> = docs_of(&expense)
            .iter()
            .flat_map(|document| list_of(document, "Blocks"))
            .collect();
        assert_eq!(
            joined,
            blocks_of(&detected),
            "case {text:?}: an expense document's Blocks must be the blocks DetectDocumentText reports for that page"
        );
        assert_block_graph(text, &expense, &joined, None);
    }
}

#[tokio::test]
async fn analyze_expense_refuses_a_document_it_cannot_read() {
    let router = harness();

    // A PNG signature followed by bytes that are not a PNG body: a document in a format
    // Textract accepts whose bytes cannot be decoded, so no page can be read out of it.
    let truncated_png = [
        0x89u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x01, 0x02, 0xFF,
    ];
    let too_many_bytes = vec![b'a'; state::MAX_INLINE_DOCUMENT_BYTES + 1];

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a truncated PNG, which is a supported format this service cannot decode",
            json!({ "Bytes": aws_smithy_types::base64::encode(truncated_png) }),
            "UnsupportedDocumentException",
        ),
        (
            "bytes that hold no text at all",
            json!({ "Bytes": aws_smithy_types::base64::encode(b"  \n\t\n") }),
            "UnsupportedDocumentException",
        ),
        (
            "bytes in no format this service reads",
            json!({ "Bytes": aws_smithy_types::base64::encode([0x00u8, 0x01, 0x02, 0xFF]) }),
            "UnsupportedDocumentException",
        ),
        (
            "an S3 object, which this emulator cannot read",
            json!({ "S3Object": { "Bucket": "docs", "Name": "receipt.pdf" } }),
            "InvalidS3ObjectException",
        ),
        (
            "more bytes than a synchronous call accepts",
            json!({ "Bytes": aws_smithy_types::base64::encode(&too_many_bytes) }),
            "DocumentTooLargeException",
        ),
        ("neither Bytes nor S3Object", json!({}), "InvalidParameterException"),
        (
            "both Bytes and S3Object",
            json!({
                "Bytes": aws_smithy_types::base64::encode(b"Total: 1.00"),
                "S3Object": { "Bucket": "docs", "Name": "receipt.pdf" }
            }),
            "InvalidParameterException",
        ),
        (
            "an S3 object without a Name",
            json!({ "S3Object": { "Bucket": "docs" } }),
            "InvalidParameterException",
        ),
    ];

    for (name, document, expected) in cases {
        let (status, body) = call(&router, "AnalyzeExpense", &json!({ "Document": document })).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {name:?}: expected {expected}, got {body}"
        );
    }
}
