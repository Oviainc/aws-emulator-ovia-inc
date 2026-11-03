// SPDX-License-Identifier: CC0-1.0

//! The refusals `PutBucketWebsite` answers a website configuration it cannot apply with:
//! which CODE, which SENTENCE, and which `<ArgumentName>` / `<ArgumentValue>` pair.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite in this crate owns), so what is pinned is the wire document a
//! client parses rather than any validator's internals.
//!
//! The rule under test: a refusal of a bucket's website configuration is the one S3
//! publishes for the fault it is — a member whose value, or whose absence, is not one that
//! member may carry is `InvalidArgument` and its document NAMES that member and what it held
//! (`<ArgumentName>` / `<ArgumentValue>`, the member's absence spelled `null`); a document
//! whose members are each acceptable but which asks for behaviour the service cannot carry
//! out is `InvalidRequest`; and a document that does not validate against the schema S3
//! publishes for it is `MalformedXML` with that code's own fixed sentence. Each sentence is
//! FIXED text for its class: the only thing interpolated into one is a value the request
//! carried, where the published sentence names it.
//!
//! Each table spans its class rather than restating the one document an issue recorded: the
//! rows differ in WHICH member is at fault, in what that member held (empty, a slash, a
//! status in each neighbouring class, a value that is no number at all), in HOW MANY rules
//! the document carries, and — for the schema rows — in HOW the document departs from the
//! schema (a misspelled container, a member at the wrong level, an undeclared element, a
//! wrong root, a document that is not well formed).

use service_com_amazonaws_s3_amazons3::state::{
    HTTP_ERROR_CODES, HTTP_REDIRECT_CODES, WEBSITE_DOCUMENT_REQUIRED_ELEMENTS,
    WEBSITE_DOCUMENT_SCHEMA,
};

use super::bucket_lifecycle::{create_bucket, router, send, Answer};
use super::bucket_website::{
    error_document, get_website, index_document, payload, put_website, routing_rules,
};

/// The sentence S3 publishes for a configuration that neither redirects every request nor
/// names an index document.
const MISSING_INDEX_DOCUMENT: &str =
    "A value for IndexDocument Suffix must be provided if RedirectAllRequestsTo is empty";

/// The sentence S3 publishes for an `IndexDocument` whose `Suffix` is not one.
const MALFORMED_INDEX_SUFFIX: &str = "The IndexDocument Suffix is not well formed";

/// The sentence S3 publishes for an `ErrorDocument` whose `Key` is not an object key.
const MALFORMED_ERROR_KEY: &str = "The ErrorDocument Key is not well formed";

/// The sentence S3 publishes for `RedirectAllRequestsTo` beside any other property of the
/// container.
const REDIRECT_ALL_NOT_EXCLUSIVE: &str =
    "RedirectAllRequestsTo cannot be provided in conjunction with other Routing Rules.";

/// The sentence S3 publishes for a `Protocol` outside the two schemes a website redirect may
/// be made over. It names the two values itself and echoes nothing from the request.
const UNSUPPORTED_PROTOCOL: &str =
    "Invalid protocol, protocol can be http or https. If not defined the protocol will be \
     selected automatically.";

/// The fixed sentence S3 sends with `MalformedXML`.
const MALFORMED_XML: &str =
    "The XML you provided was not well-formed or did not validate against our published \
     schema";

/// A configuration that redirects every request, used as the sibling the exclusivity rows
/// put another property beside.
const REDIRECT_EVERYTHING: &str =
    "<RedirectAllRequestsTo><HostName>www.example.com</HostName></RedirectAllRequestsTo>";

/// The host [`REDIRECT_EVERYTHING`] names, which the exclusivity refusal echoes as the value
/// of the member it names.
const REDIRECT_EVERYTHING_HOST: &str = "www.example.com";

/// The `RoutingRules` container holding the one rule `condition` (omitted when empty) and
/// `redirect` make, through the helpers the round-trip suite beside this one already owns.
fn routing_rule(condition: &str, redirect: &str) -> String {
    routing_rules(&[super::bucket_website::routing_rule(condition, redirect)])
}

/// A website configuration whose one routing rule redirects to another host under a
/// condition on the HTTP error code `code`.
fn error_code_configuration(code: &str) -> String {
    format!(
        "{}{}",
        index_document("index.html"),
        routing_rule(
            &format!(
                "<Condition><HttpErrorCodeReturnedEquals>{code}\
                 </HttpErrorCodeReturnedEquals></Condition>"
            ),
            "<Redirect><HostName>errors.example.com</HostName></Redirect>",
        )
    )
}

/// A website configuration whose one routing rule redirects with the HTTP code `code`.
fn redirect_code_configuration(code: &str) -> String {
    format!(
        "{}{}",
        index_document("index.html"),
        routing_rule(
            "<Condition><KeyPrefixEquals>docs/</KeyPrefixEquals></Condition>",
            &format!(
                "<Redirect><HostName>moved.example.com</HostName>\
                 <HttpRedirectCode>{code}</HttpRedirectCode></Redirect>"
            ),
        )
    )
}

/// The `<ArgumentValue>` of an error document, which is the EMPTY string for a member that
/// carried one — written either as an empty element or as a self-closing one.
fn argument_value(answer: &Answer) -> Option<String> {
    let body = String::from_utf8_lossy(&answer.body).to_string();
    match body.contains("<ArgumentValue/>") {
        true => Some(String::new()),
        false => answer.element("ArgumentValue"),
    }
}

/// One row: the case, the configuration refused, and the code, sentence and argument pair
/// S3's refusal of it carries.
type Row = (String, String, &'static str, String, Option<(&'static str, String)>);

/// The configuration that neither redirects everything nor names an index document, in every
/// shape it takes.
fn missing_index_rows() -> Vec<Row> {
    let argument = Some(("IndexDocument", "null".to_string()));
    [
        ("a configuration naming nothing at all", String::new()),
        (
            "an error document and nothing else",
            error_document("error.html"),
        ),
        (
            "one routing rule and nothing else",
            routing_rule(
                "<Condition><KeyPrefixEquals>docs/</KeyPrefixEquals></Condition>",
                "<Redirect><HostName>moved.example.com</HostName></Redirect>",
            ),
        ),
        (
            "an error document beside routing rules",
            format!(
                "{}{}",
                error_document("error.html"),
                routing_rule("", "<Redirect><ReplaceKeyWith>one.html</ReplaceKeyWith></Redirect>")
            ),
        ),
    ]
    .into_iter()
    .map(|(case, configuration)| {
        (
            case.to_string(),
            configuration,
            "InvalidArgument",
            MISSING_INDEX_DOCUMENT.to_string(),
            argument.clone(),
        )
    })
    .collect()
}

/// An `IndexDocument$Suffix` that is empty or carries a slash — "The suffix must not be
/// empty and must not include a slash character" (`IndexDocument$Suffix`'s own
/// documentation).
fn malformed_suffix_rows() -> Vec<Row> {
    ["", "/", "pages/index.html", "index.html/", "a/b/c.html"]
        .into_iter()
        .map(|suffix| {
            (
                format!("an index suffix of {suffix:?}"),
                index_document(suffix),
                "InvalidArgument",
                MALFORMED_INDEX_SUFFIX.to_string(),
                Some(("IndexDocument", suffix.to_string())),
            )
        })
        .collect()
}

/// `RedirectAllRequestsTo` beside each of the three properties that may not accompany it:
/// "If you specify this property, you can't specify any other property"
/// (`WebsiteConfiguration$RedirectAllRequestsTo`).
fn redirect_all_rows() -> Vec<Row> {
    [
        ("an index document", index_document("index.html")),
        ("an error document", error_document("error.html")),
        (
            "routing rules",
            routing_rule("", "<Redirect><HostName>other.example.com</HostName></Redirect>"),
        ),
    ]
    .into_iter()
    .map(|(case, sibling)| {
        (
            format!("redirect-all beside {case}"),
            format!("{REDIRECT_EVERYTHING}{sibling}"),
            "InvalidArgument",
            REDIRECT_ALL_NOT_EXCLUSIVE.to_string(),
            Some((
                "RedirectAllRequestsTo",
                REDIRECT_EVERYTHING_HOST.to_string(),
            )),
        )
    })
    .collect()
}

/// A status code outside the class the member it arrived on may name: each published bound of
/// the two ranges, one value past each, a code from the other member's class, and a value
/// that is no number at all.
fn status_code_rows() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for code in [
        (HTTP_ERROR_CODES.start() - 1).to_string(),
        (HTTP_ERROR_CODES.end() + 1).to_string(),
        "200".to_string(),
        "301".to_string(),
        "teapot".to_string(),
    ] {
        rows.push((
            format!("a routing rule conditioned on HTTP code {code}"),
            error_code_configuration(&code),
            "InvalidRequest",
            format!(
                "The provided HTTP error code ({code}) is not valid. Valid codes are 4XX or \
                 5XX."
            ),
            None,
        ));
    }
    for code in [
        (HTTP_REDIRECT_CODES.start() - 1).to_string(),
        (HTTP_REDIRECT_CODES.end() + 1).to_string(),
        "200".to_string(),
        "404".to_string(),
        "found".to_string(),
    ] {
        rows.push((
            format!("a routing rule redirecting with HTTP code {code}"),
            redirect_code_configuration(&code),
            "InvalidRequest",
            format!(
                "The provided HTTP redirect code ({code}) is not valid. Valid codes are 3XX \
                 except 300."
            ),
            None,
        ));
    }
    rows
}

/// The document faults S3 answers as a failure to validate against its published schema:
/// each one leaves the member the caller MEANT to spell unreadable, so a configuration
/// parsed from it would be missing that member rather than carrying it.
fn schema_fault_rows() -> Vec<Row> {
    [
        (
            "a misspelled container",
            "<IndexDocumentt><Suffix>index.html</Suffix></IndexDocumentt>".to_string(),
        ),
        (
            "a member spelled one level too high",
            "<Suffix>index.html</Suffix>".to_string(),
        ),
        (
            "an undeclared element beside a valid index document",
            format!("{}<Favicon>icon.png</Favicon>", index_document("index.html")),
        ),
        (
            "a routing rule's redirect spelled outside the rule",
            "<RoutingRules><Redirect><HostName>moved.example.com</HostName></Redirect>\
             </RoutingRules>"
                .to_string(),
        ),
        (
            "a container nested inside the wrong container",
            format!(
                "<IndexDocument><Suffix>index.html</Suffix>{}</IndexDocument>",
                error_document("error.html")
            ),
        ),
        (
            "an index document naming no suffix",
            "<IndexDocument></IndexDocument>".to_string(),
        ),
        (
            "an error document naming no key",
            format!(
                "{}<ErrorDocument></ErrorDocument>",
                index_document("index.html")
            ),
        ),
        (
            "a redirect-all naming no host",
            "<RedirectAllRequestsTo><Protocol>https</Protocol></RedirectAllRequestsTo>"
                .to_string(),
        ),
        (
            "a routing rule naming no redirect",
            format!(
                "{}<RoutingRules><RoutingRule><Condition>\
                 <KeyPrefixEquals>docs/</KeyPrefixEquals></Condition></RoutingRule>\
                 </RoutingRules>",
                index_document("index.html")
            ),
        ),
    ]
    .into_iter()
    .map(|(case, configuration)| {
        (
            case.to_string(),
            configuration,
            "MalformedXML",
            MALFORMED_XML.to_string(),
            None,
        )
    })
    .collect()
}

/// A `Protocol` outside the published set, on each of the two members that carry one — and in
/// a case the published values are not spelled in, which is as much outside the set as a
/// scheme the API does not name.
fn protocol_rows() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for protocol in ["ftp", "HTTPS", "Http", "", "httpx"] {
        rows.push((
            format!("redirect every request over {protocol:?}"),
            format!(
                "<RedirectAllRequestsTo><HostName>www.example.com</HostName>\
                 <Protocol>{protocol}</Protocol></RedirectAllRequestsTo>"
            ),
            "InvalidRequest",
            UNSUPPORTED_PROTOCOL.to_string(),
            None,
        ));
        rows.push((
            format!("a routing rule redirecting over {protocol:?}"),
            format!(
                "{}{}",
                index_document("index.html"),
                routing_rule(
                    "<Condition><KeyPrefixEquals>docs/</KeyPrefixEquals></Condition>",
                    &format!(
                        "<Redirect><HostName>moved.example.com</HostName>\
                         <Protocol>{protocol}</Protocol></Redirect>"
                    ),
                )
            ),
            "InvalidRequest",
            UNSUPPORTED_PROTOCOL.to_string(),
            None,
        ));
    }
    // Two `Protocol` elements in one document, the unpublished one FIRST: the refusal is
    // about that element, so a published scheme later in the document does not clear it.
    rows.push((
        "two routing rules, the first redirecting over a scheme the API does not name"
            .to_string(),
        format!(
            "{}{}",
            index_document("index.html"),
            routing_rules(&[
                super::bucket_website::routing_rule(
                    "<Condition><KeyPrefixEquals>first/</KeyPrefixEquals></Condition>",
                    "<Redirect><HostName>moved.example.com</HostName>\
                     <Protocol>ftp</Protocol></Redirect>",
                ),
                super::bucket_website::routing_rule(
                    "<Condition><KeyPrefixEquals>second/</KeyPrefixEquals></Condition>",
                    "<Redirect><HostName>moved.example.com</HostName>\
                     <Protocol>https</Protocol></Redirect>",
                ),
            ])
        ),
        "InvalidRequest",
        UNSUPPORTED_PROTOCOL.to_string(),
        None,
    ));
    rows
}

/// Every refused row of every class above, plus the three single-member classes that take
/// one row each.
fn refusal_rows() -> Vec<Row> {
    let mut rows = missing_index_rows();
    rows.extend(malformed_suffix_rows());
    rows.extend(redirect_all_rows());
    rows.extend(status_code_rows());
    rows.extend(protocol_rows());
    rows.extend(schema_fault_rows());
    rows.push((
        "an empty error document key".to_string(),
        format!("{}{}", index_document("index.html"), error_document("")),
        "InvalidArgument",
        MALFORMED_ERROR_KEY.to_string(),
        Some(("ErrorDocument", String::new())),
    ));
    rows.push((
        "one routing rule naming two replacements for the key".to_string(),
        format!(
            "{}{}",
            index_document("index.html"),
            routing_rule(
                "",
                "<Redirect><ReplaceKeyWith>one.html</ReplaceKeyWith>\
                 <ReplaceKeyPrefixWith>one/</ReplaceKeyPrefixWith></Redirect>",
            )
        ),
        "InvalidRequest",
        "You can only define ReplaceKeyPrefix or ReplaceKey but not both.".to_string(),
        None,
    ));
    rows.push((
        "a condition naming neither of its members".to_string(),
        format!(
            "{}{}",
            index_document("index.html"),
            routing_rule(
                "<Condition></Condition>",
                "<Redirect><HostName>moved.example.com</HostName></Redirect>",
            )
        ),
        "InvalidRequest",
        "Condition cannot be empty. To redirect all requests without a condition, the \
         condition element shouldn't be present."
            .to_string(),
        None,
    ));
    rows
}

#[tokio::test]
async fn a_refused_website_configuration_answers_the_refusal_s3_publishes_for_it() {
    let router = router();
    let bucket = "website-published-refusals";
    create_bucket(&router, bucket, None, None).await;

    // The configuration the bucket holds throughout: every refusal below must leave it
    // exactly as it is, so a row that is refused cannot be mistaken for one that was
    // applied.
    let held = format!(
        "{}{}",
        index_document("home.html"),
        error_document("oops.html")
    );
    let setup = put_website(&router, bucket, &held).await;
    assert_eq!(
        setup.status, 200,
        "the setup put must succeed, got {} with body {:?}",
        setup.status,
        String::from_utf8_lossy(&setup.body)
    );

    for (case, configuration, code, message, argument) in refusal_rows() {
        let refused = put_website(&router, bucket, &configuration).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case:?}: expected error code {code:?}, got {:?} with body {:?}",
            refused.error_code(),
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_message(),
            message,
            "case {case:?}: expected the published sentence {message:?}, got {:?}",
            refused.error_message()
        );
        match &argument {
            Some((name, value)) => {
                assert_eq!(
                    refused.element("ArgumentName").as_deref(),
                    Some(*name),
                    "case {case:?}: expected ArgumentName {name:?}, got {:?} with body {:?}",
                    refused.element("ArgumentName"),
                    String::from_utf8_lossy(&refused.body)
                );
                assert_eq!(
                    argument_value(&refused).as_deref(),
                    Some(value.as_str()),
                    "case {case:?}: expected ArgumentValue {value:?}, got {:?} with body {:?}",
                    argument_value(&refused),
                    String::from_utf8_lossy(&refused.body)
                );
            }
            None => assert_eq!(
                refused.element("ArgumentName"),
                None,
                "case {case:?}: a refusal that is not about one member names no argument, got \
                 body {:?}",
                String::from_utf8_lossy(&refused.body)
            ),
        }

        let read = get_website(&router, bucket).await;
        assert_eq!(
            super::bucket_website::reported(&read),
            super::bucket_website::content(&payload(&held)),
            "case {case:?}: a refused put must leave the stored configuration untouched, got \
             body {:?}",
            String::from_utf8_lossy(&read.body)
        );
    }
}

#[tokio::test]
async fn a_document_that_is_not_well_formed_is_refused_as_such() {
    let router = router();
    let bucket = "website-not-well-formed";
    create_bucket(&router, bucket, None, None).await;

    // Not a document at all: an unclosed element, a mismatched end tag, and text where the
    // root element should be. The code's own sentence covers both halves of this class
    // ("not well-formed or did not validate against our published schema").
    for (case, body) in [
        (
            "an unclosed element",
            "<WebsiteConfiguration><IndexDocument><Suffix>index.html</Suffix>".to_string(),
        ),
        (
            "a mismatched end tag",
            "<WebsiteConfiguration><IndexDocument><Suffix>index.html</Suffix>\
             </IndexDocument></WebsiteConfig>"
                .to_string(),
        ),
        ("no element at all", "not xml".to_string()),
    ] {
        let refused = send(&router, "PUT", &format!("/{bucket}?website"), &[], &body).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            "MalformedXML",
            "case {case:?}: expected MalformedXML, got {:?} with body {:?}",
            refused.error_code(),
            String::from_utf8_lossy(&refused.body)
        );

        let read = get_website(&router, bucket).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: nothing may be stored by a refused put, so the bucket still has \
             no website configuration, got {} with body {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
    }
}

#[tokio::test]
async fn a_configuration_the_api_accepts_is_stored_and_reported_back() {
    let router = router();

    // The other side of the rules above: a value inside the published class, and a container
    // the API leaves every member of optional, is stored and reported back unchanged.
    //
    // (case, the configuration sent, the configuration the read must report — the same
    // document except where two spellings of one element carry the same content.)
    let mut rows: Vec<(String, String, String)> = Vec::new();
    for code in [
        HTTP_ERROR_CODES.start().to_string(),
        "404".to_string(),
        "503".to_string(),
        HTTP_ERROR_CODES.end().to_string(),
    ] {
        let configuration = error_code_configuration(&code);
        rows.push((
            format!("a condition on HTTP error code {code}"),
            configuration.clone(),
            configuration,
        ));
    }
    for code in [
        HTTP_REDIRECT_CODES.start().to_string(),
        "302".to_string(),
        "307".to_string(),
        HTTP_REDIRECT_CODES.end().to_string(),
    ] {
        let configuration = redirect_code_configuration(&code);
        rows.push((
            format!("a redirect with HTTP code {code}"),
            configuration.clone(),
            configuration,
        ));
    }
    // Every member of `Redirect` is "not required if one of the siblings is present"
    // (each member's own documentation), so a rule whose redirect names NOTHING is a
    // configuration the API accepts — in either spelling of an element with no content, both
    // of which report back as the open-and-close form.
    let empty_redirect = format!(
        "{}{}",
        index_document("index.html"),
        routing_rule("", "<Redirect></Redirect>")
    );
    for (case, redirect) in [
        ("an empty redirect", "<Redirect></Redirect>"),
        ("a self-closing redirect", "<Redirect/>"),
    ] {
        rows.push((
            format!("a routing rule carrying {case}"),
            format!("{}{}", index_document("index.html"), routing_rule("", redirect)),
            empty_redirect.clone(),
        ));
    }

    for (index, (case, configuration, reported)) in rows.into_iter().enumerate() {
        let bucket = format!("website-accepted-{index}");
        create_bucket(&router, &bucket, None, None).await;
        let put = put_website(&router, &bucket, &configuration).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected the modelled 200, got {} with body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        let read = get_website(&router, &bucket).await;
        assert_eq!(
            super::bucket_website::reported(&read),
            super::bucket_website::content(&payload(&reported)),
            "case {case:?}: expected the configuration back unchanged, got body {:?}",
            String::from_utf8_lossy(&read.body)
        );
    }
}

#[tokio::test]
async fn a_put_after_a_refusal_replaces_the_whole_configuration() {
    let router = router();
    let bucket = "website-replaces-whole-configuration";
    create_bucket(&router, bucket, None, None).await;

    // A configuration that redirects every request, then a refused put, then a
    // website-serving configuration that shares no member with it: what the bucket reports
    // is the LAST accepted document in full — the index document and the routing rules, with
    // no redirect-all left behind from the first.
    let redirect_all = "<RedirectAllRequestsTo><HostName>www.example.com</HostName>\
         <Protocol>https</Protocol></RedirectAllRequestsTo>"
        .to_string();
    assert_eq!(
        put_website(&router, bucket, &redirect_all).await.status,
        200,
        "the redirect-everything configuration must be accepted"
    );

    let refused = put_website(&router, bucket, &error_document("error.html")).await;
    assert_eq!(
        refused.status, 400,
        "a configuration with no index document must be refused, got {} with body {:?}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );

    let serving = format!(
        "{}{}",
        index_document("index.html"),
        routing_rule(
            "<Condition><HttpErrorCodeReturnedEquals>404</HttpErrorCodeReturnedEquals>\
             </Condition>",
            "<Redirect><ReplaceKeyWith>not-found.html</ReplaceKeyWith></Redirect>",
        )
    );
    let replaced = put_website(&router, bucket, &serving).await;
    assert_eq!(
        replaced.status, 200,
        "the replacing configuration must be accepted, got {} with body {:?}",
        replaced.status,
        String::from_utf8_lossy(&replaced.body)
    );

    let read = get_website(&router, bucket).await;
    assert_eq!(
        super::bucket_website::reported(&read),
        super::bucket_website::content(&payload(&serving)),
        "the configuration must be replaced in full, got body {:?}",
        String::from_utf8_lossy(&read.body)
    );
}

#[test]
fn the_website_document_schema_matches_the_model() {
    // The element catalogue the document check is made of, re-derived from the MODEL rather
    // than from a second hand-written list: every structure and list reachable from
    // `WebsiteConfiguration`, each with the element names its members are written under
    // (a member's `xmlName` when it carries one, else its own name) and the members the
    // model marks `required`. A model update that renames, adds or removes one of them fails
    // here instead of quietly leaving the check behind.
    let model_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-s3-amazons3.json"
    );
    let model: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(model_path).expect("the service's Smithy model must be readable"),
    )
    .expect("the Smithy model must parse as JSON");
    let shapes = model["shapes"]
        .as_object()
        .expect("the model must carry a shapes object");

    /// The element a member is written under, and whether it is a container worth walking
    /// into.
    fn element_of(member: &str, definition: &serde_json::Value) -> String {
        definition["traits"]["smithy.api#xmlName"]
            .as_str()
            .unwrap_or(member)
            .to_string()
    }

    let mut children: Vec<(String, Vec<String>)> = Vec::new();
    let mut required: Vec<(String, String)> = Vec::new();
    let mut pending = vec!["com.amazonaws.s3#WebsiteConfiguration".to_string()];
    let mut walked: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !walked.insert(id.clone()) {
            continue;
        }
        let shape = &shapes[&id];
        let name = id
            .split('#')
            .next_back()
            .expect("a shape id names a shape")
            .to_string();
        let mut declared: Vec<String> = Vec::new();
        match shape["type"].as_str() {
            Some("structure") => {
                for (member, definition) in shape["members"]
                    .as_object()
                    .expect("a structure declares members")
                {
                    let element = element_of(member, definition);
                    declared.push(element.clone());
                    if definition["traits"].get("smithy.api#required").is_some() {
                        required.push((name.clone(), element));
                    }
                    let target = definition["target"]
                        .as_str()
                        .expect("a member names its target");
                    if matches!(shapes[target]["type"].as_str(), Some("structure" | "list")) {
                        pending.push(target.to_string());
                    }
                }
            }
            Some("list") => {
                let member = &shape["member"];
                declared.push(element_of("member", member));
                let target = member["target"]
                    .as_str()
                    .expect("a list member names its target");
                if matches!(shapes[target]["type"].as_str(), Some("structure" | "list")) {
                    pending.push(target.to_string());
                }
            }
            // A leaf carries text, not elements, and is not a row of the catalogue.
            _ => continue,
        }
        declared.sort_unstable();
        children.push((name, declared));
    }
    children.sort();
    required.sort();

    let mut declared_children: Vec<(String, Vec<String>)> = WEBSITE_DOCUMENT_SCHEMA
        .iter()
        .map(|(container, elements)| {
            let mut elements: Vec<String> = elements.iter().map(|e| (*e).to_string()).collect();
            elements.sort_unstable();
            ((*container).to_string(), elements)
        })
        .collect();
    declared_children.sort();
    assert_eq!(
        declared_children, children,
        "WEBSITE_DOCUMENT_SCHEMA must declare exactly the elements the model's \
         WebsiteConfiguration closure does; declared {declared_children:?}, the model has \
         {children:?}"
    );

    let mut declared_required: Vec<(String, String)> = WEBSITE_DOCUMENT_REQUIRED_ELEMENTS
        .iter()
        .map(|(container, element)| ((*container).to_string(), (*element).to_string()))
        .collect();
    declared_required.sort();
    assert_eq!(
        declared_required, required,
        "WEBSITE_DOCUMENT_REQUIRED_ELEMENTS must name exactly the required members of that \
         closure; declared {declared_required:?}, the model marks {required:?}"
    );
}
