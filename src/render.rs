//! Offline processing: `shodan-voice --render in.wav out.wav [--preset NAME] [--seed N] [--set knob=value ...] [--rack id,id,...] [--say TEXT] [--hear]`.

use crate::audio::engine::Engine;
use crate::lexicon;
use crate::modules::{self, titobot};
use crate::presets;
use crate::shared::{Shared, WordBus};
use crate::speech::{self, segment, segment::Segmenter};
use std::sync::Arc;

/// With `--hear`, how long after an utterance ends its words arrive: a typical recognition time
/// on the GPU, fixed so renders stay reproducible.
const HEAR_DELAY_S: f32 = 0.05;

pub fn run(args: &[String]) -> Result<(), String> {
    let mut positional = Vec::new();
    let mut preset = "ss1".to_string();
    let mut seed = 1u64;
    let mut overrides = Vec::new();
    let mut rack_ids = None;
    let mut say = None;
    let mut hear = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--preset" => preset = it.next().ok_or("--preset needs a name")?.clone(),
            "--set" => overrides.push(it.next().ok_or("--set needs key=value")?.clone()),
            "--rack" => rack_ids = Some(it.next().ok_or("--rack needs a list of module ids")?.clone()),
            "--hear" => hear = true,
            "--say" => say = Some(it.next().ok_or("--say needs the text to speak")?.clone()),
            "--seed" => seed = it.next().ok_or("--seed needs a number")?.parse().map_err(|e| format!("bad seed: {e}"))?,
            _ => positional.push(a.clone()),
        }
    }
    let [input, output] = positional.as_slice() else {
        return Err("usage: shodan-voice --render in.wav out.wav [--preset NAME] [--seed N] [--set knob=value] [--rack id,id] [--say TEXT] [--hear]".into());
    };

    let shared = Arc::new(Shared::default());
    let rack = match &rack_ids {
        None => modules::default_instances(),
        Some(ids) => ids
            .split(',')
            .map(|id| {
                modules::kind(id.trim()).map(modules::Instance::new).ok_or_else(|| {
                    let ids: Vec<_> = modules::NATIVE.iter().map(|k| k.id).collect();
                    format!("unknown module '{id}', choose from: {}", ids.join(", "))
                })
            })
            .collect::<Result<_, _>>()?,
    };
    let targets = presets::targets(&shared.io, rack.iter().map(|i| &i.shared.params));
    let preset = presets::builtin(&preset).ok_or_else(|| {
        let names: Vec<_> = presets::BUILTIN.iter().map(|p| p.name).collect();
        format!("unknown preset '{preset}', choose one of: {}", names.join(", "))
    })?;
    preset.apply(&targets);
    for o in &overrides {
        let (key, value) = o.split_once('=').ok_or_else(|| format!("--set expects knob=value, got '{o}'"))?;
        let value = value.parse().map_err(|e| format!("bad value for {key}: {e}"))?;
        if !targets.iter().any(|p| p.set_key(key, value)) {
            let keys: Vec<_> = targets.iter().flat_map(|p| p.defs()).map(|d| d.key).collect();
            return Err(format!("unknown knob '{key}', choose one of: {}", keys.join(", ")));
        }
    }

    let mut reader = hound::WavReader::open(input).map_err(|e| format!("{input}: {e}"))?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>(),
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().map(|s| s.map(|v| v as f32 * scale)).collect()
        }
    }
    .map_err(|e| format!("{input}: {e}"))?;
    let mono: Vec<f32> = samples.chunks(channels).map(|f| f.iter().sum::<f32>() / channels as f32).collect();

    let sr = spec.sample_rate as f32;
    let mut engine = Engine::new(sr, shared.clone(), modules::build(&rack, sr, seed, &shared), None);
    // After the modules exist: they only hear words said from then on.
    if let Some(text) = &say {
        let unknown = lexicon::say(text, &shared.words);
        if !unknown.is_empty() {
            println!("Not in the lexicon (silent): {}", unknown.join(" "));
        }
    }
    // What a recogniser hears in the input, each utterance due when a live one would deliver it.
    let mut heard = Vec::new();
    if hear {
        let gate = rack.iter().find(|i| i.kind.id == titobot::KIND.id).map_or(-40.0, |i| i.shared.params.get(titobot::P::TitoGate));
        let mut recognizer = speech::Recognizer::new()?;
        let mut segmenter = Segmenter::new(sr);
        let mut utterances = Vec::new();
        for chunk in mono.chunks(512) {
            segmenter.push(chunk, gate, |u| utterances.push(u));
        }
        segmenter.finish(|u| utterances.push(u));
        for u in utterances {
            let started = std::time::Instant::now();
            let text = recognizer.transcribe(&u.audio)?;
            let unknown = lexicon::say(&text, &WordBus::default());
            println!(
                "{:6.2} s  \"{text}\"  ({:.1} s of speech, recognised in {:.0} ms){}",
                u.end as f32 / sr,
                u.audio.len() as f32 / segment::RATE as f32,
                started.elapsed().as_secs_f32() * 1000.0,
                if unknown.is_empty() { String::new() } else { format!("  not in the lexicon: {}", unknown.join(" ")) }
            );
            heard.push((u.end as usize + (HEAR_DELAY_S * sr) as usize, text));
        }
    }
    let mut heard = heard.into_iter().peekable();

    let (mut l, mut r) = (vec![0.0; mono.len()], vec![0.0; mono.len()]);
    for (i, chunk) in mono.chunks(512).enumerate() {
        let s = i * 512;
        let e = s + chunk.len();
        while let Some((_, text)) = heard.next_if(|(due, _)| *due <= s) {
            lexicon::say(&text, &shared.words);
        }
        engine.process(chunk, &mut l[s..e], &mut r[s..e]);
    }

    let out_spec = hound::WavSpec { channels: 2, sample_rate: spec.sample_rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut writer = hound::WavWriter::create(output, out_spec).map_err(|e| format!("{output}: {e}"))?;
    for (a, b) in l.iter().zip(&r) {
        writer.write_sample(*a).and_then(|_| writer.write_sample(*b)).map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())?;
    println!(
        "Rendered {} s with preset '{}' to {output} ({} glitches)",
        mono.len() / spec.sample_rate as usize,
        preset.name,
        shared.meters.glitches.load(std::sync::atomic::Ordering::Relaxed)
    );
    Ok(())
}
