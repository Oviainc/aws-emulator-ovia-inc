// SPDX-License-Identifier: CC0-1.0

//! The built-in catalogue every MediaConvert account carries without creating it: the
//! system output presets and the system job templates, with the container, codec and
//! packaging vocabulary their published names are read as data from.
//!
//! Moved out of `super` unchanged, so one file holds the built-in catalogue and the state
//! module keeps the records a caller's own calls write.

use super::*;

/// The output package a built-in job template's published name states: the
/// `OutputGroupType` its one output group carries, the label that enum's own documentation
/// gives that type, and the `ContainerType` each of the group's outputs is written in.
#[derive(Debug, Clone, Copy)]
pub(super) struct OutputPackage {
    /// A [`GROUP_TYPE_FILE`]-style `OutputGroupType` value.
    pub(super) group_type: &'static str,
    /// `OutputGroup$Name`, "name of the output group": the label `OutputGroupType`'s doc
    /// gives this type.
    pub(super) label: &'static str,
    /// A [`CONTAINER_MP4`]-style `ContainerType` value for the group's outputs.
    pub(super) container: &'static str,
}

impl OutputPackage {
    /// The modelled `OutputGroupSettings`: the type, and — as `OutputGroupSettings` pairs
    /// each type with its own member — that package's settings object.
    ///
    /// The object carries no member: the only package settings a built-in template's name
    /// states are the package itself, and `Destination` ("the S3 output location and the
    /// output filename base") belongs to the job that is created from the template, not to
    /// the template.
    pub(super) fn settings(&self) -> OutputGroupSettings {
        let mut settings = OutputGroupSettings {
            r#type: Some(self.group_type.to_string()),
            ..OutputGroupSettings::default()
        };
        match self.group_type {
            GROUP_TYPE_HLS => {
                settings.hls_group_settings = Some(HlsGroupSettings::default());
            }
            GROUP_TYPE_DASH_ISO => {
                settings.dash_iso_group_settings = Some(DashIsoGroupSettings::default());
            }
            GROUP_TYPE_MS_SMOOTH => {
                settings.ms_smooth_group_settings = Some(MsSmoothGroupSettings::default());
            }
            GROUP_TYPE_CMAF => {
                settings.cmaf_group_settings = Some(CmafGroupSettings::default());
            }
            _ => {
                settings.file_group_settings = Some(FileGroupSettings::default());
            }
        }
        settings
    }
}

/// A standalone file output ("MediaConvert uses this group of settings to generate a single
/// standalone file, rather than a streaming package"), in the MP4 container.
pub(super) const PACKAGE_FILE: OutputPackage = OutputPackage {
    group_type: GROUP_TYPE_FILE,
    label: "File group",
    container: CONTAINER_MP4,
};
/// An Apple HLS package, whose segments are MPEG-2 transport streams.
pub(super) const PACKAGE_HLS: OutputPackage = OutputPackage {
    group_type: GROUP_TYPE_HLS,
    label: "Apple HLS",
    container: CONTAINER_M3U8,
};
/// A DASH ISO package, whose segments are fragmented MP4.
pub(super) const PACKAGE_DASH_ISO: OutputPackage = OutputPackage {
    group_type: GROUP_TYPE_DASH_ISO,
    label: "DASH ISO",
    container: CONTAINER_MPD,
};
/// A CMAF package, whose segments are the fragmented MP4 of [`CONTAINER_CMFC`].
pub(super) const PACKAGE_CMAF: OutputPackage = OutputPackage {
    group_type: GROUP_TYPE_CMAF,
    label: "CMAF",
    container: CONTAINER_CMFC,
};
/// A Microsoft Smooth Streaming package.
pub(super) const PACKAGE_MS_SMOOTH: OutputPackage = OutputPackage {
    group_type: GROUP_TYPE_MS_SMOOTH,
    label: "Microsoft Smooth Streaming",
    container: CONTAINER_ISMV,
};

/// The frame size and encode rate a built-in job template's name states, for a name that
/// states them.
///
/// A name that states none — an adaptive-bitrate package named after its container and
/// codecs only — leaves the output's resolution and frame rate to the input, which is what
/// `JobTemplateSettings$FollowSource` describes the service doing with "all outputs that
/// you don't manually specify".
#[derive(Debug, Clone, Copy)]
pub(super) struct Rendition {
    /// `VideoDescription$Width`, "the video resolution width, in pixels, for this output",
    /// which is the stated line count at the stated display aspect ratio.
    pub(super) width: i32,
    /// `VideoDescription$Height`, the line count the name's `<N>p` token states.
    pub(super) height: i32,
    /// The bitrate and frame rate the name's `<N>kbps` and `<N>fps` tokens state.
    pub(super) rate: EncodeRate,
}

/// One built-in ([`TYPE_SYSTEM`]) job template: the encoding configuration its published
/// name states, as a row of parameters.
///
/// A MediaConvert system job template is named after the output it produces — the output
/// package, the video and audio codecs and, where the name carries them, the display aspect
/// ratio, frame size, frame rate and video bitrate — so this row is that name read as data,
/// and [`SystemJobTemplate::settings`] builds the modelled `JobTemplateSettings` graph from
/// it, exactly as [`SystemPreset`] does for a built-in preset's `PresetSettings`: no member
/// is set that the name does not state.
#[derive(Debug, Clone, Copy)]
pub(super) struct SystemJobTemplate {
    /// The published `Name`, which is this template's identity.
    pub(super) name: &'static str,
    /// The output package the name states.
    pub(super) package: OutputPackage,
    /// A [`VIDEO_CODEC_AVC`]-style `VideoCodec` value.
    pub(super) video_codec: &'static str,
    /// The frame size and encode rate the name states, or `None` for a name that states
    /// neither.
    pub(super) rendition: Option<Rendition>,
}

impl SystemJobTemplate {
    /// The modelled `JobTemplateSettings` this row states: the one input a job created from
    /// it inherits, and the one output group of the package the name names.
    pub(super) fn settings(&self) -> JobTemplateSettings {
        let mut audio_selectors = HashMap::new();
        audio_selectors.insert(
            AUDIO_SELECTOR_NAME.to_string(),
            AudioSelector {
                default_selection: Some(AUDIO_DEFAULT_SELECTION.to_string()),
                ..AudioSelector::default()
            },
        );
        JobTemplateSettings {
            // "There can only be one input in a job template" ([`MAX_JOB_TEMPLATE_INPUTS`]):
            // the input every job created from this template inherits, carrying the audio
            // selector the output's audio track is taken from. WHERE that input is read
            // from is not stated here — `InputTemplate` models no `FileInput` at all — so
            // the job that uses the template states its own source.
            inputs: Some(vec![InputTemplate {
                audio_selectors: Some(audio_selectors),
                ..InputTemplate::default()
            }]),
            output_groups: Some(vec![OutputGroup {
                name: Some(self.package.label.to_string()),
                output_group_settings: Some(self.package.settings()),
                outputs: Some(vec![self.output()]),
                ..OutputGroup::default()
            }]),
            ..JobTemplateSettings::default()
        }
    }

    /// The one output of this row's group: the container the package is written in, the
    /// video description the name states, and the one AAC audio track it names.
    pub(super) fn output(&self) -> Output {
        Output {
            container_settings: Some(ContainerSettings {
                container: Some(self.package.container.to_string()),
                ..ContainerSettings::default()
            }),
            video_description: Some(VideoDescription {
                width: self.rendition.map(|rendition| rendition.width),
                height: self.rendition.map(|rendition| rendition.height),
                codec_settings: Some(video_codec_settings(
                    self.video_codec,
                    self.rendition.map(|rendition| rendition.rate),
                )),
                ..VideoDescription::default()
            }),
            audio_descriptions: Some(vec![aac_audio_description(
                BUILT_IN_TEMPLATE_AUDIO_BITRATE,
            )]),
            ..Output::default()
        }
    }
}

/// The key a built-in job template's input states its audio selector under.
///
/// `Input$AudioSelectors` ("Use Audio selectors to specify a track or set of tracks from the
/// input that you will use in your outputs") is a map whose keys are LABELS rather than
/// modelled values, so this one is the emulator's own, the label the service's own console
/// gives the first selector of an input.
pub(super) const AUDIO_SELECTOR_NAME: &str = "Audio Selector 1";

/// `AudioDefaultSelection`'s `DEFAULT` (`smithy_query(query="shape",shape="AudioDefaultSelection")`):
/// the selector an output with no selector of its own takes its audio from, which is what
/// makes the single audio track of a built-in template's output resolve against any input.
pub(super) const AUDIO_DEFAULT_SELECTION: &str = "DEFAULT";

/// `AacSettings$Bitrate` for the audio track of a built-in job template's output.
///
/// A CHOSEN value, not one the published name states: a system job template's name names the
/// audio codec (`Aac`) and never its bitrate. 128 kbps sits inside the member's published
/// 6000..1024000 range and is the rate the HD entries of [`SYSTEM_PRESETS`] carry for the
/// same stereo 48 kHz track, so the two built-in catalogues describe it the same way.
pub(super) const BUILT_IN_TEMPLATE_AUDIO_BITRATE: i32 = 128_000;

/// The frame rate of the catalogue's single-file entries, as the
/// `FramerateNumerator`/`FramerateDenominator` fraction: 30000/1001, the exact value of the
/// 29.97 fps their names state.
pub(super) const FRAMERATE_29_97: (i32, i32) = (30_000, 1_001);

/// The built-in ([`TYPE_SYSTEM`]) job templates this store is seeded with.
///
/// That an account holds templates it did not create is what `JobTemplate$Type` ("A job
/// template can be of two types: system or custom. System or built-in job templates
/// can't be modified or deleted by the user.") and `JobTemplateListBy`'s `SYSTEM` value
/// say. The model carries no catalogue of them, so this SET is the emulator's own, named
/// after the system job templates AWS Elemental MediaConvert publishes, and its size is well
/// inside both [`MAX_JOB_TEMPLATES`] and one page of `ListJobTemplates`; nothing in the
/// emulator or its tests keys on a particular entry, because a built-in template is
/// recognised by its `Type` and never by its name. Its rows span the five published output
/// packages (`OutputGroupType`) and both video codecs a published name abbreviates.
///
/// Each entry carries the encoding configuration its name states, because
/// `JobTemplate$Settings` IS the template's "pre-made set of encoding instructions": a
/// client lists this catalogue, picks an entry and creates a job from it, and that job
/// inherits the entry's output group ([`merge_template_settings`]). The graph of every row
/// therefore passes the same test a template a caller
/// stores has to pass
/// ([`ComAmazonawsMediaconvertMediaconvertState::job_template_settings_problem`]).
pub(super) const SYSTEM_JOB_TEMPLATES: [SystemJobTemplate; 12] = [
    SystemJobTemplate {
        name: "System-Avc_16x9_1080p_29_97fps_8500kbps",
        package: PACKAGE_FILE,
        video_codec: VIDEO_CODEC_AVC,
        rendition: Some(Rendition {
            width: 1920,
            height: 1080,
            rate: EncodeRate {
                bitrate: 8_500_000,
                framerate_numerator: FRAMERATE_29_97.0,
                framerate_denominator: FRAMERATE_29_97.1,
            },
        }),
    },
    SystemJobTemplate {
        name: "System-Avc_16x9_720p_29_97fps_5000kbps",
        package: PACKAGE_FILE,
        video_codec: VIDEO_CODEC_AVC,
        rendition: Some(Rendition {
            width: 1280,
            height: 720,
            rate: EncodeRate {
                bitrate: 5_000_000,
                framerate_numerator: FRAMERATE_29_97.0,
                framerate_denominator: FRAMERATE_29_97.1,
            },
        }),
    },
    SystemJobTemplate {
        name: "System-Avc_16x9_540p_29_97fps_3500kbps",
        package: PACKAGE_FILE,
        video_codec: VIDEO_CODEC_AVC,
        rendition: Some(Rendition {
            width: 960,
            height: 540,
            rate: EncodeRate {
                bitrate: 3_500_000,
                framerate_numerator: FRAMERATE_29_97.0,
                framerate_denominator: FRAMERATE_29_97.1,
            },
        }),
    },
    SystemJobTemplate {
        name: "System-Avc_16x9_360p_29_97fps_1200kbps",
        package: PACKAGE_FILE,
        video_codec: VIDEO_CODEC_AVC,
        rendition: Some(Rendition {
            width: 640,
            height: 360,
            rate: EncodeRate {
                bitrate: 1_200_000,
                framerate_numerator: FRAMERATE_29_97.0,
                framerate_denominator: FRAMERATE_29_97.1,
            },
        }),
    },
    SystemJobTemplate {
        name: "System-Hevc_16x9_2160p_29_97fps_20000kbps",
        package: PACKAGE_FILE,
        video_codec: VIDEO_CODEC_HEVC,
        rendition: Some(Rendition {
            width: 3840,
            height: 2160,
            rate: EncodeRate {
                bitrate: 20_000_000,
                framerate_numerator: FRAMERATE_29_97.0,
                framerate_denominator: FRAMERATE_29_97.1,
            },
        }),
    },
    SystemJobTemplate {
        name: "System-Hevc_16x9_1080p_29_97fps_6000kbps",
        package: PACKAGE_FILE,
        video_codec: VIDEO_CODEC_HEVC,
        rendition: Some(Rendition {
            width: 1920,
            height: 1080,
            rate: EncodeRate {
                bitrate: 6_000_000,
                framerate_numerator: FRAMERATE_29_97.0,
                framerate_denominator: FRAMERATE_29_97.1,
            },
        }),
    },
    SystemJobTemplate {
        name: "System-Ott_Hls_Ts_Avc_Aac",
        package: PACKAGE_HLS,
        video_codec: VIDEO_CODEC_AVC,
        rendition: None,
    },
    SystemJobTemplate {
        name: "System-Ott_Hls_Ts_Hevc_Aac",
        package: PACKAGE_HLS,
        video_codec: VIDEO_CODEC_HEVC,
        rendition: None,
    },
    SystemJobTemplate {
        name: "System-Ott_Dash_Mp4_Avc_Aac",
        package: PACKAGE_DASH_ISO,
        video_codec: VIDEO_CODEC_AVC,
        rendition: None,
    },
    SystemJobTemplate {
        name: "System-Ott_Dash_Mp4_Hevc_Aac",
        package: PACKAGE_DASH_ISO,
        video_codec: VIDEO_CODEC_HEVC,
        rendition: None,
    },
    SystemJobTemplate {
        name: "System-Ott_Cmaf_Cmfc_Avc_Aac",
        package: PACKAGE_CMAF,
        video_codec: VIDEO_CODEC_AVC,
        rendition: None,
    },
    SystemJobTemplate {
        name: "System-Ott_Smooth_Ismv_Avc_Aac",
        package: PACKAGE_MS_SMOOTH,
        video_codec: VIDEO_CODEC_AVC,
        rendition: None,
    },
];

/// `ContainerType` values a built-in preset's name names
/// (`smithy_query(query="shape",shape="ContainerType")`): the MP4 file container, the
/// MPEG-2 transport stream of an HLS output, and the fragmented MP4 of a DASH output.
pub(super) const CONTAINER_MP4: &str = "MP4";
/// See [`CONTAINER_MP4`]. `M3u8Settings`: "These settings relate to the MPEG-2 transport
/// stream (MPEG2-TS) container for the MPEG2-TS segments in your HLS outputs."
pub(super) const CONTAINER_M3U8: &str = "M3U8";
/// See [`CONTAINER_MP4`]. `MpdSettings`: "These settings relate to the fragmented MP4
/// container for the segments in your DASH outputs."
pub(super) const CONTAINER_MPD: &str = "MPD";

/// See [`CONTAINER_MP4`]. `CmfcSettings`: "These settings relate to the fragmented MP4
/// container for the segments in your CMAF outputs."
pub(super) const CONTAINER_CMFC: &str = "CMFC";
/// See [`CONTAINER_MP4`]: the container of a Microsoft Smooth Streaming package's segments.
pub(super) const CONTAINER_ISMV: &str = "ISMV";

/// `OutputGroupType` values (`smithy_query(query="shape",shape="OutputGroupType")`), the
/// output packages a group can be, with the labels that enum's own doc gives them: "Type of
/// output group (File group, Apple HLS, DASH ISO, Microsoft Smooth Streaming, CMAF)".
pub(super) const GROUP_TYPE_FILE: &str = "FILE_GROUP_SETTINGS";
/// See [`GROUP_TYPE_FILE`].
pub(super) const GROUP_TYPE_HLS: &str = "HLS_GROUP_SETTINGS";
/// See [`GROUP_TYPE_FILE`].
pub(super) const GROUP_TYPE_DASH_ISO: &str = "DASH_ISO_GROUP_SETTINGS";
/// See [`GROUP_TYPE_FILE`].
pub(super) const GROUP_TYPE_MS_SMOOTH: &str = "MS_SMOOTH_GROUP_SETTINGS";
/// See [`GROUP_TYPE_FILE`].
pub(super) const GROUP_TYPE_CMAF: &str = "CMAF_GROUP_SETTINGS";

/// `VideoCodec` values (`smithy_query(query="shape",shape="VideoCodec")`): MPEG-4 AVC and
/// HEVC, which a preset name abbreviates `Avc` and `Hevc`.
pub(super) const VIDEO_CODEC_AVC: &str = "H_264";
/// See [`VIDEO_CODEC_AVC`].
pub(super) const VIDEO_CODEC_HEVC: &str = "H_265";

/// The `AudioCodec` value every entry of the built-in catalogue names (`Aac`)
/// (`smithy_query(query="shape",shape="AudioCodec")`).
pub(super) const AUDIO_CODEC_AAC: &str = "AAC";

/// `H264FramerateControl`/`H265FramerateControl`'s `SPECIFIED`: the output's frame rate is
/// the stated `FramerateNumerator`/`FramerateDenominator` ratio rather than the input's.
pub(super) const FRAMERATE_SPECIFIED: &str = "SPECIFIED";

/// `H264RateControlMode`/`H265RateControlMode`/`AacRateControlMode`'s `CBR` — "constant
/// bitrate" — the mode under which `Bitrate` is the rate the stated Mbps names.
pub(super) const RATE_CONTROL_CBR: &str = "CBR";

/// `AacCodingMode`'s stereo mode, the channel layout of the AAC track a built-in preset
/// carries (`smithy_query(query="shape",shape="AacCodingMode")`).
pub(super) const AAC_CODING_MODE_STEREO: &str = "CODING_MODE_2_0";

/// `AacSettings$SampleRate`, "the AAC sample rate in samples per second (Hz)", for that
/// track: 48 kHz, the rate inside the member's published 8000..96000 range that every
/// container in this catalogue carries.
pub(super) const AAC_SAMPLE_RATE: i32 = 48_000;

/// The bitrate and frame rate a published built-in name states for one output.
///
/// `H264Settings$Bitrate`/`H265Settings$Bitrate` are "the average bitrate in bits per
/// second", and the frame rate is the
/// `FramerateNumerator`/`FramerateDenominator` fraction those shapes carry, so a name's
/// `<N>Hz`/`<N>fps` token is stored as that fraction rather than as a rounded rate.
#[derive(Debug, Clone, Copy)]
pub(super) struct EncodeRate {
    /// `Bitrate`, in bits per second.
    pub(super) bitrate: i32,
    /// The numerator of the frame-rate fraction.
    pub(super) framerate_numerator: i32,
    /// The denominator of that fraction.
    pub(super) framerate_denominator: i32,
}

/// `VideoCodecSettings` for a built-in catalogue entry: the codec its name states and — as
/// `VideoCodecSettings` requires, "for each codec enum that you choose, define the
/// corresponding settings object" — that codec's own settings object, carrying the bitrate
/// and frame rate when the name states them and nothing when it does not.
///
/// The ONE place in this module that builds a codec settings object, so a built-in preset
/// and a built-in job template cannot describe the same codec two different ways.
pub(super) fn video_codec_settings(video_codec: &str, rate: Option<EncodeRate>) -> VideoCodecSettings {
    let mut settings = VideoCodecSettings {
        codec: Some(video_codec.to_string()),
        ..VideoCodecSettings::default()
    };
    if video_codec == VIDEO_CODEC_HEVC {
        settings.h265_settings = Some(H265Settings {
            bitrate: rate.map(|rate| rate.bitrate),
            framerate_control: rate.map(|_| FRAMERATE_SPECIFIED.to_string()),
            framerate_numerator: rate.map(|rate| rate.framerate_numerator),
            framerate_denominator: rate.map(|rate| rate.framerate_denominator),
            rate_control_mode: rate.map(|_| RATE_CONTROL_CBR.to_string()),
            ..H265Settings::default()
        });
    } else {
        settings.h264_settings = Some(H264Settings {
            bitrate: rate.map(|rate| rate.bitrate),
            framerate_control: rate.map(|_| FRAMERATE_SPECIFIED.to_string()),
            framerate_numerator: rate.map(|rate| rate.framerate_numerator),
            framerate_denominator: rate.map(|rate| rate.framerate_denominator),
            rate_control_mode: rate.map(|_| RATE_CONTROL_CBR.to_string()),
            ..H264Settings::default()
        });
    }
    settings
}

/// The one stereo AAC audio track a built-in catalogue entry's name states, at `bitrate`
/// bits per second.
///
/// The ONE place in this module that builds an audio description, for the reason
/// [`video_codec_settings`] is the one place that builds codec settings.
pub(super) fn aac_audio_description(bitrate: i32) -> AudioDescription {
    AudioDescription {
        codec_settings: Some(AudioCodecSettings {
            codec: Some(AUDIO_CODEC_AAC.to_string()),
            aac_settings: Some(AacSettings {
                bitrate: Some(bitrate),
                coding_mode: Some(AAC_CODING_MODE_STEREO.to_string()),
                rate_control_mode: Some(RATE_CONTROL_CBR.to_string()),
                sample_rate: Some(AAC_SAMPLE_RATE),
                ..AacSettings::default()
            }),
            ..AudioCodecSettings::default()
        }),
        ..AudioDescription::default()
    }
}

/// One built-in ([`TYPE_SYSTEM`]) output preset: the encoder settings its published name
/// states, as a row of parameters.
///
/// A MediaConvert system preset is named after the output it produces — family, container,
/// video codec, audio codec, display aspect ratio, frame size, frame rate and video
/// bitrate — so this row is that name read as data, and [`SystemPreset::settings`] builds
/// the modelled `PresetSettings` graph from it: no member is set that the name does not
/// state. The display aspect ratio a name carries is the ratio of [`Self::width`] to
/// [`Self::height`], so it needs no field of its own.
#[derive(Debug, Clone, Copy)]
pub(super) struct SystemPreset {
    /// The published `Name`, which is this preset's identity.
    pub(super) name: &'static str,
    /// A [`CONTAINER_MP4`]-style `ContainerType` value.
    pub(super) container: &'static str,
    /// A [`VIDEO_CODEC_AVC`]-style `VideoCodec` value.
    pub(super) video_codec: &'static str,
    /// `VideoDescription$Width`, "the video resolution width, in pixels, for this output".
    pub(super) width: i32,
    /// `VideoDescription$Height`, the matching resolution height.
    pub(super) height: i32,
    /// The `Hz` the name states, as the numerator of the
    /// `FramerateNumerator`/`FramerateDenominator` fraction the codec settings carry.
    pub(super) framerate_numerator: i32,
    /// The denominator of that fraction.
    pub(super) framerate_denominator: i32,
    /// The `Mbps` the name states, as the codec's `Bitrate`: "the average bitrate in bits
    /// per second".
    pub(super) video_bitrate: i32,
    /// `AacSettings$Bitrate` for the output's audio track, in bits per second.
    pub(super) audio_bitrate: i32,
}

impl SystemPreset {
    /// The modelled `PresetSettings` this row states: the container, the video
    /// description with its codec settings, and the one AAC audio description.
    pub(super) fn settings(&self) -> PresetSettings {
        PresetSettings {
            container_settings: Some(ContainerSettings {
                container: Some(self.container.to_string()),
                ..ContainerSettings::default()
            }),
            video_description: Some(VideoDescription {
                width: Some(self.width),
                height: Some(self.height),
                codec_settings: Some(video_codec_settings(
                    self.video_codec,
                    Some(EncodeRate {
                        bitrate: self.video_bitrate,
                        framerate_numerator: self.framerate_numerator,
                        framerate_denominator: self.framerate_denominator,
                    }),
                )),
                ..VideoDescription::default()
            }),
            audio_descriptions: Some(vec![aac_audio_description(self.audio_bitrate)]),
            // "For each output that has captions, include one instance of
            // CaptionDescriptions": a built-in video preset names none.
            caption_descriptions: None,
        }
    }
}

/// The built-in ([`TYPE_SYSTEM`]) output presets this store is seeded with.
///
/// That an account holds presets it did not create is what `Preset$Type` ("A preset can be
/// of two types: system or custom. System or built-in preset can't be modified or deleted
/// by the user.") and `PresetListBy`'s `SYSTEM` value say: a caller builds a job from them
/// without creating anything. The model carries no catalogue of them, so this SET is the
/// emulator's own, named after the system presets AWS Elemental MediaConvert publishes, and
/// its size is well inside [`MAX_PRESETS`]; nothing in the emulator keys on a particular
/// entry, because a built-in preset is recognised by its `Type` and never by its name.
///
/// Each entry carries the encoder settings its published name states — the container, the
/// codecs, the frame size, the frame rate and the bitrate are all in the name — so the
/// settings here are that name read as data rather than a graph invented for it, exactly as
/// [`SYSTEM_JOB_TEMPLATES`]' entries are.
pub(super) const SYSTEM_PRESETS: [SystemPreset; 11] = [
    SystemPreset {
        name: "System-Generic_Hd_Mp4_Avc_Aac_16x9_1920x1080p_24Hz_6Mbps",
        container: CONTAINER_MP4,
        video_codec: VIDEO_CODEC_AVC,
        width: 1920,
        height: 1080,
        framerate_numerator: 24,
        framerate_denominator: 1,
        video_bitrate: 6_000_000,
        audio_bitrate: 128_000,
    },
    SystemPreset {
        name: "System-Generic_Hd_Mp4_Avc_Aac_16x9_1280x720p_24Hz_4.5Mbps",
        container: CONTAINER_MP4,
        video_codec: VIDEO_CODEC_AVC,
        width: 1280,
        height: 720,
        framerate_numerator: 24,
        framerate_denominator: 1,
        video_bitrate: 4_500_000,
        audio_bitrate: 96_000,
    },
    SystemPreset {
        name: "System-Generic_Sd_Mp4_Avc_Aac_4x3_640x480p_30Hz_1.5Mbps",
        container: CONTAINER_MP4,
        video_codec: VIDEO_CODEC_AVC,
        width: 640,
        height: 480,
        framerate_numerator: 30,
        framerate_denominator: 1,
        video_bitrate: 1_500_000,
        audio_bitrate: 96_000,
    },
    SystemPreset {
        name: "System-Generic_Sd_Mp4_Avc_Aac_16x9_640x360p_30Hz_0.6Mbps",
        container: CONTAINER_MP4,
        video_codec: VIDEO_CODEC_AVC,
        width: 640,
        height: 360,
        framerate_numerator: 30,
        framerate_denominator: 1,
        video_bitrate: 600_000,
        audio_bitrate: 64_000,
    },
    SystemPreset {
        name: "System-Generic_Uhd_Mp4_Hevc_Aac_16x9_3840x2160p_24Hz_20Mbps",
        container: CONTAINER_MP4,
        video_codec: VIDEO_CODEC_HEVC,
        width: 3840,
        height: 2160,
        framerate_numerator: 24,
        framerate_denominator: 1,
        video_bitrate: 20_000_000,
        audio_bitrate: 128_000,
    },
    SystemPreset {
        name: "System-Ott_Hls_Ts_Avc_Aac_16x9_1920x1080p_30Hz_8.5Mbps",
        container: CONTAINER_M3U8,
        video_codec: VIDEO_CODEC_AVC,
        width: 1920,
        height: 1080,
        framerate_numerator: 30,
        framerate_denominator: 1,
        video_bitrate: 8_500_000,
        audio_bitrate: 128_000,
    },
    SystemPreset {
        name: "System-Ott_Hls_Ts_Avc_Aac_16x9_1280x720p_30Hz_3.5Mbps",
        container: CONTAINER_M3U8,
        video_codec: VIDEO_CODEC_AVC,
        width: 1280,
        height: 720,
        framerate_numerator: 30,
        framerate_denominator: 1,
        video_bitrate: 3_500_000,
        audio_bitrate: 96_000,
    },
    SystemPreset {
        name: "System-Ott_Hls_Ts_Avc_Aac_16x9_960x540p_30Hz_1.5Mbps",
        container: CONTAINER_M3U8,
        video_codec: VIDEO_CODEC_AVC,
        width: 960,
        height: 540,
        framerate_numerator: 30,
        framerate_denominator: 1,
        video_bitrate: 1_500_000,
        audio_bitrate: 96_000,
    },
    SystemPreset {
        name: "System-Ott_Hls_Ts_Avc_Aac_16x9_640x360p_30Hz_0.6Mbps",
        container: CONTAINER_M3U8,
        video_codec: VIDEO_CODEC_AVC,
        width: 640,
        height: 360,
        framerate_numerator: 30,
        framerate_denominator: 1,
        video_bitrate: 600_000,
        audio_bitrate: 64_000,
    },
    SystemPreset {
        name: "System-Ott_Dash_Mp4_Avc_Aac_16x9_1280x720p_30Hz_3.5Mbps",
        container: CONTAINER_MPD,
        video_codec: VIDEO_CODEC_AVC,
        width: 1280,
        height: 720,
        framerate_numerator: 30,
        framerate_denominator: 1,
        video_bitrate: 3_500_000,
        audio_bitrate: 96_000,
    },
    SystemPreset {
        name: "System-Ott_Dash_Mp4_Avc_Aac_16x9_640x360p_30Hz_0.6Mbps",
        container: CONTAINER_MPD,
        video_codec: VIDEO_CODEC_AVC,
        width: 640,
        height: 360,
        framerate_numerator: 30,
        framerate_denominator: 1,
        video_bitrate: 600_000,
        audio_bitrate: 64_000,
    },
];
