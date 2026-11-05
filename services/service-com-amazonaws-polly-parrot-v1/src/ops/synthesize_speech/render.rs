// SPDX-License-Identifier: CC0-1.0

//! The synthesiser: it turns a planned utterance into a waveform.
//!
//! This is a source-filter (formant) synthesiser — the classic model of the human vocal
//! apparatus. For every segment of the plan it excites a source and filters it through
//! the resonances of the tract holding that phoneme's shape:
//!
//! * a voiced sound is a train of glottal pulses at the fundamental frequency, passed
//!   through three resonators tuned to the phoneme's first three formants;
//! * a fricative is turbulent noise through a resonator at the constriction's own
//!   frequency, and a voiced fricative is both at once;
//! * a stop is a closure and then a burst of noise at the place of articulation;
//! * a whisper replaces the glottal pulses with breath noise through the same formants;
//! * the formants GLIDE from the previous phoneme's to this one's, which is what makes a
//!   sequence of phonemes a continuous utterance rather than a sequence of tones.
//!
//! Everything about the result follows from the text: which phonemes, in which order,
//! how long each lasts, how loud and how high. The one thing it does NOT come from is a
//! recording: there is no stored audio here, nothing is echoed back, and the samples are
//! computed from the plan every time.

use super::plan::{Prosody, Utterance};
use super::phonemes::Manner;

/// The timbre of the voice the request named.
#[derive(Debug, Clone, Copy)]
pub struct Timbre {
    /// The fundamental frequency of the voice, in Hz: how high it speaks.
    pub f0_hz: f32,
}

impl Timbre {
    /// The timbre of a voice of this gender, as the voice catalog reports gender.
    ///
    /// The two frequencies are the usual averages for adult speakers; a voice the
    /// catalog does not carry is spoken at the middle of the two, so every `VoiceId`
    /// the model admits is still spoken.
    pub fn for_gender(gender: Option<&str>) -> Self {
        let f0_hz = match gender {
            Some("Male") => 110.0,
            Some("Female") => 200.0,
            _ => 155.0,
        };
        Self { f0_hz }
    }
}

/// How far into a phoneme the formants have arrived at its own targets.
const GLIDE_FRACTION: f32 = 0.4;
/// How much the voice falls across an utterance, as a fraction of its pitch.
const DECLINATION: f32 = 0.12;
/// The fade at each end of a segment, in milliseconds, so no segment boundary clicks.
const FADE_MS: f32 = 4.0;
/// How much of a stop is closure before its burst.
const CLOSURE_FRACTION: f32 = 0.55;
/// The peak the finished waveform is scaled to, leaving headroom below full scale.
const PEAK: f32 = 0.85;

/// The most samples one utterance renders at `sample_rate`.
///
/// This is [`super::plan::MAX_UTTERANCE_MS`] in samples, and it is the bound that SIZES
/// the buffer below: the renderer reserves at most this many samples and stops producing
/// when it reaches them, so no plan — however long an SSML document asks for — makes this
/// function allocate past it. A caller that needs to refuse rather than truncate asks
/// for this figure first and compares it with its own budget.
pub fn max_samples(sample_rate: u32) -> usize {
    let samples = f64::from(sample_rate.max(1)) * f64::from(super::plan::MAX_UTTERANCE_MS) / 1000.0;
    samples as usize + 1
}

/// Render `utterance` as mono samples in `[-1.0, 1.0]` at `sample_rate`.
///
/// Never longer than [`max_samples`]: the budget is applied before the buffer is
/// reserved and again before each segment, so the allocation is bounded whatever the
/// plan carries.
pub fn render(utterance: &Utterance, sample_rate: u32, timbre: Timbre) -> Vec<f32> {
    let fs = sample_rate.max(1) as f32;
    let total_ms = utterance.duration_ms().max(1.0);
    let budget = max_samples(sample_rate);
    let mut synth = Synth::new(fs);
    let mut out: Vec<f32> =
        Vec::with_capacity((((total_ms / 1000.0) * fs).max(1.0) as usize + 1).min(budget));
    // Where the formants currently are, so each phoneme glides from the last one.
    let mut formants = [500.0f32, 1500.0, 2500.0];
    let mut elapsed_ms = 0.0f32;
    // The loudness each stretch is spoken at, and how many samples it covers. It is
    // applied AFTER the waveform is scaled to the format's range, so a `<prosody volume>`
    // change survives that scaling rather than being undone by it.
    let mut loudness: Vec<(usize, f32)> = Vec::new();

    for segment in &utterance.segments {
        // The bound is consulted BEFORE each segment is produced, so the buffer cannot
        // grow past what was reserved for it.
        if out.len() >= budget {
            break;
        }
        let count = (((segment.duration_ms / 1000.0) * fs).round() as usize)
            .min(budget - out.len());
        if count == 0 {
            continue;
        }
        let Some(phoneme) = segment.phoneme else {
            // A pause is silence, and the tract relaxes during it.
            out.resize(out.len() + count, 0.0);
            loudness.push((count, 1.0));
            elapsed_ms += segment.duration_ms;
            continue;
        };
        let scale = segment.prosody.formant_scale.clamp(0.5, 2.0);
        let target = scaled(phoneme.formants, scale);
        let final_target = scaled(phoneme.glide_to.unwrap_or(phoneme.formants), scale);
        // A vowel's formants are narrow resonances; every other sound is damped more,
        // so its resonances are broader.
        let wide = phoneme.manner != Manner::Vowel;
        let gain = phoneme.amplitude();
        let closure = (count as f32 * CLOSURE_FRACTION) as usize;
        loudness.push((count, segment.prosody.volume.max(0.0)));

        for index in 0..count {
            let progress = index as f32 / count as f32;
            // The formants reach this phoneme's own targets early in it, then move on
            // to the second target a diphthong or a glide carries.
            let current = if progress < GLIDE_FRACTION {
                blend(formants, target, progress / GLIDE_FRACTION)
            } else {
                blend(
                    target,
                    final_target,
                    (progress - GLIDE_FRACTION) / (1.0 - GLIDE_FRACTION),
                )
            };
            let utterance_progress = (elapsed_ms + progress * segment.duration_ms) / total_ms;
            let f0 = (timbre.f0_hz
                * segment.prosody.pitch.clamp(0.2, 4.0)
                * (1.0 - DECLINATION * utterance_progress))
                .clamp(50.0, 500.0);
            let envelope = fade(index, count, segment.duration_ms);
            let sample = synth.tick(
                phoneme,
                &segment.prosody,
                current,
                wide,
                f0,
                index,
                closure,
                count,
            );
            out.push(sample * gain * envelope);
        }
        formants = final_target;
        elapsed_ms += segment.duration_ms;
    }
    normalize(&mut out);
    apply_loudness(&mut out, &loudness);
    out
}

/// Apply the loudness of each stretch, in order, and keep every sample inside the range
/// the encodings carry.
fn apply_loudness(samples: &mut [f32], loudness: &[(usize, f32)]) {
    let mut at = 0usize;
    for (count, volume) in loudness {
        let end = (at + count).min(samples.len());
        for sample in &mut samples[at..end] {
            *sample = (*sample * volume).clamp(-1.0, 1.0);
        }
        at = end;
    }
}

/// The formants of a phoneme, scaled by a vocal-tract-length change.
fn scaled(formants: [f32; 3], scale: f32) -> [f32; 3] {
    [
        formants[0] * scale,
        formants[1] * scale,
        formants[2] * scale,
    ]
}

/// The formants `fraction` of the way from `from` to `to`.
fn blend(from: [f32; 3], to: [f32; 3], fraction: f32) -> [f32; 3] {
    let fraction = fraction.clamp(0.0, 1.0);
    [
        from[0] + (to[0] - from[0]) * fraction,
        from[1] + (to[1] - from[1]) * fraction,
        from[2] + (to[2] - from[2]) * fraction,
    ]
}

/// The amplitude envelope at sample `index` of a segment of `count` samples: a short
/// raised-cosine fade at each end, so one segment runs into the next without a click.
fn fade(index: usize, count: usize, duration_ms: f32) -> f32 {
    if count <= 2 {
        return 1.0;
    }
    let fade_samples = ((FADE_MS / duration_ms.max(0.001)) * count as f32)
        .clamp(1.0, count as f32 / 2.0) as usize;
    let rise = if index < fade_samples {
        index as f32 / fade_samples as f32
    } else {
        1.0
    };
    let fall = if index + fade_samples >= count {
        (count - index) as f32 / fade_samples as f32
    } else {
        1.0
    };
    0.5 * (1.0 - (std::f32::consts::PI * rise).cos()) * 0.5
        * (1.0 - (std::f32::consts::PI * fall).cos())
}

/// Scale the waveform so its loudest sample sits at [`PEAK`], leaving a signal that
/// uses the format's range without clipping. An utterance that is silent throughout —
/// a `<prosody volume="silent">` one — stays silent.
fn normalize(samples: &mut [f32]) {
    let peak = samples
        .iter()
        .fold(0.0f32, |loudest, sample| loudest.max(sample.abs()));
    if peak <= f32::EPSILON {
        return;
    }
    let factor = PEAK / peak;
    for sample in samples.iter_mut() {
        *sample *= factor;
    }
}

/// The state the synthesiser carries from one sample to the next: the resonators'
/// memory, where the glottal cycle is, and the noise generator.
struct Synth {
    /// The sample rate, in Hz.
    fs: f32,
    /// The three formant resonators, in cascade.
    formants: [Resonator; 3],
    /// The resonator the frication and burst noise is shaped by.
    noise: Resonator,
    /// Where in the glottal cycle the source is, in `[0, 1)`.
    phase: f32,
    /// The noise generator's state.
    seed: u32,
}

impl Synth {
    fn new(fs: f32) -> Self {
        Self {
            fs,
            formants: [Resonator::default(); 3],
            noise: Resonator::default(),
            phase: 0.0,
            // A FIXED seed: the same request renders the same bytes every time.
            seed: 0x5065_6563,
        }
    }

    /// One sample of `phoneme`.
    #[allow(clippy::too_many_arguments)]
    fn tick(
        &mut self,
        phoneme: &'static super::phonemes::Phoneme,
        prosody: &Prosody,
        formants: [f32; 3],
        wide: bool,
        f0: f32,
        index: usize,
        closure: usize,
        count: usize,
    ) -> f32 {
        let bandwidths: [f32; 3] = if wide {
            [130.0, 180.0, 240.0]
        } else {
            [90.0, 110.0, 170.0]
        };
        for (resonator, (frequency, bandwidth)) in self
            .formants
            .iter_mut()
            .zip(formants.iter().zip(bandwidths.iter()))
        {
            resonator.tune(*frequency, *bandwidth, self.fs);
        }

        // The glottal source, or the breath that replaces it in a whisper.
        let voiced = phoneme.voiced && !prosody.whispered;
        let glottal = if voiced {
            self.glottal_pulse(f0)
        } else if prosody.whispered {
            0.5 * self.white_noise()
        } else {
            0.0
        };

        match phoneme.manner {
            Manner::Stop => {
                // A closure is near-silence (a voiced stop keeps a weak low buzz), and
                // the burst that releases it is noise at the place of articulation,
                // dying away into the following sound.
                if index < closure {
                    let buzz = if voiced { 0.12 * glottal } else { 0.0 };
                    self.cascade(buzz)
                } else {
                    let burst_length = (count - closure).max(1) as f32;
                    let decay = 1.0 - (index - closure) as f32 / burst_length;
                    self.noise
                        .tune(cap(phoneme.noise_hz, self.fs), 1500.0, self.fs);
                    let excitation = self.white_noise();
                    let noise = self.noise.feed(excitation) * decay * 2.0;
                    noise + self.cascade(0.3 * glottal)
                }
            }
            Manner::Fricative => {
                self.noise
                    .tune(cap(phoneme.noise_hz, self.fs), 1200.0, self.fs);
                let excitation = self.white_noise();
                let frication = self.noise.feed(excitation) * 2.5;
                // A voiced fricative is frication and voicing at once.
                frication + self.cascade(glottal) * if voiced { 0.6 } else { 0.0 }
            }
            Manner::Aspirate => {
                self.noise
                    .tune(cap(phoneme.noise_hz, self.fs), 1800.0, self.fs);
                let excitation = self.white_noise();
                let breath = self.noise.feed(excitation);
                self.cascade(breath)
            }
            // Vowels, approximants and nasals are the source through the formants.
            _ => self.cascade(glottal),
        }
    }

    /// The three formant resonators, in cascade.
    fn cascade(&mut self, excitation: f32) -> f32 {
        let mut signal = excitation;
        for resonator in self.formants.iter_mut() {
            signal = resonator.feed(signal);
        }
        signal
    }

    /// One sample of the glottal waveform at `f0`: a smooth pulse over the open phase
    /// of each cycle, with its mean removed so the source carries no offset.
    fn glottal_pulse(&mut self, f0: f32) -> f32 {
        /// How much of the cycle the folds are open for.
        const OPEN: f32 = 0.4;
        self.phase += f0 / self.fs;
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
        }
        let shape = if self.phase < OPEN {
            let within = std::f32::consts::PI * self.phase / OPEN;
            within.sin() * within.sin()
        } else {
            0.0
        };
        (shape - OPEN / 2.0) * 2.0
    }

    /// One sample of white noise, from a deterministic generator.
    fn white_noise(&mut self) -> f32 {
        // xorshift32: a fixed, well-known sequence, so the same text renders the same
        // noise every time.
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        (self.seed as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// A formant below the Nyquist frequency of the stream being written: a resonance above
/// it cannot be represented, so it sits just below instead.
fn cap(frequency: f32, fs: f32) -> f32 {
    frequency.clamp(50.0, fs * 0.45)
}

/// A two-pole resonator — one formant of the vocal tract.
///
/// The coefficients are the standard ones for a resonance at `frequency` with the
/// bandwidth `bandwidth`: the pole radius is `exp(-pi * bandwidth / fs)` and its angle
/// `2 * pi * frequency / fs`, with the gain chosen so the filter passes a steady input
/// unchanged.
#[derive(Debug, Clone, Copy, Default)]
struct Resonator {
    a: f32,
    b: f32,
    c: f32,
    /// The last two outputs.
    y1: f32,
    y2: f32,
    /// What the filter is currently tuned to, so it is only recomputed when it moves.
    tuned_to: (f32, f32),
}

impl Resonator {
    fn tune(&mut self, frequency: f32, bandwidth: f32, fs: f32) {
        let frequency = cap(frequency, fs);
        if self.tuned_to == (frequency, bandwidth) {
            return;
        }
        let radius = (-std::f32::consts::PI * bandwidth / fs).exp();
        let angle = 2.0 * std::f32::consts::PI * frequency / fs;
        self.c = -radius * radius;
        self.b = 2.0 * radius * angle.cos();
        self.a = 1.0 - self.b - self.c;
        self.tuned_to = (frequency, bandwidth);
    }

    fn feed(&mut self, excitation: f32) -> f32 {
        let output = self.a * excitation + self.b * self.y1 + self.c * self.y2;
        self.y2 = self.y1;
        self.y1 = output;
        output
    }
}

#[cfg(test)]
mod tests {
    use super::super::lexicon::Lexicons;
    use super::super::plan::plan;
    use super::*;

    /// The waveform of `text`, at `sample_rate`.
    fn waveform(text: &str, sample_rate: u32) -> Vec<f32> {
        let utterance = plan(text, "text", &Lexicons::default()).expect("text plans");
        render(&utterance, sample_rate, Timbre::for_gender(Some("Female")))
    }

    /// RULE: the waveform is a signal — it varies, it is as long as the utterance, and
    /// it is bounded.
    #[test]
    fn the_waveform_is_a_bounded_varying_signal() {
        for rate in [8000u32, 16000, 22050, 24000, 44100, 48000] {
            let samples = waveform("Testing the synthesiser.", rate);
            let distinct = samples
                .iter()
                .map(|sample| (sample * 1000.0) as i32)
                .collect::<std::collections::BTreeSet<_>>()
                .len();
            assert!(
                distinct > 100,
                "rate {rate}: expected a varying signal, got {distinct} distinct levels"
            );
            let peak = samples.iter().fold(0.0f32, |top, s| top.max(s.abs()));
            assert!(
                (0.5..=1.0).contains(&peak),
                "rate {rate}: expected a signal near full scale, got a peak of {peak}"
            );
            assert!(
                samples.iter().all(|sample| sample.is_finite()),
                "rate {rate}: every sample must be finite"
            );
        }
    }

    /// RULE: different text renders different audio, and the same text renders the same
    /// audio every time.
    #[test]
    fn the_waveform_follows_the_text() {
        let first = waveform("Hello there.", 16000);
        let again = waveform("Hello there.", 16000);
        assert_eq!(first, again, "the same text must render the same samples");
        let other = waveform("Something else entirely.", 16000);
        assert_ne!(
            first.len(),
            other.len(),
            "different text must render a different stream"
        );
    }

    /// RULE: the voice's gender decides how high it speaks, so two voices render
    /// differently.
    #[test]
    fn the_voice_decides_the_pitch() {
        let utterance = plan("One two three.", "text", &Lexicons::default()).expect("plans");
        let female = render(&utterance, 16000, Timbre::for_gender(Some("Female")));
        let male = render(&utterance, 16000, Timbre::for_gender(Some("Male")));
        assert_eq!(
            female.len(),
            male.len(),
            "the same utterance lasts as long in either voice"
        );
        assert_ne!(
            female, male,
            "a male and a female voice must not render the same samples"
        );
    }
}
