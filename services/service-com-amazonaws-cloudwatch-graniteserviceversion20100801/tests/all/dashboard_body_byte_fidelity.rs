// SPDX-License-Identifier: CC0-1.0

//! The sharpest consequence of keeping a dashboard document OPAQUE: two renderings of ONE JSON
//! value are two documents, and each is measured by the bytes it holds.
//!
//! `tests/all/opaque_document_fidelity.rs` walks the round trip itself over the class of
//! renderings. What is asserted here instead is the RELATION between rows of that class: for
//! documents that a parser cannot tell apart, the sizes `ListDashboards` reports differ by
//! exactly the number of bytes the texts differ by — which no size computed from a re-rendering
//! of the parsed document can satisfy, because a re-render makes every row the same length.
//!
//! "The size of the dashboard, in bytes." (`DashboardEntry$Size`, a long).

use crate::harness::{build_router, ok};
use serde_json::{json, Value};

/// One text widget written with `colon` between a name and its value and `comma` between
/// members, as a caller's own serializer would render one dashboard.
fn rendered_widget(colon: &str, comma: &str) -> String {
    format!(
        "{{\"widgets\"{colon}[{{\"type\"{colon}\"text\"{comma}\"x\"{colon}0{comma}\"y\"{colon}0\
         {comma}\"width\"{colon}6{comma}\"height\"{colon}3{comma}\"properties\"{colon}\
         {{\"markdown\"{colon}\"## Original Dashboard\"}}}}]}}"
    )
}

/// The `Size` `ListDashboards` reports for the dashboard named `name`.
async fn reported_size(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
) -> Option<i64> {
    let listed = ok(
        router,
        "ListDashboards",
        &json!({"DashboardNamePrefix": name}),
    )
    .await;
    listed
        .get("DashboardEntries")?
        .as_array()?
        .iter()
        .find(|entry| entry.get("DashboardName").and_then(Value::as_str) == Some(name))?
        .get("Size")?
        .as_i64()
}

#[tokio::test]
async fn renderings_of_one_value_are_sized_by_the_bytes_they_differ_by() {
    // Rows: renderings of the same value, increasingly far from the compact one — no
    // whitespace, one space after each separator, a space on both sides of each separator, and
    // the same value indented over several lines. Under a size taken from a re-render they
    // would all report one number.
    let router = build_router();
    let compact = rendered_widget(":", ",");
    let indented = serde_json::to_string_pretty(
        &serde_json::from_str::<Value>(&compact).expect("the row is JSON"),
    )
    .expect("the document renders");
    let rows: Vec<(&str, String)> = vec![
        ("no whitespace between tokens", compact.clone()),
        ("a space after each separator", rendered_widget(": ", ", ")),
        ("a space on both sides of each separator", rendered_widget(" : ", " , ")),
        ("indented over several lines", indented),
    ];

    let mut reported: Vec<(&str, usize, i64)> = Vec::new();
    for (index, (case, document)) in rows.iter().enumerate() {
        let name = format!("rendering-{index}");
        ok(
            &router,
            "PutDashboard",
            &json!({"DashboardName": name, "DashboardBody": document}),
        )
        .await;
        let size = reported_size(&router, &name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the stored dashboard must be listed"));
        // Absolute first: "The size of the dashboard, in bytes." (`DashboardEntry$Size`) is the
        // byte length of the text submitted, so a size carrying any additional term fails here
        // rather than cancelling out of the differences asserted below.
        assert_eq!(
            size,
            i64::try_from(document.len()).expect("a test document fits in an i64"),
            "case {case:?}: expected the {} bytes submitted, got {size}",
            document.len()
        );
        reported.push((case, document.len(), size));
    }

    for window in reported.windows(2) {
        let (lean_case, lean_bytes, lean_size) = window[0];
        let (fat_case, fat_bytes, fat_size) = window[1];
        assert!(
            fat_bytes > lean_bytes,
            "case {fat_case:?}: this rendering must hold more bytes than {lean_case:?}, got \
             {fat_bytes} against {lean_bytes}"
        );
        assert_eq!(
            fat_size - lean_size,
            i64::try_from(fat_bytes - lean_bytes).expect("the gap fits in an i64"),
            "case {fat_case:?}: the reported sizes must differ by exactly the bytes the texts \
             differ by, got {fat_size} against {lean_size} for {fat_bytes} against {lean_bytes} \
             bytes"
        );
    }

    // The widest row against the narrowest, so the relation is asserted over the whole span and
    // not only between neighbours.
    let (lean_case, lean_bytes, lean_size) = reported[0];
    let (fat_case, fat_bytes, fat_size) = reported[reported.len() - 1];
    assert_eq!(
        fat_size - lean_size,
        i64::try_from(fat_bytes - lean_bytes).expect("the gap fits in an i64"),
        "case {fat_case:?} against {lean_case:?}: expected the size gap to be the byte gap, got \
         {fat_size} against {lean_size}"
    );
}
