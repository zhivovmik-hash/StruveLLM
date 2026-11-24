use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::collections::HashSet;

fn optional_number(p: &Value, key: &str, min: f64, max: f64, integer: bool) -> Result<()> {
    if p[key].is_null() {
        return Ok(());
    }
    if !p[key]
        .as_f64()
        .map(|n| n.is_finite() && n >= min && n <= max && (!integer || n.fract() == 0.0))
        .unwrap_or(false)
    {
        bail!("Invalid {key}: expected {min}..{max}")
    }
    Ok(())
}
pub fn selection(value: &Value) -> Result<()> {
    if value.is_null() {
        return Ok(());
    }
    let Some(items) = value.as_array() else {
        bail!("Tool selection must be an array")
    };
    let mut unique = HashSet::new();
    if items.len() > 24
        || items.iter().any(|t| {
            !t["connection_id"]
                .as_str()
                .map(|s| !s.is_empty() && s.len() <= 120)
                .unwrap_or(false)
                || !t["tool"]
                    .as_str()
                    .map(|s| !s.is_empty() && s.len() <= 256)
                    .unwrap_or(false)
                || !unique.insert((
                    t["connection_id"].clone().to_string(),
                    t["tool"].clone().to_string(),
                ))
        })
    {
        bail!("Select at most 24 unique tools")
    }
    Ok(())
}
pub fn validate(p: &Value) -> Result<()> {
    for (key, min, max, integer) in [
        ("top_p", 0., 1., false),
        ("min_p", 0., 1., false),
        ("top_k", 0., 1000., true),
        ("repeat_penalty", 0., 2., false),
        ("repeat_last_n", -1., 131072., true),
        ("frequency_penalty", -2., 2., false),
        ("presence_penalty", -2., 2., false),
        ("seed", 0., 2147483647., true),
        ("keep_alive_seconds", 0., 3600., true),
        ("max_tool_calls", 0., 8., true),
        ("agent_timeout_seconds", 1., 1200., true),
        ("max_attachment_files", 0., 8., true),
    ] {
        optional_number(p, key, min, max, integer)?;
    }
    if !p["thinking"].is_null() && !p["thinking"].is_boolean() {
        bail!("Thinking must be true, false or model default")
    }
    if !p["stop"].is_null()
        && !p["stop"]
            .as_array()
            .map(|v| {
                v.len() <= 4
                    && v.iter().all(|s| {
                        s.as_str()
                            .map(|s| !s.is_empty() && s.chars().count() <= 128)
                            .unwrap_or(false)
                    })
            })
            .unwrap_or(false)
    {
        bail!("Use at most four stop sequences, 1..128 characters each")
    }
    if !p["system_prompt"].is_null()
        && !p["system_prompt"]
            .as_str()
            .map(|s| s.chars().count() <= 16000)
            .unwrap_or(false)
    {
        bail!("System instructions must be text, at most 16000 characters")
    }
    selection(&p["tool_allowlist"])?;
    if let Some(skills) = p.get("skills").filter(|v| !v.is_null()) {
        let Some(skills) = skills.as_array() else {
            bail!("Skills must be an array")
        };
        let mut ids = HashSet::new();
        let mut total = 0;
        if skills.len() > 12 {
            bail!("At most 12 skills per model")
        }
        for skill in skills {
            let id = skill["id"].as_str().unwrap_or("");
            crate::safe_id(id)?;
            if !ids.insert(id) {
                bail!("Duplicate skill id")
            }
            if !skill["name"]
                .as_str()
                .map(|s| !s.trim().is_empty() && s.chars().count() <= 100)
                .unwrap_or(false)
            {
                bail!("Skill name is required, up to 100 characters")
            }
            let content = skill["content"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Skill content is required"))?;
            if content.trim().is_empty() || content.chars().count() > 8000 {
                bail!("Skill instructions must contain 1..8000 characters")
            }
            if !skill["enabled"].is_boolean() {
                bail!("Skill enabled must be a boolean")
            }
            total += content.chars().count();
        }
        if total > 32000 {
            bail!("Combined skill instructions exceed 32000 characters")
        }
    }
    Ok(())
}
pub fn policy(model: &Value, request: &Value) -> Result<Value> {
    validate(model)?;
    selection(&request["tool_selection"])?;
    let ceiling = model["max_tool_calls"].as_u64().unwrap_or(4);
    let calls = match request.get("max_tool_calls").filter(|v| !v.is_null()) {
        Some(v) => v.as_u64().filter(|n| *n <= ceiling).ok_or_else(|| {
            anyhow::anyhow!("Task tool limit exceeds the model limit ({ceiling})")
        })?,
        None => ceiling,
    };
    let mut selected = request["tool_selection"].clone();
    if let Some(allowed) = model["tool_allowlist"].as_array() {
        if let Some(items) = selected.as_array() {
            if items.iter().any(|t| {
                !allowed
                    .iter()
                    .any(|a| a["connection_id"] == t["connection_id"] && a["tool"] == t["tool"])
            }) {
                bail!("A selected tool is blocked by this model's permissions")
            }
        } else {
            selected = json!(allowed);
        }
    }
    let use_tools = request["use_tools"] == true
        && !request["tool_selection"]
            .as_array()
            .map(Vec::is_empty)
            .unwrap_or(false);
    if use_tools
        && (calls == 0
            || model["tool_mode"] == "none"
            || selected.as_array().map(Vec::is_empty).unwrap_or(false))
    {
        bail!("Tool calling is disabled by this model's limits")
    }
    let max_files = model["max_attachment_files"].as_u64().unwrap_or(8);
    if request["attachment_ids"]
        .as_array()
        .map(|a| a.len() as u64 > max_files)
        .unwrap_or(false)
    {
        bail!("This model allows at most {max_files} attachments per message")
    }
    Ok(
        json!({"use_tools":use_tools,"max_tool_calls":calls,"tool_selection":selected,"timeout_seconds":model["agent_timeout_seconds"].as_u64().unwrap_or(1200),"max_attachment_files":max_files}),
    )
}
pub fn read_skill(path: &std::path::Path) -> Result<Value> {
    if !path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case("md") || s.eq_ignore_ascii_case("txt"))
        .unwrap_or(false)
    {
        bail!("Choose a Markdown or text skill file")
    }
    if std::fs::metadata(path)?.len() > 32768 {
        bail!("Skill file exceeds 32 KiB")
    }
    let content = std::fs::read_to_string(path)?
        .trim_start_matches('\u{feff}')
        .to_string();
    let fallback = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Skill");
    let name = if content.starts_with("---") {
        content
            .lines()
            .skip(1)
            .take_while(|s| s.trim() != "---")
            .find_map(|s| {
                s.strip_prefix("name:")
                    .map(|s| s.trim().trim_matches(['\"', '\'']).to_string())
            })
            .unwrap_or_else(|| fallback.into())
    } else {
        content
            .lines()
            .find_map(|s| s.strip_prefix("# ").map(|s| s.trim().to_string()))
            .unwrap_or_else(|| fallback.into())
    };
    let skill = json!({"id":format!("skill-{}",uuid::Uuid::new_v4().simple()),"name":name,"content":content,"enabled":true,"source":path.to_string_lossy()});
    validate(&json!({"skills":[skill.clone()]}))?;
    Ok(skill)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn limits_cannot_be_raised_by_a_task() {
        let m = json!({"max_tool_calls":2,"max_attachment_files":0,"tool_allowlist":[{"connection_id":"documents","tool":"document_read"}]});
        assert!(policy(&m, &json!({"max_tool_calls":3})).is_err());
        assert!(policy(&m, &json!({"attachment_ids":["file"]})).is_err());
        assert!(policy(
            &m,
            &json!({"tool_selection":[{"connection_id":"documents","tool":"xlsx_create"}]})
        )
        .is_err());
        assert_eq!(
            policy(&m, &json!({"use_tools":true})).unwrap()["tool_selection"],
            m["tool_allowlist"]
        );
        assert_eq!(
            policy(&m, &json!({"max_tool_calls":0})).unwrap()["max_tool_calls"],
            0
        );
        assert!(policy(&m, &json!({"max_tool_calls":0,"use_tools":true})).is_err());
    }
    #[test]
    fn invalid_settings_fail_and_zero_values_are_valid() {
        for value in [
            json!({"top_p":1.1}),
            json!({"seed":1.5}),
            json!({"agent_timeout_seconds":0}),
            json!({"max_tool_calls":9}),
            json!({"stop":[""]}),
            json!({"thinking":"false"}),
        ] {
            assert!(validate(&value).is_err(), "{value}");
        }
        validate(&json!({"seed":0,"top_k":0,"top_p":0,"min_p":0,"keep_alive_seconds":0,"max_tool_calls":0,"max_attachment_files":0})).unwrap();
    }
}
