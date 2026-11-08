// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the codec a jobs query reads off a job's outputs.
//!
//! The one rule every case below is a row of: a jobs query's `audioCodec` and `videoCodec`
//! filter keys match the codec an output is ENCODED with — the codec its effective encoder
//! settings state, whether the output carries those settings inline or states them by
//! naming a preset the account holds — so the same job matches the same value whichever of
//! the two forms its outputs state it in, and an output whose named preset the account no
//! longer holds states no codec at all.
//!
//! The key's documented meaning comes from the model: "audioCodec - Your output's audio
//! codec" and "videoCodec - Your output's video codec"
//! (`smithy_query(query="doc",shape="JobsQueryFilterKey",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`),
//! and `Output$Preset` is why an output has the two forms: "Provide the system or custom
//! preset name. You can specify either Preset or Container settings, but not both" — so an
//! output that names a preset carries no inline descriptions of its own, and the preset's
//! `PresetSettings` carry the `AudioDescriptions`/`VideoDescription` it is written with.
//!
//! Every case drives the real restJson1 wire path through the crate's one test harness, so
//! what is asserted is the bytes a client would see, never an internal call. The rest of
//! the jobs-query surface — its status, its paging and the other six filter keys — is
//! `tests/all/jobs_query.rs`.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::job_lifecycle::{Emulator, ROLE};
use super::job_search::job_ids;
use super::jobs_query::{pin_query_span, run_query, SHORT_QUERY};
use super::output_preset_reference::job_settings;

/// The `audioCodec` filter key's documented value catalogue, in the order the key's own
/// documentation lists it ("AAC | MP2 | MP3 | WAV | AIFF | AC3| EAC3 | EAC3_ATMOS | VORBIS
/// | OPUS | PASSTHROUGH | FLAC", `smithy_query(query="doc",shape="JobsQueryFilterKey")`).
const AUDIO_CODECS: [&str; 12] = [
    "AAC",
    "MP2",
    "MP3",
    "WAV",
    "AIFF",
    "AC3",
    "EAC3",
    "EAC3_ATMOS",
    "VORBIS",
    "OPUS",
    "PASSTHROUGH",
    "FLAC",
];

/// The `videoCodec` filter key's documented value catalogue, in the order the key's own
/// documentation lists it ("AV1 | AVC_INTRA | FRAME_CAPTURE | H_264 | H_265 | MPEG2 |
/// PASSTHROUGH | PRORES | UNCOMPRESSED | VC3 | VP8 | VP9 | XAVC",
/// `smithy_query(query="doc",shape="JobsQueryFilterKey")`).
const VIDEO_CODECS: [&str; 13] = [
    "AV1",
    "AVC_INTRA",
    "FRAME_CAPTURE",
    "H_264",
    "H_265",
    "MPEG2",
    "PASSTHROUGH",
    "PRORES",
    "UNCOMPRESSED",
    "VC3",
    "VP8",
    "VP9",
    "XAVC",
];

/// The `JobsQueryFilterKey` values this suite is about.
const KEY_AUDIO: &str = "audioCodec";
/// See [`KEY_AUDIO`].
const KEY_VIDEO: &str = "videoCodec";

/// The container every preset and every inline output here is written in: the codecs are
/// what the rule is about, and no row differs in its container.
const CONTAINER: &str = "MP4";

/// The preset a row creates for the codec pair at index `index`, so the preset-backed job
/// of that pair has one of its own to name.
fn pair_preset(index: usize) -> String {
    format!("codec-pair-{index}")
}

/// The preset the suite creates and then DELETES, so a job can name a reference that
/// resolved when it was submitted and resolves no longer.
const DELETED_PRESET: &str = "preset-that-went-away";

/// One fixture job, and what its outputs are ENCODED with — through whichever form states
/// it — so every expectation below is computed from the rule rather than tracked by hand.
struct FixtureJob {
    /// How an assertion names this job.
    label: String,
    /// The id `CreateJob` answered.
    id: String,
    /// Every audio codec this job's outputs are encoded with.
    audio: Vec<String>,
    /// Every video codec this job's outputs are encoded with.
    video: Vec<String>,
}

/// An output that states its encoder settings INLINE: the container it is written in, one
/// audio description and the video description, each naming its codec.
fn inline_output(audio_codec: &str, video_codec: &str) -> Value {
    json!({
        "nameModifier": "_inline",
        "containerSettings": { "container": CONTAINER },
        "audioDescriptions": [{ "codecSettings": { "codec": audio_codec } }],
        "videoDescription": { "codecSettings": { "codec": video_codec } }
    })
}

/// An output that states its encoder settings by NAMING a preset — and so, as
/// `Output$Preset` requires, carries none of its own.
fn preset_output(preset: &str) -> Value {
    json!({ "nameModifier": "_preset", "preset": preset })
}

/// Create a preset whose settings state `audio_codec` and `video_codec`, which is what an
/// output naming it is encoded with.
async fn seed_preset(emulator: &Emulator, name: &str, audio_codec: &str, video_codec: &str) {
    let created = emulator
        .create_preset(json!({
            "name": name,
            "settings": {
                "containerSettings": { "container": CONTAINER },
                "audioDescriptions": [{ "codecSettings": { "codec": audio_codec } }],
                "videoDescription": { "codecSettings": { "codec": video_codec } }
            }
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture preset {name:?} must be created: got {} with body {}",
        created.status, created.body
    );
}

/// Submit a job whose settings hold `groups`, and report its id.
async fn seed_job(emulator: &Emulator, label: &str, groups: &[Vec<Value>]) -> String {
    let created = emulator
        .create_job(json!({ "role": ROLE, "settings": job_settings(groups) }))
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture job {label:?} must be created: got {} with body {}",
        created.status, created.body
    );
    let id = created.string("/job/id");
    assert!(
        !id.is_empty(),
        "the fixture job {label:?} must be answered an id, got body {}",
        created.body
    );
    id
}

/// A preset the SERVICE publishes — its name and the two codecs its settings state — read
/// through `ListPresets`, so no case here hard-codes the built-in catalogue.
async fn published_preset(emulator: &Emulator) -> (String, String, String) {
    let listed = emulator.list_presets("listBy=SYSTEM").await;
    assert_eq!(
        listed.status, 200,
        "ListPresets must answer HTTP 200, got {} with body {}",
        listed.status, listed.body
    );
    let name = listed.string("/presets/0/name");
    assert!(
        !name.is_empty(),
        "the account must hold at least one built-in preset, got body {}",
        listed.body
    );
    let audio = listed.string("/presets/0/settings/audioDescriptions/0/codecSettings/codec");
    let video = listed.string("/presets/0/settings/videoDescription/codecSettings/codec");
    assert!(
        AUDIO_CODECS.contains(&audio.as_str()),
        "the built-in preset {name:?} must state an audio codec a jobs query can filter \
         on, got {audio:?} from body {}",
        listed.body
    );
    assert!(
        VIDEO_CODECS.contains(&video.as_str()),
        "the built-in preset {name:?} must state a video codec a jobs query can filter \
         on, got {video:?} from body {}",
        listed.body
    );
    (name, audio, video)
}

/// Every fixture job the table queries over: for each documented codec pair, one job that
/// states the pair inline and one that states it by naming a preset, plus a job built on a
/// preset the SERVICE published, a job naming a preset the account no longer holds, and a
/// job whose two output groups state their settings in the two different forms.
async fn seed_jobs(emulator: &Emulator) -> Vec<FixtureJob> {
    let mut jobs = Vec::new();

    for (index, audio) in AUDIO_CODECS.iter().enumerate() {
        let video = VIDEO_CODECS[index];
        let preset = pair_preset(index);
        seed_preset(emulator, &preset, audio, video).await;

        let label = format!("inline {audio}/{video}");
        let id = seed_job(emulator, &label, &[vec![inline_output(audio, video)]]).await;
        jobs.push(FixtureJob {
            label,
            id,
            audio: vec![(*audio).to_string()],
            video: vec![video.to_string()],
        });

        let label = format!("preset {audio}/{video}");
        let id = seed_job(emulator, &label, &[vec![preset_output(&preset)]]).await;
        jobs.push(FixtureJob {
            label,
            id,
            audio: vec![(*audio).to_string()],
            video: vec![video.to_string()],
        });
    }

    // A preset the service publishes, which an account can build a job from without having
    // created anything: the codecs come from the listing, so the row spans whatever the
    // catalogue states.
    let (published, audio, video) = published_preset(emulator).await;
    let label = format!("published preset {published}");
    let id = seed_job(emulator, &label, &[vec![preset_output(&published)]]).await;
    jobs.push(FixtureJob {
        label,
        id,
        audio: vec![audio],
        video: vec![video],
    });

    // A reference that resolved when the job was submitted and resolves no longer: it
    // states no codec, so the job matches no value of either key. Its video codec is the
    // one documented value no other fixture job uses, so the query on it is empty only
    // because nothing supplies it.
    let gone_audio = AUDIO_CODECS[0];
    let gone_video = VIDEO_CODECS[VIDEO_CODECS.len() - 1];
    seed_preset(emulator, DELETED_PRESET, gone_audio, gone_video).await;
    let label = format!("deleted preset {gone_audio}/{gone_video}");
    let id = seed_job(emulator, &label, &[vec![preset_output(DELETED_PRESET)]]).await;
    let deleted = emulator.delete_preset(DELETED_PRESET).await;
    assert_eq!(
        deleted.status, 202,
        "the fixture preset {DELETED_PRESET:?} must be deleted: got {} with body {}",
        deleted.status, deleted.body
    );
    jobs.push(FixtureJob {
        label,
        id,
        audio: Vec::new(),
        video: Vec::new(),
    });

    // One job stating its settings BOTH ways, in two output groups: a job matches a value
    // any of its outputs is encoded with, whichever form that output used.
    let inline_pair = 2;
    let preset_pair = 7;
    let label = format!(
        "mixed inline {}/{} with preset {}/{}",
        AUDIO_CODECS[inline_pair],
        VIDEO_CODECS[inline_pair],
        AUDIO_CODECS[preset_pair],
        VIDEO_CODECS[preset_pair]
    );
    let id = seed_job(
        emulator,
        &label,
        &[
            vec![inline_output(
                AUDIO_CODECS[inline_pair],
                VIDEO_CODECS[inline_pair],
            )],
            vec![preset_output(&pair_preset(preset_pair))],
        ],
    )
    .await;
    jobs.push(FixtureJob {
        label,
        id,
        audio: vec![
            AUDIO_CODECS[inline_pair].to_string(),
            AUDIO_CODECS[preset_pair].to_string(),
        ],
        video: vec![
            VIDEO_CODECS[inline_pair].to_string(),
            VIDEO_CODECS[preset_pair].to_string(),
        ],
    });

    jobs
}

/// The ids of the fixture jobs the rule says a filter on `key` with `values` matches: the
/// jobs one of whose outputs is encoded with one of those codecs.
fn rule_matches(jobs: &[FixtureJob], key: &str, values: &[String]) -> BTreeSet<String> {
    jobs.iter()
        .filter(|job| {
            let codecs = if key == KEY_AUDIO {
                &job.audio
            } else {
                &job.video
            };
            values.iter().any(|value| codecs.contains(value))
        })
        .map(|job| job.id.clone())
        .collect()
}

/// The labels of the fixture jobs `ids` names, so a failure reads as the jobs rather than
/// as a set of minted ids.
fn labels(jobs: &[FixtureJob], ids: &BTreeSet<String>) -> Vec<String> {
    jobs.iter()
        .filter(|job| ids.contains(&job.id))
        .map(|job| job.label.clone())
        .collect()
}

/// A completed jobs query reports exactly the jobs whose outputs are ENCODED with a codec
/// the filter names, for every documented value of both codec keys — whether the output
/// carries its codec settings inline or states them by naming a preset.
#[tokio::test]
async fn a_codec_filter_matches_inline_and_preset_backed_outputs_alike() {
    let emulator = Emulator::new();
    pin_query_span(&emulator, SHORT_QUERY);
    let jobs = seed_jobs(&emulator).await;

    // One case per documented value of each key — every value the key's own documentation
    // lists, including the one no fixture job supplies — plus one case naming two values of
    // a key, which is an OR over them.
    let mut cases: Vec<(String, &str, Vec<String>)> = Vec::new();
    for codec in AUDIO_CODECS {
        cases.push((
            format!("{KEY_AUDIO} {codec}"),
            KEY_AUDIO,
            vec![codec.to_string()],
        ));
    }
    for codec in VIDEO_CODECS {
        cases.push((
            format!("{KEY_VIDEO} {codec}"),
            KEY_VIDEO,
            vec![codec.to_string()],
        ));
    }
    cases.push((
        format!("{KEY_VIDEO} two values"),
        KEY_VIDEO,
        vec![VIDEO_CODECS[1].to_string(), VIDEO_CODECS[9].to_string()],
    ));

    for (case, key, values) in cases {
        let results = run_query(
            &emulator,
            json!({ "filterList": [{ "key": key, "values": values }] }),
        )
        .await;
        let actual: BTreeSet<String> = job_ids(&results).into_iter().collect();
        let expected = rule_matches(&jobs, key, &values);
        assert_eq!(
            actual,
            expected,
            "case {case:?}: expected the jobs {:?}, got {:?} from body {}",
            labels(&jobs, &expected),
            labels(&jobs, &actual),
            results.body
        );
    }
}

/// A job whose output names a preset keeps that reference: the stored job states the preset
/// and no inline descriptions of its own, and the query still matches the preset's codecs —
/// so the match FOLLOWS the reference rather than the job having been rewritten when it was
/// submitted.
#[tokio::test]
async fn a_preset_backed_job_keeps_its_reference_and_still_matches() {
    let emulator = Emulator::new();
    pin_query_span(&emulator, SHORT_QUERY);

    let preset = pair_preset(4);
    let audio = AUDIO_CODECS[4];
    let video = VIDEO_CODECS[4];
    seed_preset(&emulator, &preset, audio, video).await;
    let id = seed_job(&emulator, &preset, &[vec![preset_output(&preset)]]).await;

    let fetched = emulator.get_job(&id).await;
    assert_eq!(
        fetched.status, 200,
        "GetJob must answer HTTP 200, got {} with body {}",
        fetched.status, fetched.body
    );
    let output = fetched.at("/job/settings/outputGroups/0/outputs/0");
    assert_eq!(
        output.pointer("/preset").and_then(Value::as_str),
        Some(preset.as_str()),
        "the stored output must still name the preset, got {output}"
    );
    for member in ["audioDescriptions", "videoDescription"] {
        assert!(
            output.pointer(&format!("/{member}")).is_none(),
            "the stored output must carry no inline {member}, got {output}"
        );
    }

    for (key, value) in [(KEY_AUDIO, audio), (KEY_VIDEO, video)] {
        let results = run_query(
            &emulator,
            json!({ "filterList": [{ "key": key, "values": [value] }] }),
        )
        .await;
        let matched = job_ids(&results);
        assert_eq!(
            matched,
            vec![id.clone()],
            "case {key:?} {value:?}: expected the preset-backed job {id:?}, got {matched:?} \
             from body {}",
            results.body
        );
    }
}
