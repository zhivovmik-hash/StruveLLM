//! Local, versioned generation extensions. Data files stay under the installation root.
use crate::{root, server::App};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, fs, io::{Read, Write}, path::{Path, PathBuf}};

const LINT: &str = "h3-prompt-lint";
const PY: &str = concat!(include_str!("../media-packs/h3-prompt-lint/prompt_lint.py"), "\n", include_str!("../media-packs/h3-prompt-lint/runner.py"));
const LICENSE: &str = include_str!("../media-packs/h3-prompt-lint/LICENSE");
fn library() -> PathBuf { root().join("media-extensions") }
pub fn lint_script() -> PathBuf { library().join("h3-prompt-lint-6fd5025").join("lint.py") }
pub(crate) fn builtin_catalog() -> &'static [Value] {
    static CATALOG: std::sync::OnceLock<Vec<Value>> = std::sync::OnceLock::new();
    CATALOG.get_or_init(|| serde_json::from_str(include_str!("../media-packs/catalog.json"))
        .expect("Invalid bundled extension catalog"))
}
pub fn catalog(app: &App) -> Result<Value> {
    let comfy=crate::comfy_extensions::settings(app);
    let nodes=crate::comfy_extensions::installed_nodes(app,&comfy)?;
    Ok(json!({"catalog":builtin_catalog(),
              "installed":app.list("media_extension")?,"comfy":comfy,"comfy_nodes":nodes}))
}
fn field<'a>(p:&'a Value,key:&str)->Result<&'a str> {
    p[key].as_str().map(str::trim).filter(|s|!s.is_empty()).with_context(||format!("Missing {key}"))
}
fn inspect_lora(path:&Path) -> Result<()> {
    if !path.is_absolute() || !path.extension().is_some_and(|ext|ext.eq_ignore_ascii_case("safetensors")) {
        bail!("Choose a local LoRA .safetensors file")
    }
    let mut file=fs::File::open(path)?;
    let mut head=[0u8;8]; file.read_exact(&mut head)?;
    let n=u64::from_le_bytes(head); let size=file.metadata()?.len();
    if n==0 || n>16*1024*1024 || n+8>=size {bail!("Invalid safetensors header")}
    let mut bytes=vec![0;n as usize]; file.read_exact(&mut bytes)?;
    let header:Value=serde_json::from_slice(&bytes)?;
    let tensors=header.as_object().context("Missing safetensors tensors")?;
    let mut paired=false;
    for (key,tensor) in tensors.iter().filter(|(k,_)|k.as_str()!="__metadata__") {
        let bounds=tensor["data_offsets"].as_array().context("Invalid tensor offsets")?;
        if bounds.len()!=2 || !matches!((bounds[0].as_u64(),bounds[1].as_u64()),(Some(a),Some(b)) if a<=b && b<=size-8-n) {
            bail!("Invalid tensor bounds")
        }
        for (left,right) in [(".lora_down.",".lora_up."),(".lora_A.",".lora_B."),(".lora.down.",".lora.up.")] {
            if key.contains(left) && tensors.contains_key(&key.replace(left,right)) { paired=true; }
        }
    }
    if !paired {bail!("This file has no supported pair of LoRA up/down or A/B tensors")}
    Ok(())
}
struct TempFile(PathBuf);
impl Drop for TempFile {fn drop(&mut self){let _=fs::remove_file(&self.0);}}
fn install_file(dir:&Path, name:&str, bytes:&[u8])->Result<()> {
    fs::create_dir_all(dir)?;
    let target=dir.join(name);
    if target.exists() {
        if fs::read(&target)?!=bytes {bail!("Installed component changed: {name}")}
        return Ok(())
    }
    let temp=TempFile(dir.join(uuid::Uuid::new_v4().to_string()));
    fs::write(&temp.0,bytes)?;
    if let Err(error)=fs::rename(&temp.0,&target) {
        if !target.is_file() || fs::read(&target)?!=bytes {return Err(error.into())}
    }
    Ok(())
}
fn copy_adapter(path:&Path)->Result<(String,u64)> {
    let dir=library().join("loras"); fs::create_dir_all(&dir)?;
    let temp=TempFile(dir.join(uuid::Uuid::new_v4().to_string()));
    let mut src=fs::File::open(path)?; let mut dst=fs::File::create(&temp.0)?;
    let mut hash=Sha256::new(); let mut buffer=vec![0;1024*1024]; let mut size=0;
    loop {let n=src.read(&mut buffer)?;if n==0 {break} dst.write_all(&buffer[..n])?;hash.update(&buffer[..n]);size+=n as u64;}
    dst.sync_all()?;drop(dst);
    let id=format!("{:x}",hash.finalize()); let target=dir.join(format!("{id}.safetensors"));
    if let Err(e)=fs::rename(&temp.0,&target) {
        if !target.is_file() || target.metadata()?.len()!=size {return Err(e.into())}
        let mut existing=fs::File::open(&target)?; let mut check=Sha256::new();
        std::io::copy(&mut existing,&mut check)?;
        if format!("{:x}",check.finalize())!=id {bail!("Installed adapter checksum mismatch")}
    }
    Ok((id,size))
}
pub fn install(app:&App,p:&Value)->Result<Value>{
    let preview=p["dry_run"]==true;
    let mut entry=match field(p,"kind")? {
        "h3-prompt-lint"=>{
            if !preview {
                let path=lint_script(); let dir=path.parent().unwrap();
                install_file(dir,"lint.py",PY.as_bytes())?; install_file(dir,"LICENSE",LICENSE.as_bytes())?;
            }
            json!({"id":LINT,"kind":LINT,"name":"H3 Prompt Lint","family":"minimax-h3","version":"6fd5025","sha256":format!("{:x}",Sha256::digest(PY.as_bytes()))})
        }
        "lora"=>{
            let model=app.get("media_model",field(p,"model_id")?)?;
            let name=field(p,"name")?; let description=field(p,"description")?;
            if name.len()>160 || description.len()>2000 {bail!("Name or description too long")}
            let path=Path::new(field(p,"path")?);inspect_lora(path)?;
            let gain=p["strength"].as_f64().unwrap_or(1.0);
            if !(0.0..=2.0).contains(&gain) {bail!("LoRA strength must be 0..2")}
            let mut e=json!({"kind":"lora","name":name,"description":description,"family":model["architecture"],
                            "model_id":model["id"],"strength":gain,"bytes":path.metadata()?.len()});
            if !preview {
                let (hash,size)=copy_adapter(path)?;
                let binding=format!("{:x}",Sha256::digest(format!("{}:{hash}",model["id"])));
                e["id"]=json!(format!("lora-{}",&binding[..24]));e["sha256"]=json!(hash);e["bytes"]=json!(size);
            }
            e
        }
        _=>bail!("This package needs its own backend"),
    };
    if preview {return Ok(entry)}
    let id=field(&entry,"id")?;
    let mut store=app.store.lock().unwrap();
    if let Ok(old)=store.get("media_extension",id) {return Ok(old)}
    entry["installed"]=json!(true);
    store.save("media_extension",entry,None)
}
pub fn resolve(app:&App,model:&Value,ids:&Value)->Result<Vec<Value>> {
    if ids.is_null() {return Ok(vec![])}
    let ids=ids.as_array().context("extension_ids must be an array")?;
    if ids.len()>16 {bail!("Select up to 16 extensions")}
    let mut seen=HashSet::new(); let mut result=Vec::new();
    for id in ids {
        let id=id.as_str().context("Invalid extension ID")?;
        if !seen.insert(id) {bail!("Duplicate extension")}
        let rec=app.get("media_extension",id)?;
        if model["architecture"]!=rec["family"] || (rec["model_id"].is_string() && rec["model_id"]!=model["id"]) {bail!("Incompatible extension: {id}")}
        if rec["kind"]==LINT && (!Path::new(model["editor_command"].as_str().unwrap_or("")).is_file() || fs::read(lint_script())?!=PY.as_bytes()) {bail!("H3 Prompt Lint requires its installed module and a local Python runtime")}
        if rec["kind"]=="lora" && !library().join("loras").join(format!("{}.safetensors",field(&rec,"sha256")?)).is_file() {bail!("Missing LoRA file")}
        result.push(rec);
    }
    Ok(result)
}
pub fn apply_loras(args:&mut Vec<Value>,records:&[Value])->Result<()> {
    let loras:Vec<_>=records.iter().filter(|x|x["kind"]=="lora").collect();
    if loras.is_empty(){return Ok(())}
    let i=args.iter().position(|x|x=="-p").context("Missing renderer prompt")?+1;
    let mut prompt=args[i].as_str().context("Invalid renderer prompt")?.to_string();
    if prompt.contains("<lora:"){bail!("Select adapters in Extensions instead of embedding LoRA paths in the generated prompt")}
    for r in loras {prompt.push_str(&format!(" <lora:{}:{}>",field(r,"sha256")?,r["strength"]));}
    args[i]=json!(prompt);
    args.extend([json!("--lora-model-dir"),json!(library().join("loras"))]);Ok(())
}

pub fn configure(app:&App,p:&Value)->Result<Value>{
    let mut current=app.get("media_extension",field(p,"id")?)?;
    if current["kind"]!="lora"{bail!("This extension has no editable settings")}
    if current["revision"]!=p["revision"]{bail!("Revision conflict: reload the adapter")}
    let name=field(p,"name")?;let description=field(p,"description")?;
    let strength=p["strength"].as_f64().filter(|n|(0.0..=2.0).contains(n)).context("LoRA strength must be 0..2")?;
    if name.len()>160 || description.len()>2000 {bail!("Name or description too long")}
    current["name"]=json!(name);current["description"]=json!(description);current["strength"]=json!(strength);
    app.save("media_extension",current)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adapter_tags_are_added_only_for_selected_adapters() {
        let mut args=vec![json!("-p"),json!("test image")];
        apply_loras(&mut args,&[json!({"kind":"h3-prompt-lint"})]).unwrap();
        assert_eq!(args.len(),2);
        apply_loras(&mut args,&[json!({"kind":"lora","sha256":"abc","strength":0.5})]).unwrap();
        assert_eq!(args[1],"test image <lora:abc:0.5>");
        assert_eq!(args[2],"--lora-model-dir");
    }
}
