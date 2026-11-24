//! Context routing and validation for edits of real, completed image artifacts.
use crate::server::App;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

pub fn requires_generative_edit(request: &str) -> bool {
    let s = request.to_lowercase();
    redraw_command(&normalized_request(request)) || [
        "добав",
        "убер",
        "удал",
        "замен",
        "узор",
        "полос",
        "рисунок",
        "надпис",
        "логотип",
        "измени текстуру",
        "материал",
        "кирпич",
        "деревян",
        "измени форму",
        "поменяй форму",
        "add ",
        "remove ",
        "replace ",
        "pattern",
        "stripe",
        "logo",
        "change the texture",
        "material",
        "wooden",
        "change the shape",
        "resize ",
    ]
    .iter()
    .any(|part| s.contains(part))
}

pub fn normalized_request(s: &str) -> String {
    let mut s = s.trim().to_lowercase();
    // Natural follow-ups often start with "now" or "please". Strip only
    // complete conversational prefixes, preserving question/document guards.
    loop {
        let next = [
            "а теперь, ",
            "а теперь ",
            "теперь, ",
            "теперь ",
            "пожалуйста, ",
            "пожалуйста ",
            "and now, ",
            "and now ",
            "now, ",
            "now ",
            "please, ",
            "please ",
        ]
        .iter()
        .find_map(|p| s.strip_prefix(*p).map(str::trim_start));
        match next {
            Some(next) => s = next.to_owned(),
            None => return s,
        }
    }
}
pub fn generation_request(s: &str) -> bool {
    let s = normalized_request(s);
    [
        "нарисуй ",
        "нарисуйте ",
        "создай картинку",
        "создай изображение",
        "сгенерируй изображение",
        "сгенерируй картинку",
        "draw ",
        "generate an image",
        "create an image",
        "/image ",
    ]
    .iter()
    .any(|p| s.starts_with(p))
}
fn redraw_command(normalized: &str) -> bool {
    matches!(normalized.split(|c: char| !c.is_alphabetic()).next(),
        Some("перерисуй" | "перерисуйте" | "перерисовать" | "redraw" | "repaint"))
}

pub fn explicit_redraw(s: &str) -> bool {
    redraw_command(&normalized_request(s)) && edit_request(s)
}

pub fn edit_request(s: &str) -> bool {
    let s = normalized_request(s);
    // Questions and requests about code/documents must stay in the ordinary chat.
    if [
        "как ",
        "почему ",
        "зачем ",
        "объясни ",
        "расскажи ",
        "how ",
        "why ",
        "explain ",
    ]
    .iter()
    .any(|p| s.starts_with(p))
    {
        return false;
    }
    if [
        "код",
        "скрипт",
        "функци",
        "документ",
        "текст ",
        "текста",
        "тексте",
        "таблиц",
        "code",
        "script",
        "function",
        "document",
        "spreadsheet",
    ]
    .iter()
    .any(|p| s.contains(p))
        && !["на картин", "на изображ", "in the image", "in the picture"]
            .iter()
            .any(|p| s.contains(p))
    {
        return false;
    }
    if redraw_command(&s) {
        return true;
    }
    [
        "сделай ",
        "сделайте ",
        "измени ",
        "измените ",
        "перекрась ",
        "замени ",
        "убери ",
        "удали ",
        "добавь ",
        "исправь ",
        "поменяй ",
        "осветли ",
        "затемни ",
        "верни ",
        "я просил ",
        "я просила ",
        "я же просил",
        "нарисуй его",
        "нарисуй её",
        "нарисуй ее",
        "make ",
        "change ",
        "recolor ",
        "replace ",
        "remove ",
        "add ",
        "fix ",
        "brighten ",
        "darken ",
        "i asked ",
        "edit ",
    ]
    .iter()
    .any(|p| s.starts_with(p))
        || (s.contains("не вижу") && s.contains("изображен"))
        || s.contains("исправь ошибки")
        || s.contains("can't see the image")
}
pub fn latest_message_image(chat: &Value) -> Option<Value> {
    chat["messages"].as_array()?.iter().rev().find_map(|m| {
        if m["role"] != "assistant" || m["kind"] != "image" {
            return None;
        }
        let file = m["attachments"]
            .as_array()?
            .iter()
            .find(|a| a["kind"] == "image")?;
        Some(json!({"attachment":file,"run_id":m["run_id"],"prompt":m["generation"]["prompt"]}))
    })
}
pub fn source(app: &App, p: &Value) -> Result<Option<Value>> {
    let Some(id) = p["chat_id"].as_str() else {
        return Ok(None);
    };
    let chat = app.get("chat", id)?;
    if chat["project_id"] != p["project_id"] {
        bail!("Chat belongs to another project")
    }
    let Some(mut source) = latest_message_image(&chat) else {
        return Ok(None);
    };
    let run = app.get(
        "run",
        source["run_id"].as_str().context("Invalid image run")?,
    )?;
    let file = app.get(
        "attachment",
        source["attachment"]["id"]
            .as_str()
            .context("Invalid image attachment")?,
    )?;
    if run["status"] != "completed"
        || run["kind"] != "image"
        || run["chat_id"] != chat["id"]
        || !run["artifacts"]
            .as_array()
            .map(|a| a.iter().any(|a| a["id"] == file["id"]))
            .unwrap_or(false)
    {
        bail!("Previous image has no successful generation receipt")
    }
    source["attachment"] = file;
    Ok(Some(source))
}
pub fn plan(v: &Value) -> Result<()> {
    if !matches!(v["operation"].as_str(), Some("recolor" | "inpaint")) {
        bail!("The model could not plan this image edit; describe the object and desired change more precisely")
    }
    if !v["target"]
        .as_str()
        .map(|s| !s.trim().is_empty() && s.len() < 500)
        .unwrap_or(false)
        || !v["prompt"]
            .as_str()
            .map(|s| !s.trim().is_empty() && s.len() < 4000 && !s.contains('\0'))
            .unwrap_or(false)
    {
        bail!("Invalid edit description")
    }
    for (key, n) in [("box", 4), ("point", 2)] {
        let a = v[key].as_array().context("Missing object coordinates")?;
        if a.len() != n
            || a.iter()
                .any(|v| !v.as_u64().map(|v| v <= 1000).unwrap_or(false))
        {
            bail!("Coordinates must be integers in 0..1000")
        }
    }
    let b = &v["box"];
    let p = &v["point"];
    if !(b[0].as_u64() < p[0].as_u64()
        && p[0].as_u64() < b[2].as_u64()
        && b[1].as_u64() < p[1].as_u64()
        && p[1].as_u64() < b[3].as_u64())
    {
        bail!("Object point must lie inside its box")
    }
    if v["operation"] == "recolor"
        && !v["color"]
            .as_array()
            .map(|a| {
                a.len() == 3
                    && a.iter()
                        .all(|v| v.as_u64().map(|v| v <= 255).unwrap_or(false))
            })
            .unwrap_or(false)
    {
        bail!("Missing RGB edit color")
    }
    if !v["strength"]
        .as_f64()
        .map(|v| (0.15..=0.8).contains(&v))
        .unwrap_or(false)
    {
        bail!("Edit strength must be 0.15..0.8")
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn patterns_are_not_uniform_color_changes() {
        assert!(requires_generative_edit("Добавь на чашку крупный узор из красных полос. Сохрани её форму и синий основной цвет."));
        assert!(requires_generative_edit("Make the roof wooden"));
        assert!(!requires_generative_edit("Сделай купол белым"));
        assert!(!requires_generative_edit("Change the cup to blue"));
    }
    #[test]
    fn followups_and_questions() {
        for s in [
            "Сделай купол белым",
            "Я просил белый купол",
            "Я не вижу новое изображение. Нарисуй его, исправь ошибки",
            "Please remove the person",
            "Перекрась крышу в синий",
            "Теперь сделай красные полосы жёлтыми",
            "А теперь, пожалуйста, измени текстуру чашки",
            "Now, please change the stripes to yellow",
        ] {
            assert!(edit_request(s), "{s}")
        }
        for s in [
            "Почему купол тёмный?",
            "Сделай функцию на Rust",
            "Исправь код",
            "Объясни как изменить фото",
            "Сколько будет 2 + 2?",
            "Теперь объясни, как изменить фото",
            "Теперь сделай функцию на Rust",
        ] {
            assert!(!edit_request(s), "{s}")
        }
        assert!(generation_request("Нарисуй лес"));
        assert!(generation_request("Пожалуйста, теперь нарисуй лес"));
        assert!(!generation_request("Объясни команду нарисуй"));
    }
    #[test]
    fn redraw_routes_to_a_real_generative_edit() {
        for request in [
            "Перерисуй компас в соответствии с тем, что ты сейчас написал. На циферблате буквы, обозначающие стороны света: N (север), S (юг), E (восток) и W (запад). В центре расположены две стрелки — одна указывает на север, другая на юг.",
            "Теперь, пожалуйста, перерисуйте его.",
            "Redraw it with the markings we discussed.",
            "Please repaint.",
        ] {
            assert!(explicit_redraw(request), "{request}");
            assert!(requires_generative_edit(request), "{request}");
        }
        for request in ["Почему ты так нарисовал?", "Что должно быть изображено на компасе?",
                        "Перерисуй таблицу в документе", "Explain how to redraw it"] {
            assert!(!explicit_redraw(request), "{request}");
        }
    }
    #[test]
    fn real_latest_artifact_not_hallucinated_filename() {
        let chat = json!({"messages":[{"role":"assistant","kind":"image","run_id":"first","attachments":[{"kind":"image","id":"a"}]},{"role":"assistant","content":"[Attached image: image.png]"},{"role":"assistant","kind":"image","run_id":"second","attachments":[{"kind":"image","id":"b"}]},{"role":"assistant","content":"Done","attachments":[]}]});
        assert_eq!(
            latest_message_image(&chat).unwrap()["attachment"]["id"],
            "b"
        );
        assert!(latest_message_image(
            &json!({"messages":[{"role":"assistant","content":"[Attached image: image.png]"}]})
        )
        .is_none());
    }
    #[test]
    fn reject_unusable_plans() {
        let mut v = json!({"operation":"recolor","target":"roof","prompt":"white roof","box":[10,20,300,500],"point":[150,200],"color":[255,255,255],"strength":0.45});
        assert!(plan(&v).is_ok());
        v["point"] = json!([1, 2]);
        assert!(plan(&v).is_err());
        v["point"] = json!([150, 200]);
        v["color"] = json!([999, 0, 0]);
        assert!(plan(&v).is_err());
    }
}
