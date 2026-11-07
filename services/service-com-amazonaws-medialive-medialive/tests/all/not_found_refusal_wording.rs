// SPDX-License-Identifier: CC0-1.0

//! HOW a MediaLive refusal reports that what the request addressed does not
//! exist, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! `NotFoundException` is a client error at HTTP 404 whose single member
//! `Message` (jsonName `message`) is OPTIONAL
//! (`smithy_query(query="shape",shape="NotFoundException")`), so the model fixes
//! the status and the code and leaves the message to the service. The three
//! rules below are what the service does with it, and each is a TABLE because
//! each generalises over a whole class of requests:
//!
//! 1. the LABEL: the message names the resource family with THAT family's own
//!    label, which is per-family and is never derived from the shape name —
//!    MediaLive words some families' label as the shape name
//!    (`InputSecurityGroup`) and others as an English phrase (`SDI source`), so
//!    the label is a constant of the family taken from its recorded refusal and
//!    every entry point that resolves one of its identifiers uses it;
//! 2. the GRANULARITY: the identifier a refusal names is the resource the
//!    operation RESOLVED. A multiplex program is addressed by two
//!    `@httpLabel`s
//!    (`smithy_query(query="operation",operation="DescribeMultiplexProgram")`:
//!    GET /prod/multiplexes/{MultiplexId}/programs/{ProgramName}), and the read
//!    resolves that pair as ONE program lookup, so its refusal names the pair
//!    even when it is the enclosing multiplex that is missing. The program's
//!    write paths resolve the enclosing multiplex first and report THAT
//!    multiplex's own not-found, which is what their recorded refusals carry,
//!    and so does an operation addressed to the multiplex itself;
//! 3. the PRESENCE: a resource of the cloudwatch alarm-template hierarchy — a
//!    template group or a template — is reported missing with the status and the
//!    error code ALONE, with no `message` member, as its sibling rule-template
//!    hierarchy already is (`crate::event_bridge_rule_template_not_found`),
//!    while every family outside the two hierarchies states its message. The
//!    response envelope is unaffected: only `message` is absent.
//!
//! `StartUpdateSignalMap`'s alarm-group resolution obeys rule 3 too; its row
//! lives in `crate::signal_map_update`, beside that operation's other refusals,
//! because it needs a settled signal map to reach.

use serde_json::{json, Value};

use crate::multiplex_program_deletion::delete_program;
use crate::multiplex_program_update::update_program;
use crate::multiplex_programs::{create_program, describe_program, multiplex, program_settings};
use crate::support::{entry_point, percent_encoded, GROUPS};
use crate::wire::{call, error_code, error_message, medialive};

/// The collection URIs of the resources rule 3 is about
/// (`smithy_query(query="operation",
/// operation="CreateCloudWatchAlarmTemplate,CreateSignalMap")`: POST
/// /prod/cloudwatch-alarm-templates, POST /prod/signal-maps).
const TEMPLATES: &str = "/prod/cloudwatch-alarm-templates";
const SIGNAL_MAPS: &str = "/prod/signal-maps";

/// The envelope members a MediaLive failure carries beside `message`
/// (`crate::error_envelope`), which rule 3 leaves alone.
const ENVELOPE_MEMBERS: [&str; 5] = ["code", "statusCode", "retryable", "requestId", "time"];

/// One resource family: the label its own refusals name it with, the collection
/// its single-resource operations hang off, and the methods of those operations
/// that answer `NotFoundException` for an identifier naming nothing.
struct Family {
    /// The label the family's message opens with. The catalog below is the set
    /// of labels, one per family, each taken from that family's recorded
    /// refusal.
    label: &'static str,
    collection: &'static str,
    methods: &'static [&'static str],
}

/// The label catalog: one entry per family whose own operations resolve an
/// identifier of its own, including the families the issue did not cite, so the
/// two spellings AWS uses are pinned side by side and neither can be derived
/// from the other.
const FAMILIES: [Family; 6] = [
    // The shape-name spelling (`smithy_query(query="operation",
    // operation="DescribeInputSecurityGroup")`: GET
    // /prod/inputSecurityGroups/{InputSecurityGroupId}).
    Family {
        label: "InputSecurityGroup",
        collection: "/prod/inputSecurityGroups",
        methods: &["GET", "PUT", "DELETE"],
    },
    // The English-phrase spelling, for the resource whose shape is `SdiSource`
    // (GET /prod/sdiSources/{SdiSourceId}).
    Family {
        label: "SDI source",
        collection: "/prod/sdiSources",
        methods: &["GET", "PUT", "DELETE"],
    },
    Family {
        label: "Channel",
        collection: "/prod/channels",
        methods: &["GET", "DELETE"],
    },
    Family {
        label: "Input",
        collection: "/prod/inputs",
        methods: &["GET", "DELETE"],
    },
    Family {
        label: "Multiplex",
        collection: "/prod/multiplexes",
        methods: &["GET", "DELETE"],
    },
    Family {
        label: "Network",
        collection: "/prod/networks",
        methods: &["GET", "DELETE"],
    },
];

/// The body an update carries, so the request reaches the identifier resolution
/// instead of being refused for the members it names. A read and a delete carry
/// none.
fn body_for(family: &Family, method: &str) -> Option<Value> {
    match (family.collection, method) {
        ("/prod/inputSecurityGroups", "PUT") => {
            Some(json!({ "whitelistRules": [{ "cidr": "203.0.113.0/24" }] }))
        }
        ("/prod/sdiSources", "PUT") => Some(json!({ "name": "renamed-by-a-refused-update" })),
        _ => None,
    }
}

/// Rule 1: a refusal about a resource of a family names that family by ITS
/// label, whichever of the family's operations answered, and the label is the
/// family's own: the shape name for some families, an English phrase for others.
///
/// The rows are the label catalog crossed with the forms an identifier can take
/// — the numeric shape this service mints, a non-numeric one, and one carrying a
/// character an `@httpLabel` must percent-encode, which the message reports
/// decoded because it is composed from the request's own label value.
#[tokio::test]
async fn a_not_found_names_its_resource_family_with_that_familys_own_label() {
    let identifiers: [(&str, &str); 3] = [
        ("an unminted id of the shape this service mints", "9999999"),
        ("an identifier that is not numeric at all", "no-such-resource"),
        ("an identifier a label must percent-encode", "no such resource"),
    ];

    let router = medialive();
    for family in &FAMILIES {
        for method in family.methods {
            for (identifier_case, identifier) in identifiers {
                let case = format!(
                    "{method} {} with {identifier_case}",
                    family.collection
                );
                let (status, body) = call(
                    &router,
                    method,
                    &format!("{}/{}", family.collection, percent_encoded(identifier)),
                    body_for(family, method),
                )
                .await;

                assert_eq!(
                    status, 404,
                    "case {case:?}: expected HTTP 404, got {status}: {body}"
                );
                assert_eq!(
                    error_code(status, &body),
                    "NotFoundException",
                    "case {case:?}: expected the modelled NotFoundException, got {body}"
                );
                let expected = format!("{} {identifier} not found", family.label);
                assert_eq!(
                    error_message(&body),
                    expected,
                    "case {case:?}: expected the message {expected:?}, got {:?}",
                    error_message(&body)
                );
            }
        }
    }
}

/// Rule 1, from a second entry point: `BatchDelete` reports a resource it cannot
/// find through its own per-resource failure channel, and that entry uses the
/// family's label too — so a client cannot tell from the wording which entry
/// point it asked through.
///
/// The rows are the families `BatchDelete` takes ids of
/// (`smithy_query(query="operation", operation="BatchDelete")`: ChannelIds,
/// InputIds, InputSecurityGroupIds, MultiplexIds).
#[tokio::test]
async fn a_batch_reports_a_missing_resource_with_the_same_label() {
    let rows: [(&str, &str); 4] = [
        ("channelIds", "Channel"),
        ("inputIds", "Input"),
        ("inputSecurityGroupIds", "InputSecurityGroup"),
        ("multiplexIds", "Multiplex"),
    ];

    let router = medialive();
    for (member, label) in rows {
        let identifier = "9999999";
        let (status, body) = call(
            &router,
            "POST",
            "/prod/batch/delete",
            Some(json!({ member: [identifier] })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {member:?}: a batch reports each resource on its own, so the call \
             itself answers 200, got {status}: {body}"
        );
        assert_eq!(
            body["failed"],
            json!([{
                "id": identifier,
                "code": "NotFoundException",
                "message": format!("{label} {identifier} not found"),
            }]),
            "case {member:?}: expected the batch's failure entry to carry the family's \
             label, got {:?}",
            body["failed"]
        );
    }
}

/// Rule 2: `DescribeMultiplexProgram` resolves the two labels its URI names a
/// program by as ONE program lookup, so it reports the program —
/// `<multiplexId>/<programName>` — even when it is the enclosing multiplex that
/// does not exist; the program's write paths resolve that multiplex first, so an
/// absent multiplex is reported as the multiplex's own not-found there, as it is
/// for an operation addressed to the multiplex itself.
///
/// The rows differ along WHICH of the two labels names nothing (the multiplex,
/// the program, or both), along the form of the unresolvable value (a numeric id
/// nothing minted, a non-numeric one, a name another multiplex holds, a name a
/// label must percent-encode) and along the three operations the pair addresses.
#[tokio::test]
async fn a_multiplex_programs_not_found_names_what_the_operation_resolved() {
    let router = medialive();
    let live = multiplex(&router, "the live multiplex", "live").await;
    let (status, program) = create_program(
        &router,
        &live,
        "news",
        "token-news",
        program_settings(1),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected the program to be created, got {status}: {program}"
    );
    // A second multiplex holds a program of its own, so a request that asks the
    // first multiplex for THAT name is refused rather than served from the wrong
    // multiplex.
    let other = multiplex(&router, "the other multiplex", "other").await;
    let (status, program) = create_program(
        &router,
        &other,
        "elsewhere",
        "token-elsewhere",
        program_settings(1),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected the other multiplex's program to be created, got {status}: {program}"
    );

    // (case, the MultiplexId the request addresses, the ProgramName it
    //  addresses, whether that MultiplexId names the multiplex that exists)
    let rows: [(&str, &str, &str, bool); 6] = [
        ("a multiplex id nothing minted, naming a live program", "9999999", "news", false),
        ("a multiplex id that is not numeric", "no-such-multiplex", "news", false),
        ("neither label naming anything", "9999999", "no-such-program", false),
        ("a program name the live multiplex does not hold", &live, "no-such-program", true),
        ("a program name only the other multiplex holds", &live, "elsewhere", true),
        ("a program name a label must percent-encode", &live, "no such program", true),
    ];

    for (case, multiplex_id, program_name, multiplex_exists) in rows {
        // `live` is borrowed into the rows, so the id of the multiplex that
        // exists is spelled by the row itself.
        let encoded = percent_encoded(program_name);
        let program = format!("Multiplex program {multiplex_id}/{program_name} not found");
        // A write resolves the enclosing multiplex first, so it names the
        // program only when that multiplex is the one that exists.
        let written = if multiplex_exists {
            program.clone()
        } else {
            format!("Multiplex {multiplex_id} not found")
        };
        for (operation, expected, answered) in [
            (
                "DescribeMultiplexProgram",
                program.clone(),
                describe_program(&router, multiplex_id, &encoded).await,
            ),
            (
                "UpdateMultiplexProgram",
                written.clone(),
                update_program(&router, multiplex_id, &encoded, Some(program_settings(2))).await,
            ),
            (
                "DeleteMultiplexProgram",
                written.clone(),
                delete_program(&router, multiplex_id, &encoded).await,
            ),
        ] {
            let (status, body) = answered;
            let case = format!("{operation} on {case}");
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status}: {body}"
            );
            assert_eq!(
                error_code(status, &body),
                "NotFoundException",
                "case {case:?}: expected the modelled NotFoundException, got {body}"
            );
            assert_eq!(
                error_message(&body),
                expected,
                "case {case:?}: expected the message {expected:?}, got {:?}",
                error_message(&body)
            );
        }
    }

    // The contrast: an operation addressed to the MULTIPLEX, not to one of its
    // programs, names the multiplex alone — the program collection's listing and
    // the multiplex's own read (`smithy_query(query="operation",
    // operation="ListMultiplexPrograms,DescribeMultiplex")`: GET
    // /prod/multiplexes/{MultiplexId}/programs and GET
    // /prod/multiplexes/{MultiplexId}).
    for (case, uri) in [
        ("ListMultiplexPrograms", "/prod/multiplexes/9999999/programs"),
        ("DescribeMultiplex", "/prod/multiplexes/9999999"),
    ] {
        let (status, body) = call(&router, "GET", uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status}: {body}"
        );
        assert_eq!(
            error_message(&body),
            "Multiplex 9999999 not found",
            "case {case:?}: expected the refusal to name the multiplex it addressed, got {:?}",
            error_message(&body)
        );
    }

    // None of those refusals touched the programs that do exist.
    for (holder, name) in [(&live, "news"), (&other, "elsewhere")] {
        let (status, described) = describe_program(&router, holder, name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the stored program to still read back, got {status}: {described}"
        );
        assert_eq!(
            described["programName"],
            json!(name),
            "case {name:?}: expected the stored program to report its own name, got {described}"
        );
    }
}

/// Every REQUIRED member of `CreateCloudWatchAlarmTemplate` besides
/// `GroupIdentifier` (`smithy_query(query="operation",
/// operation="CreateCloudWatchAlarmTemplate")`), with one modelled value each,
/// so the create reaches its group resolution rather than being refused for the
/// members it omits.
fn template_members(name: &str) -> Value {
    json!({
        "name": name,
        "comparisonOperator": "GreaterThanOrEqualToThreshold",
        "evaluationPeriods": 1,
        "metricName": "InputVideoFrameRate",
        "period": 60,
        "statistic": "Average",
        "targetResourceType": "CLOUDFRONT_DISTRIBUTION",
        "threshold": 29.97,
        "treatMissingData": "notBreaching",
    })
}

/// Rule 3: a resource of the cloudwatch alarm-template hierarchy that an
/// identifier does not resolve to is reported with the 404 and the error code
/// alone — the `message` member states nothing — whichever operation resolved
/// it, while the response envelope stays complete.
///
/// The rows span both levels of the hierarchy (the group and the template), the
/// operations that address one directly (get, update, delete), the operations of
/// OTHER resources that resolve a group (a template create, a template listing's
/// filter, a signal-map listing's filter, a signal-map create) and the forms an
/// identifier takes: an id of the minted shape that nothing holds, a name
/// nothing holds, the `aws-` form an AWS-provided resource's id carries, and a
/// name a live resource of another family holds.
#[tokio::test]
async fn a_cloudwatch_alarm_template_not_found_states_no_message() {
    let router = medialive();
    let entry = entry_point(&router, "entry").await;

    // One live resource of ANOTHER family, whose name therefore names no
    // template group and no template.
    let (status, created) = call(
        &router,
        "POST",
        SIGNAL_MAPS,
        Some(json!({ "name": "live-map", "discoveryEntryPointArn": entry })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected the signal map to be created, got {status}: {created}"
    );

    let unresolvable: [(&str, &str); 4] = [
        // `__stringMin7Max11PatternAws097`, the shape both levels' ids target.
        ("an id of the minted shape that nothing holds", "9000000"),
        ("a name nothing holds", "no-such-resource"),
        ("an aws- prefixed id nothing holds", "aws-9000000"),
        ("the name of a live resource of another family", "live-map"),
    ];

    for (identifier_case, identifier) in unresolvable {
        let label = percent_encoded(identifier);
        let answers = [
            (
                "GetCloudWatchAlarmTemplateGroup",
                call(&router, "GET", &format!("{GROUPS}/{label}"), None).await,
            ),
            (
                "UpdateCloudWatchAlarmTemplateGroup",
                call(
                    &router,
                    "PATCH",
                    &format!("{GROUPS}/{label}"),
                    Some(json!({ "description": "mine now" })),
                )
                .await,
            ),
            (
                "DeleteCloudWatchAlarmTemplateGroup",
                call(&router, "DELETE", &format!("{GROUPS}/{label}"), None).await,
            ),
            (
                "GetCloudWatchAlarmTemplate",
                call(&router, "GET", &format!("{TEMPLATES}/{label}"), None).await,
            ),
            (
                "UpdateCloudWatchAlarmTemplate",
                call(
                    &router,
                    "PATCH",
                    &format!("{TEMPLATES}/{label}"),
                    Some(json!({ "description": "mine now" })),
                )
                .await,
            ),
            (
                "DeleteCloudWatchAlarmTemplate",
                call(&router, "DELETE", &format!("{TEMPLATES}/{label}"), None).await,
            ),
            (
                "CreateCloudWatchAlarmTemplate into a group nothing holds",
                {
                    let mut body = template_members(&format!("template-{}", identifier.len()));
                    body["groupIdentifier"] = json!(identifier);
                    call(&router, "POST", TEMPLATES, Some(body)).await
                },
            ),
            (
                "ListCloudWatchAlarmTemplates filtered by that group",
                call(
                    &router,
                    "GET",
                    &format!("{TEMPLATES}?groupIdentifier={label}"),
                    None,
                )
                .await,
            ),
            (
                "ListSignalMaps filtered by that group",
                call(
                    &router,
                    "GET",
                    &format!("{SIGNAL_MAPS}?cloudWatchAlarmTemplateGroupIdentifier={label}"),
                    None,
                )
                .await,
            ),
            (
                "CreateSignalMap associated with that group",
                call(
                    &router,
                    "POST",
                    SIGNAL_MAPS,
                    Some(json!({
                        "name": format!("map-{}", identifier.len()),
                        "discoveryEntryPointArn": entry,
                        "cloudWatchAlarmTemplateGroupIdentifiers": [identifier],
                    })),
                )
                .await,
            ),
        ];

        for (operation, (status, body)) in answers {
            let case = format!("{operation} with {identifier_case}");
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status}: {body}"
            );
            assert_eq!(
                error_code(status, &body),
                "NotFoundException",
                "case {case:?}: expected the modelled NotFoundException, got {body}"
            );
            assert!(
                body.get("message").is_none_or(Value::is_null),
                "case {case:?}: expected the refusal to state no message, got {:?}",
                body.get("message")
            );
            // Only `message` is absent: the envelope a client reads a failure by
            // is exactly as complete as on any other refusal.
            for member in ENVELOPE_MEMBERS {
                assert!(
                    body.get(member).is_some_and(|value| !value.is_null()),
                    "case {case:?}: expected the envelope member {member:?} beside the \
                     absent message, got {body}"
                );
            }
        }
    }

    // The contrast: the signal map is a resource of the same monitor API and is
    // NOT of the template hierarchy, so its own 404 states its message.
    let (status, body) = call(&router, "GET", &format!("{SIGNAL_MAPS}/no-such-map"), None).await;
    assert_eq!(status, 404, "expected HTTP 404, got {status}: {body}");
    assert_eq!(
        error_message(&body),
        "Signal map no-such-map not found",
        "expected the signal map's own refusal to state its message, got {:?}",
        error_message(&body)
    );
}
