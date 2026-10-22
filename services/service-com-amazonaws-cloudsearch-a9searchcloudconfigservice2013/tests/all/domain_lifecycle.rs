// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CloudSearch domain lifecycle and its index configuration.
//!
//! Every case drives a real awsQuery request through the `MultiServiceRouter` — the
//! same path a socket-served request takes — and reads the XML the service wrote back
//! through the emulator's OWN XML codec (`aws_smithy_schema_server::xml`) into the
//! operation's modelled output shape, so what is pinned is the wire behaviour and the
//! assertions are typed rather than string matches. Nothing is seeded: the only way
//! state gets into the store here is through `CreateDomain` / `DefineIndexField` /
//! `IndexDocuments` / `Delete*`, which is what makes a read-back assertion evidence
//! that the response was computed from what was persisted.
//!
//! The tables span the classes the rules generalise over: every `IndexFieldType` the
//! model declares (not just the `text` field the issue cited), domain names across the
//! modelled length and pattern bounds, and both sides of each bound.

use std::sync::Arc;
use std::time::Duration;

use aws_smithy_schema_server::xml::XmlShapeDeserializer;
use aws_smithy_schema_server::DeserializableStruct;
use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudsearch_a9searchcloudconfigservice2013 as cloudsearch;

use cloudsearch::ops::types::{
    CreateDomainOutput, DeleteDomainOutput, DeleteIndexFieldOutput, DescribeDomainsOutput,
    DescribeIndexFieldsOutput, DomainStatus, IndexDocumentsOutput, IndexField, IndexFieldStatus,
    INDEX_FIELD_TYPE_VALUES,
};
use cloudsearch::state::{
    options_members_set, API_VERSION, DEFAULT_SEARCH_INSTANCE_TYPE, MAX_DOMAINS,
    MAX_INDEX_FIELDS_PER_DOMAIN, OPTIONS_MEMBER_BY_FIELD_TYPE, PROVISIONING_DELAY,
    RESERVED_FIELD_NAME,
};

/// The canonical account id every generated emulator presents, which this service's
/// `DomainId`, ARNs and endpoints are built from.
const ACCOUNT: &str = "000000000000";

/// The region the `Authorization` header below is scoped to, and so the region every
/// ARN and endpoint in these responses carries.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`cloudsearch`) — the routing signal the router reads first — and the region
/// the service mints ARNs and endpoints for. The signature itself is never checked:
/// the test router runs with request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/cloudsearch/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// Long enough that a transition due `PROVISIONING_DELAY` after a call has certainly
/// become due, without pinning the exact span.
fn past_provisioning() -> Duration {
    PROVISIONING_DELAY + Duration::from_millis(250)
}

/// What one dispatched request answered.
#[derive(Debug)]
struct Reply {
    status: u16,
    body: String,
}

impl Reply {
    /// The `<{action}Result>` payload, deserialized into the operation's modelled
    /// output shape by the emulator's own XML reader.
    ///
    /// The awsQuery success envelope is
    /// `<{Action}Response><{Action}Result>…</{Action}Result><ResponseMetadata>…`, so the
    /// result element is located by its own name and the codec does every bit of the
    /// parsing — this test adds no parser of its own.
    fn output<T: DeserializableStruct>(&self, action: &str) -> T {
        let open = format!("<{action}Result>");
        let close = format!("</{action}Result>");
        let start = self
            .body
            .find(&open)
            .unwrap_or_else(|| panic!("no {open} in the response: {}", self.body));
        let end = self
            .body
            .find(&close)
            .unwrap_or_else(|| panic!("no {close} in the response: {}", self.body))
            + close.len();
        let mut reader = XmlShapeDeserializer::new(&self.body.as_bytes()[start..end]);
        T::deserialize(&mut reader).unwrap_or_else(|error| {
            panic!("{action}Result did not deserialize ({error}): {}", self.body)
        })
    }

    /// The awsQuery error code (`<ErrorResponse><Error><Code>`), when this is a failure.
    fn error_code(&self) -> Option<String> {
        element_text(&self.body, "Code")
    }

    /// The awsQuery error message, when this is a failure.
    fn error_message(&self) -> Option<String> {
        element_text(&self.body, "Message")
    }

    /// The `<entry><key>…</key><value>…</value></entry>` pairs of the one map this
    /// response carries, sorted.
    ///
    /// The modelled output type cannot be used for a map: the emulator's XML reader
    /// deserializes a WRAPPED map nested in a struct as empty — `XmlElementDeserializer::read_map`
    /// re-wraps the entry run in a `<map>` element and `XmlShapeDeserializer::read_map`
    /// then skips that element as an unknown one — so the pairs are read from the
    /// envelope the service wrote. Every other member in this suite goes through the
    /// typed path.
    fn map_entries(&self) -> Vec<(String, String)> {
        let mut entries: Vec<(String, String)> = self
            .body
            .split("<entry>")
            .skip(1)
            .filter_map(|segment| {
                let entry = segment.split("</entry>").next()?;
                Some((element_text(entry, "key")?, element_text(entry, "value")?))
            })
            .collect();
        entries.sort();
        entries
    }
}

/// The text of the first `<tag>` element in `body`, with the five predefined XML
/// entities resolved. Used only for the error envelope, which is not a modelled shape
/// and so has no output type to deserialize into.
fn element_text(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(
        body[start..end]
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&"),
    )
}

fn build_engine() -> Arc<engine::InMemoryEngine> {
    Arc::new(engine::InMemoryEngine::new())
}

fn build_router(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(cloudsearch::EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one awsQuery call: `Action` plus the form-urlencoded parameters.
async fn call(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> Reply {
    let mut body = format!("Action={action}&Version=2013-01-01");
    for (key, value) in params {
        body.push('&');
        body.push_str(key);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cloudsearch.us-east-1.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body))
        .expect("build the request");
    let response = router.dispatch(request).await;
    Reply {
        status: response.status().as_u16(),
        body: String::from_utf8_lossy(response.body()).to_string(),
    }
}

/// Percent-encode one form value, leaving RFC 3986's unreserved set alone.
fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

async fn create_domain(router: &MultiServiceRouter, name: &str) -> Reply {
    call(router, "CreateDomain", &[("DomainName", name)]).await
}

async fn describe_domains(router: &MultiServiceRouter, names: &[&str]) -> Vec<DomainStatus> {
    let mut params = Vec::new();
    for (index, name) in names.iter().enumerate() {
        params.push((
            format!("DomainNames.member.{}", index + 1),
            (*name).to_string(),
        ));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let reply = call(router, "DescribeDomains", &borrowed).await;
    assert_eq!(reply.status, 200, "DescribeDomains: {}", reply.body);
    reply
        .output::<DescribeDomainsOutput>("DescribeDomains")
        .domain_status_list
}

/// The one domain `DescribeDomains` reports for `name`.
async fn describe_domain(router: &MultiServiceRouter, name: &str) -> DomainStatus {
    let mut found = describe_domains(router, &[name]).await;
    assert_eq!(
        found.len(),
        1,
        "a filtered DescribeDomains must report exactly one domain for {name:?}, got {found:?}"
    );
    found.remove(0)
}

async fn describe_index_fields(
    router: &MultiServiceRouter,
    domain: &str,
    field_names: &[&str],
    deployed: Option<bool>,
) -> Reply {
    let mut params: Vec<(String, String)> = vec![("DomainName".to_string(), domain.to_string())];
    for (index, name) in field_names.iter().enumerate() {
        params.push((
            format!("FieldNames.member.{}", index + 1),
            (*name).to_string(),
        ));
    }
    if let Some(deployed) = deployed {
        params.push(("Deployed".to_string(), deployed.to_string()));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    call(router, "DescribeIndexFields", &borrowed).await
}

/// The index fields `DescribeIndexFields` reports, in the order the service wrote them.
async fn index_fields(
    router: &MultiServiceRouter,
    domain: &str,
    field_names: &[&str],
    deployed: Option<bool>,
) -> Vec<IndexFieldStatus> {
    let reply = describe_index_fields(router, domain, field_names, deployed).await;
    assert_eq!(reply.status, 200, "DescribeIndexFields: {}", reply.body);
    reply
        .output::<DescribeIndexFieldsOutput>("DescribeIndexFields")
        .index_fields
}

/// Define one index field, carrying the `ReturnEnabled` option of the options
/// structure its type accepts (every one of the eleven structures declares that member).
async fn define_index_field(
    router: &MultiServiceRouter,
    domain: &str,
    name: &str,
    field_type: &str,
    options_member: &str,
    return_enabled: bool,
) -> Reply {
    let option_key = format!("IndexField.{options_member}.ReturnEnabled");
    let option_value = return_enabled.to_string();
    call(
        router,
        "DefineIndexField",
        &[
            ("DomainName", domain),
            ("IndexField.IndexFieldName", name),
            ("IndexField.IndexFieldType", field_type),
            (option_key.as_str(), option_value.as_str()),
        ],
    )
    .await
}

/// The one `IndexField` options member that `field_type` accepts, from the service's
/// model-derived table.
fn options_member(field_type: &str) -> &'static str {
    OPTIONS_MEMBER_BY_FIELD_TYPE
        .iter()
        .find(|(name, _)| *name == field_type)
        .map(|(_, member)| *member)
        .unwrap_or_else(|| panic!("no options member is tabled for field type {field_type:?}"))
}

/// The `ReturnEnabled` option of whichever options structure this field carries — the
/// one member all eleven of them declare, so one accessor covers every type.
fn return_enabled(field: &IndexField) -> Option<bool> {
    field
        .int_options
        .as_ref()
        .and_then(|options| options.return_enabled)
        .or_else(|| {
            field
                .double_options
                .as_ref()
                .and_then(|options| options.return_enabled)
        })
        .or_else(|| {
            field
                .literal_options
                .as_ref()
                .and_then(|options| options.return_enabled)
        })
        .or_else(|| {
            field
                .text_options
                .as_ref()
                .and_then(|options| options.return_enabled)
        })
        .or_else(|| {
            field
                .date_options
                .as_ref()
                .and_then(|options| options.return_enabled)
        })
        .or_else(|| {
            field
                .lat_lon_options
                .as_ref()
                .and_then(|options| options.return_enabled)
        })
        .or_else(|| {
            field
                .int_array_options
                .as_ref()
                .and_then(|options| options.return_enabled)
        })
        .or_else(|| {
            field
                .double_array_options
                .as_ref()
                .and_then(|options| options.return_enabled)
        })
        .or_else(|| {
            field
                .literal_array_options
                .as_ref()
                .and_then(|options| options.return_enabled)
        })
        .or_else(|| {
            field
                .text_array_options
                .as_ref()
                .and_then(|options| options.return_enabled)
        })
        .or_else(|| {
            field
                .date_array_options
                .as_ref()
                .and_then(|options| options.return_enabled)
        })
}

/// The names of the fields in `statuses`, in the order they were reported.
fn names_of(statuses: &[IndexFieldStatus]) -> Vec<String> {
    statuses
        .iter()
        .map(|status| status.options.index_field_name.clone())
        .collect()
}

#[tokio::test]
async fn a_created_domain_is_readable_through_describe_domains_and_list_domain_names() {
    let engine = build_engine();
    let router = build_router(&engine);

    // Domain names spanning what the modelled `DomainName` constraint allows: the
    // minimum length, a name mixing digits and hyphens, and the maximum length. Every
    // identifier asserted below is DERIVED from the name, so a row that differs in the
    // name must differ in the identifiers too.
    let names: [(&str, &str); 3] = [
        ("minimum length (3)", "abc"),
        ("digits and hyphens", "imdb-movies-2013"),
        ("maximum length (28)", "abcdefghijklmnopqrstuvwxyz01"),
    ];

    for (case, name) in names {
        let reply = create_domain(&router, name).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: CreateDomain must succeed, got {} {}",
            reply.status, reply.body
        );
        let status = reply
            .output::<CreateDomainOutput>("CreateDomain")
            .domain_status
            .unwrap_or_else(|| panic!("case {case:?}: CreateDomain must report a DomainStatus"));
        assert_eq!(
            (
                status.domain_name.as_str(),
                status.domain_id.as_str(),
                status.arn.as_deref(),
            ),
            (
                name,
                format!("{ACCOUNT}/{name}").as_str(),
                Some(format!("arn:aws:cloudsearch:{REGION}:{ACCOUNT}:domain/{name}").as_str()),
            ),
            "case {case:?}: the name, the id and the ARN are all derived from the request, \
             got {status:?}"
        );
        // Initialising, per the modelled `Created` doc: not created yet, processing,
        // and no service endpoints to report.
        assert_eq!(
            (
                status.created,
                status.deleted,
                status.processing,
                status.requires_index_documents,
            ),
            (Some(false), Some(false), Some(true), false),
            "case {case:?}: a brand-new domain is initialising, got {status:?}"
        );
        assert!(
            status.doc_service.is_none() && status.search_service.is_none(),
            "case {case:?}: an initialising domain has no endpoints yet, got {status:?}"
        );
        assert_eq!(
            (
                status.search_instance_type.as_deref(),
                status.search_partition_count,
                status.search_instance_count,
            ),
            (Some(DEFAULT_SEARCH_INSTANCE_TYPE), Some(1), Some(1)),
            "case {case:?}: a new domain starts on one instance of the default type, \
             got {status:?}"
        );
    }

    // ListDomainNames reports every live domain, mapped to its API version.
    let listed = call(&router, "ListDomainNames", &[]).await;
    assert_eq!(listed.status, 200, "ListDomainNames: {}", listed.body);
    let mut expected: Vec<(String, String)> = names
        .iter()
        .map(|(_, name)| ((*name).to_string(), API_VERSION.to_string()))
        .collect();
    expected.sort();
    assert_eq!(
        listed.map_entries(),
        expected,
        "ListDomainNames must map every created domain to its API version"
    );

    // DescribeDomains with no filter reports all three, in name order; with a filter,
    // only the match.
    let all: Vec<String> = describe_domains(&router, &[])
        .await
        .iter()
        .map(|status| status.domain_name.clone())
        .collect();
    assert_eq!(
        all,
        vec![
            "abc".to_string(),
            "abcdefghijklmnopqrstuvwxyz01".to_string(),
            "imdb-movies-2013".to_string(),
        ],
        "an unfiltered DescribeDomains reports every stored domain"
    );
    let one: Vec<String> = describe_domains(&router, &["imdb-movies-2013"])
        .await
        .iter()
        .map(|status| status.domain_name.clone())
        .collect();
    assert_eq!(
        one,
        vec!["imdb-movies-2013".to_string()],
        "a filtered DescribeDomains reports only the named domain"
    );

    // Once initialisation is due, every domain reports created, idle and endpointed —
    // and the endpoints are computed from the name and the signed region.
    tokio::time::sleep(past_provisioning()).await;
    for (case, name) in names {
        let status = describe_domain(&router, name).await;
        assert_eq!(
            (status.created, status.processing),
            (Some(true), Some(false)),
            "case {case:?}: an initialised domain is created and idle, got {status:?}"
        );
        assert_eq!(
            (
                status.doc_service.as_ref().and_then(|s| s.endpoint.as_deref()),
                status
                    .search_service
                    .as_ref()
                    .and_then(|s| s.endpoint.as_deref()),
            ),
            (
                Some(format!("doc-{name}-{ACCOUNT}.{REGION}.cloudsearch.amazonaws.com").as_str()),
                Some(
                    format!("search-{name}-{ACCOUNT}.{REGION}.cloudsearch.amazonaws.com").as_str()
                ),
            ),
            "case {case:?}: both endpoints are computed from the domain, got {status:?}"
        );
    }
}

#[tokio::test]
async fn a_domain_name_outside_the_modelled_constraint_is_refused_and_a_repeat_conflicts() {
    let engine = build_engine();
    let router = build_router(&engine);

    // The modelled `DomainName` constraint is len 3..28 with pattern
    // `^[a-z][a-z0-9\-]+$`. Each published bound is exercised on both sides, plus one
    // row per character class the pattern excludes.
    let refused: [(&str, &str); 6] = [
        ("one under the minimum length", "ab"),
        (
            "one over the maximum length",
            "abcdefghijklmnopqrstuvwxyz012",
        ),
        ("does not start with a letter", "0movies"),
        ("upper case", "Movies"),
        ("underscore", "my_movies"),
        ("trailing dot", "movies."),
    ];

    for (case, name) in refused {
        let reply = create_domain(&router, name).await;
        assert_eq!(
            (reply.status, reply.error_code().as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: name {name:?} is outside the modelled constraint, got {} {:?} {}",
            reply.status,
            reply.error_code(),
            reply.body
        );
        assert!(
            reply
                .error_message()
                .is_some_and(|message| message.to_lowercase().contains("domainname")),
            "case {case:?}: the refusal must name the member it refused, got {:?}",
            reply.error_message()
        );
    }

    // Both bounds are inclusive and accepted.
    for (case, name) in [
        ("exactly the minimum length", "abc"),
        ("exactly the maximum length", "abcdefghijklmnopqrstuvwxyz01"),
    ] {
        let reply = create_domain(&router, name).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: name {name:?} is inside the modelled constraint, got {} {}",
            reply.status, reply.body
        );
    }

    // A second create of a stored name conflicts, whatever else happened in between.
    let again = create_domain(&router, "abc").await;
    assert_eq!(
        (again.status, again.error_code().as_deref()),
        (409, Some("ResourceAlreadyExists")),
        "re-creating a stored domain must conflict, got {} {:?} {}",
        again.status,
        again.error_code(),
        again.body
    );
}

#[tokio::test]
async fn every_modelled_index_field_type_is_stored_and_read_back() {
    let engine = build_engine();
    let router = build_router(&engine);
    create_domain(&router, "fields").await;

    // The table is the model's own `IndexFieldType` value list — every type, not only
    // the `text` field the issue cited — paired with the one options structure that
    // type accepts.
    assert_eq!(
        INDEX_FIELD_TYPE_VALUES.len(),
        OPTIONS_MEMBER_BY_FIELD_TYPE.len(),
        "the options table must cover exactly the modelled IndexFieldType values"
    );

    for (index, field_type) in INDEX_FIELD_TYPE_VALUES.iter().enumerate() {
        let member = options_member(field_type);
        // A distinct, order-revealing name per row, and the `ReturnEnabled` option
        // alternating so the stored value cannot be a constant.
        let name = format!("f{index:02}_{}", field_type.replace('-', "_"));
        let wanted = index % 2 == 0;

        let reply = define_index_field(&router, "fields", &name, field_type, member, wanted).await;
        assert_eq!(
            reply.status, 200,
            "case {field_type:?}: DefineIndexField must succeed, got {} {}",
            reply.status, reply.body
        );
        let status = reply
            .output::<cloudsearch::ops::types::DefineIndexFieldOutput>("DefineIndexField")
            .index_field;
        assert_eq!(
            (
                status.options.index_field_name.as_str(),
                status.options.index_field_type.as_str(),
                return_enabled(&status.options),
            ),
            (name.as_str(), *field_type, Some(wanted)),
            "case {field_type:?}: the stored field must round-trip, got {status:?}"
        );
        // Exactly the one structure this type accepts is populated — no other type's
        // options leaked into the record.
        assert_eq!(
            options_members_set(&status.options),
            vec![member],
            "case {field_type:?}: only {member} belongs on this field, got {status:?}"
        );
        assert_eq!(
            (
                status.status.state.as_str(),
                status.status.pending_deletion,
                status.status.update_version,
            ),
            ("RequiresIndexDocuments", Some(false), 1),
            "case {field_type:?}: a newly defined field awaits a rebuild, got {status:?}"
        );
    }

    // Every definition is readable back, in name order, from the pending configuration.
    let read_back: Vec<(String, String)> = index_fields(&router, "fields", &[], None)
        .await
        .iter()
        .map(|status| {
            (
                status.options.index_field_name.clone(),
                status.options.index_field_type.clone(),
            )
        })
        .collect();
    let mut expected: Vec<(String, String)> = INDEX_FIELD_TYPE_VALUES
        .iter()
        .enumerate()
        .map(|(index, field_type)| {
            (
                format!("f{index:02}_{}", field_type.replace('-', "_")),
                (*field_type).to_string(),
            )
        })
        .collect();
    expected.sort();
    assert_eq!(
        read_back, expected,
        "DescribeIndexFields must report every persisted field, in name order"
    );

    // Defining a field is what makes the domain need a rebuild.
    let domain = describe_domain(&router, "fields").await;
    assert!(
        domain.requires_index_documents,
        "a defined field leaves the domain needing IndexDocuments, got {domain:?}"
    );

    // A `FieldNames` filter narrows the answer; a name no field carries contributes
    // nothing rather than failing.
    let first_field = format!("f00_{}", INDEX_FIELD_TYPE_VALUES[0].replace('-', "_"));
    let filtered = index_fields(
        &router,
        "fields",
        &[first_field.as_str(), "nosuchfield"],
        None,
    )
    .await;
    assert_eq!(
        names_of(&filtered),
        vec![first_field.clone()],
        "a FieldNames filter reports only the names that exist"
    );
    assert!(
        index_fields(&router, "fields", &["nosuchfield"], None)
            .await
            .is_empty(),
        "a filter matching nothing reports an empty list"
    );
}

#[tokio::test]
async fn a_definition_inconsistent_with_its_own_field_type_is_refused() {
    let engine = build_engine();
    let router = build_router(&engine);
    create_domain(&router, "fields").await;

    // Each row carries the options structure of the NEXT type in the modelled list, so
    // every type is tested against an options member it does not accept.
    for (index, field_type) in INDEX_FIELD_TYPE_VALUES.iter().enumerate() {
        let wrong =
            options_member(INDEX_FIELD_TYPE_VALUES[(index + 1) % INDEX_FIELD_TYPE_VALUES.len()]);
        let right = options_member(field_type);
        let reply = define_index_field(
            &router,
            "fields",
            &format!("bad{index:02}"),
            field_type,
            wrong,
            true,
        )
        .await;
        assert_eq!(
            (reply.status, reply.error_code().as_deref()),
            (409, Some("InvalidType")),
            "case {field_type:?} with {wrong}: an options structure the type does not \
             accept must be refused, got {} {:?} {}",
            reply.status,
            reply.error_code(),
            reply.body
        );
        let message = reply.error_message().unwrap_or_default();
        assert!(
            message.contains(right) && message.contains(wrong),
            "case {field_type:?}: the refusal must name the accepted and the offending \
             structure, got {message:?}"
        );
    }

    // Nothing was stored by any refused definition.
    assert!(
        index_fields(&router, "fields", &[], None).await.is_empty(),
        "a refused definition must store nothing"
    );

    // The reserved field name is refused whatever type it carries.
    for field_type in INDEX_FIELD_TYPE_VALUES {
        let reply = define_index_field(
            &router,
            "fields",
            RESERVED_FIELD_NAME,
            field_type,
            options_member(field_type),
            true,
        )
        .await;
        assert_eq!(
            (reply.status, reply.error_code().as_deref()),
            (400, Some("ValidationException")),
            "case {field_type:?}: the reserved name must be refused, got {} {:?} {}",
            reply.status,
            reply.error_code(),
            reply.body
        );
    }

    // An unknown domain is a ResourceNotFound, for every per-domain operation.
    let unknown: [(&str, Reply); 3] = [
        (
            "DefineIndexField",
            define_index_field(&router, "nosuchdomain", "title", "text", "TextOptions", true).await,
        ),
        (
            "DescribeIndexFields",
            describe_index_fields(&router, "nosuchdomain", &[], None).await,
        ),
        (
            "IndexDocuments",
            call(&router, "IndexDocuments", &[("DomainName", "nosuchdomain")]).await,
        ),
    ];
    for (case, reply) in unknown {
        assert_eq!(
            (reply.status, reply.error_code().as_deref()),
            (409, Some("ResourceNotFound")),
            "case {case:?}: an unknown domain must be ResourceNotFound, got {} {:?} {}",
            reply.status,
            reply.error_code(),
            reply.body
        );
    }
}

#[tokio::test]
async fn redefining_a_field_updates_it_in_place_and_bumps_the_update_version() {
    let engine = build_engine();
    let router = build_router(&engine);
    create_domain(&router, "fields").await;

    let first = define_index_field(&router, "fields", "title", "text", "TextOptions", true)
        .await
        .output::<cloudsearch::ops::types::DefineIndexFieldOutput>("DefineIndexField")
        .index_field;
    let created_at = first.status.creation_date;

    for (case, expected_version, wanted) in [
        ("second definition", 2, false),
        ("third definition", 3, true),
    ] {
        let status = define_index_field(&router, "fields", "title", "text", "TextOptions", wanted)
            .await
            .output::<cloudsearch::ops::types::DefineIndexFieldOutput>("DefineIndexField")
            .index_field;
        assert_eq!(
            (
                status.status.update_version,
                status.status.creation_date,
                return_enabled(&status.options),
            ),
            (expected_version, created_at, Some(wanted)),
            "case {case:?}: redefining bumps UpdateVersion, keeps CreationDate and takes \
             the new options, got {status:?}"
        );
    }

    let stored = index_fields(&router, "fields", &[], None).await;
    assert_eq!(
        stored.len(),
        1,
        "redefining a name must update one entry, not add another, got {:?}",
        names_of(&stored)
    );
}

#[tokio::test]
async fn index_documents_deploys_the_pending_configuration() {
    let engine = build_engine();
    let router = build_router(&engine);
    create_domain(&router, "movies").await;

    // Defined out of name order, so the reported names cannot be an echo of the
    // request order.
    for (name, field_type) in [("zip", "literal"), ("title", "text"), ("year", "int")] {
        let reply = define_index_field(
            &router,
            "movies",
            name,
            field_type,
            options_member(field_type),
            true,
        )
        .await;
        assert_eq!(reply.status, 200, "define {name}: {}", reply.body);
    }

    // Before any rebuild there is no deployed configuration at all.
    assert!(
        index_fields(&router, "movies", &[], Some(true))
            .await
            .is_empty(),
        "nothing is deployed until IndexDocuments runs"
    );

    let indexed = call(&router, "IndexDocuments", &[("DomainName", "movies")]).await;
    assert_eq!(indexed.status, 200, "{}", indexed.body);
    assert_eq!(
        indexed
            .output::<IndexDocumentsOutput>("IndexDocuments")
            .field_names
            .unwrap_or_default(),
        vec!["title".to_string(), "year".to_string(), "zip".to_string()],
        "IndexDocuments reports the indexed field names in name order, got {}",
        indexed.body
    );

    // The call satisfies the requirement at once; the rebuild itself is in flight.
    let during = describe_domain(&router, "movies").await;
    assert_eq!(
        (during.requires_index_documents, during.processing),
        (false, Some(true)),
        "a rebuild clears RequiresIndexDocuments and sets Processing, got {during:?}"
    );
    let states: Vec<String> = index_fields(&router, "movies", &[], None)
        .await
        .iter()
        .map(|status| status.status.state.clone())
        .collect();
    assert_eq!(
        states,
        vec![
            "Processing".to_string(),
            "Processing".to_string(),
            "Processing".to_string(),
        ],
        "while the rebuild runs every field is Processing"
    );

    tokio::time::sleep(past_provisioning()).await;

    // The rebuild completed: the pending copies are the deployed ones, and Active.
    for (case, deployed_filter) in [("pending view", None), ("deployed view", Some(true))] {
        let rows: Vec<(String, String)> = index_fields(&router, "movies", &[], deployed_filter)
            .await
            .iter()
            .map(|status| {
                (
                    status.options.index_field_name.clone(),
                    status.status.state.clone(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                ("title".to_string(), "Active".to_string()),
                ("year".to_string(), "Active".to_string()),
                ("zip".to_string(), "Active".to_string()),
            ],
            "case {case:?}: an activated configuration reads the same either way, got {rows:?}"
        );
    }

    let after = describe_domain(&router, "movies").await;
    assert_eq!(
        after.processing,
        Some(false),
        "once the rebuild is done the domain is idle again, got {after:?}"
    );
}

#[tokio::test]
async fn delete_index_field_pends_the_deletion_and_the_next_rebuild_retires_it() {
    let engine = build_engine();
    let router = build_router(&engine);
    create_domain(&router, "movies").await;
    for (name, field_type) in [("title", "text"), ("year", "int")] {
        define_index_field(
            &router,
            "movies",
            name,
            field_type,
            options_member(field_type),
            true,
        )
        .await;
    }
    call(&router, "IndexDocuments", &[("DomainName", "movies")]).await;
    tokio::time::sleep(past_provisioning()).await;

    let deleted = call(
        &router,
        "DeleteIndexField",
        &[("DomainName", "movies"), ("IndexFieldName", "title")],
    )
    .await;
    assert_eq!(deleted.status, 200, "{}", deleted.body);
    let status = deleted
        .output::<DeleteIndexFieldOutput>("DeleteIndexField")
        .index_field;
    assert_eq!(
        (
            status.options.index_field_name.as_str(),
            status.status.pending_deletion,
        ),
        ("title", Some(true)),
        "DeleteIndexField reports the removed field as pending deletion, got {status:?}"
    );

    // The field is out of the pending configuration but still serves the live index.
    assert_eq!(
        names_of(&index_fields(&router, "movies", &[], None).await),
        vec!["year".to_string()],
        "a field awaiting deletion is out of the pending configuration"
    );
    let deployed_rows: Vec<(String, Option<bool>)> =
        index_fields(&router, "movies", &[], Some(true))
            .await
            .iter()
            .map(|status| {
                (
                    status.options.index_field_name.clone(),
                    status.status.pending_deletion,
                )
            })
            .collect();
    assert_eq!(
        deployed_rows,
        vec![
            ("title".to_string(), Some(true)),
            ("year".to_string(), Some(false)),
        ],
        "the deployed configuration still serves the field, marked for deletion"
    );
    assert!(
        describe_domain(&router, "movies")
            .await
            .requires_index_documents,
        "deleting a deployed field leaves the domain needing a rebuild"
    );

    // A name that is not in the pending configuration is not found — whether it was
    // never defined or is already awaiting deletion.
    for (case, field) in [
        ("already pending deletion", "title"),
        ("never defined", "nosuchfield"),
    ] {
        let reply = call(
            &router,
            "DeleteIndexField",
            &[("DomainName", "movies"), ("IndexFieldName", field)],
        )
        .await;
        assert_eq!(
            (reply.status, reply.error_code().as_deref()),
            (409, Some("ResourceNotFound")),
            "case {case:?}: got {} {:?} {}",
            reply.status,
            reply.error_code(),
            reply.body
        );
    }

    // The next rebuild retires it from both configurations.
    let rebuilt = call(&router, "IndexDocuments", &[("DomainName", "movies")]).await;
    assert_eq!(
        rebuilt
            .output::<IndexDocumentsOutput>("IndexDocuments")
            .field_names
            .unwrap_or_default(),
        vec!["year".to_string()],
        "a rebuild indexes only the fields still in the pending configuration"
    );
    tokio::time::sleep(past_provisioning()).await;
    for (case, deployed_filter) in [("pending view", None), ("deployed view", Some(true))] {
        assert_eq!(
            names_of(&index_fields(&router, "movies", &[], deployed_filter).await),
            vec!["year".to_string()],
            "case {case:?}: the retired field is gone from both configurations"
        );
    }

    // A field deleted before it was ever deployed has nothing to retire: it goes now,
    // and the deletion on its own gives the domain no new work, so it leaves the
    // rebuild flag exactly as it found it.
    define_index_field(&router, "movies", "rating", "double", "DoubleOptions", true).await;
    let flag_before = describe_domain(&router, "movies")
        .await
        .requires_index_documents;
    let dropped = call(
        &router,
        "DeleteIndexField",
        &[("DomainName", "movies"), ("IndexFieldName", "rating")],
    )
    .await;
    assert_eq!(dropped.status, 200, "{}", dropped.body);
    let flag_after = describe_domain(&router, "movies")
        .await
        .requires_index_documents;
    assert_eq!(
        flag_after, flag_before,
        "deleting a never-deployed field must not change RequiresIndexDocuments: there \
         is no deployed copy for a rebuild to retire"
    );
    assert_eq!(
        names_of(&index_fields(&router, "movies", &[], None).await),
        vec!["year".to_string()],
        "a never-deployed field disappears as soon as it is deleted"
    );
}

#[tokio::test]
async fn delete_domain_retires_the_domain_and_every_later_read_sees_it() {
    let engine = build_engine();
    let router = build_router(&engine);
    create_domain(&router, "movies").await;
    create_domain(&router, "books").await;
    define_index_field(&router, "movies", "title", "text", "TextOptions", true).await;
    tokio::time::sleep(past_provisioning()).await;

    let deleted = call(&router, "DeleteDomain", &[("DomainName", "movies")]).await;
    assert_eq!(deleted.status, 200, "{}", deleted.body);
    let status = deleted
        .output::<DeleteDomainOutput>("DeleteDomain")
        .domain_status
        .expect("DeleteDomain must report the domain it deleted");
    assert_eq!(
        (status.domain_name.as_str(), status.deleted, status.processing),
        ("movies", Some(true), Some(true)),
        "DeleteDomain reports the domain deleted and cleaning up, got {status:?}"
    );

    // It is no longer a domain requests can be sent to.
    let listed = call(&router, "ListDomainNames", &[]).await;
    assert_eq!(
        listed.map_entries(),
        vec![("books".to_string(), API_VERSION.to_string())],
        "a deleted domain is not listed"
    );

    let unknown: [(&str, Reply); 4] = [
        (
            "DescribeIndexFields",
            describe_index_fields(&router, "movies", &[], None).await,
        ),
        (
            "DefineIndexField",
            define_index_field(&router, "movies", "year", "int", "IntOptions", true).await,
        ),
        (
            "IndexDocuments",
            call(&router, "IndexDocuments", &[("DomainName", "movies")]).await,
        ),
        (
            "DeleteIndexField",
            call(
                &router,
                "DeleteIndexField",
                &[("DomainName", "movies"), ("IndexFieldName", "title")],
            )
            .await,
        ),
    ];
    for (case, reply) in unknown {
        assert_eq!(
            (reply.status, reply.error_code().as_deref()),
            (409, Some("ResourceNotFound")),
            "case {case:?}: a deleted domain is not found, got {} {:?} {}",
            reply.status,
            reply.error_code(),
            reply.body
        );
    }

    // While cleanup runs the record is still reported, with Deleted=true — the
    // modelled `DomainStatus$Deleted` doc.
    let during: Vec<(String, Option<bool>)> = describe_domains(&router, &[])
        .await
        .iter()
        .map(|status| (status.domain_name.clone(), status.deleted))
        .collect();
    assert_eq!(
        during,
        vec![
            ("books".to_string(), Some(false)),
            ("movies".to_string(), Some(true)),
        ],
        "a deleting domain stays in DescribeDomains with Deleted=true"
    );

    // Deleting it again is the same answer, not an error.
    let again = call(&router, "DeleteDomain", &[("DomainName", "movies")]).await;
    assert_eq!(
        (
            again.status,
            again
                .output::<DeleteDomainOutput>("DeleteDomain")
                .domain_status
                .and_then(|status| status.deleted),
        ),
        (200, Some(true)),
        "a repeated delete reports the same state, got {} {}",
        again.status,
        again.body
    );

    // Once cleanup is due the record is gone.
    tokio::time::sleep(past_provisioning()).await;
    let after: Vec<String> = describe_domains(&router, &[])
        .await
        .iter()
        .map(|status| status.domain_name.clone())
        .collect();
    assert_eq!(
        after,
        vec!["books".to_string()],
        "once cleanup completes the record is purged"
    );

    // And the name is free again.
    let recreated = create_domain(&router, "movies").await;
    assert_eq!(
        recreated.status, 200,
        "a cleaned-up name can be created again, got {} {}",
        recreated.status, recreated.body
    );

    // This operation models no ResourceNotFoundException, so deleting a domain that
    // never existed is a success carrying no DomainStatus.
    let missing = call(&router, "DeleteDomain", &[("DomainName", "nosuchdomain")]).await;
    assert_eq!(
        missing.status, 200,
        "DeleteDomain on an unknown name is not an error, got {} {}",
        missing.status, missing.body
    );
    assert!(
        missing
            .output::<DeleteDomainOutput>("DeleteDomain")
            .domain_status
            .is_none(),
        "there is no status to report for a domain that never existed, got {}",
        missing.body
    );
}

#[tokio::test]
async fn a_write_past_a_published_store_cap_is_refused_with_the_modelled_limit_error() {
    let engine = build_engine();
    let router = build_router(&engine);

    // Fill the per-account domain store to its cap. Every create up to the cap
    // succeeds — the cap is the limit, not a smaller limit enforced early.
    for index in 0..MAX_DOMAINS {
        let name = format!("d{index:03}-cap");
        let reply = create_domain(&router, &name).await;
        assert_eq!(
            reply.status, 200,
            "domain {index} of {MAX_DOMAINS} must be created, got {} {}",
            reply.status, reply.body
        );
    }

    // One past the cap is refused, and refused with the error the model declares for
    // exactly this — nothing already stored is evicted to make room.
    let over = create_domain(&router, "one-too-many").await;
    assert_eq!(
        (over.status, over.error_code().as_deref()),
        (409, Some("LimitExceeded")),
        "a create past the domain cap must be refused, got {} {:?} {}",
        over.status,
        over.error_code(),
        over.body
    );
    let listed = call(&router, "ListDomainNames", &[]).await;
    assert_eq!(
        listed.map_entries().len(),
        MAX_DOMAINS,
        "the refused create must not have evicted or added anything"
    );

    // The same shape per domain for index fields.
    for index in 0..MAX_INDEX_FIELDS_PER_DOMAIN {
        let name = format!("f{index:03}");
        let reply =
            define_index_field(&router, "d000-cap", &name, "text", "TextOptions", true).await;
        assert_eq!(
            reply.status, 200,
            "field {index} of {MAX_INDEX_FIELDS_PER_DOMAIN} must be defined, got {} {}",
            reply.status, reply.body
        );
    }
    let over_field =
        define_index_field(&router, "d000-cap", "one_too_many", "text", "TextOptions", true).await;
    assert_eq!(
        (over_field.status, over_field.error_code().as_deref()),
        (409, Some("LimitExceeded")),
        "a new field past the per-domain cap must be refused, got {} {:?} {}",
        over_field.status,
        over_field.error_code(),
        over_field.body
    );

    // Redefining a name already stored does not grow the store, so the cap does not
    // block it.
    let redefine =
        define_index_field(&router, "d000-cap", "f000", "text", "TextOptions", false).await;
    assert_eq!(
        (
            redefine.status,
            redefine
                .output::<cloudsearch::ops::types::DefineIndexFieldOutput>("DefineIndexField")
                .index_field
                .status
                .update_version,
        ),
        (200, 2),
        "redefining a stored field is allowed at the cap, got {} {}",
        redefine.status,
        redefine.body
    );
    assert_eq!(
        index_fields(&router, "d000-cap", &[], None).await.len(),
        MAX_INDEX_FIELDS_PER_DOMAIN,
        "neither the refusal nor the redefinition changed the field count"
    );
}

#[tokio::test]
async fn the_operations_this_slice_does_not_cover_still_answer_501() {
    let engine = build_engine();
    let router = build_router(&engine);
    create_domain(&router, "movies").await;

    // A stub must stay a 501, never become an empty 200: a client cannot tell an empty
    // success from a real implementation. Each row is an operation whose only required
    // input member is `DomainName`, so the request reaches the handler rather than
    // being refused at the boundary.
    let rows: [(&str, &str); 4] = [
        ("BuildSuggesters", "BuildSuggesters"),
        ("DescribeSuggesters", "DescribeSuggesters"),
        ("DescribeAnalysisSchemes", "DescribeAnalysisSchemes"),
        (
            "DescribeServiceAccessPolicies",
            "DescribeServiceAccessPolicies",
        ),
    ];
    for (case, action) in rows {
        let reply = call(&router, action, &[("DomainName", "movies")]).await;
        assert_eq!(
            (reply.status, reply.error_code().as_deref()),
            (501, Some("NotImplemented")),
            "case {case:?}: an unimplemented operation must answer 501, got {} {:?} {}",
            reply.status,
            reply.error_code(),
            reply.body
        );
    }
}
