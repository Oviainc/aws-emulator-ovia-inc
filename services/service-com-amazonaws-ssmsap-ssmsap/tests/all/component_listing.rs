// SPDX-License-Identifier: CC0-1.0

//! `ListComponents`: the components of the registered applications, read back from state.
//!
//! The rule pinned here: every component a registration describes — one per
//! `ComponentsInfo` entry, plus the SAP HANA component of the SAP system its own `Sid`
//! names — is reported once by `ListComponents` as a
//! `ComponentSummary` carrying its application's id, the `ComponentId` the SID and type
//! it was declared with identify it by, that type, and its own ARN; the listing is scoped
//! to the Region the request was signed for and to the application it names when it names
//! one, it is paged by `MaxResults`/`NextToken` over the whole set, and an
//! `ApplicationId` naming no registration in that Region is refused with the 404
//! `ResourceNotFoundException` the operation models.
//!
//! The tables span what the rule generalises over: how many components a registration
//! declares (0, 1, 2, and the 5 the model's `ComponentInfoList` admits at most), which
//! `ComponentType`s they are (over the model's own catalogue, not one cited type), whether
//! a declaration repeats another, how many applications the Region holds, the Region the
//! call is signed for, and the page size.
//!
//! Every call goes through `MultiServiceRouter::dispatch` (the harness
//! `tests/all/application_lifecycle.rs` owns), so the restJson1 routing, the boundary
//! constraint walk, the handler and the response serialization all run exactly as they do
//! in the emulator binary. Nothing reaches into `crate::state`: a registration is only
//! believed when a later `ListComponents` / `GetApplication` call reports it back.

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::COMPONENT_TYPE_VALUES;
use service_com_amazonaws_ssmsap_ssmsap::state::{
    conditionally_required_members, ConditionalMember, MAX_COMPONENTS_PER_APPLICATION,
    MAX_COMPONENTS_PER_REGION, MAX_DECLARED_COMPONENTS, MAX_PAGE_SIZE,
};

use crate::application_lifecycle::{
    conditional_value, settle, valid_registration, Emulator, Reply,
};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// The regional endpoint most rows sign for.
const REGION: &str = "us-east-1";

/// A second regional endpoint, so the scoping rows have another store to be kept out of.
const OTHER_REGION: &str = "eu-west-1";

/// A Region nothing is registered in: a listing there is empty rather than another
/// Region's.
const UNUSED_REGION: &str = "ap-southeast-2";

/// The account every emulated ARN carries, from the core's single source of truth.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The modelled error code `ListComponents` declares for an application that is not
/// registered (`smithy_query(query="operation", operation="ListComponents")`:
/// `ResourceNotFoundException`, `@httpError 404`).
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

/// The most components one registration may DECLARE: the model gives `ComponentInfoList`
/// `@length(min: 0, max: 5)` (`smithy_query(query="shape", shape="ComponentInfoList")`),
/// which the crate re-states as `MAX_DECLARED_COMPONENTS`.
const MAX_COMPONENTS_PER_REGISTRATION: usize = MAX_DECLARED_COMPONENTS;

/// The `ComponentType` of the SAP HANA component of an application's own SAP system: one
/// of the model's `ComponentType` values (`smithy_query(query="shape",
/// shape="ComponentType")`).
const HANA: &str = "HANA";

/// The `Sid` a [`valid_registration`] of `app_type` carries, when that type requires one.
///
/// The builder carries every member the type conditionally requires
/// (`CONDITIONALLY_REQUIRED_MEMBERS`), so this is read off the same table rather than
/// written out again: a registration carrying a System ID describes the SAP HANA
/// component of that SAP system.
fn registration_sid(app_type: &str) -> Option<String> {
    conditionally_required_members(app_type)
        .iter()
        .any(|member| matches!(member, ConditionalMember::Sid))
        .then(|| {
            conditional_value(ConditionalMember::Sid)
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
}

/// One `ComponentSummary` a listing reported, reduced to the members the model gives it
/// (`smithy_query(query="shape", shape="ComponentSummary")`). `Tags` is read separately,
/// because its absence is its own assertion.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Summary {
    application_id: String,
    component_id: String,
    component_type: String,
    arn: String,
}

/// The `ComponentId` a component declared with `sid` and `component_type` is addressed
/// by: a component is identified by the SID and the type it was declared with, the EC2
/// instance naming only where it runs.
fn component_id(sid: &str, component_type: &str) -> String {
    format!("{sid}-{component_type}")
}

/// The ARN a component of `application_id` in `region` carries: a `ComponentId` is unique
/// only within its application, so the resource path carries both, under the same
/// `<kind>/<id>` form as the application's own ARN.
fn component_arn(region: &str, application_id: &str, component_id: &str) -> String {
    format!("arn:aws:ssm-sap:{region}:{ACCOUNT}:component/{application_id}/{component_id}")
}

/// What `ListComponents` must report for the component `(component_type, sid)` of
/// `application_id` registered in `region`.
fn expected_summary(
    region: &str,
    application_id: &str,
    component_type: &str,
    sid: &str,
) -> Summary {
    let component_id = component_id(sid, component_type);
    Summary {
        arn: component_arn(region, application_id, &component_id),
        application_id: application_id.to_string(),
        component_id,
        component_type: component_type.to_string(),
    }
}

/// The `ComponentSummary` list a reply reported, in the order reported.
fn listed(reply: &Reply) -> Vec<Summary> {
    reply
        .member("Components")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let read = |name: &str| {
                        item.get(name)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    Summary {
                        application_id: read("ApplicationId"),
                        component_id: read("ComponentId"),
                        component_type: read("ComponentType"),
                        arn: read("Arn"),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether any reported `ComponentSummary` carries `Tags`.
fn any_tags_reported(reply: &Reply) -> bool {
    reply
        .member("Components")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| item.get("Tags").is_some_and(|tags| !tags.is_null()))
        })
}

/// The continuation token a reply hands out, if it hands one out.
fn next_token(reply: &Reply) -> Option<String> {
    reply
        .member("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `Components` of a `GetApplication` reply: the component IDS of the application, in
/// the order the registration declared them.
fn application_component_ids(reply: &Reply) -> Vec<String> {
    reply
        .member("Application")
        .and_then(|application| application.get("Components"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// A `RegisterApplication` body for `id` declaring `components` as
/// `(ComponentType, Sid)` pairs, with every member the type requires.
///
/// Each declaration gets its OWN `Ec2InstanceId` (inside the modelled
/// `^i-[\w\d]{17}$`), so two declarations that agree on the SID and the type differ in
/// the member that does not identify a component.
fn registration(
    id: &str,
    app_type: &str,
    components: &[(&str, &str)],
    tags: &[(&str, &str)],
) -> Value {
    let mut body = valid_registration(id, app_type);
    let object = body.as_object_mut().expect("the body is a JSON object");
    if !components.is_empty() {
        object.insert(
            "ComponentsInfo".to_string(),
            Value::Array(
                components
                    .iter()
                    .enumerate()
                    .map(|(index, (component_type, sid))| {
                        json!({
                            "ComponentType": component_type,
                            "Sid": sid,
                            "Ec2InstanceId": format!("i-{index:017}"),
                        })
                    })
                    .collect(),
            ),
        );
    }
    if !tags.is_empty() {
        object.insert(
            "Tags".to_string(),
            Value::Object(
                tags.iter()
                    .map(|(key, value)| ((*key).to_string(), json!(value)))
                    .collect(),
            ),
        );
    }
    body
}

/// Register an application in `region`, asserting the registration itself succeeded, and
/// wait out the clock so the discovery that registration runs has produced the
/// application's components.
///
/// A component exists only once discovery has produced it, so a listing made while the
/// registration is still under way reports none — which the discovery-window rows of
/// `tests/all/component_discovery.rs` pin; every row below is about what the listing
/// reports once discovery has completed.
async fn register(
    emulator: &Emulator,
    region: &str,
    id: &str,
    app_type: &str,
    components: &[(&str, &str)],
    tags: &[(&str, &str)],
) -> Reply {
    let reply = emulator
        .send_in_region(
            region,
            "/register-application",
            registration(id, app_type, components, tags),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {region:?}/{id:?}: expected 200 from RegisterApplication, got {reply:?}"
    );
    settle().await;
    reply
}

/// One registration, and the components the API must then report for it.
#[derive(Debug)]
struct Case {
    id: &'static str,
    app_type: &'static str,
    /// `(ComponentType, Sid)` per `ComponentsInfo` entry, in declaration order.
    declared: &'static [(&'static str, &'static str)],
    tags: &'static [(&'static str, &'static str)],
}

/// The rows: 0, 1, 2 and the model's maximum of 5 declared components, both application
/// types, repeated declarations, and `ComponentType`s taken from the model's own
/// catalogue (`COMPONENT_TYPE_VALUES`) rather than from one cited type — including the
/// entries at the end of it, which no issue cites.
const CASES: &[Case] = &[
    // Nothing declared: the only component is the SAP HANA component the System ID of a
    // HANA registration names, the emulator discovering nothing of its own.
    Case {
        id: "HDB-TEST",
        app_type: "HANA",
        declared: &[],
        tags: &[("env", "dev")],
    },
    // One component, whose SID differs from the application's.
    Case {
        id: "A",
        app_type: "SAP_ABAP",
        declared: &[("ASCS", "PRD")],
        tags: &[],
    },
    // Three declarations of two components: the first two agree on SID and type, so they
    // describe ONE component, and the third shares the SID with another type.
    Case {
        id: "hana_99",
        app_type: "HANA",
        declared: &[("HANA", "HDB"), ("HANA", "HDB"), ("HANA_NODE", "HDB")],
        tags: &[("env", "prod"), ("owner", "sap-team")],
    },
    // The largest `ComponentsInfo` the model admits, over five of the catalogue's types,
    // with a SID at the other end of the `^[A-Z][A-Z0-9]{2}$` class.
    Case {
        id: "abap.prod_01",
        app_type: "SAP_ABAP",
        declared: &[
            ("ABAP", "ZZ9"),
            ("DIALOG", "ZZ9"),
            ("ERS", "ZZ9"),
            ("WD", "ZZ9"),
            ("WEBDISP", "ZZ9"),
        ],
        tags: &[],
    },
];

/// The summaries `ListComponents` must report for an application of `app_type`
/// registered in `region` as `application_id` declaring `declared`, in no particular
/// order: one per DISTINCT `(Sid, ComponentType)` declared, plus the SAP HANA component
/// of the application's own SAP system, which a registration naming a System ID
/// describes without declaring it.
fn expected_summaries(
    region: &str,
    application_id: &str,
    app_type: &str,
    declared: &[(&str, &str)],
) -> Vec<Summary> {
    let mut expected: Vec<Summary> = Vec::new();
    for (component_type, sid) in declared {
        let summary = expected_summary(region, application_id, component_type, sid);
        if !expected.contains(&summary) {
            expected.push(summary);
        }
    }
    if let Some(sid) = registration_sid(app_type) {
        let summary = expected_summary(region, application_id, HANA, &sid);
        if !expected.contains(&summary) {
            expected.push(summary);
        }
    }
    expected
}

impl Case {
    /// The summaries `ListComponents` must report for this registration, in the order a
    /// listing reports them (by `ComponentId`, within the one application).
    ///
    /// `Summary` compares by `ApplicationId` and then `ComponentId` (the order its
    /// members are declared in), so sorting the summaries IS the order the listing
    /// documents — it does not re-derive whatever key the handler pages by.
    fn expected(&self, region: &str) -> Vec<Summary> {
        let mut expected = expected_summaries(region, self.id, self.app_type, self.declared);
        expected.sort();
        expected
    }
}

/// RULE: every component a registration describes is listed once for its application, with
/// the id, type and ARN it was registered with — and the component ids `GetApplication`
/// reports are exactly the components the listing reports.
#[tokio::test]
async fn every_declared_component_is_listed_for_its_application() {
    for case in CASES {
        let emulator = Emulator::new();
        register(
            &emulator,
            REGION,
            case.id,
            case.app_type,
            case.declared,
            case.tags,
        )
        .await;
        let expected = case.expected(REGION);
        assert!(
            expected.len() <= MAX_COMPONENTS_PER_APPLICATION,
            "case {case:?}: a registration leaves an application with at most \
             {MAX_COMPONENTS_PER_APPLICATION} components, the row expects {}",
            expected.len()
        );

        // Named by its `ApplicationId`, and with `ApplicationId` omitted: this Region
        // holds one application, so both report the same components.
        for body in [json!({ "ApplicationId": case.id }), json!({})] {
            let reply = emulator
                .send_in_region(REGION, "/list-components", body.clone())
                .await;
            assert_eq!(
                reply.status, 200,
                "case {case:?} via {body}: expected 200 from ListComponents, got {reply:?}"
            );
            assert_eq!(
                listed(&reply),
                expected,
                "case {case:?} via {body}: expected {expected:?}, got {reply:?}"
            );
            assert_eq!(
                next_token(&reply),
                None,
                "case {case:?} via {body}: a listing that fits one page hands out no \
                 token, got {reply:?}"
            );
            // The tags a registration carried are the APPLICATION's, held under the
            // application's ARN; a component is tagged through its own ARN and nothing
            // has tagged one.
            assert!(
                !any_tags_reported(&reply),
                "case {case:?} via {body}: a component reports only its own tags, got \
                 {reply:?}"
            );
        }

        // The two readers of one record agree: `Application$Components` is the list of
        // component ids, in declaration order, and the listing reports exactly those
        // components.
        let fetched = emulator
            .send_in_region(REGION, "/get-application", json!({"ApplicationId": case.id}))
            .await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected 200 from GetApplication, got {fetched:?}"
        );
        let mut from_application = application_component_ids(&fetched);
        from_application.sort();
        let listed_ids: Vec<String> = expected
            .iter()
            .map(|summary| summary.component_id.clone())
            .collect();
        assert_eq!(
            from_application, listed_ids,
            "case {case:?}: GetApplication and ListComponents must report the same \
             components, got {from_application:?} and {listed_ids:?}"
        );
    }
}

/// RULE: the listing reports the components of the application it names and of no other,
/// of the Region it was signed for and of no other; an `ApplicationId` naming no
/// registration there is refused with the 404 `ResourceNotFoundException` the operation
/// models.
#[tokio::test]
async fn the_listing_is_scoped_to_the_named_application_and_to_its_region() {
    let emulator = Emulator::new();

    // Three applications in one Region, each with components of its own.
    for case in CASES.iter().filter(|case| !case.declared.is_empty()) {
        register(
            &emulator,
            REGION,
            case.id,
            case.app_type,
            case.declared,
            case.tags,
        )
        .await;
    }
    // And one in another Region reusing an id of the first: an id is unique per account
    // and Region, so this is a different application with different components.
    let shared = &CASES[1];
    register(
        &emulator,
        OTHER_REGION,
        shared.id,
        shared.app_type,
        &[("HANA", "QAS")],
        &[],
    )
    .await;

    // Named: exactly that application's components, in that Region.
    for case in CASES.iter().filter(|case| !case.declared.is_empty()) {
        let reply = emulator
            .send_in_region(REGION, "/list-components", json!({"ApplicationId": case.id}))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200 from ListComponents, got {reply:?}"
        );
        assert_eq!(
            listed(&reply),
            case.expected(REGION),
            "case {case:?}: expected only this application's components, got {reply:?}"
        );
    }

    // Unnamed: every component of the Region, ordered by application and then component.
    let mut expected_all: Vec<Summary> = CASES
        .iter()
        .filter(|case| !case.declared.is_empty())
        .flat_map(|case| case.expected(REGION))
        .collect();
    expected_all.sort();
    let reply = emulator
        .send_in_region(REGION, "/list-components", json!({}))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {REGION:?}: expected 200 from ListComponents, got {reply:?}"
    );
    assert_eq!(
        listed(&reply),
        expected_all,
        "case {REGION:?}: expected every component of the Region, got {reply:?}"
    );

    // The other Region reports ITS application's components, with its own ARNs — the
    // shared id resolves there to the registration made there.
    let reply = emulator
        .send_in_region(
            OTHER_REGION,
            "/list-components",
            json!({"ApplicationId": shared.id}),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {OTHER_REGION:?}: expected 200 from ListComponents, got {reply:?}"
    );
    assert_eq!(
        listed(&reply),
        vec![expected_summary(OTHER_REGION, shared.id, "HANA", "QAS")],
        "case {OTHER_REGION:?}: expected only this Region's component, got {reply:?}"
    );

    // A Region nothing was registered in holds no components, and says so with an empty
    // list rather than a refusal.
    let reply = emulator
        .send_in_region(UNUSED_REGION, "/list-components", json!({}))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {UNUSED_REGION:?}: expected 200 from ListComponents, got {reply:?}"
    );
    assert_eq!(
        listed(&reply),
        Vec::new(),
        "case {UNUSED_REGION:?}: expected an empty listing, got {reply:?}"
    );

    // An id naming no registration in the Region the call reached: the ids span the
    // modelled `ApplicationId` class (shortest, longest, every character class the
    // pattern admits) and include an application registered only in ANOTHER Region.
    let longest = "L".repeat(60);
    let unregistered: Vec<String> = vec![
        "NO-SUCH".to_string(),
        "a".to_string(),
        "app.1-2_3".to_string(),
        "0".to_string(),
        longest,
        // Registered in `REGION` only, so it is absent from the Region asked here.
        CASES[2].id.to_string(),
    ];
    for reference in unregistered {
        let reply = emulator
            .send_in_region(
                OTHER_REGION,
                "/list-components",
                json!({"ApplicationId": reference}),
            )
            .await;
        assert_eq!(
            reply.status, 404,
            "case {reference:?}: expected 404, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            RESOURCE_NOT_FOUND_EXCEPTION,
            "case {reference:?}: expected {RESOURCE_NOT_FOUND_EXCEPTION}, got {reply:?}"
        );
        let stated = message(&reply);
        let expected = format!("Application {reference} doesn't exist.");
        assert_eq!(
            stated, expected,
            "case {reference:?}: expected {expected:?}, got {stated:?}"
        );
    }
}

/// How many applications the paging rows register, and how many components each declares:
/// the counts differ per application so a page boundary falls inside one application for
/// some rows and between two for others.
const PAGING_SHAPE: &[usize] = &[4, 5, 3, 1];

/// RULE: a continuation token belongs to the listing that issued it — a token one listing
/// handed out is refused by the other rather than resumed from a key that means something
/// else there.
#[tokio::test]
async fn a_token_of_another_listing_is_refused_rather_than_resumed_from() {
    let emulator = Emulator::new();

    // Two applications, each with components, so both listings hold more than one page
    // at `MaxResults` 1 and therefore both hand out a token.
    for case in CASES.iter().filter(|case| !case.declared.is_empty()).take(2) {
        register(
            &emulator,
            REGION,
            case.id,
            case.app_type,
            case.declared,
            case.tags,
        )
        .await;
    }

    // Each listing's own token, and the other listing it must not be accepted by.
    for (issuer, other) in [
        ("/list-applications", "/list-components"),
        ("/list-components", "/list-applications"),
    ] {
        let reply = emulator
            .send_in_region(REGION, issuer, json!({"MaxResults": 1}))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {issuer:?}: expected 200, got {reply:?}"
        );
        let token = next_token(&reply)
            .unwrap_or_else(|| panic!("case {issuer:?}: expected a token, got {reply:?}"));

        // The issuer resumes from its own token.
        let resumed = emulator
            .send_in_region(REGION, issuer, json!({"NextToken": token}))
            .await;
        assert_eq!(
            resumed.status, 200,
            "case {issuer:?}: a listing resumes from its own token, got {resumed:?}"
        );

        // The other listing does not: the token names a position in a listing whose keys
        // mean something else.
        let reply = emulator
            .send_in_region(REGION, other, json!({"NextToken": token}))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {issuer:?} token at {other:?}: expected 400, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {issuer:?} token at {other:?}: expected {VALIDATION_EXCEPTION}, got \
             {reply:?}"
        );
        let stated = message(&reply);
        let expected = format!("NextToken {token} is not a valid pagination token");
        assert_eq!(
            stated, expected,
            "case {issuer:?} token at {other:?}: expected {expected:?}, got {stated:?}"
        );
    }
}

/// RULE: a registration declares at most [`MAX_COMPONENTS_PER_REGISTRATION`] components,
/// so one declaring more is refused as a 400 `ValidationException` and stores
/// nothing — while one declaring exactly that many is accepted.
///
/// Either refusal satisfies the rule: the bound is the MODEL's (`ComponentInfoList`
/// `@length(max: 5)`), so the boundary validator refuses the request before the handler's
/// own re-statement of it can; what the row pins is the status, the error shape and that
/// nothing was stored, not which check fired.
#[tokio::test]
async fn a_registration_declaring_more_components_than_allowed_is_refused() {
    // Declarations at the bound, and one and three past it. Types come from the model's
    // own catalogue, cycled so each entry names a distinct component.
    for count in [
        MAX_COMPONENTS_PER_REGISTRATION,
        MAX_COMPONENTS_PER_REGISTRATION + 1,
        MAX_COMPONENTS_PER_REGISTRATION + 3,
    ] {
        let emulator = Emulator::new();
        let id = "bound-case";
        let declared: Vec<(&str, &str)> = (0..count)
            .map(|index| {
                (
                    COMPONENT_TYPE_VALUES[index % COMPONENT_TYPE_VALUES.len()],
                    "PRD",
                )
            })
            .collect();
        let reply = emulator
            .send_in_region(
                REGION,
                "/register-application",
                registration(id, "SAP_ABAP", &declared, &[]),
            )
            .await;

        if count <= MAX_COMPONENTS_PER_REGISTRATION {
            assert_eq!(
                reply.status, 200,
                "case {count} components: a declaration at the bound is accepted, got \
                 {reply:?}"
            );
            // The declared components are the application's once the discovery the
            // registration runs has produced them.
            settle().await;
            let listed_components = emulator
                .send_in_region(REGION, "/list-components", json!({"ApplicationId": id}))
                .await;
            assert_eq!(
                listed(&listed_components).len(),
                count,
                "case {count} components: expected {count} listed, got \
                 {listed_components:?}"
            );
            continue;
        }

        assert_eq!(
            reply.status, 400,
            "case {count} components: expected 400, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {count} components: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );
        // And nothing was stored: the application does not exist, so a listing scoped to
        // it is refused as a missing application.
        let listed_components = emulator
            .send_in_region(REGION, "/list-components", json!({"ApplicationId": id}))
            .await;
        assert_eq!(
            listed_components.status, 404,
            "case {count} components: a refused registration stores nothing, got \
             {listed_components:?}"
        );
    }
}

/// The id of the `index`th application the paging rows register.
fn paging_id(index: usize) -> String {
    format!("page-app-{index}")
}

/// RULE: `MaxResults` bounds a page and `NextToken` resumes after it, so a caller that
/// follows the tokens to exhaustion sees every component of the Region exactly once, in
/// one order, whatever the page size.
#[tokio::test]
async fn paging_walks_every_component_exactly_once() {
    let emulator = Emulator::new();

    let mut expected_all: Vec<Summary> = Vec::new();
    for (index, count) in PAGING_SHAPE.iter().enumerate() {
        let id = paging_id(index);
        // Types taken from the model's own catalogue, so a type added to it widens these
        // rows without this test changing.
        let declared: Vec<(&str, &str)> = COMPONENT_TYPE_VALUES
            .iter()
            .cycle()
            .skip(index)
            .take(*count)
            .map(|component_type| (*component_type, "PRD"))
            .collect();
        assert!(
            *count <= MAX_COMPONENTS_PER_REGISTRATION,
            "case {id:?}: a registration declares at most \
             {MAX_COMPONENTS_PER_REGISTRATION} components, the row declares {count}"
        );
        register(&emulator, REGION, &id, "SAP_ABAP", &declared, &[]).await;
        for (component_type, sid) in &declared {
            expected_all.push(expected_summary(REGION, &id, component_type, sid));
        }
    }
    expected_all.sort();
    let total = expected_all.len();

    // Page sizes across the modelled `MaxResults` range (1..50): the smallest, two that
    // divide and do not divide the total, one larger than the total, the maximum, and the
    // request that names none (which takes the largest page the range allows).
    let sizes: Vec<Option<i32>> = vec![
        Some(1),
        Some(2),
        Some(5),
        Some(total as i32 + 1),
        Some(MAX_PAGE_SIZE),
        None,
    ];
    for max_results in sizes {
        let limit = max_results.unwrap_or(MAX_PAGE_SIZE) as usize;
        let mut seen: Vec<Summary> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = serde_json::Map::new();
            if let Some(max_results) = max_results {
                body.insert("MaxResults".to_string(), json!(max_results));
            }
            if let Some(token) = &token {
                body.insert("NextToken".to_string(), json!(token));
            }
            let reply = emulator
                .send_in_region(REGION, "/list-components", Value::Object(body))
                .await;
            assert_eq!(
                reply.status, 200,
                "case MaxResults={max_results:?} page {pages}: expected 200, got {reply:?}"
            );
            let page = listed(&reply);
            assert!(
                page.len() <= limit,
                "case MaxResults={max_results:?} page {pages}: a page holds at most \
                 {limit} items, got {}",
                page.len()
            );
            seen.extend(page);
            pages += 1;
            token = next_token(&reply);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= total + 1,
                "case MaxResults={max_results:?}: paging must terminate, got {pages} pages \
                 for {total} components"
            );
        }
        assert_eq!(
            seen, expected_all,
            "case MaxResults={max_results:?}: expected every component exactly once in \
             one order, got {seen:?}"
        );
        assert_eq!(
            pages,
            total.div_ceil(limit),
            "case MaxResults={max_results:?}: expected {} pages of at most {limit} of \
             {total} components, got {pages}",
            total.div_ceil(limit)
        );
    }

    // A token this service did not issue is refused rather than resumed from. It is 18
    // characters, so the modelled `NextToken` pattern (`^.{16,2048}$`) admits it and the
    // refusal is the handler's.
    let forged = "not-a-real-token!!";
    let reply = emulator
        .send_in_region(REGION, "/list-components", json!({"NextToken": forged}))
        .await;
    assert_eq!(
        reply.status, 400,
        "case {forged:?}: expected 400, got {reply:?}"
    );
    assert_eq!(
        reply.code(),
        VALIDATION_EXCEPTION,
        "case {forged:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
    );
    let stated = message(&reply);
    let expected = format!("NextToken {forged} is not a valid pagination token");
    assert_eq!(
        stated, expected,
        "case {forged:?}: expected {expected:?}, got {stated:?}"
    );
}

/// The longest `ApplicationId` the model admits: `@length(min: 1, max: 60)`
/// (`smithy_query(query="shape", shape="ApplicationId")`).
const MAX_APPLICATION_ID_LENGTH: usize = 60;

/// The `ApplicationType` the ordering rows register: one whose registration describes
/// both a component (the SAP HANA component of its own System ID) and a database, so the
/// same two applications are reported by the component listing AND by the sibling
/// database listing.
const ORDERING_APP_TYPE: &str = "HANA";

/// What each ordering row declares, as `(ComponentType, Sid)` pairs from the model's own
/// `ComponentType` catalogue: two components whose DECLARATION order is the reverse of
/// their `ComponentId` order (`<Sid>-<ComponentType>`), so the order the listing reports
/// cannot have come from the order the registration declared them in.
const ORDERING_DECLARED: &[(&str, &str)] = &[("HANA_NODE", "ZZ9"), ("ABAP", "AAA")];

/// Pairs of `ApplicationId`s in the order a listing must report them, spanning what the
/// ordering rule generalises over: the character the longer id of a prefix pair continues
/// with, the id length, and a pair where neither id is a prefix of the other.
///
/// The continuations are one per character class the modelled `ApplicationId` pattern
/// admits (`^[\w\d\.-]+$`, `smithy_query(query="shape", shape="ApplicationId")`): the `-`
/// and `.` it admits BELOW the `/` of a path, and the `_`, the digit and the letters of
/// either case it admits above it. A prefix pair's shorter id comes first whichever
/// character the longer one continues with, so a listing keyed on anything but the ids
/// themselves gets the `-` and `.` rows the wrong way round.
fn ordering_id_pairs() -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = ['-', '.', '_', '9', 'Z', 'a']
        .into_iter()
        .map(|continuation| ("app".to_string(), format!("app{continuation}")))
        .collect();
    // The shortest id the modelled length admits, and a prefix pair at the longest.
    pairs.push(("a".to_string(), "a-".to_string()));
    let longest_prefix = "L".repeat(MAX_APPLICATION_ID_LENGTH - 1);
    pairs.push((longest_prefix.clone(), format!("{longest_prefix}-")));
    // Neither id a prefix of the other: they differ at a character, `-` before `.`.
    pairs.push(("app-1".to_string(), "app.2".to_string()));
    pairs
}

/// Every component the Region reports, in the order reported, following the continuation
/// tokens to exhaustion at `max_results` items per page.
async fn every_component_of_the_region(
    emulator: &Emulator,
    max_results: Option<i32>,
    case: &str,
) -> Vec<Summary> {
    let mut seen: Vec<Summary> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut body = serde_json::Map::new();
        if let Some(max_results) = max_results {
            body.insert("MaxResults".to_string(), json!(max_results));
        }
        if let Some(token) = &token {
            body.insert("NextToken".to_string(), json!(token));
        }
        let reply = emulator
            .send_in_region(REGION, "/list-components", Value::Object(body))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case} page {pages}: expected 200 from ListComponents, got {reply:?}"
        );
        seen.extend(listed(&reply));
        pages += 1;
        token = next_token(&reply);
        if token.is_none() {
            return seen;
        }
        assert!(
            pages <= MAX_COMPONENTS_PER_REGION,
            "case {case}: paging must terminate, got {pages} pages holding {} components",
            seen.len()
        );
    }
}

/// The `ApplicationId`s the Region's database listing reports, in the order reported and
/// with repeats collapsed: which application that sibling listing puts first.
async fn application_order_of_the_database_listing(emulator: &Emulator, case: &str) -> Vec<String> {
    let reply = emulator
        .send_in_region(REGION, "/list-databases", json!({}))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected 200 from ListDatabases, got {reply:?}"
    );
    let mut order: Vec<String> = Vec::new();
    for database in reply
        .member("Databases")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let application_id = database
            .get("ApplicationId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if order.last() != Some(&application_id) {
            order.push(application_id);
        }
    }
    order
}

/// RULE: a listing keyed by a pair of ids is ordered by those ids themselves — here by
/// (`ApplicationId`, `ComponentId`) — so an id another id is a prefix of is reported
/// first whichever character the longer id continues with, the pages are the successive
/// runs of that one order, and the Region's other listings of the same applications agree
/// about which application comes first.
#[tokio::test]
async fn the_listing_is_ordered_by_application_id_then_component_id() {
    for (first, second) in ordering_id_pairs() {
        let case = format!("{first:?} before {second:?}");
        let emulator = Emulator::new();
        // The id that must be reported SECOND is registered FIRST, so neither the order
        // of the listing nor its grouping into pages can come from the order the
        // applications were registered in.
        for id in [&second, &first] {
            register(&emulator, REGION, id, ORDERING_APP_TYPE, ORDERING_DECLARED, &[]).await;
        }

        let mut expected: Vec<Summary> = [&first, &second]
            .into_iter()
            .flat_map(|id| expected_summaries(REGION, id, ORDERING_APP_TYPE, ORDERING_DECLARED))
            .collect();
        expected.sort();
        assert_eq!(
            expected.first().map(|summary| summary.application_id.clone()),
            Some(first.clone()),
            "case {case}: the row expects {first:?} to be reported first, got {expected:?}"
        );

        // Every page size from one component per page to more than the listing holds,
        // plus the request that names none: each reports the same components, once each,
        // in that one order.
        for max_results in [Some(1), Some(2), Some(3), Some(MAX_PAGE_SIZE), None] {
            let reported = every_component_of_the_region(&emulator, max_results, &case).await;
            assert_eq!(
                reported, expected,
                "case {case} at {max_results:?} per page: expected {expected:?}, got \
                 {reported:?}"
            );
        }

        // The sibling listing of the same Region, keyed by (ApplicationId, DatabaseId),
        // puts the same application first.
        let databases = application_order_of_the_database_listing(&emulator, &case).await;
        assert_eq!(
            databases,
            vec![first.clone(), second.clone()],
            "case {case}: the component and database listings of one Region must agree \
             about which application comes first, got {databases:?}"
        );
    }
}
