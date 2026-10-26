// SPDX-License-Identifier: CC0-1.0

//! The wire form a managed endpoint's creation instant goes onto the restJson1 wire in.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness [`crate::virtual_cluster_job_run_loop::Emr`] and
//! driven with restJson1 HTTP requests, so routing, deserialization, constraint
//! validation, the handler and serialization all run exactly as they do for an AWS SDK
//! client.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. `Endpoint$createdAt` targets `Date`, the `timestamp` shape carrying
//!    `@timestampFormat("date-time")`
//!    (`smithy_query(query="shape", shape="com.amazonaws.emrcontainers#Endpoint")`), so it
//!    goes onto the wire as an RFC 3339 date-time STRING — never as an epoch number — on
//!    every path that reports an endpoint. The rows span the reader
//!    (`DescribeManagedEndpoint`, `ListManagedEndpoints`), the stored record, the page an
//!    element is on, the filter that selected it, and the endpoint state the read sees.
//! 2. One service answers ONE wire form for that modelled type: the endpoint rows and the
//!    virtual-cluster row of the same listing render their instant the same way.
//! 3. The rendering is a function of the stored record alone: every path that reports one
//!    endpoint reports a byte-identical `createdAt`, and the string re-renders to itself
//!    after a date-time parse.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::Value;
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::created_metadata_wire_form::assert_date_time_string;
use crate::managed_endpoint_listing::{
    create_endpoint, describe_endpoint, list_endpoints, stored_bodies, wait_for_the_clock,
};
use crate::virtual_cluster_job_run_loop::{field, query_instant, text, Emr};

/// One row of the table: the case it names, the response body read, and the JSON pointer
/// at the `Date` member in it.
struct Row {
    case: String,
    body: Value,
    pointer: String,
}

impl Row {
    fn new(case: impl Into<String>, body: &Value, pointer: impl Into<String>) -> Self {
        Self {
            case: case.into(),
            body: body.clone(),
            pointer: pointer.into(),
        }
    }

    /// The rendered value this row points at.
    fn rendered(&self) -> String {
        text(&self.body, &self.pointer, &self.case)
    }
}

/// RULES 1, 2 and 3: every path that reports an endpoint renders its creation instant as
/// an RFC 3339 string, the same one the rest of the service renders, and the same one on
/// every path.
#[tokio::test]
async fn an_endpoint_created_at_is_an_rfc3339_string_on_every_read_path() {
    let emr = Emr::new();
    let before = state::now_millis();
    let cluster = emr.cluster("ep-wire-vc", "ep-wire-vc-tok").await;

    // Several endpoints, differing along what the create carried beside the required
    // members, so the rule is asserted over more than one stored record.
    let mut stored: Vec<(&'static str, String)> = Vec::new();
    for (label, body) in stored_bodies() {
        let created = create_endpoint(&emr, &cluster, body).await;
        stored.push((label, text(created.ok(label), "/id", label)));
    }

    let mut rows: Vec<Row> = Vec::new();

    // The single-resource reader, while the endpoint is still in its transient state.
    for (label, id) in &stored {
        let answer = describe_endpoint(&emr, &cluster, id).await;
        let body = answer.ok(label).clone();
        rows.push(Row::new(
            format!("DescribeManagedEndpoint of {label:?} before the transition"),
            &body,
            "/endpoint/createdAt",
        ));
    }

    // The whole listing, one row per element.
    let unpaged = list_endpoints(&emr, &cluster, "")
        .await
        .ok("the whole listing")
        .clone();
    for index in 0..stored.len() {
        rows.push(Row::new(
            format!("ListManagedEndpoints element {index} of the whole listing"),
            &unpaged,
            format!("/endpoints/{index}/createdAt"),
        ));
    }

    // One endpoint per page, so an element reported as the first of a resumed page is
    // covered too, not only the first page's.
    let mut query = "?maxResults=1".to_string();
    let mut page = 0;
    loop {
        let case = format!("ListManagedEndpoints page {page} of maxResults=1");
        let body = list_endpoints(&emr, &cluster, &query).await.ok(&case).clone();
        rows.push(Row::new(case.clone(), &body, "/endpoints/0/createdAt"));
        page += 1;
        match body.get("nextToken").and_then(Value::as_str) {
            Some(token) => query = format!("?maxResults=1&nextToken={token}"),
            None => break,
        }
        assert!(
            page <= stored.len(),
            "case {case:?}: paging reported more pages than the {} stored endpoints",
            stored.len()
        );
    }

    // A filtered listing: the instant bounds select the same endpoints through a different
    // branch of the handler.
    let filtered_case = "ListManagedEndpoints filtered by createdAfter and a type";
    let filtered = list_endpoints(
        &emr,
        &cluster,
        &format!(
            "?createdAfter={}&types=JUPYTER_ENTERPRISE_GATEWAY",
            query_instant(before - 1_000)
        ),
    )
    .await
    .ok(filtered_case)
    .clone();
    rows.push(Row::new(
        filtered_case,
        &filtered,
        "/endpoints/0/createdAt",
    ));

    // RULE 2's control row: the same instant on the cluster these endpoints gateway into,
    // reported by a reader of another shape of the same service.
    let described_cluster = emr
        .send("GET", &format!("/virtualclusters/{cluster}"), None)
        .await
        .ok("the virtual cluster these endpoints belong to")
        .clone();
    rows.push(Row::new(
        "DescribeVirtualCluster of the cluster holding the endpoints",
        &described_cluster,
        "/virtualCluster/createdAt",
    ));

    // The state the clock owes a fresh endpoint has come due by now, so both readers are
    // exercised again on a settled record.
    wait_for_the_clock();
    for (label, id) in &stored {
        let answer = describe_endpoint(&emr, &cluster, id).await;
        let body = answer.ok(label).clone();
        rows.push(Row::new(
            format!("DescribeManagedEndpoint of {label:?} after the transition"),
            &body,
            "/endpoint/createdAt",
        ));
    }
    let settled = list_endpoints(&emr, &cluster, "")
        .await
        .ok("the whole listing once the endpoints have settled")
        .clone();
    for index in 0..stored.len() {
        rows.push(Row::new(
            format!("ListManagedEndpoints element {index} once the endpoints have settled"),
            &settled,
            format!("/endpoints/{index}/createdAt"),
        ));
    }

    let after = state::now_millis();

    assert!(
        rows.len() >= 2 * stored.len() + 4,
        "every endpoint read path must be covered, got {} rows for {} endpoints",
        rows.len(),
        stored.len()
    );

    for row in &rows {
        // RULE 1: the JSON type first, then the format, then the instant.
        assert_date_time_string(&row.case, &row.body, &row.pointer, before, after);
        // RULE 3: the rendering is canonical — parsing it and rendering it again is the
        // same string, so no path reports a second spelling of the same instant.
        let rendered = row.rendered();
        let parsed = DateTime::from_str(&rendered, Format::DateTime)
            .unwrap_or_else(|error| panic!("case {:?}: {rendered:?}: {error}", row.case));
        let again = parsed
            .fmt(Format::DateTime)
            .unwrap_or_else(|error| panic!("case {:?}: {rendered:?}: {error}", row.case));
        assert_eq!(
            again, rendered,
            "case {:?}: the parsed instant re-renders as {again:?}, not as the {rendered:?} \
             that was on the wire",
            row.case
        );
    }

    // RULE 3 across paths: for each stored endpoint, the describe and both listings report
    // one and the same rendered instant. A listing is newest first, so the endpoint stored
    // at `index` is the element `stored.len() - 1 - index` of it.
    for (index, (label, id)) in stored.iter().enumerate() {
        let described = describe_endpoint(&emr, &cluster, id).await;
        let from_describe = text(described.ok(label), "/endpoint/createdAt", label);
        let element_index = stored.len() - 1 - index;
        for (what, body) in [
            ("the whole listing", &unpaged),
            ("the settled listing", &settled),
        ] {
            let element = field(body, &format!("/endpoints/{element_index}"), label).clone();
            assert_eq!(
                text(&element, "/id", label),
                *id,
                "case {label:?}: {what} reports another endpoint at element \
                 {element_index}: {element}"
            );
            assert_eq!(
                text(&element, "/createdAt", label),
                from_describe,
                "case {label:?}: {what} renders createdAt differently from the describe"
            );
        }
    }
}
