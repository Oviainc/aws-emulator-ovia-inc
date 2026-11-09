// SPDX-License-Identifier: CC0-1.0

//! The state helpers' own unit tests.

use super::*;

use std::collections::BTreeSet;

/// The selection the helper's own tests page under; its shape does not matter
/// here, only that it is the same one on both sides of a token.
const SELECTION: &str = "prefix=;scope=Local";

/// The instant the helper's own tests serve a listing at. Fixed, so a case ages a
/// token by the amount it names rather than by waiting for a clock.
fn served_at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// [`in_schema_name_order`] puts a selection into the CODEPOINT order of its schema
/// names, whatever order the store handed it over in, and pairs each item with the
/// name itself — the key [`page_by_key`] then compares.
///
/// Driven over a generated cross product rather than a handful of names: every
/// combination of a separator, a casing and a trailing fragment, which is what the
/// rule generalises over. The expectation is computed independently of the helper,
/// by comparing the names character by character.
#[test]
fn a_selection_is_collated_in_codepoint_order_of_its_names() {
    // Fragments crossed into names: the separators a schema name carries, both
    // casings of the words around them, and tails that differ in length, in digits
    // and in nothing but case.
    let separators = ["", ".", "-", "@", "_", "..", "-@"];
    let heads = ["svc", "SVC", "Svc9"];
    let tails = ["a", "A", "ab", "a1", "1a", "", "aB2"];

    let mut names: Vec<String> = Vec::new();
    for head in heads {
        for separator in separators {
            for tail in tails {
                names.push(format!("{head}{separator}{tail}"));
            }
        }
    }
    names.sort_unstable();
    names.dedup();

    // The order the rule names, computed without the helper.
    let mut expected = names.clone();
    expected.sort_by(|left, right| left.chars().cmp(right.chars()));

    // Handed over in the REVERSE of it, so no case could pass by reporting the
    // order it was given.
    let stored: Vec<String> = expected.iter().rev().cloned().collect();
    let collated = in_schema_name_order(stored.iter().map(|name| (name, name)));

    let served: Vec<String> = collated
        .iter()
        .map(|(_, name)| (*name).to_string())
        .collect();
    assert_eq!(
        served, expected,
        "the selection must come out in the codepoint order of its names"
    );
    for (key, name) in &collated {
        assert_eq!(
            key, *name,
            "case {name:?}: the paging key of an item is its own name, got {key:?}"
        );
    }

    // That key is a total order over distinct names: no two of them share one, so a
    // `NextToken` naming a position can never be ambiguous.
    let keys: BTreeSet<&String> = collated.iter().map(|(key, _)| key).collect();
    assert_eq!(
        keys.len(),
        collated.len(),
        "every distinct name must have its own key, got {} keys for {} names",
        keys.len(),
        collated.len()
    );
}

/// A listing shorter than the page size is one page and carries no token.
#[test]
fn a_single_short_page_has_no_token() {
    let entries = vec![("a", 1), ("b", 2)];
    let page =
        page_by_key(entries.into_iter(), SELECTION, None, 10, served_at()).expect("fresh token");
    assert_eq!(page.items, vec![1, 2], "every entry is on the page");
    assert!(
        page.next_token.is_none(),
        "a short page must not carry a token, got {:?}",
        page.next_token
    );
}

/// A page filled exactly by the last entries carries no token either, so a
/// client never follows a token to an empty page.
#[test]
fn an_exactly_filled_final_page_has_no_token() {
    let entries = vec![("a", 1), ("b", 2)];
    let page =
        page_by_key(entries.into_iter(), SELECTION, None, 2, served_at()).expect("fresh token");
    assert!(
        page.next_token.is_none(),
        "an exactly-filled final page must not carry a token, got {:?}",
        page.next_token
    );
}

/// Following the token resumes strictly after the last item of the page before.
#[test]
fn the_token_resumes_after_the_last_item() {
    let entries = vec![("a", 1), ("b", 2), ("c", 3)];
    let first = page_by_key(entries.clone().into_iter(), SELECTION, None, 2, served_at())
        .expect("fresh token");
    assert_eq!(first.items, vec![1, 2]);
    let token = first.next_token.expect("a further entry remains");
    let second = page_by_key(entries.into_iter(), SELECTION, Some(&token), 2, served_at())
        .expect("own token");
    assert_eq!(second.items, vec![3], "the next page resumes after 'b'");
    assert!(second.next_token.is_none(), "and terminates");
}

/// A token left EMPTY names no position, so the page starts at the beginning of
/// the selection — the same page a request carrying no token at all is served.
#[test]
fn an_empty_token_serves_the_first_page() {
    let entries = vec![("a", 1), ("b", 2), ("c", 3)];
    for page_size in [1_usize, 2, 3, 10] {
        let omitted = page_by_key(
            entries.clone().into_iter(),
            SELECTION,
            None,
            page_size,
            served_at(),
        )
        .expect("no token at all");
        let empty = page_by_key(
            entries.clone().into_iter(),
            SELECTION,
            Some(""),
            page_size,
            served_at(),
        )
        .unwrap_or_else(|_| {
            panic!("case {page_size}: an empty token must serve the first page")
        });
        assert_eq!(
            empty.items, omitted.items,
            "case {page_size}: expected the first page {:?}, got {:?}",
            omitted.items, empty.items
        );
        assert_eq!(
            empty.next_token, omitted.next_token,
            "case {page_size}: expected the token the first page mints {:?}, got {:?}",
            omitted.next_token, empty.next_token
        );
    }
}

/// A token this service did not mint is refused rather than silently restarting
/// the listing from the top.
#[test]
fn a_foreign_token_is_refused() {
    let entries = vec![("a", 1)];
    // The empty value is NOT in this table: a token left empty is no token at
    // all, and is covered by `an_empty_token_serves_the_first_page`.
    for foreign in ["not-a-token", "zz", "6162", " ", ".", "..", "6162.6162"] {
        assert!(
            page_by_key(
                entries.clone().into_iter(),
                SELECTION,
                Some(foreign),
                10,
                served_at()
            )
            .is_err(),
            "case {foreign:?}: a token this service did not mint must be refused"
        );
    }

    // And a token of this service's own shape whose issue instant names no instant
    // a clock can represent, is not a number, or is followed by a field this
    // service does not mint: a value a caller sends is never trusted to be one of
    // this service's own.
    let selection = ::hex::encode(SELECTION);
    let key = ::hex::encode("a");
    for malformed in [
        format!("{selection}.{key}.ffffffffffffffff"),
        format!("{selection}.{key}.-1"),
        format!("{selection}.{key}.zz"),
        format!("{selection}.{key}."),
        format!("{selection}.{key}.{:016x}.0", 0),
    ] {
        assert!(
            page_by_key(
                entries.clone().into_iter(),
                SELECTION,
                Some(&malformed),
                10,
                served_at()
            )
            .is_err(),
            "case {malformed:?}: a token this service did not mint must be refused"
        );
    }
}

/// A token names a position within ONE listing: presenting it to a listing with
/// a different selection is refused, rather than silently skipping or repeating
/// items.
#[test]
fn a_token_does_not_resume_a_differently_selected_listing() {
    let entries = vec![("a", 1), ("b", 2), ("c", 3)];
    let first = page_by_key(entries.clone().into_iter(), SELECTION, None, 1, served_at())
        .expect("fresh token");
    let token = first.next_token.expect("further entries remain");
    assert!(
        page_by_key(
            entries.into_iter(),
            "prefix=b;scope=Local",
            Some(&token),
            1,
            served_at()
        )
        .is_err(),
        "a token issued for one selection must not resume another"
    );
}

/// A token resumes its listing for as long as it lives and no longer: the instant
/// it was minted at travels in it, and a request made once [`NEXT_TOKEN_TTL`] has
/// passed is refused rather than served from a position that is no longer current.
///
/// Driven over the ages the rule generalises over: the instant of issue, inside the
/// window, each side of the published bound, far past it, and a request whose clock
/// stands before the token was minted.
#[test]
fn a_token_resumes_its_listing_only_until_it_expires() {
    let entries = vec![("a", 1), ("b", 2), ("c", 3)];
    let issued = page_by_key(entries.clone().into_iter(), SELECTION, None, 1, served_at())
        .expect("fresh token");
    let token = issued.next_token.expect("further entries remain");

    for (case, presented_at, resumes) in [
        ("at the instant of issue", served_at(), true),
        (
            "half the window later",
            served_at() + NEXT_TOKEN_TTL / 2,
            true,
        ),
        (
            "a second before the window closes",
            served_at() + NEXT_TOKEN_TTL - Duration::from_secs(1),
            true,
        ),
        ("the window exactly", served_at() + NEXT_TOKEN_TTL, false),
        (
            "a second past the window",
            served_at() + NEXT_TOKEN_TTL + Duration::from_secs(1),
            false,
        ),
        (
            "a week past the window",
            served_at() + NEXT_TOKEN_TTL + Duration::from_secs(7 * 24 * 60 * 60),
            false,
        ),
        (
            "a clock standing before the token was minted",
            served_at() - Duration::from_secs(60),
            true,
        ),
    ] {
        let resumed = page_by_key(
            entries.clone().into_iter(),
            SELECTION,
            Some(&token),
            1,
            presented_at,
        );
        assert_eq!(
            resumed.is_ok(),
            resumes,
            "case {case:?}: expected the token to resume: {resumes}, got {resumed:?}"
        );
        if let Ok(page) = resumed {
            assert_eq!(
                page.items,
                vec![2],
                "case {case:?}: a living token resumes after the page it was minted for"
            );
        }
    }
}

/// A selection fingerprint is UNAMBIGUOUS: two selections render the same
/// fingerprint only when they are the same listing with the same values, whatever
/// characters those values hold — so a token minted for one selection never
/// resumes another.
///
/// Driven over a cross product of values that hold the characters a rendering
/// separates fields with, in each member slot, plus the member names themselves as
/// values.
#[test]
fn two_different_selections_cannot_render_one_fingerprint() {
    let values = [
        "",
        "a",
        "b",
        ";",
        "=",
        ":",
        "a;",
        "a;b",
        "a;second=b",
        "a;second=b;second=",
        "1:a",
        "second",
        "listing",
    ];

    let mut rendered: BTreeMap<String, (&str, &str, &str)> = BTreeMap::new();
    for listing in ["FirstListing", "SecondListing"] {
        for first in values {
            for second in values {
                let case = (listing, first, second);
                let fingerprint = selection_fingerprint(
                    listing,
                    &[("first", first), ("second", second)],
                );
                if let Some(other) = rendered.insert(fingerprint.clone(), case) {
                    panic!(
                        "case {case:?}: renders the fingerprint {fingerprint:?} that \
                             {other:?} renders, so a token for one would resume the other"
                    );
                }
            }
        }
    }

    // And a token minted for one of them resumes none of the others.
    let entries = vec![("a", 1), ("b", 2)];
    let own = selection_fingerprint("FirstListing", &[("first", "a;second=b"), ("second", "")]);
    let token = page_by_key(entries.clone().into_iter(), &own, None, 1, served_at())
        .expect("fresh token")
        .next_token
        .expect("a further entry remains");
    for (case, other) in [
        (
            "the separator moved into the other member",
            selection_fingerprint(
                "FirstListing",
                &[("first", "a"), ("second", "b;second=")],
            ),
        ),
        (
            "the same values under another listing",
            selection_fingerprint(
                "SecondListing",
                &[("first", "a;second=b"), ("second", "")],
            ),
        ),
    ] {
        assert!(
            page_by_key(
                entries.clone().into_iter(),
                &other,
                Some(&token),
                1,
                served_at()
            )
            .is_err(),
            "case {case:?}: a token minted for {own:?} must not resume {other:?}"
        );
    }
}

/// `Limit` resolves to a page size that is never zero and never larger than the
/// server-side ceiling, so no single response can materialise the whole store.
#[test]
fn a_requested_page_size_is_clamped_to_the_server_ceiling() {
    let ceiling = i32::try_from(MAX_PAGE_SIZE).expect("the ceiling fits an i32");
    // Rows: absent, the published bounds of the clamp, one past each, and the
    // extremes of the modelled `__integer`.
    for (limit, expected) in [
        (None, Some(MAX_PAGE_SIZE)),
        (Some(1), Some(1)),
        (Some(ceiling - 1), Some(MAX_PAGE_SIZE - 1)),
        (Some(ceiling), Some(MAX_PAGE_SIZE)),
        (Some(ceiling + 1), Some(MAX_PAGE_SIZE)),
        (Some(i32::MAX), Some(MAX_PAGE_SIZE)),
        (Some(0), None),
        (Some(-1), None),
        (Some(i32::MIN), None),
    ] {
        let resolved = requested_page_size(limit).ok();
        assert_eq!(
            resolved, expected,
            "case limit={limit:?}: expected {expected:?}, got {resolved:?}"
        );
    }
}

/// The status a binding reports is one the model accepts, and it is the clock —
/// not a read — that moves it: in progress until the instant generation is due,
/// complete from then on, whoever asks and however often.
#[test]
fn a_code_binding_status_follows_the_clock() {
    let created = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let binding = CodeBinding {
        language: "Java8".to_string(),
        schema_version: 3,
        created_date: created,
        generation_due: created + CODE_GENERATION_DURATION,
    };

    // Rows: the put instant, just before the due instant, the due instant itself,
    // and long after it.
    for (offset, expected_status, expected_modified) in [
        (Duration::ZERO, CODE_GENERATION_IN_PROGRESS, created),
        (
            CODE_GENERATION_DURATION - Duration::from_millis(1),
            CODE_GENERATION_IN_PROGRESS,
            created,
        ),
        (
            CODE_GENERATION_DURATION,
            CODE_GENERATION_COMPLETE,
            created + CODE_GENERATION_DURATION,
        ),
        (
            Duration::from_secs(3600),
            CODE_GENERATION_COMPLETE,
            created + CODE_GENERATION_DURATION,
        ),
    ] {
        let report = binding.report_at(created + offset);
        assert!(
            crate::ops::types::CODE_GENERATION_STATUS_VALUES.contains(&report.status),
            "case +{offset:?}: {:?} is not a modelled CodeGenerationStatus",
            report.status
        );
        assert_eq!(
            report.status, expected_status,
            "case +{offset:?}: expected status {expected_status}, got {}",
            report.status
        );
        assert_eq!(
            report.last_modified, expected_modified,
            "case +{offset:?}: expected last-modified {expected_modified:?}, got {:?}",
            report.last_modified
        );
        assert_eq!(
            report.creation_date, created,
            "case +{offset:?}: the creation date never moves"
        );
        assert_eq!(
            report.schema_version, "3",
            "case +{offset:?}: expected the stored version in decimal, got {}",
            report.schema_version
        );
    }
}

/// The content budget counts every version of every schema in every registry —
/// everything a caller's `Content` makes this service retain.
#[test]
fn the_content_budget_counts_every_stored_version() {
    let mut state = ComAmazonawsSchemasSchemasState::default();
    assert_eq!(
        stored_content_bytes(&state),
        0,
        "a store holding nothing of the caller's retains nothing on their behalf"
    );

    let now = std::time::SystemTime::now();
    for (registry_name, contents) in [("r1", ["ab", "cde"]), ("r2", ["f", "ghij"])] {
        let mut registry = Registry {
            name: registry_name.to_string(),
            arn: registry_arn(DEFAULT_REGION, registry_name),
            description: None,
            tags: BTreeMap::new(),
            scope: RegistryScope::Local,
            schemas: BTreeMap::new(),
            policy: None,
        };
        for (index, content) in contents.iter().enumerate() {
            let name = format!("s{index}");
            registry.schemas.insert(
                name.clone(),
                Schema {
                    arn: schema_arn(DEFAULT_REGION, registry_name, &name),
                    name,
                    description: None,
                    r#type: "OpenApi3".to_string(),
                    tags: BTreeMap::new(),
                    latest_version: 1,
                    last_modified: now,
                    code_bindings: BTreeMap::new(),
                    versions: std::iter::once((
                        1,
                        SchemaVersion {
                            version: 1,
                            content: (*content).to_string(),
                            r#type: "OpenApi3".to_string(),
                            created_date: now,
                        },
                    ))
                    .collect(),
                    minted_by_token: BTreeMap::new(),
                    published_event: None,
                },
            );
        }
        state.registries.insert(registry_name.to_string(), registry);
    }

    // 2 + 3 + 1 + 4, summed across both registries.
    assert_eq!(
        stored_content_bytes(&state),
        10,
        "every stored version's content counts towards the budget"
    );
}

/// Content that enters the store is a JSON schema document. The rows vary along
/// the FORM the content takes — well-formed JSON objects of both modelled types,
/// and every other shape a JSON body can have — not along one example of it.
#[test]
fn content_is_accepted_only_as_a_json_schema_document() {
    // An OpenAPI 3 document as the specification describes one: a JSON object
    // carrying `openapi` and `info`.
    const OPENAPI: &str = r#"{"openapi":"3.0.0","info":{"title":"t","version":"1.0.0"},"paths":{}}"#;
    for (case, content, schema_type, accepted) in [
        ("openapi-document", OPENAPI, "OpenApi3", true),
        (
            "openapi-with-components-and-extensions",
            r#"{"openapi":"3.0.3","info":{"title":"t","version":"1"},"paths":{},"components":{"schemas":{"E":{"type":"object","properties":{"id":{"type":"string"}}}}},"x-vendor":1}"#,
            "OpenApi3",
            true,
        ),
        // A partial document is still a document: content is stored and handed
        // back verbatim, so what is refused is content that is not one at all.
        ("openapi-version-only", r#"{"openapi":"3.0.0"}"#, "OpenApi3", true),
        ("openapi-empty-object", "{}", "OpenApi3", true),
        ("openapi-not-json", "not a schema at all", "OpenApi3", false),
        ("openapi-truncated-json", r#"{"openapi":"#, "OpenApi3", false),
        ("openapi-array", "[]", "OpenApi3", false),
        (
            "draft4-empty-object",
            "{}",
            // Draft 4 makes every keyword optional, so the empty schema is valid.
            "JSONSchemaDraft4",
            true,
        ),
        (
            "draft4-typical",
            r#"{"$schema":"http://json-schema.org/draft-04/schema#","type":"object","required":["id"]}"#,
            "JSONSchemaDraft4",
            true,
        ),
        ("draft4-array", "[]", "JSONSchemaDraft4", false),
        ("draft4-number", "42", "JSONSchemaDraft4", false),
        ("draft4-string", "\"schema\"", "JSONSchemaDraft4", false),
        ("draft4-boolean", "true", "JSONSchemaDraft4", false),
        ("draft4-null", "null", "JSONSchemaDraft4", false),
        ("draft4-empty-body", "", "JSONSchemaDraft4", false),
        ("draft4-truncated-json", "{\"type\":", "JSONSchemaDraft4", false),
        ("draft4-trailing-garbage", "{} }", "JSONSchemaDraft4", false),
    ] {
        let outcome = validate_content(content, schema_type);
        assert_eq!(
            outcome.is_ok(),
            accepted,
            "case {case:?}: expected accepted={accepted}, got {outcome:?}"
        );
        if let Err(refusal) = outcome {
            assert_eq!(
                refusal.code, CODE_MALFORMED_REQUEST,
                "case {case:?}: a refusal carries the service's error code for the \
                     condition, got {:?}",
                refusal.code
            );
            assert!(
                refusal.message.contains("Content"),
                "case {case:?}: the message must name the member it is about, got {:?}",
                refusal.message
            );
        }
    }
}

/// A fresh store already holds the service-owned catalogue: every entry of
/// [`aws_managed_schemas`] is seeded into the AWS-scoped registry, with one
/// stored version of the type AWS stores it as, the document rendered for it and
/// the description derived from its name.
#[test]
fn a_fresh_store_holds_the_aws_managed_catalogue() {
    let state = ComAmazonawsSchemasSchemasState::default();
    let catalogue = aws_managed_schemas();
    assert_eq!(
        catalogue.len(),
        aws_managed_event_types().len(),
        "every event type the catalogue names must render a document"
    );
    for registry_name in AWS_MANAGED_REGISTRIES {
        let registry = state
            .registries
            .get(*registry_name)
            .unwrap_or_else(|| panic!("case {registry_name:?}: must be seeded"));
        assert_eq!(
            registry.scope,
            RegistryScope::Aws,
            "case {registry_name:?}: a seeded registry is AWS-scoped, got {:?}",
            registry.scope
        );
        assert!(
            !registry.schemas.is_empty(),
            "case {registry_name:?}: a seeded registry must hold schemas"
        );
    }
    let registry = &state.registries[AWS_MANAGED_REGISTRY];
    assert_eq!(
        registry.schemas.len(),
        catalogue.len(),
        "the registry holds exactly the catalogue, got {} of {}",
        registry.schemas.len(),
        catalogue.len()
    );
    for entry in catalogue {
        let schema_name = &entry.name;
        let schema = registry
            .schemas
            .get(schema_name)
            .unwrap_or_else(|| panic!("case {schema_name:?}: must be seeded"));
        assert_eq!(
            schema.r#type, AWS_MANAGED_SCHEMA_TYPE,
            "case {schema_name:?}: expected {AWS_MANAGED_SCHEMA_TYPE}, got {:?}",
            schema.r#type
        );
        assert_eq!(
            schema.description, entry.description,
            "case {schema_name:?}: expected {:?}, got {:?}",
            entry.description, schema.description
        );
        let version = schema
            .version_for(None)
            .unwrap_or_else(|| panic!("case {schema_name:?}: has a latest version"));
        assert_eq!(
            version.content, entry.content,
            "case {schema_name:?}: the seeded content is stored verbatim"
        );
        // The catalogue's documents are what ExportSchema converts, so each one has
        // to BE an OpenAPI 3 document that defines the component it is published
        // for — the one named after the event — and no reference in it may dangle.
        let parsed: ::serde_json::Value = ::serde_json::from_str(&entry.content)
            .unwrap_or_else(|e| panic!("case {schema_name:?}: must be JSON: {e}"));
        let components = &parsed["components"]["schemas"];
        assert!(
            components.is_object(),
            "case {schema_name:?}: must carry components.schemas, got {parsed:?}"
        );
        let event_name = schema_name
            .split_once('@')
            .map(|(_, event_name)| event_name)
            .unwrap_or_else(|| panic!("case {schema_name:?}: is <source>@<EventName>"));
        assert!(
            components[event_name].is_object(),
            "case {schema_name:?}: must define the component it is published for, \
                 got {components:?}"
        );
        for referenced in referenced_components(&parsed) {
            assert!(
                components[&referenced].is_object(),
                "case {schema_name:?}: the reference to {referenced:?} must name a \
                     defined component, got {components:?}"
            );
        }
    }
}

/// Every component a document references by `$ref`, at any depth.
fn referenced_components(value: &::serde_json::Value) -> Vec<String> {
    let mut referenced: Vec<String> = Vec::new();
    match value {
        ::serde_json::Value::Object(members) => {
            for (name, member) in members {
                if name == "$ref" {
                    if let Some(component) = member
                        .as_str()
                        .and_then(|target| target.strip_prefix(OPENAPI_COMPONENT_REF))
                    {
                        referenced.push(component.to_string());
                    }
                }
                referenced.extend(referenced_components(member));
            }
        }
        ::serde_json::Value::Array(items) => {
            for item in items {
                referenced.extend(referenced_components(item));
            }
        }
        _ => {}
    }
    referenced
}

/// Every event type the catalogue names is published under `<source>@<EventName>`
/// and described by the sentence that name yields, and no name is seeded twice.
#[test]
fn the_catalogue_names_and_describes_every_event_type() {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for event in aws_managed_event_types() {
        let name = event.schema_name();
        assert_eq!(
            name,
            format!("{}@{}", event.source, event.event_name),
            "case {name:?}: a catalogue name is <source>@<EventName>"
        );
        assert_eq!(
            managed_schema_description(&name),
            Some(format!(
                "Schema for event type {}, published by AWS service {}",
                event.event_name, event.source
            )),
            "case {name:?}: wrong description"
        );
        assert!(
            event.source.starts_with("aws."),
            "case {name:?}: an AWS event source is `aws.<service>`"
        );
        assert!(
            seen.insert(name.clone()),
            "case {name:?}: the catalogue names it twice"
        );
    }
    assert_eq!(
        managed_schema_description("no-event-type-here"),
        None,
        "a name that is not <source>@<EventName> describes no event type"
    );
}

/// A service-owned registry reports the region of the request asking, because no
/// request created it; a caller's reports the ARN its create minted.
#[test]
fn a_managed_registry_reports_the_asking_region() {
    let state = ComAmazonawsSchemasSchemasState::default();
    let managed_name = AWS_MANAGED_REGISTRY;
    let managed = &state.registries[managed_name];
    for region in ["us-east-1", "eu-west-2", "ap-southeast-1"] {
        assert_eq!(
            managed.arn_in(region),
            registry_arn(region, managed_name),
            "case {region:?}: a managed registry's ARN names the asking region"
        );
    }

    let local = Registry {
        name: "mine".to_string(),
        arn: registry_arn("us-west-2", "mine"),
        description: None,
        tags: BTreeMap::new(),
        scope: RegistryScope::Local,
        policy: None,
        schemas: BTreeMap::new(),
    };
    assert_eq!(
        local.arn_in("eu-west-2"),
        registry_arn("us-west-2", "mine"),
        "a caller's registry keeps the ARN its create minted"
    );
}

/// Minting versions hands out strictly increasing, never-repeated numbers, even
/// across a delete of the newest one.
#[test]
fn minting_a_version_never_repeats_a_number() {
    let now = SystemTime::now();
    let mut schema = Schema {
        name: "s".to_string(),
        arn: schema_arn(DEFAULT_REGION, "r", "s"),
        description: None,
        r#type: "OpenApi3".to_string(),
        tags: BTreeMap::new(),
        latest_version: 1,
        last_modified: now,
        versions: std::iter::once((
            1,
            SchemaVersion {
                version: 1,
                content: "one".to_string(),
                r#type: "OpenApi3".to_string(),
                created_date: now,
            },
        ))
        .collect(),
        minted_by_token: BTreeMap::new(),
        code_bindings: BTreeMap::new(),
        published_event: None,
    };

    let mut minted = vec![1];
    for round in 0..4 {
        let version = schema
            .add_version(format!("content-{round}"), "OpenApi3".to_string(), now)
            .version;
        assert!(
            !minted.contains(&version),
            "case round={round}: version {version} was minted twice ({minted:?})"
        );
        minted.push(version);
        // Deleting the newest version must not let the next mint reuse it.
        if round == 1 {
            schema.versions.remove(&version);
        }
    }
    assert_eq!(
        minted,
        vec![1, 2, 3, 4, 5],
        "versions are minted in sequence, got {minted:?}"
    );
}
