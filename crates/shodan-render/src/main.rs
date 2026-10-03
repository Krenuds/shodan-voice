//! Headless offline render: `shodan-render in.wav|- out.wav|- [--preset NAME] [--seed N] [--set knob=value] [--rack id,id] [--say TEXT]`.
//! The same as `shodan-voice --render`; `-` reads stdin / writes stdout so audio can be piped through.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = shodan_core::render::run("shodan-render", &args) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
