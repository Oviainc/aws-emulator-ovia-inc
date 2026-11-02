// SPDX-License-Identifier: CC0-1.0

//! Which ARNs this service RESOLVES, driven over the wire through the calls that take
//! one.
//!
//! The rule pinned here: an ARN names a resource of this account and Region exactly when
//! it is the ARN of a registered application or of one of the resources derived from that
//! registration — its SAP HANA databases and its components — so every ARN a response of
//! this service hands a caller is an ARN the ARN-keyed TAGGING calls accept
//! (`ListTagsForResource`, `TagResource`, `UntagResource`), what is held under it is the
//! same set every read of that resource reports, and only an ARN naming none of those
//! resources is refused as not found (HTTP 404). The
//! resource-permission calls are narrower — they are database-scoped — so a component ARN
//! is no target for them whether or not it resolves, which is the other half of the rule
//! this suite pins.
//!
//! The component is the resource kind this suite spans, because it is the one no table
//! covered: the sibling suites (`tests/all/resource_tags.rs`, `tests/all/tag_resource.rs`,
//! `tests/all/untag_resource.rs`, `tests/all/resource_permissions.rs`) span the
//! application and the database. The rows differ along what the rule generalises over:
//! how many components a registration describes, how it describes them (declared in
//! `ComponentsInfo`, derived from the registration's own `Sid`, or both for the same
//! component), which `ComponentType`s they are, the tag count, key form across the
//! modelled `TagKey` class and value length at a modelled bound, and the ways a
//! component ARN can name nothing (a component never described, a component of an
//! application never registered, another Region, a deregistered application).
//!
//! Every call goes through `MultiServiceRouter::dispatch` (the harness
//! `tests/all/application_lifecycle.rs` owns), so the restJson1 routing, the boundary
//! constraint walk, the handler and the response serialization all run exactly as they do
//! in the emulator binary. Nothing reaches into `crate::state` directly: the ARNs the
//! cases name are the ARNs `ListComponents` itself reported, and a tag is only believed
//! when a later read reports it back.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::state::{ConditionalMember, RESOURCE_NOT_FOUND_MESSAGE};

use crate::application_lifecycle::{
    conditional_value, settle, tag_pairs, valid_registration, Emulator, Reply,
};
use crate::resource_permissions::{assert_invalid_database_arn, RESOURCE_ARN_MEMBER};
use crate::resource_tags::{
    list_tags, list_tags_in_region, register, TAG_KEY_FORMS, TAG_VALUE_MAX_LENGTH,
};
use crate::tag_resource::{pairs, tag, tag_in_region};
use crate::untag_resource::{untag, untag_in_region};
use crate::validation_message_rendering::message;

/// The region the suite signs for; it is the region every expected ARN carries.
const REGION: &str = "us-east-1";

/// A second regional endpoint, so a component can be addressed at a Region its
/// application was not registered in.
const OTHER_REGION: &str = "us-west-2";

/// The account every emulated ARN carries, from the core's single source of truth.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The error code and status an ARN naming no resource is answered with by the
/// ARN-keyed tagging routes: the class those routes really carry, which is NOT the
/// modelled `ResourceNotFoundException` shape name.
const TAGGING_NOT_FOUND_CODE: &str =
    service_com_amazonaws_ssmsap_ssmsap::scaffolded::NOT_FOUND_CODE;

/// The `ComponentType` of the SAP HANA component of an application's own SAP system: one
/// of the model's `ComponentType` values (`smithy_query(query="shape",
/// shape="ComponentType")`).
const HANA: &str = "HANA";

/// The Amazon EC2 instance every declared component of these rows runs on — the instance
/// [`valid_registration`] registers the application against, so the registration names
/// no instance a component is not on.
const INSTANCE: &str = "i-0123456789abcdef0";

/// A source resource ARN of another service entirely: a source is the counterparty a
/// restore is permitted for, so it need not be a resource of this service at all.
const FOREIGN_SOURCE: &str = "arn:aws:s3:::a-bucket/an-object";

/// The only `PermissionActionType` the model declares
/// (`smithy_query(query="shape", shape="PermissionActionType")`: `RESTORE`).
const RESTORE: &str = "RESTORE";

/// The `Sid` a `valid_registration` of a HANA application carries, read off the same
/// table the builder fills it from, so the component the registration's own System ID
/// describes is named here the way the registration describes it.
fn registration_sid() -> String {
    conditional_value(ConditionalMember::Sid)
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `ComponentId` of a component declared with `sid` and `component_type`: a component
/// is identified by the System ID and the type it was declared with, the EC2 instance
/// naming only where it runs.
fn component_id(sid: &str, component_type: &str) -> String {
    format!("{sid}-{component_type}")
}

/// The ARN a component of `application_id` in `region` carries: a `ComponentId` is unique
/// only within its application, so the resource path carries both, under the same
/// `<kind>/<id>` form as the application's own ARN.
fn component_arn(region: &str, application_id: &str, component_id: &str) -> String {
    format!("arn:aws:ssm-sap:{region}:{ACCOUNT}:component/{application_id}/{component_id}")
}

/// A `RegisterApplication` body for a HANA application whose `ComponentsInfo` declares
/// one entry per `(component_type, sid)` of `declared`.
fn registration_declaring(application_id: &str, declared: &[(&str, &str)]) -> Value {
    let mut body = valid_registration(application_id, "HANA");
    let members = body
        .as_object_mut()
        .expect("a registration body is a JSON object");
    if !declared.is_empty() {
        let entries: Vec<Value> = declared
            .iter()
            .map(|(component_type, sid)| {
                json!({
                    "ComponentType": component_type,
                    "Sid": sid,
                    "Ec2InstanceId": INSTANCE,
                })
            })
            .collect();
        members.insert("ComponentsInfo".to_string(), Value::Array(entries));
    }
    body
}

/// One `ComponentSummary` a listing reported, reduced to what these cases read.
#[derive(Debug)]
struct Listed {
    id: String,
    arn: String,
    tags: BTreeMap<String, String>,
}

/// The components `ListComponents` reports for `application_id`, in the order reported,
/// asserting the listing itself was served.
async fn list_components(emulator: &Emulator, case: &str, application_id: &str) -> Vec<Listed> {
    let reply = emulator
        .send("/list-components", json!({"ApplicationId": application_id}))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from ListComponents, got {reply:?}"
    );
    reply
        .member("Components")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| Listed {
                    id: item
                        .get("ComponentId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    arn: item
                        .get("Arn")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    tags: tag_pairs(item.get("Tags").filter(|value| !value.is_null())),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `Tags` `GetComponent` reports for one component, asserting the read was served.
async fn component_tags(
    emulator: &Emulator,
    case: &str,
    application_id: &str,
    component_id: &str,
) -> BTreeMap<String, String> {
    let reply = emulator
        .send(
            "/get-component",
            json!({"ApplicationId": application_id, "ComponentId": component_id}),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from GetComponent, got {reply:?}"
    );
    tag_pairs(reply.member("Tags"))
}

/// The tag set `ListTagsForResource` reports for `resource_arn`, asserting the read was
/// served.
async fn tags_held(
    emulator: &Emulator,
    case: &str,
    resource_arn: &str,
) -> BTreeMap<String, String> {
    let reply = list_tags(emulator, resource_arn).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from ListTagsForResource for {resource_arn:?}, got \
         {reply:?}"
    );
    tag_pairs(reply.member("tags"))
}

/// Assert `reply` is the not-found refusal the tagging routes carry for an ARN that names
/// nothing: the fixed sentence, which reports nothing of the ARN.
fn assert_not_found(case: &str, arn: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected 404 for {arn:?}, got {reply:?}"
    );
    assert_eq!(
        reply.code(),
        TAGGING_NOT_FOUND_CODE,
        "case {case:?}: expected {TAGGING_NOT_FOUND_CODE} for {arn:?}, got {:?}",
        reply.code()
    );
    let stated = message(reply);
    assert_eq!(
        stated, RESOURCE_NOT_FOUND_MESSAGE,
        "case {case:?}: expected {RESOURCE_NOT_FOUND_MESSAGE:?} for {arn:?}, got {stated:?}"
    );
}

/// One row: a registration, the components it describes, and the tags each of those
/// components is tagged with through its own ARN.
struct Case {
    /// How an assertion names the row.
    name: &'static str,
    application_id: &'static str,
    /// The `(ComponentType, Sid)` entries the registration DECLARES in `ComponentsInfo`.
    declared: &'static [(&'static str, &'static str)],
    /// The tags the row attaches to every component of its registration.
    tags: Vec<(String, String)>,
}

impl Case {
    /// The `ComponentId`s the registration describes: one per declared entry, each once,
    /// plus the SAP HANA component of the SAP system its own `Sid` names when nothing
    /// declared that one already.
    fn expected_component_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        for (component_type, sid) in self.declared {
            let id = component_id(sid, component_type);
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        let own = component_id(&registration_sid(), HANA);
        if !ids.contains(&own) {
            ids.push(own);
        }
        ids
    }

    fn expected_tags(&self) -> BTreeMap<String, String> {
        self.tags.iter().cloned().collect()
    }
}

/// The rows: component count, how the registration describes each component, the
/// `ComponentType`s, and the tag set attached through the component's own ARN.
fn cases() -> Vec<Case> {
    vec![
        // The component the registration's own System ID describes, and nothing else:
        // the shape a minimal HANA registration has.
        Case {
            name: "the SAP HANA component of the registration's own SAP system",
            application_id: "comp-arn-own",
            declared: &[],
            tags: pairs(&[("env", "dev")]),
        },
        // One declared component beside the derived one, so a row covers both ways a
        // registration describes a component at once.
        Case {
            name: "one declared component beside the derived one",
            application_id: "comp-arn-declared",
            declared: &[("ABAP", "AB1")],
            tags: pairs(&[("env", "prod"), ("owner", "sap-team"), ("tier", "1")]),
        },
        // Three declared components spanning more of the modelled `ComponentType`
        // catalogue, each tagged with keys drawn from the whole modelled `TagKey` class.
        Case {
            name: "three declared components of three types",
            application_id: "comp-arn-many",
            declared: &[("ASCS", "AS1"), ("ERS", "ER1"), ("WEBDISP", "WD1")],
            tags: TAG_KEY_FORMS
                .iter()
                .enumerate()
                .map(|(index, key)| ((*key).to_string(), format!("value-{index}")))
                .collect(),
        },
        // A declaration OF the component the registration's own `Sid` already describes:
        // one component, described twice, so its ARN resolves once.
        Case {
            name: "the derived component declared explicitly",
            application_id: "comp-arn-redeclared",
            declared: &[("HANA", "HDB")],
            tags: vec![
                ("shortest".to_string(), "x".to_string()),
                ("longest".to_string(), "v".repeat(TAG_VALUE_MAX_LENGTH)),
            ],
        },
    ]
}

/// RULE: the ARN this service reports for a component names a resource this Region holds,
/// so the ARN-keyed tagging calls serve it — the read reports the empty set for an
/// untagged component, a write persists what it carries under that ARN, every read of
/// that component reports the same set, and a delete removes it.
#[tokio::test]
async fn the_component_arn_this_service_reports_is_one_the_tagging_calls_serve() {
    let emulator = Emulator::new();

    for case in cases() {
        register(
            &emulator,
            case.name,
            registration_declaring(case.application_id, case.declared),
        )
        .await;

        let listed = list_components(&emulator, case.name, case.application_id).await;
        let reported_ids: Vec<String> = listed.iter().map(|item| item.id.clone()).collect();
        let mut expected_ids = case.expected_component_ids();
        expected_ids.sort();
        let mut sorted_ids = reported_ids.clone();
        sorted_ids.sort();
        assert_eq!(
            sorted_ids, expected_ids,
            "case {:?}: expected the described components {expected_ids:?}, got \
             {reported_ids:?}",
            case.name
        );

        for item in &listed {
            let label = format!("{}/{}", case.name, item.id);
            // The ARN the listing itself reported, never one built here.
            assert_eq!(
                item.arn,
                component_arn(REGION, case.application_id, &item.id),
                "case {label:?}: expected the component's own ARN, got {:?}",
                item.arn
            );

            // Nothing has tagged it yet, so the read is the EMPTY set — not a refusal.
            let before = tags_held(&emulator, &label, &item.arn).await;
            assert!(
                before.is_empty(),
                "case {label:?}: nothing tagged the component, expected an empty tag set, \
                 got {before:?}"
            );

            let reply = tag(&emulator, &item.arn, &case.tags).await;
            assert_eq!(
                reply.status, 200,
                "case {label:?}: expected 200 from TagResource for {:?}, got {reply:?}",
                item.arn
            );

            let held = tags_held(&emulator, &label, &item.arn).await;
            assert_eq!(
                held,
                case.expected_tags(),
                "case {label:?}: expected the tags the call carried {:?}, got {held:?}",
                case.expected_tags()
            );

            // One source of truth: what the tagging read reports is what every other read
            // of the same component reports.
            let by_get = component_tags(&emulator, &label, case.application_id, &item.id).await;
            assert_eq!(
                by_get, held,
                "case {label:?}: expected GetComponent to report the same tags {held:?}, got \
                 {by_get:?}"
            );
        }

        // And the listing reports each component's own set, now that they are tagged.
        let relisted = list_components(&emulator, case.name, case.application_id).await;
        for item in &relisted {
            assert_eq!(
                item.tags,
                case.expected_tags(),
                "case {:?}/{}: expected ListComponents to report the component's own tags \
                 {:?}, got {:?}",
                case.name,
                item.id,
                case.expected_tags(),
                item.tags
            );
        }

        // Deleting through the same ARN leaves the component reachable and untagged.
        let keys: Vec<String> = case.tags.iter().map(|(key, _)| key.clone()).collect();
        for item in &listed {
            let label = format!("{}/{}", case.name, item.id);
            let reply = untag(&emulator, &item.arn, &keys).await;
            assert_eq!(
                reply.status, 200,
                "case {label:?}: expected 200 from UntagResource, got {reply:?}"
            );
            let left = tags_held(&emulator, &label, &item.arn).await;
            assert!(
                left.is_empty(),
                "case {label:?}: expected every key named to be gone, got {left:?}"
            );
        }
    }
}

/// RULE: a component's tag set belongs to the COMPONENT its ARN names, so tagging one
/// component changes neither its siblings nor the application and databases of the same
/// registration.
#[tokio::test]
async fn a_component_holds_its_own_tag_set() {
    let emulator = Emulator::new();

    register(
        &emulator,
        "siblings",
        registration_declaring("comp-arn-siblings", &[("ASCS", "AS1"), ("ERS", "ER1")]),
    )
    .await;
    let listed = list_components(&emulator, "siblings", "comp-arn-siblings").await;
    assert!(
        listed.len() >= 2,
        "case \"siblings\": expected the registration to describe several components, got \
         {listed:?}"
    );

    // One tag per component, each naming the component it was attached to, so a set
    // reported for the wrong resource is visible in the value.
    for item in &listed {
        let tags = pairs(&[("component", item.id.as_str())]);
        let reply = tag(&emulator, &item.arn, &tags).await;
        assert_eq!(
            reply.status, 200,
            "case \"siblings\"/{}: expected 200 from TagResource, got {reply:?}",
            item.id
        );
    }

    for item in &listed {
        let label = format!("siblings/{}", item.id);
        let held = tags_held(&emulator, &label, &item.arn).await;
        let expected: BTreeMap<String, String> =
            [("component".to_string(), item.id.clone())].into_iter().collect();
        assert_eq!(
            held, expected,
            "case {label:?}: expected only this component's own tag {expected:?}, got {held:?}"
        );
    }

    // The application of the same registration holds none of them.
    let application = format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:application/comp-arn-siblings");
    let held = tags_held(&emulator, "siblings/application", &application).await;
    assert!(
        held.is_empty(),
        "case \"siblings/application\": a component's tags are not the application's, \
         expected an empty tag set, got {held:?}"
    );
}

/// RULE: the resource-permission calls are database-scoped, so a component ARN is no
/// target for them — even the ARN of a component this Region does hold is refused with
/// the modelled `ValidationException` (HTTP 400) stating that it is not a valid database
/// ARN, and nothing is granted or reported for it.
#[tokio::test]
async fn a_component_arn_is_no_resource_permission_target() {
    let emulator = Emulator::new();

    register(
        &emulator,
        "a permission target",
        registration_declaring("comp-arn-perm", &[("WEBDISP", "WD1")]),
    )
    .await;
    let listed = list_components(&emulator, "a permission target", "comp-arn-perm").await;
    let source = format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:database/comp-arn-perm/HDB");

    for item in &listed {
        let label = format!("permission on {}", item.id);
        let granted = emulator
            .send(
                "/put-resource-permission",
                json!({
                    "ActionType": RESTORE,
                    "ResourceArn": item.arn,
                    "SourceResourceArn": source,
                }),
            )
            .await;
        assert_invalid_database_arn(
            &granted,
            Some(RESOURCE_ARN_MEMBER),
            &item.arn,
            &format!("{label} (PutResourcePermission)"),
        );

        let read = emulator
            .send(
                "/get-resource-permission",
                json!({"ActionType": RESTORE, "ResourceArn": item.arn}),
            )
            .await;
        assert_invalid_database_arn(
            &read,
            None,
            &item.arn,
            &format!("{label} (GetResourcePermission)"),
        );

        // The component itself still resolves for the calls it IS a resource of: the
        // refusal is about the kind of ARN the permission calls take, not about the
        // component having stopped existing.
        let held = tags_held(&emulator, &label, &item.arn).await;
        assert!(
            held.is_empty(),
            "case {label:?}: nothing tagged the component, expected an empty tag set, got \
             {held:?}"
        );
    }
}

/// RULE: an ARN naming no resource of this account and Region is refused by the tagging
/// routes with their own not-found class (HTTP 404) and its fixed sentence — so a
/// component ARN whose component no registration describes is refused by every ARN-keyed
/// call, while one that does resolve is served.
#[tokio::test]
async fn a_component_arn_naming_nothing_here_is_refused_as_not_found() {
    let emulator = Emulator::new();

    register(
        &emulator,
        "a registered application",
        registration_declaring("comp-arn-present", &[("ASCS", "AS1")]),
    )
    .await;
    register(
        &emulator,
        "an application to deregister",
        registration_declaring("comp-arn-gone", &[]),
    )
    .await;
    let derived = component_id(&registration_sid(), HANA);
    // Tag the component of the application about to be deregistered: what is held for a
    // component cannot outlive the registration the component came from.
    let gone_arn = component_arn(REGION, "comp-arn-gone", &derived);
    let reply = tag(&emulator, &gone_arn, &pairs(&[("env", "dev")])).await;
    assert_eq!(
        reply.status, 200,
        "case \"a component of a live registration\": expected 200 from TagResource, got \
         {reply:?}"
    );
    let reply = emulator
        .send(
            "/deregister-application",
            json!({"ApplicationId": "comp-arn-gone"}),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from DeregisterApplication, got {reply:?}"
    );
    // Deregistration is asynchronous: the component and what is held for it go when the
    // removal the clock owes the registration falls due.
    settle().await;

    // (label, the region the call is made at, the ARN it names)
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "a component the registration never described",
            REGION,
            component_arn(REGION, "comp-arn-present", "ZZ1-ERS"),
        ),
        (
            "a component id of the right form of an unregistered application",
            REGION,
            component_arn(REGION, "comp-arn-never-registered", &derived),
        ),
        (
            "a component of an application that was deregistered",
            REGION,
            gone_arn.clone(),
        ),
        (
            "a component read at another regional endpoint",
            OTHER_REGION,
            component_arn(REGION, "comp-arn-present", "AS1-ASCS"),
        ),
        (
            "a component ARN carrying another Region",
            REGION,
            component_arn(OTHER_REGION, "comp-arn-present", "AS1-ASCS"),
        ),
    ];

    for (label, region, arn) in rows {
        let listed = list_tags_in_region(&emulator, region, &arn).await;
        assert_not_found(&format!("{label} (ListTagsForResource)"), &arn, &listed);

        let tagged = tag_in_region(&emulator, region, &arn, &pairs(&[("env", "dev")])).await;
        assert_not_found(&format!("{label} (TagResource)"), &arn, &tagged);

        let untagged = untag_in_region(&emulator, region, &arn, &["env".to_string()]).await;
        assert_not_found(&format!("{label} (UntagResource)"), &arn, &untagged);

        let granted = emulator
            .send_in_region(
                region,
                "/put-resource-permission",
                json!({
                    "ActionType": RESTORE,
                    "ResourceArn": arn,
                    "SourceResourceArn": FOREIGN_SOURCE,
                }),
            )
            .await;
        assert_invalid_database_arn(
            &granted,
            Some(RESOURCE_ARN_MEMBER),
            &arn,
            &format!("{label} (PutResourcePermission)"),
        );
    }

    // The component that DOES resolve is served at the Region it was registered in, so
    // the refusals above are about what the ARN names and not about the route.
    let present = component_arn(REGION, "comp-arn-present", "AS1-ASCS");
    let held = tags_held(&emulator, "a component that resolves", &present).await;
    assert!(
        held.is_empty(),
        "case \"a component that resolves\": nothing tagged it, expected an empty tag set, \
         got {held:?}"
    );
}

/// RULE: what is held per resource cannot outlive the registration that resource came
/// from, so a tag attached to a component is gone once the deregistration the clock owes
/// its application falls due — a registration made again under the same id starts with
/// nothing held for its components.
#[tokio::test]
async fn tags_of_a_component_go_with_its_registration() {
    let emulator = Emulator::new();

    let application_id = "comp-arn-recycled";
    let component = component_id(&registration_sid(), HANA);
    let arn = component_arn(REGION, application_id, &component);

    register(
        &emulator,
        "the first registration",
        registration_declaring(application_id, &[]),
    )
    .await;
    let reply = tag(&emulator, &arn, &pairs(&[("env", "dev"), ("owner", "sap")])).await;
    assert_eq!(
        reply.status, 200,
        "case \"the first registration\": expected 200 from TagResource, got {reply:?}"
    );
    let held = tags_held(&emulator, "the first registration", &arn).await;
    assert_eq!(
        held.len(),
        2,
        "case \"the first registration\": expected the two tags attached, got {held:?}"
    );

    let reply = emulator
        .send(
            "/deregister-application",
            json!({"ApplicationId": application_id}),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from DeregisterApplication, got {reply:?}"
    );
    settle().await;

    let refused = list_tags(&emulator, &arn).await;
    assert_not_found("after the deregistration", &arn, &refused);

    // Registered again under the same id, the component resolves again — and holds
    // nothing, because the tags went with the registration they were attached under.
    register(
        &emulator,
        "the second registration",
        registration_declaring(application_id, &[]),
    )
    .await;
    let held = tags_held(&emulator, "the second registration", &arn).await;
    assert!(
        held.is_empty(),
        "case \"the second registration\": expected no tag to have outlived the first \
         registration, got {held:?}"
    );
}
