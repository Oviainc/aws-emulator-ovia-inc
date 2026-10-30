Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 9f84f44c6767c5c874b7cc6e16fb6469b8a0f678

# A blank Output.Preset is silently dropped instead of refused, so a graph that states both Preset and ContainerSettings is stored

_Filed: 2026-10-28T23:42:07Z_
_Operation: CreateJobTemplate | Class: validation_

> _Note: this issue was identified against the code base at commit `9f84f44c6767c5c874b7cc6e16fb6469b8a0f678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every settings graph this crate stores goes through `output_problem`, which decides whether an output states its encoder settings by asking `named_preset`. `named_preset` gives `Output$Preset` the OPTIONAL FREE TEXT reading: a value made only of whitespace is treated as if the member were absent. Every OTHER reference member in this crate gets the opposite reading — the crate's one `carries_no_characters` rule, whose own doc says a blank reference is refused with a 400 "rather than with a lookup of a value nothing could hold" — and that rule is applied even to a NESTED reference inside the same kind of list (`HopDestinations[].Queue`, refused by `hop_destination_blank_queue_problem` as `HopDestinations.Queue must not be empty.`).

The observable consequence is a request that is accepted where its non-blank twin is refused. An output carrying `"preset": "   "` TOGETHER WITH `"containerSettings": {"container": "MP4"}` passes `output_problem`: `named_preset` returns `None`, so the `BothPresetAndContainer` arm is never reached and the `NoContainer` arm is skipped because the container is there. The request answers 201/200 and the blank `Preset` is persisted verbatim in the stored `Settings`/`JobTemplateSettings`, so `GetJobTemplate`, `ListJobTemplates`, `GetJob`, `ListJobs`, `SearchJobs` and `GetJobsQueryResults` all echo back an output that states its encoder settings twice, one of which resolves to no preset at all. The identical request with a non-blank preset name is refused with `BothPresetAndContainer` (400), and the identical blank preset WITHOUT container settings is refused — but for the wrong reason (`NoContainer`, "must specify either Preset or ContainerSettings.Container"), never as the blank reference it is.

This affects both doors into a stored template (`CreateJobTemplate` -> `job_template_settings_problem`, `UpdateJobTemplate` -> same) and the job door (`CreateJob` -> `job_settings_problem`); all three reach the one `output_problem`/`named_preset` pair in the state module.

## Evidence

The crate's one rule for a reference member, with the list of members it governs — services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:588-612:

    /// Whether a string member the caller SENT carries no characters at all.
    ///
    /// Every member in this service that names a resource — the `Name` of a queue, a preset or
    /// a job template, and the references (`Queue`, `JobTemplate`, `Role`, the `JobId` a share
    /// names, the `Arn` of a certificate or of a tagged resource) that point at one — targets
    /// the model's bare `__string` ... Neither carries a resource identity: a key with no characters addresses nothing
    /// a later read can reach, and a reference with none resolves to nothing. ONE test for all
    /// of them, so no door that mints or resolves such a member can disagree about which
    /// values are identities, answered with the bad request [`blank_member_message`] words
    /// rather than with a lookup of a value nothing could hold.
    pub fn carries_no_characters(value: &str) -> bool { value.trim().is_empty() }

The same rule applied to a NESTED reference of the same shape — state.rs:1064-1078:

    /// `HopDestination$Queue` is a reference to a queue resource exactly as `CreateJob$Queue`
    /// and `CreateJobTemplate$Queue` are ([`hop_destination_queue_name`]), so it takes the same
    /// test every resource-naming member in this service takes ([`carries_no_characters`]): a
    /// hop that states a queue carrying no characters names neither a queue this account holds
    /// nor the member left out ... so it is the bad request [`blank_member_message`] words
    pub fn hop_destination_blank_queue_problem(hops: &[HopDestination]) -> Option<String> {
        hop_destination_queues(hops).any(carries_no_characters).then(|| blank_member_message("HopDestinations.Queue"))
    }

The other side — the one place `Output$Preset` is read, which drops a blank value instead (state.rs:1287-1298):

    /// The preset an output names, or `None` when it names none.
    /// ... A value made only of whitespace carries no name at all, the same reading every other
    /// optional free-text member in this module gets ([`optional_free_text`]).
    fn named_preset(output: &Output) -> Option<&str> {
        output.preset.as_deref().filter(|preset| !preset.trim().is_empty())
    }

...and the branch that is therefore skipped — state.rs:1270-1278:

    let container = container_settings_name_a_container(entry.container_settings.as_ref());
    let Some(preset) = named_preset(entry) else {
        return (!container).then_some(SettingsProblem::NoContainer { group, output });
    };
    if container { return Some(SettingsProblem::BothPresetAndContainer { group, output }); }

The model sentence the skipped branch enforces (`Output$Preset`): "Use Preset to specify a preset for your transcoding settings. Provide the system or custom preset name. You can specify either Preset or Container settings, but not both."

Test coverage: the blank preset is only exercised WITHOUT container settings, and is pinned to the wrong refusal — tests/all/output_preset_reference.rs:258-262

        Row::refused(
            "a preset name made only of whitespace",
            vec![vec![output(Some("   "), None)]],
            &["OutputGroups[0].Outputs[0]"],
        ),

and tests/all/job_template_settings_validation.rs:223-232 (`case: "an output whose preset reference is blank"`, `names: "OutputGroups[0].Outputs[0]"`). No test sends a blank `preset` ALONGSIDE `containerSettings.container`, which is the accepted-and-persisted case above.

## Steps to Reproduce

1. Boot the emulator and point a mediaconvert client at it.
2. create_job_template(Name='blank-preset', Settings={'Inputs':[{}],'OutputGroups':[{'OutputGroupSettings':{'Type':'FILE_GROUP_SETTINGS','FileGroupSettings':{'Destination':'s3://bucket/out/'}},'Outputs':[{'NameModifier':'_1','Preset':'   ','ContainerSettings':{'Container':'MP4'}}]}]}).
3. Observe HTTP 201. Expected: the same BadRequestException the identical request with a non-blank Preset gets (SettingsProblem::BothPresetAndContainer, "you can specify either Preset or Container settings, but not both"), or the blank-member refusal every other reference in the crate answers (`blank_member_message`).
4. get_job_template(Name='blank-preset') and observe the stored output still carries `"preset": "   "` beside its container settings — a reference no reader can resolve, persisted into state.
5. Repeat the same body against update_job_template(Name=...) and against create_job(Role=..., Settings=<same graph plus an Inputs[].FileInput>): both accept it too, because all three doors reach the one `output_problem`/`named_preset` pair.
6. Contrast: the same output with `"queue": "   "` placed in `hopDestinations` instead is refused 400 with `HopDestinations.Queue must not be empty.`, showing the two readings of a blank nested reference inside this crate.
