// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DOCUMENT a metrics configuration write carries, and for the
//! continuation token its listing pages with.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite owns), so what is pinned is the wire behaviour — status, body,
//! error code and the ids the envelope names its request by — rather than any handler's
//! internals.
//!
//! The rules under test, with their sources:
//!
//! * A `MetricsFilter` is EXACTLY ONE member: "a filter must be a prefix, an object tag, an
//!   access point ARN, or a conjunction (MetricsAndOperator)"
//!   (`smithy_query(query="doc",shape="MetricsFilter")`). A document naming none of them, or
//!   more than one, is not a document of that shape — which is a document that does not
//!   validate against the schema the API publishes, so it is refused `MalformedXML` (HTTP
//!   400) with that code's fixed published sentence, and NOTHING of it is recorded.
//! * "The access point ARN used when evaluating a metrics filter"
//!   (`MetricsFilter$AccessPointArn`) and "the access point ARN used when evaluating an
//!   `AND` predicate" (`MetricsAndOperator$AccessPointArn`) take the ARN of an ACCESS POINT
//!   — `arn:<partition>:s3:<region>:<account-id>:accesspoint/<name>`, a regional,
//!   account-owned resource — so a value of any other shape, the ARN of the bucket the
//!   access point fronts above all, names no access point and is the same schema refusal.
//!   An access point in ANOTHER account or Region is still an access point and is recorded.
//! * Every refusal of this plane, whichever layer makes it, is named by the pair of ids S3
//!   names a request by: "`x-amz-request-id`: A value created by Amazon S3 that uniquely
//!   identifies the request" and its `x-amz-id-2` partner (Amazon S3 API Reference, *Common
//!   Response Headers*), mirrored in the error document's `<RequestId>` / `<HostId>`. The
//!   shape is the contract, so a refusal the request's own codec or the id argument made is
//!   named the same way a handler's is.
//! * "The continuation token is an opaque value that Amazon S3 understands"
//!   (`ListBucketMetricsConfigurationsOutput$NextContinuationToken`): the token a truncated
//!   page hands back resumes exactly where that page stopped, is stable across identical
//!   reads, is not a rendering of the id it resumes at, and a value this listing did not
//!   mint names no page of it at all (`MalformedContinuationToken`, HTTP 400) — whatever
//!   shape that value has.
//!
//! The tables span what each rule generalises over: for the filter, HOW MANY members the
//! union carries and WHICH ones; for the ARN, which section of the ARN grammar the value
//! breaks and whether it travels as the union's own member or inside the conjunction; for
//! the ids, WHICH layer refused the call; for the token, which forms a caller could hand
//! back a value this listing never issued in.

use service_com_amazonaws_s3_amazons3::ops::put_bucket_metrics_configuration::PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS;
use service_com_amazonaws_s3_amazons3::state::{
    INVALID_CONFIGURATION_ID_CODE, MALFORMED_CONTINUATION_TOKEN_CODE,
    MALFORMED_CONTINUATION_TOKEN_MESSAGE, MALFORMED_XML_CODE, MALFORMED_XML_MESSAGE,
    NO_SUCH_CONFIGURATION_CODE,
};
use super::bucket_lifecycle::{router, send, Answer};
use super::bucket_metrics_configuration::{
    bucket, configuration_payload, get_metrics, put_metrics, put_metrics_payload,
    reported_configurations, Filter, ACCESS_POINT_ARN, XMLNS,
};
use super::bucket_metrics_configuration_listing::{
    list_metrics, listed_ids, stand_up, CONFIGURATIONS_PER_PAGE,
};
use super::served_envelope_values::{correlation_headers, is_request_id};

/// The configuration body as text, for an assertion message.
fn body_of(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body).into_owned()
}

/// A `MetricsConfiguration` document carrying `filter` as its `<Filter>` element verbatim —
/// for the shapes no [`Filter`] can express, which is what a union's arity rule is about.
fn document(id: &str, filter: &str) -> String {
    format!(
        "<MetricsConfiguration xmlns=\"{XMLNS}\"><Id>{id}</Id><Filter>{filter}</Filter>\
         </MetricsConfiguration>"
    )
}

/// One `<Tag>` child, as `Tag$Key` / `Tag$Value` spell it.
const TAG: &str = "<Tag><Key>team</Key><Value>platform</Value></Tag>";

/// One `<And>` child carrying the two predicates the operator requires.
const AND: &str = "<And><Prefix>logs/</Prefix><Tag><Key>tier</Key><Value>gold</Value></Tag></And>";

#[tokio::test]
async fn a_filter_is_exactly_one_of_the_members_the_union_chooses_between() {
    let router = router();
    let name = "metrics-filter-arity";
    bucket(&router, name).await;

    // (case, the `<Filter>` element's children) — the rows differ along HOW MANY members
    // the union carries and WHICH of the four it names: none at all (empty and
    // self-closing), each pair, a triple, and all four.
    let refused: Vec<(&str, String)> = vec![
        ("no member at all", String::new()),
        (
            "a prefix and a tag",
            format!("<Prefix>both/</Prefix>{TAG}"),
        ),
        (
            "a tag and a conjunction",
            format!("{TAG}{AND}"),
        ),
        (
            "an access point ARN and a tag",
            format!("<AccessPointArn>{ACCESS_POINT_ARN}</AccessPointArn>{TAG}"),
        ),
        (
            "a prefix and an access point ARN",
            format!("<Prefix>both/</Prefix><AccessPointArn>{ACCESS_POINT_ARN}</AccessPointArn>"),
        ),
        (
            "a prefix, a tag and a conjunction",
            format!("<Prefix>three/</Prefix>{TAG}{AND}"),
        ),
        (
            "all four members at once",
            format!(
                "<Prefix>four/</Prefix>{TAG}<AccessPointArn>{ACCESS_POINT_ARN}\
                 </AccessPointArn>{AND}"
            ),
        ),
        ("the same member twice", format!("{TAG}{TAG}")),
    ];

    for (index, (case, filter)) in refused.into_iter().enumerate() {
        let id = format!("arity-{index}");
        let answer = put_metrics_payload(&router, name, &id, &document(&id, &filter), &[]).await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected HTTP 400, got {} ({})",
            answer.status,
            body_of(&answer)
        );
        assert_eq!(
            answer.error_code(),
            MALFORMED_XML_CODE,
            "case {case:?}: expected the code {MALFORMED_XML_CODE}, got {}",
            answer.error_code()
        );
        assert_eq!(
            answer.error_message(),
            MALFORMED_XML_MESSAGE,
            "case {case:?}: expected the message {MALFORMED_XML_MESSAGE:?}, got {}",
            answer.error_message()
        );
        // Refused before anything was written: the id the document named carries nothing.
        let read = get_metrics(&router, name, &id).await;
        assert_eq!(
            read.error_code(),
            NO_SUCH_CONFIGURATION_CODE,
            "case {case:?}: a refused document records nothing, so the read must answer \
             {NO_SUCH_CONFIGURATION_CODE}, got {} ({})",
            read.error_code(),
            read.status
        );
    }

    // A self-closing `<Filter/>` names no member either, and no [`Filter`] can spell it.
    let empty_element = format!(
        "<MetricsConfiguration xmlns=\"{XMLNS}\"><Id>self-closing</Id><Filter/>\
         </MetricsConfiguration>"
    );
    let answer = put_metrics_payload(&router, name, "self-closing", &empty_element, &[]).await;
    assert_eq!(
        answer.error_code(),
        MALFORMED_XML_CODE,
        "case \"a self-closing Filter element\": expected {MALFORMED_XML_CODE}, got {} ({})",
        answer.error_code(),
        body_of(&answer)
    );

    // And EXACTLY ONE member of each kind is recorded, so the rule refuses the arity rather
    // than the members.
    let accepted: Vec<(&str, Filter)> = vec![
        ("a prefix alone", Filter::Prefix("one/".to_string())),
        (
            "a tag alone",
            Filter::Tag("team".to_string(), "platform".to_string()),
        ),
        (
            "an access point ARN alone",
            Filter::AccessPointArn(ACCESS_POINT_ARN.to_string()),
        ),
        (
            "a conjunction alone",
            Filter::and(Some("logs/"), &[("tier", "gold")], None),
        ),
    ];
    for (index, (case, filter)) in accepted.into_iter().enumerate() {
        let id = format!("one-member-{index}");
        let answer = put_metrics(&router, name, &id, Some(&filter)).await;
        assert_eq!(
            answer.status,
            PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS,
            "case {case:?}: expected HTTP {PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS}, \
             got {} ({})",
            answer.status,
            body_of(&answer)
        );
        let reported = reported_configurations(&get_metrics(&router, name, &id).await);
        assert_eq!(
            reported,
            vec![(id.clone(), Some(filter.clone()))],
            "case {case:?}: expected the filter that was put, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn an_access_point_arn_filter_names_an_access_point() {
    let router = router();
    let name = "metrics-access-point-arn";
    bucket(&router, name).await;

    // (case, the ARN) — accepted rows span the ARN sections a caller may vary: the account,
    // the Region and the partition, plus an access point name carrying a dash.
    let accepted: Vec<(&str, &str)> = vec![
        ("this account's own access point", ACCESS_POINT_ARN),
        (
            "an access point in another account",
            "arn:aws:s3:us-east-1:111122223333:accesspoint/foreign-reports",
        ),
        (
            "an access point in another Region",
            "arn:aws:s3:eu-west-1:000000000000:accesspoint/eu-reports",
        ),
        (
            "an access point in another partition",
            "arn:aws-us-gov:s3:us-gov-west-1:000000000000:accesspoint/gov-reports",
        ),
    ];
    for (index, (case, arn)) in accepted.iter().enumerate() {
        // Once as the union's own member, once as the conjunction's predicate: the two
        // members take the same value, so neither may judge it its own way.
        let rows = [
            ("alone", Filter::AccessPointArn((*arn).to_string())),
            (
                "inside a conjunction",
                Filter::and(Some("reports/"), &[], Some(arn)),
            ),
        ];
        for (placement, filter) in rows {
            let id = format!("ap-{index}-{placement}").replace(' ', "-");
            let answer = put_metrics(&router, name, &id, Some(&filter)).await;
            assert_eq!(
                answer.status,
                PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS,
                "case {case:?} ({placement}): expected HTTP \
                 {PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
                answer.status,
                body_of(&answer)
            );
            let reported = reported_configurations(&get_metrics(&router, name, &id).await);
            assert_eq!(
                reported,
                vec![(id.clone(), Some(filter.clone()))],
                "case {case:?} ({placement}): expected the filter that was put, got \
                 {reported:?}"
            );
        }
    }

    // (case, the value) — each row breaks ONE part of the access point ARN grammar: the
    // `arn:` prefix, the section count, the service, the Region, the account, the resource
    // kind and the resource name. The bucket-style ARN is the row the trace this suite
    // follows carried: an access point ARN is not a bucket ARN.
    let refused: Vec<(&str, &str)> = vec![
        ("the ARN of a bucket", "arn:aws:s3:::metrics-access-point-arn"),
        ("a bare bucket name", "metrics-access-point-arn"),
        ("no arn: prefix", "aws:s3:us-east-1:000000000000:accesspoint/reports"),
        ("too few sections", "arn:aws:s3:us-east-1:accesspoint/reports"),
        (
            "another service's ARN",
            "arn:aws:sqs:us-east-1:000000000000:accesspoint/reports",
        ),
        (
            "no Region",
            "arn:aws:s3::000000000000:accesspoint/reports",
        ),
        ("no account id", "arn:aws:s3:us-east-1::accesspoint/reports"),
        (
            "the bucket's resource form under an access point's sections",
            "arn:aws:s3:us-east-1:000000000000:metrics-access-point-arn",
        ),
        (
            "the access point kind with no name",
            "arn:aws:s3:us-east-1:000000000000:accesspoint/",
        ),
    ];
    for (index, (case, arn)) in refused.iter().enumerate() {
        let rows = [
            ("alone", Filter::AccessPointArn((*arn).to_string())),
            (
                "inside a conjunction",
                Filter::and(Some("reports/"), &[], Some(arn)),
            ),
        ];
        for (placement, filter) in rows {
            let id = format!("not-ap-{index}-{placement}").replace(' ', "-");
            let answer = put_metrics(&router, name, &id, Some(&filter)).await;
            assert_eq!(
                answer.status,
                400,
                "case {case:?} ({placement}): expected HTTP 400, got {} ({})",
                answer.status,
                body_of(&answer)
            );
            assert_eq!(
                answer.error_code(),
                MALFORMED_XML_CODE,
                "case {case:?} ({placement}): expected the code {MALFORMED_XML_CODE}, got {}",
                answer.error_code()
            );
            assert_eq!(
                answer.error_message(),
                MALFORMED_XML_MESSAGE,
                "case {case:?} ({placement}): expected the message {MALFORMED_XML_MESSAGE:?}, \
                 got {}",
                answer.error_message()
            );
            let read = get_metrics(&router, name, &id).await;
            assert_eq!(
                read.error_code(),
                NO_SUCH_CONFIGURATION_CODE,
                "case {case:?} ({placement}): a refused document records nothing, so the read \
                 must answer {NO_SUCH_CONFIGURATION_CODE}, got {} ({})",
                read.error_code(),
                read.status
            );
        }
    }
}

#[tokio::test]
async fn every_refusal_of_the_metrics_plane_names_its_request_by_an_s3_request_id() {
    let router = router();
    let name = "metrics-refusal-ids";
    stand_up(
        &router,
        "refusal ids",
        name,
        &[("kept".to_string(), Some(Filter::Prefix("kept/".to_string())))],
    )
    .await;

    // (case, the answer, the code expected) — the rows differ along WHICH layer refused the
    // call: this service's own codec (the document's union), the boundary's reading of the
    // `id` argument on a write and on a delete, and the handlers themselves.
    let rows: Vec<(&str, Answer, &str)> = vec![
        (
            "a put whose document names two filter members",
            put_metrics_payload(
                &router,
                name,
                "two-members",
                &document("two-members", &format!("<Prefix>both/</Prefix>{TAG}")),
                &[],
            )
            .await,
            MALFORMED_XML_CODE,
        ),
        (
            "a put that states no configuration id at all",
            send(
                &router,
                "PUT",
                &format!("/{name}?metrics"),
                &[],
                &configuration_payload("unaddressed", None),
            )
            .await,
            INVALID_CONFIGURATION_ID_CODE,
        ),
        (
            "a delete that states no configuration id at all",
            send(&router, "DELETE", &format!("/{name}?metrics"), &[], "").await,
            INVALID_CONFIGURATION_ID_CODE,
        ),
        (
            "a put whose document names another configuration",
            put_metrics_payload(
                &router,
                name,
                "addressed",
                &configuration_payload("named-otherwise", None),
                &[],
            )
            .await,
            MALFORMED_XML_CODE,
        ),
        (
            "a delete of an id the bucket carries nothing under",
            send(
                &router,
                "DELETE",
                &format!("/{name}?metrics&id=never-put"),
                &[],
                "",
            )
            .await,
            NO_SUCH_CONFIGURATION_CODE,
        ),
        (
            "a listing continuing at a token it never issued",
            list_metrics(&router, name, Some("not-a-token"), &[]).await,
            MALFORMED_CONTINUATION_TOKEN_CODE,
        ),
    ];

    let mut seen: Vec<String> = Vec::new();
    for (case, answer, code) in &rows {
        assert_eq!(
            answer.error_code(),
            *code,
            "case {case:?}: expected the code {code}, got {} ({})",
            answer.error_code(),
            body_of(answer)
        );
        let (request_id, extended) = correlation_headers(answer, case);
        assert!(
            is_request_id(&request_id),
            "case {case:?}: expected an S3-shaped request id, got {request_id:?}"
        );
        // The error document mirrors the two headers, so the id a client reads out of the
        // body is the one an operator reads off the response.
        assert_eq!(
            answer.element("RequestId").as_deref(),
            Some(request_id.as_str()),
            "case {case:?}: the envelope's RequestId must be the header's {request_id:?}, \
             got {:?} in {:?}",
            answer.element("RequestId"),
            body_of(answer)
        );
        assert_eq!(
            answer.element("HostId").as_deref(),
            Some(extended.as_str()),
            "case {case:?}: the envelope's HostId must be the header's {extended:?}, got {:?}",
            answer.element("HostId")
        );
        // "A value created by Amazon S3 that uniquely identifies the request".
        assert!(
            !seen.contains(&request_id),
            "case {case:?}: the request id {request_id:?} already named an earlier response, \
             got {seen:?}"
        );
        seen.push(request_id);
    }
}

#[tokio::test]
async fn a_continuation_token_is_opaque_and_only_this_listing_can_mint_one() {
    let router = router();
    let name = "metrics-token-opacity";
    // One configuration past a page, so the first page is truncated and hands a token back.
    let total = CONFIGURATIONS_PER_PAGE + 1;
    let configurations: Vec<(String, Option<Filter>)> = (0..total)
        // Fixed-width ids, so ascending id order is also ascending numeric order.
        .map(|index| (format!("cfg-{index:04}"), None))
        .collect();
    stand_up(&router, "a bucket past one page", name, &configurations).await;

    let first = list_metrics(&router, name, None, &[]).await;
    let token = first.element("NextContinuationToken").unwrap_or_else(|| {
        panic!(
            "case \"the first page\": a truncated listing carries a NextContinuationToken, \
             got {:?}",
            body_of(&first)
        )
    });
    // The id the token resumes at: the first one this page did not report.
    let resumed_at = configurations[CONFIGURATIONS_PER_PAGE].0.clone();

    // Opaque: the token is no rendering of the position it names, in any spelling a caller
    // could compute for itself.
    for (spelling, computed) in [
        ("the id itself", resumed_at.clone()),
        ("hex of the id", hex::encode(&resumed_at)),
        (
            "base64 of the id",
            aws_smithy_types::base64::encode(resumed_at.as_str()),
        ),
    ] {
        assert_ne!(
            token, computed,
            "case \"the token's spelling\": a token must not be {spelling}, got {token:?}"
        );
    }

    // It resumes exactly where the page stopped.
    let second = list_metrics(&router, name, Some(&token), &[]).await;
    assert_eq!(
        listed_ids(&second),
        vec![resumed_at.clone()],
        "case \"the second page\": expected the listing to resume at {resumed_at:?}, got {:?}",
        body_of(&second)
    );

    // And two identical reads of one unchanged bucket hand back the SAME token, so a client
    // that compares the two sees one position rather than two.
    let again = list_metrics(&router, name, None, &[]).await;
    assert_eq!(
        again.element("NextContinuationToken").as_deref(),
        Some(token.as_str()),
        "case \"a second read of the same page\": expected the same token, got {:?}",
        again.element("NextContinuationToken")
    );

    // (case, the value a caller hands back) — every one of them is a value this listing
    // never issued: ids and cursors spelled the way the service's own internals would
    // render them, a token of the MINTED shape, and one a single character away from the
    // token this page really handed back. (A token minted for a SIBLING configuration
    // family's listing is refused too; that one is asserted where the codec lives, in
    // `crate::state::configuration_tokens`, which can mint one for each family without
    // standing up four buckets past a page.)
    let minted_shape = aws_smithy_types::base64::encode(vec![0_u8; 48]);
    let forged: Vec<(&str, String)> = vec![
        ("an id", resumed_at.clone()),
        ("hex of a cursor", hex::encode(format!("s3mc1 {resumed_at}"))),
        (
            "base64 of a cursor",
            aws_smithy_types::base64::encode(format!("metrics\u{1f}{resumed_at}")),
        ),
        ("a minted token's shape", minted_shape),
        (
            "a token with its last character changed",
            flip_last(&token),
        ),
        ("not an encoding at all", "not.a.token".to_string()),
    ];
    for (case, value) in forged {
        let answer = list_metrics(&router, name, Some(&value), &[]).await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected HTTP 400, got {} ({})",
            answer.status,
            body_of(&answer)
        );
        assert_eq!(
            answer.error_code(),
            MALFORMED_CONTINUATION_TOKEN_CODE,
            "case {case:?}: expected the code {MALFORMED_CONTINUATION_TOKEN_CODE}, got {}",
            answer.error_code()
        );
        assert_eq!(
            answer.error_message(),
            MALFORMED_CONTINUATION_TOKEN_MESSAGE,
            "case {case:?}: expected the message {MALFORMED_CONTINUATION_TOKEN_MESSAGE:?}, \
             got {}",
            answer.error_message()
        );
    }

    // Every refusal above left the listing exactly as it was.
    let unchanged = list_metrics(&router, name, None, &[]).await;
    assert_eq!(
        listed_ids(&unchanged).len(),
        CONFIGURATIONS_PER_PAGE,
        "case \"after every refusal\": expected the first page untouched, got {:?}",
        listed_ids(&unchanged).len()
    );
}

/// `token` with its last character replaced by another of the token alphabet — a value one
/// character away from one this listing minted, which only the authenticator inside can
/// tell apart.
fn flip_last(token: &str) -> String {
    let mut characters: Vec<char> = token.chars().collect();
    if let Some(last) = characters.last_mut() {
        *last = if *last == 'A' { 'B' } else { 'A' };
    }
    characters.into_iter().collect()
}
