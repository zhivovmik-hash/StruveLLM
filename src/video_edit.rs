//! Explicit duration follow-ups operate only on a real video in this chat.
use crate::server::App;
use anyhow::{bail, Result};
use serde_json::{json, Value};

pub fn duration_frames(content: &str) -> Result<Option<u64>> {
    let s = content.to_lowercase();
    for (start, ch) in s.char_indices() {
        if !ch.is_ascii_digit() {
            continue;
        }
        if start > 0
            && s[..start]
                .chars()
                .last()
                .is_some_and(|c| c.is_ascii_digit() || c == '.' || c == ',')
        {
            continue;
        }
        let part = &s[start..];
        let end = part
            .find(|c: char| !c.is_ascii_digit() && c != '.' && c != ',')
            .unwrap_or(part.len());
        let tail = part[end..].trim_start();
        if !(tail.starts_with("сек")
            || tail.starts_with("second")
            || tail.starts_with("sec ")
            || tail == "s")
        {
            continue;
        }
        if let Ok(seconds) = part[..end].replace(',', ".").parse::<f64>() {
            if !(1.0..=15.0).contains(&seconds) {
                bail!("Video duration must be 1–15 seconds in this prototype")
            }
            return Ok(Some((seconds * 24.0).round() as u64));
        }
    }
    Ok(None)
}
pub fn aligned_frames(frames: u64) -> u64 {
    5 + frames.saturating_sub(5).div_ceil(17) * 17
}
pub fn followup(app: &App, p: &Value) -> Result<Option<Value>> {
    if !matches!(p["mode"].as_str(), Some("auto" | "video")) {
        return Ok(None);
    }
    let s = crate::image_edit::normalized_request(p["content"].as_str().unwrap_or(""));
    if ![
        "продли",
        "продолжительность",
        "длительность",
        "ролик должен",
        "видео должно",
        "extend ",
        "duration ",
        "make it ",
    ]
    .iter()
    .any(|x| s.starts_with(x))
    {
        return Ok(None);
    }
    let Some(frames) = duration_frames(&s)? else {
        return Ok(None);
    };
    let Some(id) = p["chat_id"].as_str() else {
        return Ok(None);
    };
    let chat = app.get("chat", id)?;
    if chat["project_id"] != p["project_id"] {
        bail!("Chat belongs to another project")
    }
    let source=chat["messages"].as_array().and_then(|messages|messages.iter().rev().find_map(|m|{
        if m["role"]!="assistant"||m["kind"]!="video"{return None}
        let f=m["attachments"].as_array()?.iter().find(|f|f["kind"]=="video")?;
        Some(json!({"attachment":f,"run_id":m["run_id"],"prompt":m["generation"]["prompt"],"target_frames":frames}))
    }));
    Ok(source)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requested_seconds_and_native_alignment_are_distinct() {
        assert_eq!(
            duration_frames("продолжительность ролика должна быть 7 секунд").unwrap(),
            Some(168)
        );
        assert_eq!(duration_frames("Extend to 7.5 seconds").unwrap(), Some(180));
        assert_eq!(
            duration_frames("Создай видео на 15 секунд").unwrap(),
            Some(360)
        );
        assert!(duration_frames("15.1 seconds").is_err());
        assert_eq!(duration_frames("39 кадров").unwrap(), None);
        assert!(duration_frames("100 секунд").is_err());
        assert_eq!(aligned_frames(168), 175);
        assert_eq!(aligned_frames(168 - 39 + 1), 141);
        assert_eq!(aligned_frames(39), 39);
        assert_eq!(aligned_frames(360), 362);
    }
}
