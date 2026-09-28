//! Offline processing: `shodan-voice --render in.wav out.wav [--preset NAME] [--seed N]`.

use crate::audio::engine::Engine;
use crate::presets;
use crate::shared::Shared;
use std::sync::Arc;

pub fn run(args: &[String]) -> Result<(), String> {
    let mut positional = Vec::new();
    let mut preset = "ss1".to_string();
    let mut seed = 1u64;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--preset" => preset = it.next().ok_or("--preset needs a name")?.clone(),
            "--seed" => seed = it.next().ok_or("--seed needs a number")?.parse().map_err(|e| format!("bad seed: {e}"))?,
            _ => positional.push(a.clone()),
        }
    }
    let [input, output] = positional.as_slice() else {
        return Err("usage: shodan-voice --render in.wav out.wav [--preset NAME] [--seed N]".into());
    };

    let shared = Arc::new(Shared::default());
    let preset = presets::builtin(&preset).ok_or_else(|| {
        let names: Vec<_> = presets::BUILTIN.iter().map(|p| p.name).collect();
        format!("unknown preset '{preset}', choose one of: {}", names.join(", "))
    })?;
    preset.apply(&shared.params);

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

    let mut engine = Engine::new(spec.sample_rate as f32, shared.clone(), seed, None);
    let (mut l, mut r) = (vec![0.0; mono.len()], vec![0.0; mono.len()]);
    for (i, chunk) in mono.chunks(512).enumerate() {
        let s = i * 512;
        let e = s + chunk.len();
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
