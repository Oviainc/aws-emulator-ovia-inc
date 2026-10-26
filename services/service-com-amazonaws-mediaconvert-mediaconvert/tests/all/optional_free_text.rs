// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the OPTIONAL FREE-TEXT members the named MediaConvert
//! resources carry: a job template's and a preset's `Category` and `Description`, and a
//! queue's `Description`.
//!
//! The rule under test: a value that carries no characters carries no more information than
//! the member's absence, so a create stores it as ABSENT and an update that sends one
//! CLEARS the stored member — while a value that does carry characters is stored exactly as
//! sent, surrounding spaces included, because these members are free text and are not
//! normalised. Each one is "Optional" by its own documentation —
//! `smithy_query(query="operation",operation="CreateJobTemplate,CreatePreset,CreateQueue")`.
//!
//! Every case drives the real restJson1 wire path through the shared harness in
//! `job_lifecycle` — the router picks the service from the request's SigV4 scope, the
//! framework deserializes and validates, the handler runs, and the response is serialized —
//! so what is asserted is the bytes a client would see.

use serde_json::{json, Value};

use super::job_lifecycle::{Emulator, Reply};
use super::job_template_crud::settings as job_template_settings;

/// A structurally real `PresetSettings`: the encoder settings of one output, which must say
/// what container the output is written in (`ContainerSettings$Container`, "Container for
/// this output").
fn preset_settings() -> Value {
    json!({ "containerSettings": { "container": "MP4" } })
}

/// A JSON object holding one member, built without a literal key so the member under test
/// comes from the table.
fn one_member(member: &str, value: &str) -> Value {
    let mut body = Value::Object(serde_json::Map::new());
    body[member] = json!(value);
    body
}

/// One kind of resource that carries optional free text, and how to drive it over the wire.
struct Resource {
    label: &'static str,
    /// The collection the resource lives in, as the operation's `@http` URI spells it.
    collection: &'static str,
    /// The response member every operation on this resource answers with.
    view: &'static str,
    /// The optional free-text members the model declares on this resource.
    members: &'static [&'static str],
    /// The members a create of this resource cannot leave out, beside `Name`.
    required: fn() -> Value,
}

/// Every resource with such a member, so the rule is asserted across the whole class rather
/// than on the one operation a report happened to name. The member lists come from
/// `smithy_query(query="operation",operation="CreateJobTemplate,CreatePreset,CreateQueue")`:
/// a job template and a preset carry `Category` and `Description`, a queue carries
/// `Description` only.
fn resources() -> Vec<Resource> {
    vec![
        Resource {
            label: "job template",
            collection: "jobTemplates",
            view: "jobTemplate",
            members: &["category", "description"],
            required: || json!({ "settings": job_template_settings(1, 1) }),
        },
        Resource {
            label: "preset",
            collection: "presets",
            view: "preset",
            members: &["category", "description"],
            required: || json!({ "settings": preset_settings() }),
        },
        Resource {
            label: "queue",
            collection: "queues",
            view: "queue",
            members: &["description"],
            required: || json!({}),
        },
    ]
}

impl Resource {
    /// `POST /2017-08-29/<collection>`.
    async fn create(&self, emulator: &Emulator, name: &str, members: Value) -> Reply {
        let mut body = (self.required)();
        body["name"] = json!(name);
        for (member, value) in members.as_object().cloned().unwrap_or_default() {
            body[member] = value;
        }
        emulator
            .send("POST", &format!("/2017-08-29/{}", self.collection), Some(body))
            .await
    }

    /// `PUT /2017-08-29/<collection>/{Name}`.
    async fn update(&self, emulator: &Emulator, name: &str, members: Value) -> Reply {
        emulator
            .send(
                "PUT",
                &format!("/2017-08-29/{}/{name}", self.collection),
                Some(members),
            )
            .await
    }

    /// `GET /2017-08-29/<collection>/{Name}`.
    async fn get(&self, emulator: &Emulator, name: &str) -> Reply {
        emulator
            .send("GET", &format!("/2017-08-29/{}/{name}", self.collection), None)
            .await
    }

    /// `GET /2017-08-29/<collection>[?query]`, and the entries it answered with.
    async fn list(&self, emulator: &Emulator, query: &str) -> (Reply, Vec<Value>) {
        let uri = if query.is_empty() {
            format!("/2017-08-29/{}", self.collection)
        } else {
            format!("/2017-08-29/{}?{query}", self.collection)
        };
        let reply = emulator.send("GET", &uri, None).await;
        let entries = reply
            .at(&format!("/{}s", self.view))
            .as_array()
            .cloned()
            .unwrap_or_default();
        (reply, entries)
    }

    /// One member of this resource's view in a reply, where an absent member reads as
    /// `null` — what the wire shows for a member the store does not hold.
    fn member(&self, reply: &Reply, member: &str) -> Value {
        reply.at(&format!("/{}/{member}", self.view)).clone()
    }
}

/// One value a request may carry for a free-text member, and what the store must hold for
/// it afterwards: `None` for an absent member, `Some(value)` for the exact stored string.
struct Row {
    case: &'static str,
    sent: &'static str,
    stored: Option<&'static str>,
}

impl Row {
    /// What a read of the member must answer: the stored string, or `null` for a member the
    /// store does not hold.
    fn expected(&self) -> Value {
        self.stored.map_or(Value::Null, |value| json!(value))
    }
}

/// The table spans the class along what the rule generalises over: the FORM of a value that
/// carries no characters (none at all, one space, several, a tab, a newline, a mix) and, as
/// the controls, values that do carry characters — including ones whose characters sit
/// between spaces, which are stored verbatim because carrying no characters is the only
/// thing tested.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the empty string",
            sent: "",
            stored: None,
        },
        Row {
            case: "a single space",
            sent: " ",
            stored: None,
        },
        Row {
            case: "several spaces",
            sent: "   ",
            stored: None,
        },
        Row {
            case: "a tab",
            sent: "\t",
            stored: None,
        },
        Row {
            case: "a newline",
            sent: "\n",
            stored: None,
        },
        Row {
            case: "a mix of whitespace",
            sent: " \t\r\n ",
            stored: None,
        },
        Row {
            case: "one character",
            sent: "a",
            stored: Some("a"),
        },
        Row {
            case: "a plain word",
            sent: "nightly",
            stored: Some("nightly"),
        },
        Row {
            case: "characters with spaces around them, kept verbatim",
            sent: "  the nightly ladder  ",
            stored: Some("  the nightly ladder  "),
        },
        Row {
            case: "characters with spaces inside them",
            sent: "the nightly ladder",
            stored: Some("the nightly ladder"),
        },
    ]
}

/// A create that sends a free-text member carrying no characters stores no member, so the
/// create's own answer and every later read report it absent — while a value with characters
/// is stored exactly as sent.
#[tokio::test]
async fn a_blank_free_text_member_is_created_absent() {
    let emulator = Emulator::new();

    for resource in resources() {
        for &member in resource.members {
            for (index, row) in rows().iter().enumerate() {
                let label = format!("{} {member} on create: {}", resource.label, row.case);
                let name = format!("created-{}-{member}-{index}", resource.collection);
                let created = resource
                    .create(&emulator, &name, one_member(member, row.sent))
                    .await;
                assert_eq!(
                    created.status, 201,
                    "case {label:?}: expected HTTP 201, got {} with body {}",
                    created.status, created.body
                );
                assert_eq!(
                    resource.member(&created, member),
                    row.expected(),
                    "case {label:?}: expected {member} {:?} in the create reply, got {}",
                    row.stored,
                    created.body
                );

                let fetched = resource.get(&emulator, &name).await;
                assert_eq!(
                    fetched.status, 200,
                    "case {label:?}: expected HTTP 200 reading it back, got {} with body {}",
                    fetched.status, fetched.body
                );
                assert_eq!(
                    resource.member(&fetched, member),
                    row.expected(),
                    "case {label:?}: expected {member} {:?} read back, got {}",
                    row.stored,
                    fetched.body
                );
            }
        }
    }
}

/// An update that sends a free-text member carrying no characters CLEARS the stored one, so
/// a value an earlier call put there can be removed; a value with characters replaces it
/// verbatim. The sibling free-text member is left alone either way, because an update
/// changes only the members it names.
#[tokio::test]
async fn a_blank_free_text_member_is_cleared_by_an_update() {
    let emulator = Emulator::new();

    for resource in resources() {
        for &member in resource.members {
            // The member that must survive the update untouched: the other free-text member
            // this resource declares, where it has one.
            let untouched = resource
                .members
                .iter()
                .copied()
                .find(|other| *other != member);
            for (index, row) in rows().iter().enumerate() {
                let label = format!("{} {member} on update: {}", resource.label, row.case);
                let name = format!("updated-{}-{member}-{index}", resource.collection);
                let mut initial = one_member(member, "stored by the create");
                if let Some(other) = untouched {
                    initial[other] = json!("left alone");
                }
                let created = resource.create(&emulator, &name, initial).await;
                assert_eq!(
                    created.status, 201,
                    "case {label:?}: expected HTTP 201 creating it, got {} with body {}",
                    created.status, created.body
                );

                let updated = resource
                    .update(&emulator, &name, one_member(member, row.sent))
                    .await;
                assert_eq!(
                    updated.status, 200,
                    "case {label:?}: expected HTTP 200 updating it, got {} with body {}",
                    updated.status, updated.body
                );
                assert_eq!(
                    resource.member(&updated, member),
                    row.expected(),
                    "case {label:?}: expected {member} {:?} in the update reply, got {}",
                    row.stored,
                    updated.body
                );

                let fetched = resource.get(&emulator, &name).await;
                assert_eq!(
                    resource.member(&fetched, member),
                    row.expected(),
                    "case {label:?}: expected {member} {:?} read back, got {}",
                    row.stored,
                    fetched.body
                );
                if let Some(other) = untouched {
                    assert_eq!(
                        resource.member(&fetched, other),
                        json!("left alone"),
                        "case {label:?}: expected {other} to be untouched, got {}",
                        fetched.body
                    );
                }
            }
        }
    }
}

/// A listing projects the stored record, so a resource created with a category carrying no
/// characters reports no category there either — which is what makes the category filter's
/// assumption hold: a record with no category matches no category a caller can name.
#[tokio::test]
async fn a_listing_reports_a_blank_category_as_absent() {
    let emulator = Emulator::new();

    for resource in resources()
        .into_iter()
        .filter(|resource| resource.members.contains(&"category"))
    {
        let named = format!("{}-named", resource.collection);
        let blank = format!("{}-blank", resource.collection);
        for (name, category) in [(&named, "nightly"), (&blank, "   ")] {
            let created = resource
                .create(&emulator, name, one_member("category", category))
                .await;
            assert_eq!(
                created.status, 201,
                "case {} {name:?}: expected HTTP 201, got {} with body {}",
                resource.label, created.status, created.body
            );
        }

        let (listed, entries) = resource.list(&emulator, "").await;
        let blank_entry = entries
            .iter()
            .find(|entry| entry.pointer("/name").and_then(Value::as_str) == Some(blank.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case {}: {blank:?} must appear in the listing, got {}",
                    resource.label, listed.body
                )
            });
        assert_eq!(
            blank_entry.pointer("/category").unwrap_or(&Value::Null),
            &Value::Null,
            "case {}: expected no category on {blank:?} in the listing, got {blank_entry}",
            resource.label
        );

        let (filtered, entries) = resource.list(&emulator, "category=nightly").await;
        let names: Vec<&str> = entries
            .iter()
            .filter_map(|entry| entry.pointer("/name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            names,
            vec![named.as_str()],
            "case {}: expected only {named:?} under the named category, got {}",
            resource.label,
            filtered.body
        );
    }
}
