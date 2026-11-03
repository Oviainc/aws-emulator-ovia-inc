// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a general purpose bucket's WEBSITE configuration:
//! `PutBucketWebsite` stores it and `GetBucketWebsite` reports it back.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! the bucket-lifecycle suite in this crate already owns), so what is pinned is the wire
//! behaviour — status, body, error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * `PutBucketWebsite` "sets the configuration of the website that is specified in the
//!   website subresource" (the operation's own documentation): the `WebsiteConfiguration`
//!   payload becomes the bucket's website configuration IN FULL — a replacement, not a
//!   merge — and the call answers the modelled 200 with an empty body.
//! * `GetBucketWebsite` reports exactly that configuration back, every member and every
//!   routing rule unchanged and in the order it was given, and a bucket with no website
//!   configuration answers `NoSuchWebsiteConfiguration` with HTTP 404 rather than an empty
//!   document (every output member is optional, so an empty 200 would report a
//!   configuration that does not exist).
//! * A configuration the API's own documentation rules out — `RedirectAllRequestsTo`
//!   beside any other property, a configuration that neither redirects everything nor
//!   names an index document, an index suffix that is empty or carries a slash, a
//!   `Condition` naming nothing, both key replacements at once, a redirect
//!   code that is not an HTTP redirect, or more than the 50 routing rules per website
//!   configuration AWS documents — is refused with HTTP 400 and leaves the stored
//!   configuration untouched.
//! * A website call naming a bucket this emulator does not hold answers the service's
//!   `NoSuchBucket` (HTTP 404), the shape both handlers widen in.
//!
//! What a row asserts is the ROUND TRIP, not one hand-written document: the request
//! payload and the reported document are compared leaf by leaf ([`content`]), so a row
//! says "this configuration comes back exactly as it went in" without restating it.

use std::collections::BTreeMap;

use quick_xml::events::Event as XmlEvent;
use quick_xml::Reader;
use service_com_amazonaws_s3_amazons3::state::{
    HTTP_REDIRECT_CODES, MAX_WEBSITE_ROUTING_RULES, PROTOCOL_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The whole `WebsiteConfiguration` document for the members `inner` spells.
pub(crate) fn payload(inner: &str) -> String {
    format!("<WebsiteConfiguration xmlns=\"{XMLNS}\">{inner}</WebsiteConfiguration>")
}

/// `PUT /<bucket>?website` — `PutBucketWebsite`.
pub(crate) async fn put_website(router: &MultiServiceRouter, bucket: &str, inner: &str) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?website"),
        &[],
        &payload(inner),
    )
    .await
}

/// `GET /<bucket>?website` — `GetBucketWebsite`.
pub(crate) async fn get_website(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(router, "GET", &format!("/{bucket}?website"), &[], "").await
}

/// An `IndexDocument` container naming `suffix`.
pub(crate) fn index_document(suffix: &str) -> String {
    format!("<IndexDocument><Suffix>{suffix}</Suffix></IndexDocument>")
}

/// An `ErrorDocument` container naming `key`.
pub(crate) fn error_document(key: &str) -> String {
    format!("<ErrorDocument><Key>{key}</Key></ErrorDocument>")
}

/// A `RoutingRule` carrying `condition` (omitted when empty) and `redirect`.
///
/// `pub(crate)`, with [`routing_rules`]: the published-refusal suite beside this one builds
/// the same documents through these helpers rather than spelling the XML a second way.
pub(crate) fn routing_rule(condition: &str, redirect: &str) -> String {
    format!("<RoutingRule>{condition}{redirect}</RoutingRule>")
}

/// The `RoutingRules` container holding `rules`.
///
/// `RoutingRules` is a non-flattened list whose member carries `xmlName` `RoutingRule`
/// (`smithy_query(query="shape",shape="RoutingRules")`), so each rule is a `<RoutingRule>`
/// child of the container.
pub(crate) fn routing_rules(rules: &[String]) -> String {
    format!("<RoutingRules>{}</RoutingRules>", rules.concat())
}

/// Every leaf element of an XML document, as `(path, text)` in document order.
///
/// A path names each ancestor with its position among the siblings of that name
/// (`WebsiteConfiguration[1]/RoutingRules[1]/RoutingRule[2]/Redirect[1]/HostName[1]`), so a
/// comparison of two documents' leaves is sensitive to the ORDER of repeated siblings — the
/// routing rules — while saying nothing about the order the members of one container are
/// serialized in. Whitespace-only text is skipped, the way an XML reader treats the
/// insignificant whitespace a pretty-printer may add; an element with no content at all is
/// a leaf carrying the empty string.
fn leaves(document: &str) -> Vec<(String, String)> {
    let mut reader = Reader::from_str(document);
    // One counter map per open level: how many times each element name has been seen among
    // the children of that level.
    let mut counts: Vec<BTreeMap<String, usize>> = vec![BTreeMap::new()];
    let mut path: Vec<String> = Vec::new();
    let mut found: Vec<(String, String)> = Vec::new();

    // The next position for `name` among the children of the open level.
    fn position(counts: &mut [BTreeMap<String, usize>], name: &str) -> usize {
        let level = counts
            .last_mut()
            .expect("a level is open for as long as the document is being read");
        let seen = level.entry(name.to_string()).or_insert(0);
        *seen += 1;
        *seen
    }

    loop {
        let event = reader
            .read_event()
            .unwrap_or_else(|error| panic!("the document must be well-formed XML: {error}"));
        match event {
            XmlEvent::Start(start) => {
                let name = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                let position = position(&mut counts, &name);
                path.push(format!("{name}[{position}]"));
                counts.push(BTreeMap::new());
            }
            XmlEvent::Empty(empty) => {
                let name = String::from_utf8_lossy(empty.name().as_ref()).into_owned();
                let position = position(&mut counts, &name);
                path.push(format!("{name}[{position}]"));
                found.push((path.join("/"), String::new()));
                path.pop();
            }
            XmlEvent::Text(text) => {
                let decoded = text
                    .xml10_content()
                    .unwrap_or_else(|error| panic!("the document must decode: {error}"))
                    .into_owned();
                if !decoded.trim().is_empty() {
                    found.push((path.join("/"), decoded));
                }
            }
            XmlEvent::End(_) => {
                path.pop();
                counts.pop();
            }
            XmlEvent::Eof => break,
            _ => {}
        }
    }
    found
}

/// The CONTENT of an XML document: its leaves sorted by path. See [`leaves`].
///
/// `pub(crate)`, with [`reported`] beside it: the cors suite in this same test crate
/// compares a stored subresource with the document that put it the same way, rather than
/// standing up a second XML reader.
pub(crate) fn content(document: &str) -> Vec<(String, String)> {
    let mut leaves = leaves(document);
    leaves.sort_by(|left, right| left.0.cmp(&right.0));
    leaves
}

/// The content of what a request answered, as a document.
pub(crate) fn reported(answer: &Answer) -> Vec<(String, String)> {
    content(&String::from_utf8_lossy(&answer.body))
}

/// The configuration that redirects every request to another host, used as the REPLACEMENT
/// in the round-trip rows: it shares no member with any configuration that serves the
/// bucket as a website, so a merge rather than a replacement is visible.
const REDIRECT_EVERYTHING: &str =
    "<RedirectAllRequestsTo><HostName>replacement.example.com</HostName></RedirectAllRequestsTo>";

/// The table of website configurations every row of which must round-trip unchanged.
///
/// The rows differ along what the store-and-report rule generalises over: WHICH of the
/// container's four members the configuration names, which VALUES they carry (every
/// `Protocol` the enum publishes and each published bound of an HTTP redirect code, not the
/// one value the issue's recorded call sent), how MANY routing rules it carries (none, one,
/// several and the documented ceiling), the ORDER those rules arrive in, and the order the
/// members themselves are spelled in.
pub(crate) fn round_trip_rows() -> Vec<(String, String)> {
    let index = index_document("index.html");
    let prefix_condition = "<Condition><KeyPrefixEquals>docs/</KeyPrefixEquals></Condition>";
    let error_condition =
        "<Condition><HttpErrorCodeReturnedEquals>404</HttpErrorCodeReturnedEquals></Condition>";
    let both_conditions = "<Condition><HttpErrorCodeReturnedEquals>403</HttpErrorCodeReturnedEquals>\
         <KeyPrefixEquals>private/</KeyPrefixEquals></Condition>";

    let mut rows: Vec<(String, String)> = vec![
        ("an index document alone".to_string(), index.clone()),
        (
            "an index document whose suffix is not index.html".to_string(),
            index_document("home.htm"),
        ),
        (
            "an index document whose suffix is not ASCII".to_string(),
            index_document("índice.html"),
        ),
        (
            "an index and an error document".to_string(),
            format!("{index}{}", error_document("error.html")),
        ),
        (
            "an error document key under a prefix".to_string(),
            format!("{index}{}", error_document("errors/404.html")),
        ),
        (
            "the two documents spelled in the reverse of the model's member order"
                .to_string(),
            format!("{}{index}", error_document("error.html")),
        ),
        (
            "redirect every request, naming no protocol".to_string(),
            REDIRECT_EVERYTHING.to_string(),
        ),
        (
            "a routing rule replacing a key prefix under a key-prefix condition".to_string(),
            format!(
                "{index}{}",
                routing_rules(&[routing_rule(
                    prefix_condition,
                    "<Redirect><ReplaceKeyPrefixWith>documents/</ReplaceKeyPrefixWith></Redirect>",
                )])
            ),
        ),
        (
            "a routing rule conditioned on an HTTP error code".to_string(),
            format!(
                "{index}{}",
                routing_rules(&[routing_rule(
                    error_condition,
                    "<Redirect><ReplaceKeyWith>report-error.html</ReplaceKeyWith></Redirect>",
                )])
            ),
        ),
        (
            "a routing rule whose condition names both its members".to_string(),
            format!(
                "{index}{}",
                routing_rules(&[routing_rule(
                    both_conditions,
                    "<Redirect><HostName>errors.example.com</HostName></Redirect>",
                )])
            ),
        ),
        (
            "a routing rule with no condition at all".to_string(),
            format!(
                "{index}{}",
                routing_rules(&[routing_rule(
                    "",
                    "<Redirect><HostName>everything.example.com</HostName></Redirect>",
                )])
            ),
        ),
        (
            "a routing rule whose redirect names every member it may".to_string(),
            format!(
                "{index}{}",
                routing_rules(&[routing_rule(
                    prefix_condition,
                    "<Redirect><HostName>moved.example.com</HostName>\
                     <HttpRedirectCode>301</HttpRedirectCode><Protocol>https</Protocol>\
                     <ReplaceKeyPrefixWith>new/</ReplaceKeyPrefixWith></Redirect>",
                )])
            ),
        ),
        (
            "three routing rules keep the order they arrived in".to_string(),
            format!(
                "{index}{}",
                routing_rules(&[
                    routing_rule(
                        "<Condition><KeyPrefixEquals>first/</KeyPrefixEquals></Condition>",
                        "<Redirect><ReplaceKeyWith>first.html</ReplaceKeyWith></Redirect>",
                    ),
                    routing_rule(
                        "<Condition><KeyPrefixEquals>second/</KeyPrefixEquals></Condition>",
                        "<Redirect><ReplaceKeyWith>second.html</ReplaceKeyWith></Redirect>",
                    ),
                    routing_rule(
                        "<Condition><KeyPrefixEquals>third/</KeyPrefixEquals></Condition>",
                        "<Redirect><ReplaceKeyWith>third.html</ReplaceKeyWith></Redirect>",
                    ),
                ])
            ),
        ),
        (
            format!(
                "exactly the {MAX_WEBSITE_ROUTING_RULES} routing rules per website \
                 configuration AWS documents"
            ),
            format!(
                "{index}{}",
                routing_rules(
                    &(0..MAX_WEBSITE_ROUTING_RULES)
                        .map(|rule| routing_rule(
                            &format!(
                                "<Condition><KeyPrefixEquals>page-{rule}/</KeyPrefixEquals>\
                                 </Condition>"
                            ),
                            &format!(
                                "<Redirect><ReplaceKeyWith>page-{rule}.html</ReplaceKeyWith>\
                                 </Redirect>"
                            ),
                        ))
                        .collect::<Vec<String>>()
                )
            ),
        ),
    ];

    // One row per value the `Protocol` enum publishes, on both members that carry one
    // (`RedirectAllRequestsTo$Protocol` and `Redirect$Protocol`) — the whole catalog rather
    // than the one value the issue recorded.
    for protocol in PROTOCOL_VALUES {
        rows.push((
            format!("redirect every request over {protocol}"),
            format!(
                "<RedirectAllRequestsTo><HostName>www.example.com</HostName>\
                 <Protocol>{protocol}</Protocol></RedirectAllRequestsTo>"
            ),
        ));
        rows.push((
            format!("a routing rule redirecting over {protocol}"),
            format!(
                "{index}{}",
                routing_rules(&[routing_rule(
                    prefix_condition,
                    &format!(
                        "<Redirect><HostName>moved.example.com</HostName>\
                         <Protocol>{protocol}</Protocol></Redirect>"
                    ),
                )])
            ),
        ));
    }

    // Each published bound of the HTTP redirect code range, and one value inside it: the
    // code travels as a string in the model, so the bounds are where a check on it shows.
    for code in [
        *HTTP_REDIRECT_CODES.start(),
        302,
        *HTTP_REDIRECT_CODES.end(),
    ] {
        rows.push((
            format!("a routing rule redirecting with HTTP code {code}"),
            format!(
                "{index}{}",
                routing_rules(&[routing_rule(
                    prefix_condition,
                    &format!(
                        "<Redirect><HostName>moved.example.com</HostName>\
                         <HttpRedirectCode>{code}</HttpRedirectCode></Redirect>"
                    ),
                )])
            ),
        ));
    }

    rows
}

#[tokio::test]
async fn putting_a_website_configuration_stores_it_and_reads_back_through_get() {
    let router = router();

    for (index, (case, configuration)) in round_trip_rows().into_iter().enumerate() {
        let bucket = format!("website-round-trip-{index}");
        let created = create_bucket(&router, &bucket, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket must be created first, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );

        let put = put_website(&router, &bucket, &configuration).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected the modelled 200 from PutBucketWebsite, got {} with \
             body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        assert!(
            put.body.is_empty(),
            "case {case:?}: PutBucketWebsite's output is Unit, so the body must be empty, \
             got {:?}",
            String::from_utf8_lossy(&put.body)
        );

        let read = get_website(&router, &bucket).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetBucketWebsite on the bucket just \
             configured, got {} with body {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            reported(&read),
            content(&payload(&configuration)),
            "case {case:?}: expected the stored configuration back unchanged, got body {:?}",
            String::from_utf8_lossy(&read.body)
        );

        // Reading twice changes nothing: the read reports, it does not configure.
        let again = get_website(&router, &bucket).await;
        assert_eq!(
            reported(&again),
            reported(&read),
            "case {case:?}: a second read must report exactly what the first did, got body \
             {:?}",
            String::from_utf8_lossy(&again.body)
        );

        // The second put REPLACES: what comes back is the new container alone, with
        // nothing merged in from the configuration above — and the replacement shares no
        // member with any website-serving configuration, so a merge would show.
        let replaced = put_website(&router, &bucket, REDIRECT_EVERYTHING).await;
        assert_eq!(
            replaced.status, 200,
            "case {case:?}: the replacing put must succeed, got {} with body {:?}",
            replaced.status,
            String::from_utf8_lossy(&replaced.body)
        );
        let reread = get_website(&router, &bucket).await;
        assert_eq!(
            reported(&reread),
            content(&payload(REDIRECT_EVERYTHING)),
            "case {case:?}: the website configuration must be replaced, not merged, got \
             body {:?}",
            String::from_utf8_lossy(&reread.body)
        );
    }
}

#[tokio::test]
async fn reading_a_bucket_with_no_website_configuration_is_not_found() {
    let router = router();

    // Two buckets that have never been configured, one of them in a non-default Region, so
    // the answer is about the missing subresource rather than about one bucket.
    for (case, bucket, constraint) in [
        (
            "a bucket created with no location constraint",
            "website-absent-default",
            None,
        ),
        (
            "a bucket created in another Region",
            "website-absent-regional",
            Some("eu-west-1"),
        ),
    ] {
        let created = create_bucket(&router, bucket, constraint, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket must be created first, got {}",
            created.status
        );

        let read = get_website(&router, bucket).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a bucket with no website configuration answers the documented \
             not-found rather than an empty document, got {} with body {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.error_code(),
            "NoSuchWebsiteConfiguration",
            "case {case:?}: expected the documented error code, got {:?}",
            read.error_code()
        );

        // Reading twice answers the same way: the read reports, it does not configure.
        let again = get_website(&router, bucket).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: a second read must answer the same way, got {}",
            again.status
        );

        // And the bucket still takes a put afterwards, so the rows above failed on the
        // missing subresource rather than on a broken bucket.
        let put = put_website(&router, bucket, &index_document("index.html")).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: the bucket must accept a website configuration, got {} with \
             body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        let read = get_website(&router, bucket).await;
        assert_eq!(
            reported(&read),
            content(&payload(&index_document("index.html"))),
            "case {case:?}: the configured bucket must report what it was given, got body \
             {:?}",
            String::from_utf8_lossy(&read.body)
        );
    }
}

#[tokio::test]
async fn a_refused_website_configuration_changes_nothing() {
    let router = router();
    let bucket = "website-refusals";
    create_bucket(&router, bucket, None, None).await;

    // The configuration the bucket holds throughout: every refusal below must leave it
    // exactly as it is.
    let stored = format!(
        "{}{}{}",
        index_document("index.html"),
        error_document("error.html"),
        routing_rules(&[routing_rule(
            "<Condition><KeyPrefixEquals>docs/</KeyPrefixEquals></Condition>",
            "<Redirect><ReplaceKeyPrefixWith>documents/</ReplaceKeyPrefixWith></Redirect>",
        )])
    );
    let put = put_website(&router, bucket, &stored).await;
    assert_eq!(
        put.status, 200,
        "the setup put must succeed, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );

    // (case, the refused configuration, the wire error code it must answer with).
    //
    // One row per way a configuration can fall outside what the API accepts. The code is
    // per row because S3 publishes a code per class of fault — a member whose value, or
    // whose absence, is not one it may carry is `InvalidArgument`; a configuration asking for
    // behaviour the service cannot carry out (a redirect over a scheme it does not make, two
    // replacements for one key, more rules than it holds) is `InvalidRequest`; and a document
    // that does not validate against the published schema is `MalformedXML`. Each refusal's
    // own sentence and `<ArgumentName>` pair are pinned in
    // [`super::bucket_website_refusals`]; what this table says is that the status is 400 and
    // nothing is stored, whichever class it is.
    let past_the_ceiling = routing_rules(
        &(0..=MAX_WEBSITE_ROUTING_RULES)
            .map(|rule| {
                routing_rule(
                    "",
                    &format!("<Redirect><ReplaceKeyWith>page-{rule}.html</ReplaceKeyWith></Redirect>"),
                )
            })
            .collect::<Vec<String>>(),
    );
    let rows: Vec<(String, String, &str)> = vec![
        (
            "RedirectAllRequestsTo beside an index document".to_string(),
            format!("{REDIRECT_EVERYTHING}{}", index_document("index.html")),
            "InvalidArgument",
        ),
        (
            "RedirectAllRequestsTo beside an error document".to_string(),
            format!("{REDIRECT_EVERYTHING}{}", error_document("error.html")),
            "InvalidArgument",
        ),
        (
            "RedirectAllRequestsTo beside routing rules".to_string(),
            format!(
                "{REDIRECT_EVERYTHING}{}",
                routing_rules(&[routing_rule(
                    "",
                    "<Redirect><HostName>other.example.com</HostName></Redirect>",
                )])
            ),
            "InvalidArgument",
        ),
        (
            "a configuration naming nothing at all".to_string(),
            String::new(),
            "InvalidArgument",
        ),
        (
            "an error document with no index document".to_string(),
            error_document("error.html"),
            "InvalidArgument",
        ),
        (
            "routing rules with no index document".to_string(),
            routing_rules(&[routing_rule(
                "",
                "<Redirect><HostName>other.example.com</HostName></Redirect>",
            )]),
            "InvalidArgument",
        ),
        (
            "an index suffix carrying a slash".to_string(),
            index_document("pages/index.html"),
            "InvalidArgument",
        ),
        (
            "an empty index suffix".to_string(),
            index_document(""),
            "InvalidArgument",
        ),
        (
            format!("one routing rule past the documented {MAX_WEBSITE_ROUTING_RULES}"),
            format!("{}{past_the_ceiling}", index_document("index.html")),
            "InvalidRequest",
        ),
        (
            "a Condition naming neither of its members".to_string(),
            format!(
                "{}{}",
                index_document("index.html"),
                routing_rules(&[routing_rule(
                    "<Condition></Condition>",
                    "<Redirect><HostName>other.example.com</HostName></Redirect>",
                )])
            ),
            "InvalidRequest",
        ),
        (
            "both key replacements in one Redirect".to_string(),
            format!(
                "{}{}",
                index_document("index.html"),
                routing_rules(&[routing_rule(
                    "",
                    "<Redirect><ReplaceKeyWith>one.html</ReplaceKeyWith>\
                     <ReplaceKeyPrefixWith>one/</ReplaceKeyPrefixWith></Redirect>",
                )])
            ),
            "InvalidRequest",
        ),
        (
            "a redirect code one below the published range".to_string(),
            redirect_code_configuration(&(HTTP_REDIRECT_CODES.start() - 1).to_string()),
            "InvalidRequest",
        ),
        (
            "a redirect code one above the published range".to_string(),
            redirect_code_configuration(&(HTTP_REDIRECT_CODES.end() + 1).to_string()),
            "InvalidRequest",
        ),
        (
            "a redirect code that is not a number".to_string(),
            redirect_code_configuration("found"),
            "InvalidRequest",
        ),
        (
            "a protocol outside the published set".to_string(),
            "<RedirectAllRequestsTo><HostName>www.example.com</HostName>\
             <Protocol>ftp</Protocol></RedirectAllRequestsTo>"
                .to_string(),
            "InvalidRequest",
        ),
        (
            "a published protocol differing only in case".to_string(),
            "<RedirectAllRequestsTo><HostName>www.example.com</HostName>\
             <Protocol>HTTPS</Protocol></RedirectAllRequestsTo>"
                .to_string(),
            "InvalidRequest",
        ),
        (
            "an empty error document key".to_string(),
            format!("{}{}", index_document("index.html"), error_document("")),
            "InvalidArgument",
        ),
    ];

    for (case, configuration, expected_code) in rows {
        let refused = put_website(&router, bucket, &configuration).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected the configuration to be refused with HTTP 400, got {} \
             with body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            expected_code,
            "case {case:?}: expected error code {expected_code:?}, got {:?} with body {:?}",
            refused.error_code(),
            String::from_utf8_lossy(&refused.body)
        );

        let read = get_website(&router, bucket).await;
        assert_eq!(
            reported(&read),
            content(&payload(&stored)),
            "case {case:?}: a refused put must leave the stored website configuration \
             untouched, got body {:?}",
            String::from_utf8_lossy(&read.body)
        );
    }
}

/// A website configuration whose one routing rule redirects with `code` — the shape the
/// redirect-code rows of both tables above and below share.
fn redirect_code_configuration(code: &str) -> String {
    format!(
        "{}{}",
        index_document("index.html"),
        routing_rules(&[routing_rule(
            "",
            &format!(
                "<Redirect><HostName>moved.example.com</HostName>\
                 <HttpRedirectCode>{code}</HttpRedirectCode></Redirect>"
            ),
        )])
    )
}

#[tokio::test]
async fn a_routing_rules_container_naming_no_rule_is_no_routing_rules() {
    let router = router();
    let bucket = "website-empty-routing-rules";
    create_bucket(&router, bucket, None, None).await;

    // The XML boundary parses a `RoutingRules` container with no `<RoutingRule>` child to
    // the same absent list as no container at all, so the two configurations are the SAME
    // configuration: accepted, and reported with no `RoutingRules` in the document. Pinned
    // here because it is what makes "no routing rules" one state in the store rather than
    // two indistinguishable ones.
    let put = put_website(
        &router,
        bucket,
        &format!("{}{}", index_document("index.html"), routing_rules(&[])),
    )
    .await;
    assert_eq!(
        put.status, 200,
        "a RoutingRules container naming no rule carries no rule, so the put must succeed, \
         got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );

    let read = get_website(&router, bucket).await;
    assert_eq!(
        reported(&read),
        content(&payload(&index_document("index.html"))),
        "the bucket must report the index document alone, with no RoutingRules container, \
         got body {:?}",
        String::from_utf8_lossy(&read.body)
    );
}

#[tokio::test]
async fn website_calls_on_a_bucket_this_emulator_does_not_hold_are_no_such_bucket() {
    let router = router();
    create_bucket(&router, "held-website-bucket", None, None).await;

    // Names that are NOT the one bucket created above: a different name, a prefix of it,
    // an extension of it, and one differing only in case (bucket names are
    // case-sensitive). `NoSuchBucket` carries `httpError 404` in the model.
    for bucket in [
        "absent-website-bucket",
        "held",
        "held-website-bucket-2",
        "Held-Website-Bucket",
    ] {
        for (operation, answer) in [
            (
                "PutBucketWebsite",
                put_website(&router, bucket, &index_document("index.html")).await,
            ),
            ("GetBucketWebsite", get_website(&router, bucket).await),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {bucket:?}/{operation}: expected HTTP 404 for a bucket this emulator \
                 does not hold, got {} with body {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchBucket",
                "case {bucket:?}/{operation}: expected the modelled error code, got {:?}",
                answer.error_code()
            );
        }
    }

    // And the bucket that IS held configures normally, so the rows above failed on the
    // lookup rather than on the operations being unreachable.
    let configuration = format!(
        "{}{}",
        index_document("index.html"),
        error_document("error.html")
    );
    let put = put_website(&router, "held-website-bucket", &configuration).await;
    assert_eq!(
        put.status, 200,
        "the held bucket must accept a website configuration, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );
    let read = get_website(&router, "held-website-bucket").await;
    assert_eq!(
        reported(&read),
        content(&payload(&configuration)),
        "the held bucket must report the configuration it was given, got body {:?}",
        String::from_utf8_lossy(&read.body)
    );
}
