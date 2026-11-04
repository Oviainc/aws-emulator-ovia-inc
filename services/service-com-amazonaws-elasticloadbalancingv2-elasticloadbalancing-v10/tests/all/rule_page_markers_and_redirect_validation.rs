// SPDX-License-Identifier: CC0-1.0

//! How a paginated read of this service hands a caller its place in a collection, and
//! what a rule's conditions and redirect actions must look like to be stored at all —
//! driven over the wire exactly as a client drives them.
//!
//! Three rules are asserted here, each over a SET of inputs rather than over the one call
//! a recording happens to carry:
//!
//! * a read handed no `PageSize` reports the WHOLE selection in one page and no
//!   `NextMarker`, and a `NextMarker` is the service's own opaque token — the position it
//!   names travels inside it, so a value this store did not mint for this collection names
//!   no position and is refused;
//! * a rule reports its conditions in the order they were supplied, and a condition names
//!   only the members the model gives its field a spelling for;
//! * each component of a redirect's target URI holds the form its member documents, and a
//!   redirect modifies at least one of the four components that avoid a loop.
//!
//! Every accepted write is read back through `DescribeRules`, and after every refusal the
//! listener's rules are read again to prove nothing was stored or changed.

use crate::listener_rules::{
    add_rule, create_listener, create_load_balancer, create_rule, fixed_response, forward_to,
    host_header, rules_of, summaries,
};
use crate::provisioning_loop::{
    action, call, create_http_target_group, emulator, first_text, members, p, Reply,
};
use smithy_server_runtime::MultiServiceRouter;

/// The error code this service answers an input its own documented rules refuse with.
const VALIDATION_ERROR: &str = "ValidationError";

/// The awsQuery error code of the refusal every operation that takes an action models for
/// one that is not valid (`smithy_query(query="operation",operation="CreateRule")`:
/// `InvalidLoadBalancerActionException`, `awsQueryError` code `InvalidLoadBalancerAction`).
const INVALID_ACTION: &str = "InvalidLoadBalancerAction";

/// The code the framework answers a `Marker` this store did not issue with.
const MARKER_REFUSAL_CODE: &str = "ValidationException";

/// The `<Message>` of a refusal, with the XML escapes the serializer writes read back —
/// an error message quoting a member name or a reserved keyword carries apostrophes and
/// ampersands, and what the contract states is the text, not its escaping.
fn refusal_message(reply: &Reply) -> Option<String> {
    reply.text("Message").map(|message| {
        message
            .replace("&apos;", "'")
            .replace("&quot;", "\"")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&")
    })
}

/// Read one listener's rules with `fields` beside the listener.
async fn describe_rules(
    router: &MultiServiceRouter,
    listener_arn: &str,
    fields: &[(String, String)],
) -> Reply {
    let mut form = action("DescribeRules");
    form.push(p("ListenerArn", listener_arn));
    form.extend(fields.iter().cloned());
    call(router, &form).await
}

/// A load balancer, a target group and one HTTP listener on `port`, for a suite that needs
/// somewhere to hang rules.
async fn listener_on(
    router: &MultiServiceRouter,
    name: &str,
    port: i32,
) -> (String, String, String) {
    let load_balancer = create_load_balancer(router, name).await;
    let group = create_http_target_group(router, &format!("{name}-tg")).await;
    let listener = create_listener(router, &load_balancer, &group, port).await;
    (load_balancer, group, listener)
}

/// Rule 1a: a read handed no `PageSize` reports every rule of the selection in one page,
/// with no `NextMarker` beside it — whatever the collection holds.
#[tokio::test]
async fn a_read_handed_no_page_size_reports_the_whole_collection_and_no_marker() {
    let (router, _engine) = emulator();
    let (_lb, group, listener) = listener_on(&router, "rule-page-default", 80).await;

    // The rows differ in HOW MANY rules the collection holds: only the listener's own
    // default rule, one added rule, and several — the published page size is 400, so a
    // collection far smaller than one page and one of any size answer the same way.
    let mut added = 0usize;
    for held in [0usize, 1, 2, 3, 7] {
        while added < held {
            added += 1;
            let priority = i32::try_from(added).expect("a rule priority");
            add_rule(
                &router,
                &format!("rule {added}"),
                &listener,
                priority,
                &[host_header(&format!("host-{added}.example.com")), forward_to(&group)].concat(),
            )
            .await;
        }
        let case = format!("{} rules", held + 1);
        let reply = describe_rules(&router, &listener, &[]).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected 200, got {} {}",
            reply.status, reply.body
        );
        let reported = summaries(&reply);
        assert_eq!(
            reported.len(),
            held + 1,
            "case {case}: expected every rule of the listener in one page, got {reported:?}"
        );
        assert_eq!(
            reply.text("NextMarker"),
            None,
            "case {case}: a whole collection leaves nothing to resume from, got {}",
            reply.body
        );
        assert_eq!(
            reported.last().map(|(_, is_default)| is_default.as_str()),
            Some("true"),
            "case {case}: the default rule is reported last, got {reported:?}"
        );
    }
}

/// Rule 1b: a `NextMarker` is the service's own opaque token — every walk of the
/// collection reports each rule once in order and ends with no marker, and a value this
/// store did not mint for this collection names no position in it.
#[tokio::test]
async fn a_next_marker_is_this_stores_own_opaque_token() {
    let (router, _engine) = emulator();
    let (load_balancer, group, listener) = listener_on(&router, "rule-page-marker", 80).await;
    // A second listener, so this load balancer has a collection of its own to issue a
    // marker for — the one a rules walk must not accept.
    let sibling = create_listener(&router, &load_balancer, &group, 8080).await;

    for priority in 1..=4 {
        add_rule(
            &router,
            &format!("rule {priority}"),
            &listener,
            priority,
            &[
                host_header(&format!("host-{priority}.example.com")),
                forward_to(&group),
            ]
            .concat(),
        )
        .await;
    }
    let whole = rules_of(&router, "the whole collection", &listener).await;
    assert_eq!(
        whole.len(),
        5,
        "case the whole collection: expected the four added rules and the default rule, \
         got {whole:?}"
    );

    // Every page size from one rule per page to one past the whole collection: the pages
    // concatenate to the unpaged order, every marker is this store's opaque token rather
    // than a position a caller could have computed, and the walk ends.
    for page_size in 1..=whole.len() + 1 {
        let case = format!("PageSize={page_size}");
        let mut walked: Vec<(String, String)> = Vec::new();
        let mut marker: Option<String> = None;
        let mut page_index = 0usize;
        for _ in 0..whole.len() + 2 {
            let mut fields = vec![p("PageSize", &page_size.to_string())];
            if let Some(marker) = marker.as_deref() {
                fields.push(p("Marker", marker));
            }
            let reply = describe_rules(&router, &listener, &fields).await;
            assert_eq!(
                reply.status, 200,
                "case {case}: expected 200, got {} {}",
                reply.status, reply.body
            );
            let page = summaries(&reply);
            assert!(
                page.len() <= page_size,
                "case {case}: a page holds at most {page_size} rules, got {page:?}"
            );
            walked.extend(page);
            page_index += 1;
            marker = reply.text("NextMarker");
            if let Some(issued) = marker.as_deref() {
                let ordinal = (page_index * page_size).to_string();
                assert_ne!(
                    issued, ordinal,
                    "case {case}: a marker is not the position it names, got {issued}"
                );
                assert!(
                    issued.len() > ordinal.len()
                        && issued.chars().all(|char| char.is_ascii_hexdigit()),
                    "case {case}: a marker is this store's opaque token, got {issued}"
                );
            } else {
                break;
            }
        }
        assert_eq!(
            marker, None,
            "case {case}: the walk must end with no NextMarker"
        );
        assert_eq!(
            walked, whole,
            "case {case}: paging reports every rule once, in the same order, got {walked:?}"
        );
    }

    // A marker this read ISSUED resumes exactly after the page it came with.
    let first = describe_rules(&router, &listener, &[p("PageSize", "1")]).await;
    let issued = first
        .text("NextMarker")
        .expect("the first page of five rules leaves a marker to resume from");
    let resumed = describe_rules(
        &router,
        &listener,
        &[p("Marker", &issued), p("PageSize", "1")],
    )
    .await;
    assert_eq!(
        (resumed.status, summaries(&resumed)),
        (200, vec![whole[1].clone()]),
        "case the marker this read issued: expected the rule after the first, got {} {}",
        resumed.status,
        resumed.body
    );

    // Values a caller could write rather than round-trip: the ordinals the position would
    // be, text that is no token at all, a marker issued for ANOTHER collection of this
    // same store, and one issued by another store altogether.
    let listeners_marker = {
        let mut form = action("DescribeListeners");
        form.push(p("LoadBalancerArn", &load_balancer));
        form.push(p("PageSize", "1"));
        let reply = call(&router, &form).await;
        reply
            .text("NextMarker")
            .expect("two listeners paged one at a time leave a marker")
    };
    let (other_store, _other_engine) = emulator();
    let elsewhere = {
        let (_lb, group, listener) = listener_on(&other_store, "rule-page-elsewhere", 80).await;
        add_rule(
            &other_store,
            "a rule in the other store",
            &listener,
            1,
            &[
                host_header("other.example.com"),
                forward_to(&group),
            ]
            .concat(),
        )
        .await;
        describe_rules(&other_store, &listener, &[p("PageSize", "1")])
            .await
            .text("NextMarker")
            .expect("the other store's first page leaves a marker too")
    };
    for (case, value) in [
        ("the ordinal of the second position", "1".to_string()),
        ("the ordinal of the first position", "0".to_string()),
        ("a marker that is no token", "not-a-marker".to_string()),
        ("an empty marker", String::new()),
        ("an ordinal past the end", "9999".to_string()),
        ("another collection's marker", listeners_marker),
        ("another store's marker", elsewhere),
    ] {
        let refused = describe_rules(&router, &listener, &[p("Marker", &value)]).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, MARKER_REFUSAL_CODE),
            "case {case}: expected 400 {MARKER_REFUSAL_CODE}, got {} {}",
            refused.status,
            refused.body
        );
        assert!(
            !refused.body.contains("<Rules"),
            "case {case}: a refusal reports no rule, got {}",
            refused.body
        );
    }

    // The rules themselves are unchanged by every read above: a report may not change what
    // a later caller sees.
    assert_eq!(
        rules_of(&router, "after every read", &listener).await,
        whole,
        "case after every read: the collection is unchanged"
    );
    assert_eq!(
        rules_of(&router, "the sibling listener", &sibling).await.len(),
        1,
        "case the sibling listener: it carries only its own default rule"
    );
}

/// One row of the condition-order table: the case, and the condition fields in the order
/// the request lists them.
struct OrderRow {
    case: &'static str,
    fields: &'static [&'static str],
}

/// The orders differ in WHICH field comes first and in how many conditions a rule carries
/// — the fields are the ones `RuleCondition$Field` documents, and a rule may repeat
/// `http-header` and `query-string`.
const ORDER_ROWS: &[OrderRow] = &[
    OrderRow {
        case: "the order the recorded rule was created with",
        fields: &["host-header", "http-header", "path-pattern"],
    },
    OrderRow {
        case: "the same three fields, re-ordered",
        fields: &["path-pattern", "host-header", "http-header"],
    },
    OrderRow {
        case: "a header condition first",
        fields: &["http-header", "path-pattern", "host-header"],
    },
    OrderRow {
        case: "one condition only",
        fields: &["source-ip"],
    },
    OrderRow {
        case: "a field that may repeat, around one that may not",
        fields: &["http-header", "host-header", "http-header", "query-string"],
    },
];

/// The request fields of one condition on `field`, at `index`, naming values only through
/// the field's own configuration shape so the order under test is the only thing that
/// varies.
fn condition(index: usize, field: &str) -> Vec<(String, String)> {
    let at = |member: &str| format!("Conditions.member.{index}.{member}");
    let mut fields = vec![p(&at("Field"), field)];
    match field {
        "host-header" => fields.push(p(&at("HostHeaderConfig.Values.member.1"), "a.example.com")),
        "path-pattern" => fields.push(p(&at("PathPatternConfig.Values.member.1"), "/a")),
        "http-header" => {
            fields.push(p(
                &at("HttpHeaderConfig.HttpHeaderName"),
                &format!("X-Probe-{index}"),
            ));
            fields.push(p(&at("HttpHeaderConfig.Values.member.1"), "probe"));
        }
        "query-string" => {
            fields.push(p(&at("QueryStringConfig.Values.member.1.Key"), "mode"));
            fields.push(p(&at("QueryStringConfig.Values.member.1.Value"), "debug"));
        }
        // `source-ip` and `http-request-method` name their values the same way.
        _ => fields.push(p(&at("SourceIpConfig.Values.member.1"), "10.0.0.0/8")),
    }
    fields
}

/// Rule 2a: a rule reports its conditions in the order they were supplied — the order is
/// the rule's, not a grouping by field.
#[tokio::test]
async fn a_rule_reports_its_conditions_in_the_order_they_were_supplied() {
    let (router, _engine) = emulator();
    let (_lb, _group, listener) = listener_on(&router, "rule-condition-order", 80).await;

    for (index, row) in ORDER_ROWS.iter().enumerate() {
        let priority = i32::try_from(index + 1).expect("a rule priority");
        let mut fields: Vec<(String, String)> = Vec::new();
        for (position, field) in row.fields.iter().enumerate() {
            fields.extend(condition(position + 1, field));
        }
        fields.extend(fixed_response("404"));
        let created = create_rule(&router, &listener, &priority.to_string(), &fields).await;
        assert_eq!(
            created.status, 200,
            "case {}: expected 200, got {} {}",
            row.case, created.status, created.body
        );
        let arn = first_text(&created.body, "RuleArn")
            .unwrap_or_else(|| panic!("case {}: no RuleArn in {}", row.case, created.body));

        // Read back through `DescribeRules` rather than trusted from the create's answer.
        let read = describe_rules(&router, &listener, &[]).await;
        let stored = members(&read.body, "Rules")
            .into_iter()
            .find(|rule| rule.contains(&arn))
            .unwrap_or_else(|| panic!("case {}: no rule {arn} in {}", row.case, read.body));
        let reported: Vec<String> = members(&stored, "Conditions")
            .iter()
            .map(|condition| first_text(condition, "Field").unwrap_or_default())
            .collect();
        assert_eq!(
            reported, row.fields,
            "case {}: expected the conditions in the order they were supplied, got {reported:?}",
            row.case
        );
    }
}

/// One row of the condition-member table: the case, the field, the top-level member the
/// request names beside it, and whether the model gives that field that spelling.
struct MemberRow {
    case: &'static str,
    field: &'static str,
    member: &'static str,
    accepted: bool,
}

/// The rows span every field `RuleCondition$Field` documents against both members the
/// model gives only SOME fields a top-level spelling for: `Values` "only when Field is
/// host-header or path-pattern", `RegexValues` "only when Field is http-header,
/// host-header, or path-pattern".
const MEMBER_ROWS: &[MemberRow] = &[
    MemberRow {
        case: "a host header's values",
        field: "host-header",
        member: "Values",
        accepted: true,
    },
    MemberRow {
        case: "a path pattern's values",
        field: "path-pattern",
        member: "Values",
        accepted: true,
    },
    MemberRow {
        case: "an http header's values",
        field: "http-header",
        member: "Values",
        accepted: false,
    },
    MemberRow {
        case: "a query string's values",
        field: "query-string",
        member: "Values",
        accepted: false,
    },
    MemberRow {
        case: "an http request method's values",
        field: "http-request-method",
        member: "Values",
        accepted: false,
    },
    MemberRow {
        case: "a source ip's values",
        field: "source-ip",
        member: "Values",
        accepted: false,
    },
    MemberRow {
        case: "a host header's regular expressions",
        field: "host-header",
        member: "RegexValues",
        accepted: true,
    },
    MemberRow {
        case: "a path pattern's regular expressions",
        field: "path-pattern",
        member: "RegexValues",
        accepted: true,
    },
    MemberRow {
        case: "an http header's regular expressions",
        field: "http-header",
        member: "RegexValues",
        accepted: true,
    },
    MemberRow {
        case: "a query string's regular expressions",
        field: "query-string",
        member: "RegexValues",
        accepted: false,
    },
    MemberRow {
        case: "an http request method's regular expressions",
        field: "http-request-method",
        member: "RegexValues",
        accepted: false,
    },
    MemberRow {
        case: "a source ip's regular expressions",
        field: "source-ip",
        member: "RegexValues",
        accepted: false,
    },
];

/// The refusal a condition naming a member its field has no spelling for earns, as the
/// service words it.
fn unsupported_member_message(member: &str, field: &str) -> String {
    format!("'{member}' is not supported for a condition of type '{field}'")
}

/// Rule 2b: a condition names only the members the model gives its field a spelling for,
/// on the create that stores the conditions and on the modification that replaces them.
#[tokio::test]
async fn a_condition_may_name_only_the_members_its_field_has_a_spelling_for() {
    let (router, _engine) = emulator();
    let (_lb, _group, listener) = listener_on(&router, "rule-condition-members", 80).await;
    // One rule to MODIFY, so the same clause is asserted on both operations.
    let existing = add_rule(
        &router,
        "the rule a modification replaces the conditions of",
        &listener,
        1,
        &[
            host_header("keep.example.com"),
            fixed_response("404"),
        ]
        .concat(),
    )
    .await;
    let before = rules_of(&router, "before the table", &listener).await;

    for (index, row) in MEMBER_ROWS.iter().enumerate() {
        let priority = i32::try_from(index + 10).expect("a rule priority");
        let fields = [
            vec![
                p("Conditions.member.1.Field", row.field),
                p(
                    &format!("Conditions.member.1.{}.member.1", row.member),
                    "a.example.com",
                ),
            ],
            fixed_response("404"),
        ]
        .concat();

        let created = create_rule(&router, &listener, &priority.to_string(), &fields).await;
        if row.accepted {
            assert_eq!(
                created.status, 200,
                "case {} on CreateRule: expected 200, got {} {}",
                row.case, created.status, created.body
            );
        } else {
            assert_eq!(
                (created.status, created.error_code().as_str()),
                (400, VALIDATION_ERROR),
                "case {} on CreateRule: expected 400 {VALIDATION_ERROR}, got {} {}",
                row.case,
                created.status,
                created.body
            );
            assert_eq!(
                refusal_message(&created),
                Some(unsupported_member_message(row.member, row.field)),
                "case {} on CreateRule: expected the refusal naming the member and the \
                 field, got {}",
                row.case,
                created.body
            );
        }

        // The same clause on ModifyRule, which replaces a stored rule's conditions.
        let mut form = action("ModifyRule");
        form.push(p("RuleArn", &existing));
        form.extend(fields.iter().cloned());
        let modified = call(&router, &form).await;
        if row.accepted {
            assert_eq!(
                modified.status, 200,
                "case {} on ModifyRule: expected 200, got {} {}",
                row.case, modified.status, modified.body
            );
        } else {
            assert_eq!(
                (modified.status, modified.error_code().as_str()),
                (400, VALIDATION_ERROR),
                "case {} on ModifyRule: expected 400 {VALIDATION_ERROR}, got {} {}",
                row.case,
                modified.status,
                modified.body
            );
            assert_eq!(
                refusal_message(&modified),
                Some(unsupported_member_message(row.member, row.field)),
                "case {} on ModifyRule: expected the same refusal the create earns, got {}",
                row.case,
                modified.body
            );
        }
    }

    // Every refused create stored nothing: the rules the listener holds are the one it
    // started with plus exactly the accepted rows.
    let accepted = MEMBER_ROWS.iter().filter(|row| row.accepted).count();
    let after = rules_of(&router, "after the table", &listener).await;
    assert_eq!(
        after.len(),
        before.len() + accepted,
        "case after the table: expected only the accepted rows to have been stored, \
         got {after:?}"
    );
}

/// One row of the redirect table: the case, the `RedirectConfig` members the request
/// names, and the refusal the configuration earns (`None` when it is accepted).
struct RedirectRow {
    case: &'static str,
    members: &'static [(&'static str, &'static str)],
    refused: Option<&'static str>,
}

/// The refusal a `Path` that is not an absolute path, or that reuses a component a path may
/// not reuse, earns — the service's own wording for the form that member documents.
const PATH_REFUSAL: &str = "The Path parameter must be a valid path, should start with a \
                            '/', and may contain up to one of each of these placeholders: \
                            '#{path}', '#{host}', '#{port}'.";

/// The refusal a `Port` that is neither a port number inside the published range nor the
/// keyword that retains the request's own port earns.
const PORT_REFUSAL: &str =
    "The Port parameter must be either an integer between 1 and 65535 or '#{port}'.";

/// The refusal a redirect that modifies none of the four components the model names earns.
const LOOP_REFUSAL: &str = "The redirect configuration is not valid because it creates a loop.";

/// The rows differ along WHICH component the request names and what it names it: each of
/// the five components of the target URI, both published bounds of the port, the
/// placeholders a path may and may not reuse, and the configurations that modify nothing.
const REDIRECT_ROWS: &[RedirectRow] = &[
    RedirectRow {
        case: "a host rewrite",
        members: &[("Host", "moved.example.com")],
        refused: None,
    },
    RedirectRow {
        case: "a path that reuses the original",
        members: &[("Path", "/new/#{path}")],
        refused: None,
    },
    RedirectRow {
        case: "the lowest port",
        members: &[("Port", "1")],
        refused: None,
    },
    RedirectRow {
        case: "the highest port",
        members: &[("Port", "65535")],
        refused: None,
    },
    RedirectRow {
        case: "the keyword that retains the port, beside a modified host",
        members: &[("Port", "#{port}"), ("Host", "kept.example.com")],
        refused: None,
    },
    RedirectRow {
        case: "every component at once",
        members: &[
            ("Protocol", "HTTPS"),
            ("Host", "all.example.com"),
            ("Port", "443"),
            ("Path", "/landing"),
            ("Query", "from=elb"),
        ],
        refused: None,
    },
    RedirectRow {
        case: "a path with no leading slash",
        members: &[("Path", "no-leading-slash")],
        refused: Some(PATH_REFUSAL),
    },
    RedirectRow {
        case: "a path reusing one component twice",
        members: &[("Path", "/#{path}/#{path}")],
        refused: Some(PATH_REFUSAL),
    },
    RedirectRow {
        case: "a path reusing a component a path may not reuse",
        members: &[("Path", "/#{query}")],
        refused: Some(PATH_REFUSAL),
    },
    RedirectRow {
        case: "one below the lowest port",
        members: &[("Port", "0")],
        refused: Some(PORT_REFUSAL),
    },
    RedirectRow {
        case: "one past the highest port",
        members: &[("Port", "65536")],
        refused: Some(PORT_REFUSAL),
    },
    RedirectRow {
        case: "a port that is not a number",
        members: &[("Port", "https")],
        refused: Some(PORT_REFUSAL),
    },
    RedirectRow {
        case: "a redirect that modifies nothing",
        members: &[],
        refused: Some(LOOP_REFUSAL),
    },
    RedirectRow {
        case: "every component written as the keyword that retains it",
        members: &[
            ("Protocol", "#{protocol}"),
            ("Host", "#{host}"),
            ("Port", "#{port}"),
            ("Path", "/#{path}"),
        ],
        refused: Some(LOOP_REFUSAL),
    },
    RedirectRow {
        case: "a redirect that modifies only the query",
        members: &[("Query", "#{query}&value=xyz")],
        refused: Some(LOOP_REFUSAL),
    },
];

/// The request fields of one redirect action built from a row, under `prefix` — the member
/// a rule carries its actions in, or the one a listener carries its default actions in.
fn redirect_action(prefix: &str, row: &RedirectRow) -> Vec<(String, String)> {
    let mut fields = vec![
        p(&format!("{prefix}.member.1.Type"), "redirect"),
        p(
            &format!("{prefix}.member.1.RedirectConfig.StatusCode"),
            "HTTP_301",
        ),
    ];
    for (member, value) in row.members {
        fields.push(p(
            &format!("{prefix}.member.1.RedirectConfig.{member}"),
            value,
        ));
    }
    fields
}

/// Rule 3: each component of a redirect's target URI holds the form its member documents,
/// and a redirect modifies at least one of the components that avoid a loop — wherever a
/// redirect action is set, and whatever a refusal would otherwise have stored.
#[tokio::test]
async fn a_redirect_holds_its_components_form_and_modifies_what_avoids_a_loop() {
    let (router, _engine) = emulator();
    let (load_balancer, _group, listener) = listener_on(&router, "rule-redirect-form", 80).await;
    let existing = add_rule(
        &router,
        "the rule a modification replaces the actions of",
        &listener,
        1,
        &[
            host_header("keep.example.com"),
            fixed_response("404"),
        ]
        .concat(),
    )
    .await;
    let before = rules_of(&router, "before the table", &listener).await;
    let mut port = 8_000;

    for (index, row) in REDIRECT_ROWS.iter().enumerate() {
        let priority = i32::try_from(index + 10).expect("a rule priority");
        let fields = [
            vec![p("Conditions.member.1.Field", "path-pattern")],
            vec![p(
                "Conditions.member.1.PathPatternConfig.Values.member.1",
                "/a",
            )],
            redirect_action("Actions", row),
        ]
        .concat();

        // (a) the create that first stores the action.
        let created = create_rule(&router, &listener, &priority.to_string(), &fields).await;
        match row.refused {
            None => assert_eq!(
                created.status, 200,
                "case {} on CreateRule: expected 200, got {} {}",
                row.case, created.status, created.body
            ),
            Some(message) => {
                assert_eq!(
                    (created.status, created.error_code().as_str()),
                    (400, INVALID_ACTION),
                    "case {} on CreateRule: expected 400 {INVALID_ACTION}, got {} {}",
                    row.case,
                    created.status,
                    created.body
                );
                assert_eq!(
                    refusal_message(&created).as_deref(),
                    Some(message),
                    "case {} on CreateRule: expected the refusal the form earns, got {}",
                    row.case,
                    created.body
                );
            }
        }

        // (b) the modification that replaces a stored rule's actions.
        let mut form = action("ModifyRule");
        form.push(p("RuleArn", &existing));
        form.extend(redirect_action("Actions", row));
        let modified = call(&router, &form).await;
        match row.refused {
            None => assert_eq!(
                modified.status, 200,
                "case {} on ModifyRule: expected 200, got {} {}",
                row.case, modified.status, modified.body
            ),
            Some(message) => {
                assert_eq!(
                    (modified.status, modified.error_code().as_str()),
                    (400, INVALID_ACTION),
                    "case {} on ModifyRule: expected 400 {INVALID_ACTION}, got {} {}",
                    row.case,
                    modified.status,
                    modified.body
                );
                assert_eq!(
                    refusal_message(&modified).as_deref(),
                    Some(message),
                    "case {} on ModifyRule: expected the same refusal the create earns, got {}",
                    row.case,
                    modified.body
                );
            }
        }

        // (c) a listener's DEFAULT actions, which take the same kind of action.
        port += 1;
        let mut form = action("CreateListener");
        form.push(p("LoadBalancerArn", &load_balancer));
        form.push(p("Protocol", "HTTP"));
        form.push(p("Port", &port.to_string()));
        form.extend(redirect_action("DefaultActions", row));
        let listener_reply = call(&router, &form).await;
        match row.refused {
            None => assert_eq!(
                listener_reply.status, 200,
                "case {} on CreateListener: expected 200, got {} {}",
                row.case, listener_reply.status, listener_reply.body
            ),
            Some(message) => {
                assert_eq!(
                    (listener_reply.status, listener_reply.error_code().as_str()),
                    (400, INVALID_ACTION),
                    "case {} on CreateListener: expected 400 {INVALID_ACTION}, got {} {}",
                    row.case,
                    listener_reply.status,
                    listener_reply.body
                );
                assert_eq!(
                    refusal_message(&listener_reply).as_deref(),
                    Some(message),
                    "case {} on CreateListener: expected the same refusal a rule's action \
                     earns, got {}",
                    row.case,
                    listener_reply.body
                );
            }
        }
    }

    // Exactly the accepted rows were stored, and the rule a refused modification named
    // still carries the actions the last accepted modification left it with.
    let accepted = REDIRECT_ROWS.iter().filter(|row| row.refused.is_none()).count();
    let after = rules_of(&router, "after the table", &listener).await;
    assert_eq!(
        after.len(),
        before.len() + accepted,
        "case after the table: expected only the accepted rows to have been stored, \
         got {after:?}"
    );
    let read = describe_rules(&router, &listener, &[]).await;
    let stored = members(&read.body, "Rules")
        .into_iter()
        .find(|rule| rule.contains(&existing))
        .unwrap_or_else(|| panic!("case after the table: no rule {existing} in {}", read.body));
    assert!(
        stored.contains("<Type>redirect</Type>"),
        "case after the table: the modified rule keeps the last accepted action, got {stored}"
    );
}
