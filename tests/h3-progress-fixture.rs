//! Deterministic stdout from a model-free worker. Deliberately produces no video.
use std::{
    env,
    io::{self, Write},
    thread,
    time::Duration,
};

fn main() {
    let args: Vec<_> = env::args().collect();
    let steps = args.windows(2).find(|pair| pair[0] == "--steps").unwrap()[1]
        .parse::<u32>()
        .unwrap();
    let lines = [
        "[I] MiniMax-H3 FL2VA".to_string(),
        format!("|====| {steps}/{steps} - 1.45it/s"),
        "[I] get_learned_condition completed, taking 239.74s".to_string(),
        format!("|####| {steps}/{steps} - 35.31MB/s"),
        format!("|===>| 1/{steps} - 24.21s/it - ETA 01:34\x1b[K[I] Struve H3 sequence: 535 tokens"),
        format!("|====| {steps}/{steps} - 25.32s/it\x1b[K[I] sampling completed, taking 99.75s"),
        "[I] decode_first_stage completed, taking 3.25s".to_string(),
    ];
    for line in lines {
        print!("{line}\r");
        io::stdout().flush().unwrap();
        thread::sleep(Duration::from_millis(900));
    }
    std::process::exit(2);
}
