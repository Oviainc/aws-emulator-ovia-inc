// SPDX-License-Identifier: CC0-1.0

//! The artifact-list members of an action declaration, end to end over the wire the
//! service speaks (`awsJson1_1`).
//!
//! The rule under test: a pipeline structure this service REPORTS carries
//! `inputArtifacts` and `outputArtifacts` on every action — an empty list where the
//! action declares none — so a caller reads both members unconditionally on every
//! action of every structure `CreatePipeline`, `UpdatePipeline` and `GetPipeline`
//! answer with. `ActionDeclaration$inputArtifacts` and `$outputArtifacts` target
//! lists with no minimum length, so "consumes nothing" is the empty list, not the
//! absent member.
//!
//! The table spans what the rule generalises over: which of the two members the
//! request declares (neither, one, the other, both, each declared as an explicitly
//! empty array), how many names each carries, and how many actions and stages the
//! declaration has. Every case is driven through the router and read back from the
//! store, so nothing here passes on an echoed request.

use serde_json::{json, Value};

// The harness and the role ARN are shared with the create/read suite rather than
// copied (`tests/all/pipeline_lifecycle.rs`).
use crate::pipeline_lifecycle::{Emulator, ROLE_ARN};

/// The two artifact-list members of `ActionDeclaration`, as
/// `smithy_query(query="shape", shape="ActionDeclaration")` names them.
const ARTIFACT_MEMBERS: [&str; 2] = ["inputArtifacts", "outputArtifacts"];

/// One action in a case's declaration.
///
/// `inputs`/`outputs` are `None` when the REQUEST leaves that member out entirely —
/// the case the rule is about — and `Some(names)` when it declares it, including
/// `Some(&[])` for an explicitly empty array.
struct ActionSpec {
    name: &'static str,
    category: &'static str,
    inputs: Option<Vec<&'static str>>,
    outputs: Option<Vec<&'static str>>,
}

impl ActionSpec {
    fn source(name: &'static str, outputs: Option<Vec<&'static str>>) -> Self {
        Self {
            name,
            category: "Source",
            inputs: None,
            outputs,
        }
    }

    fn deploy(
        name: &'static str,
        inputs: Option<Vec<&'static str>>,
        outputs: Option<Vec<&'static str>>,
    ) -> Self {
        Self {
            name,
            category: "Deploy",
            inputs,
            outputs,
        }
    }

    /// This action as the REQUEST carries it: a member the spec leaves `None` is
    /// absent from the JSON, which is exactly what an SDK sends for it.
    fn request(&self) -> Value {
        let mut action = json!({
            "name": self.name,
            "actionTypeId": {
                "category": self.category,
                "owner": "Custom",
                "provider": "MyProvider",
                "version": "1",
            },
            "configuration": {"Key": "value"},
        });
        for (member, names) in ARTIFACT_MEMBERS
            .iter()
            .zip([self.inputs.as_ref(), self.outputs.as_ref()])
        {
            if let Some(names) = names {
                let entries: Vec<Value> = names.iter().map(|name| json!({"name": name})).collect();
                action[*member] = Value::Array(entries);
            }
        }
        action
    }

    /// The artifact names the reported action must carry for `member`: the ones it
    /// declared, and NO names where it declared the member not at all.
    fn expected(&self, member: &str) -> Vec<&'static str> {
        let declared = if member == ARTIFACT_MEMBERS[0] {
            self.inputs.as_ref()
        } else {
            self.outputs.as_ref()
        };
        declared.cloned().unwrap_or_default()
    }
}

/// One stage: its name and its actions, in declaration order.
struct StageSpec {
    name: &'static str,
    actions: Vec<ActionSpec>,
}

impl StageSpec {
    fn request(&self) -> Value {
        json!({
            "name": self.name,
            "actions": self.actions.iter().map(ActionSpec::request).collect::<Vec<Value>>(),
        })
    }
}

fn declaration(name: &str, stages: &[StageSpec]) -> Value {
    json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": "my-bucket"},
        "stages": stages.iter().map(StageSpec::request).collect::<Vec<Value>>(),
    })
}

/// Assert the rule over one reported structure: every action carries both artifact
/// members as a LIST (never absent), holding the names that action declared.
fn assert_artifact_lists(case: &str, label: &str, reported: &Value, stages: &[StageSpec]) {
    for (stage_index, stage_spec) in stages.iter().enumerate() {
        let stage = &reported["stages"][stage_index];
        assert_eq!(
            stage["name"],
            json!(stage_spec.name),
            "case {case:?} ({label}): stage {stage_index} must be {:?}, got {reported}",
            stage_spec.name
        );
        for (action_index, action_spec) in stage_spec.actions.iter().enumerate() {
            let action = &stage["actions"][action_index];
            for member in ARTIFACT_MEMBERS {
                let actual = action[member].as_array().unwrap_or_else(|| {
                    panic!(
                        "case {case:?} ({label}): action {:?} must report {member} as a list, \
                         got {}",
                        action_spec.name, action[member]
                    )
                });
                let actual_names: Vec<&str> = actual
                    .iter()
                    .map(|artifact| {
                        artifact["name"].as_str().unwrap_or_else(|| {
                            panic!(
                                "case {case:?} ({label}): action {:?} {member} entry must name \
                                 an artifact, got {artifact}",
                                action_spec.name
                            )
                        })
                    })
                    .collect();
                assert_eq!(
                    actual_names,
                    action_spec.expected(member),
                    "case {case:?} ({label}): action {:?} {member}, got {}",
                    action_spec.name,
                    action[member]
                );
            }
        }
    }
}

fn cases() -> Vec<(&'static str, Vec<StageSpec>)> {
    vec![
        (
            "a one-action source pipeline declaring neither member",
            vec![StageSpec {
                name: "Source",
                actions: vec![ActionSpec::source("Src", None)],
            }],
        ),
        (
            "a source action declaring only outputArtifacts",
            vec![StageSpec {
                name: "Source",
                actions: vec![ActionSpec::source("Src", Some(vec!["SourceArtifact"]))],
            }],
        ),
        (
            "a deploy action declaring only inputArtifacts",
            vec![
                StageSpec {
                    name: "Source",
                    actions: vec![ActionSpec::source("Src", Some(vec!["SourceArtifact"]))],
                },
                StageSpec {
                    name: "Deploy",
                    actions: vec![ActionSpec::deploy(
                        "Dep",
                        Some(vec!["SourceArtifact"]),
                        None,
                    )],
                },
            ],
        ),
        (
            "both members declared as explicitly empty arrays",
            vec![StageSpec {
                name: "Source",
                actions: vec![ActionSpec {
                    name: "Src",
                    category: "Source",
                    inputs: Some(Vec::new()),
                    outputs: Some(Vec::new()),
                }],
            }],
        ),
        (
            "an action declaring both members, with several names each",
            vec![
                StageSpec {
                    name: "Source",
                    actions: vec![ActionSpec::source("Src", Some(vec!["A", "B"]))],
                },
                StageSpec {
                    name: "Deploy",
                    actions: vec![ActionSpec::deploy(
                        "Dep",
                        Some(vec!["A", "B"]),
                        Some(vec!["C", "D", "E"]),
                    )],
                },
            ],
        ),
        (
            "several actions in one stage, each declaring a different mix",
            vec![
                StageSpec {
                    name: "Source",
                    actions: vec![
                        ActionSpec::source("SrcA", Some(vec!["FromA"])),
                        ActionSpec::source("SrcB", None),
                    ],
                },
                StageSpec {
                    name: "Deploy",
                    actions: vec![
                        ActionSpec::deploy("DepNeither", None, None),
                        ActionSpec::deploy("DepInput", Some(vec!["FromA"]), None),
                        ActionSpec::deploy("DepOutput", None, Some(vec!["FromDeploy"])),
                        ActionSpec::deploy("DepEmpty", Some(Vec::new()), Some(Vec::new())),
                    ],
                },
            ],
        ),
    ]
}

/// Every structure the definition surface reports — the create's answer, the read
/// back, and the update's answer — carries both artifact members on every action.
#[tokio::test]
async fn every_reported_action_declaration_carries_both_artifact_lists() {
    for (index, (case, stages)) in cases().into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("pipe-{index}");
        let sent = declaration(&name, &stages);

        let created = emulator
            .ok("CreatePipeline", &json!({"pipeline": sent}))
            .await;
        assert_artifact_lists(case, "CreatePipeline", &created["pipeline"], &stages);

        let fetched = emulator.ok("GetPipeline", &json!({"name": name})).await;
        assert_artifact_lists(case, "GetPipeline", &fetched["pipeline"], &stages);

        // The edit-then-update cycle: the structure the caller READ BACK is sent
        // straight to UpdatePipeline, so a member the read dropped would be lost from
        // the stored pipeline. The answer and the next read must still carry both.
        let updated = emulator
            .ok(
                "UpdatePipeline",
                &json!({"pipeline": fetched["pipeline"].clone()}),
            )
            .await;
        assert_artifact_lists(case, "UpdatePipeline", &updated["pipeline"], &stages);
        assert_eq!(
            updated["pipeline"]["version"], 2,
            "case {case:?}: an update stores the next version, got {updated}"
        );

        let refetched = emulator.ok("GetPipeline", &json!({"name": name})).await;
        assert_artifact_lists(case, "GetPipeline after update", &refetched["pipeline"], &stages);
        assert_eq!(
            refetched["pipeline"]["stages"], updated["pipeline"]["stages"],
            "case {case:?}: the read-back structure is the one the update stored, got {refetched}"
        );
    }
}

/// The round trip is a FIXED POINT: feeding a reported structure back through
/// UpdatePipeline reports the same stages again, so an edit-then-update cycle neither
/// grows nor loses the artifact members however many times it runs.
#[tokio::test]
async fn reading_a_structure_back_and_updating_with_it_is_a_fixed_point() {
    let emulator = Emulator::new();
    let stages = vec![
        StageSpec {
            name: "Source",
            actions: vec![ActionSpec::source("Src", Some(vec!["SourceArtifact"]))],
        },
        StageSpec {
            name: "Deploy",
            actions: vec![
                ActionSpec::deploy("DepNeither", None, None),
                ActionSpec::deploy("DepInput", Some(vec!["SourceArtifact"]), None),
            ],
        },
    ];
    let created = emulator
        .ok(
            "CreatePipeline",
            &json!({"pipeline": declaration("fixed-point", &stages)}),
        )
        .await;
    let mut previous = created["pipeline"]["stages"].clone();

    for round in 1..=3 {
        let fetched = emulator
            .ok("GetPipeline", &json!({"name": "fixed-point"}))
            .await;
        let updated = emulator
            .ok(
                "UpdatePipeline",
                &json!({"pipeline": fetched["pipeline"].clone()}),
            )
            .await;
        let stages_now = updated["pipeline"]["stages"].clone();
        assert_eq!(
            stages_now, previous,
            "round {round}: a re-submitted structure must come back unchanged, got {updated}"
        );
        assert_artifact_lists(
            "fixed point",
            &format!("round {round}"),
            &updated["pipeline"],
            &stages,
        );
        previous = stages_now;
    }
}
