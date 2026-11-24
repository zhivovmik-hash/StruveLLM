//! Read the H3 sampler's own progress lines. Other bars in the same log count
//! tensors, reference encoding or VAE tiles and must not count as diffusion steps.
use serde_json::{json, Value};
use std::fs::File;
use std::io::Read;
use std::path::Path;

pub(crate) struct H3Progress {
    total: u64,
    stage: &'static str,
    completed: u64,
    file: Option<File>,
    line: Vec<u8>,
    skip_line: bool,
}

impl H3Progress {
    pub(crate) fn new(total: u64) -> Self {
        Self {
            total,
            stage: "",
            completed: 0,
            file: None,
            line: Vec::new(),
            skip_line: false,
        }
    }

    fn consume_line(&mut self) {
        let line = String::from_utf8_lossy(&self.line);
        if self.stage.is_empty()
            && (line.starts_with("[I] running in MiniMax H3 AV FLOW mode")
                || line.starts_with("[I] MiniMax-H3 FL2VA"))
        {
            self.stage = "conditioning";
        } else if line
            .strip_prefix("[I] generate_video ")
            .is_some_and(|rest| rest.as_bytes().first().is_some_and(u8::is_ascii_digit))
            || line.starts_with("[I] get_learned_condition completed")
        {
            self.stage = "diffusion";
        } else if line.starts_with("[I] sampling completed") {
            self.stage = "vae_decode";
            self.completed = self.total;
        } else if line.starts_with("[I] decode_first_stage completed") {
            self.stage = "writing_video";
        } else if self.stage == "diffusion" {
            if let Some(done) = sampler_step(&line, self.total) {
                self.completed = self.completed.max(done)
            }
        }
    }

    fn consume(&mut self, bytes: &[u8]) -> Option<Value> {
        let before = (self.stage, self.completed);
        for &byte in bytes {
            // sd.cpp clears the remainder of the line after the progress bar.
            // A newline may arrive a full step later.
            let clear_to_end = byte == b'K' && self.line.ends_with(b"\x1b[");
            if clear_to_end {
                self.line.truncate(self.line.len() - 2)
            }
            if matches!(byte, b'\r' | b'\n') || clear_to_end {
                if !self.skip_line && !self.line.is_empty() {
                    self.consume_line()
                }
                self.line.clear();
                self.skip_line = false;
            } else if self.line.len() < 4096 {
                self.line.push(byte);
            } else {
                self.skip_line = true;
            }
        }
        (before != (self.stage, self.completed)).then(|| {
            json!({
                "stage":self.stage, "steps_completed":self.completed, "steps_total":self.total
            })
        })
    }

    pub(crate) fn poll(&mut self, path: &Path) -> Option<Value> {
        if self.file.is_none() {
            self.file = File::open(path).ok()
        }
        let mut bytes = [0; 16 * 1024];
        let count = self.file.as_mut()?.read(&mut bytes).ok()?;
        self.consume(&bytes[..count])
    }
}

fn sampler_step(line: &str, expected: u64) -> Option<u64> {
    let bar = line.trim_start().strip_prefix('|')?;
    let (_, detail) = bar.split_once('|')?;
    let (count, rate_and_eta) = detail.split_once(" - ")?;
    let rate = rate_and_eta.split_ascii_whitespace().next()?;
    let number = rate
        .strip_suffix("s/it")
        .or_else(|| rate.strip_suffix("it/s"))?;
    number.parse::<f64>().ok()?;
    let (done, total) = count.trim().split_once('/')?;
    let done = done.parse().ok()?;
    (total.parse::<u64>().ok()? == expected && done <= expected).then_some(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "set STRUVE_H3_FOUR_STEP_LOG to a completed four-step sd.cpp log"]
    fn replay_recorded_four_step_render() {
        let trace = std::fs::read(std::env::var_os("STRUVE_H3_FOUR_STEP_LOG").expect("trace path"))
            .unwrap();
        let mut parser = H3Progress::new(4);
        let mut actual = Vec::new();
        for chunk in trace.chunks(13) {
            if let Some(value) = parser.consume(chunk) {
                actual.push((
                    value["stage"].as_str().unwrap().to_owned(),
                    value["steps_completed"].as_u64().unwrap(),
                ));
            }
        }
        let expected = [
            ("conditioning", 0),
            ("diffusion", 0),
            ("diffusion", 1),
            ("diffusion", 2),
            ("diffusion", 3),
            ("diffusion", 4),
            ("vae_decode", 4),
            ("writing_video", 4),
        ];
        assert_eq!(
            actual
                .iter()
                .map(|(name, step)| (name.as_str(), *step))
                .collect::<Vec<_>>(),
            expected
        );
    }

    #[test]
    fn counts_sampler_steps_only_and_handles_split_cr_progress() {
        let mut p = H3Progress::new(4);
        assert_eq!(
            p.consume(b"[I] MiniMax-H3 FL2VA --- video.cpp:768\n")
                .unwrap()["stage"],
            "conditioning"
        );
        assert!(p.consume(b"  |====| 4/4 - 1.4it/s\r\n").is_none());
        assert_eq!(
            p.consume(b"[I] generate_video 864x480x362\n").unwrap()["steps_completed"],
            0
        );
        assert!(p
            .consume(b"  |####| 4/4 - 86.00MB/s\r  |==>| 1/4 - ")
            .is_none());
        assert_eq!(
            p.consume(b"2205.17s/it - ETA 1:50:15\x1b[K[I] Struve H3 sequence: 46360 tokens\n")
                .unwrap()["steps_completed"],
            1
        );
        assert!(p
            .consume(b"  |====| 7/7 - 32.50it/s\r  |=>| 0/4 - 0.01s/it\r")
            .is_none());
        assert_eq!(
            p.consume(b"  |====| 4/4 - 2198.11s/it\x1b[K[I] sampling completed, taking 9200s\n")
                .unwrap()["stage"],
            "vae_decode"
        );
        assert!(p.consume(b"  |==>| 1/4 - 1.20s/it\r").is_none());
        assert_eq!(
            p.consume(b"[I] decode_first_stage completed, taking 553s\n")
                .unwrap()["stage"],
            "writing_video"
        );
        assert_eq!(p.completed, 4);
    }
}
