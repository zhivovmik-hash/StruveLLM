use crate::{comfy_extensions, media_extensions::builtin_catalog, server::App};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest,Sha256};
use std::{collections::{HashMap,HashSet}, sync::{Arc,OnceLock,RwLock}, time::Duration};

#[derive(Clone,Default,Serialize,Deserialize)]
struct Index {
    updated:i64,
    packages:Vec<Value>,
}
static INDEX:OnceLock<RwLock<Arc<Index>>>=OnceLock::new();
static FETCH:tokio::sync::Mutex<()>=tokio::sync::Mutex::const_new(());
fn index(app:&App)->&RwLock<Arc<Index>> {
    INDEX.get_or_init(||{
        let saved=app.get("extension_registry","comfy").ok().and_then(|v|serde_json::from_value(v).ok()).unwrap_or_default();
        RwLock::new(Arc::new(saved))
    })
}
pub fn package_id(slug:&str)->String{
    builtin_catalog().iter().find(|c|c["registry"]["id"].as_str().is_some_and(|r|r.eq_ignore_ascii_case(slug)))
        .and_then(|c|c["id"].as_str()).map(str::to_owned)
        .unwrap_or_else(||format!("cnr-{:x}",Sha256::digest(slug.to_ascii_lowercase().as_bytes())))
}
pub fn safe_slug(s:&str)->bool{
    let stem=s.split('.').next().unwrap_or("").to_ascii_uppercase();
    !s.is_empty() && s.len()<=160 && !s.starts_with(['.','-']) && !s.ends_with('.') &&
    s.bytes().all(|b|b.is_ascii_alphanumeric()||b"-_.".contains(&b)) &&
    !matches!(stem.as_str(),"CON"|"PRN"|"AUX"|"NUL") &&
    !(stem.len()==4&&(stem.starts_with("COM")||stem.starts_with("LPT"))&&stem.as_bytes()[3].is_ascii_digit())
}
fn item(node:&Value)->Result<Value>{
    let slug=node["id"].as_str().filter(|s|!s.is_empty()).context("Registry entry without an ID")?;
    let version=&node["latest_version"];
    let ver=version["version"].as_str().unwrap_or("");
    let can_install=safe_slug(slug)&&!ver.is_empty()&&ver.len()<=100&&ver.bytes().next().is_some_and(|b|b.is_ascii_digit())&&
        ver.bytes().all(|b|b.is_ascii_alphanumeric()||b".+-".contains(&b))&&version["deprecated"]!=true&&
        version["status"]=="NodeVersionStatusActive"&&node["status"]=="NodeStatusActive";
    let labels=format!("{} {} {} {} {}",slug,node["name"],node["category"],node["description"],version["tags"]).to_lowercase();
    let mut families=Vec::new();
    for (family, terms) in [
        ("sd15",&["sd 1.5","sd1.5","sd15","sd-1.5"][..]),("sdxl",&["sdxl","sd xl"][..]),
        ("sd35",&["sd3.5","sd 3.5","sd-3.5"][..]),("flux1",&["flux.1","flux1","flux 1","flux-1"][..]),
        ("flux2-klein",&["klein"][..]),("minimax-h3",&["minimax-h3","minimax h3","minimax_h3"][..]),
        ("h3-image",&["h3 image","h3-image"][..])
    ]{if terms.iter().any(|t|labels.contains(t)){families.push(family)}}
    Ok(json!({"id":package_id(slug),"slug":slug,"families":families,"name":node["name"].as_str().filter(|s|!s.is_empty()).unwrap_or(slug),
        "description":node["description"].as_str().unwrap_or("").chars().take(2000).collect::<String>(),
        "version":ver,"installable":can_install,"downloads":node["downloads"].as_u64().unwrap_or(0),
        "publisher":node["publisher"]["name"].as_str().or(node["publisher"]["id"].as_str()).unwrap_or(""),
        "tags":version["tags"],"compatibility":version["supported_comfyui_version"]}))
}
async fn page(client:&reqwest::Client,n:u64)->Result<Value>{
    for retry in 0..3{
        let response=client.get("https://api.comfy.org/nodes").query(&[("limit",100),("page",n)]).send().await;
        match response{
            Ok(r) if r.status().is_success()=>return Ok(r.json().await?),
            Ok(r) if retry<2&&(r.status().is_server_error()||r.status().as_u16()==429)=>{},
            Err(_) if retry<2=>{},
            Ok(r)=>bail!("Comfy Registry HTTP {} (page {n})",r.status()),
            Err(e)=>return Err(e.into()),
        }
        tokio::time::sleep(Duration::from_secs(1<<retry)).await;
    }
    unreachable!()
}
pub async fn refresh(app:&App,force:bool)->Result<Value>{
    let _guard=FETCH.lock().await;
    let current=index(app).read().unwrap().clone();
    let now=chrono::Utc::now().timestamp();
    if !force && now-current.updated<86400 {return Ok(json!({"updated":current.updated,"count":current.packages.len()}))}
    let client=reqwest::Client::builder().user_agent("StruveLLM/1.0").timeout(Duration::from_secs(20)).build()?;
    let first=page(&client,1).await?;
    let pages=first["totalPages"].as_u64().filter(|n|*n>0&&*n<=10000).context("Invalid registry page count")?;
    let expected=first["total"].as_u64().context("Missing registry total")?;
    let mut found=HashMap::new();
    let mut append=|data:&Value,n:u64|->Result<()>{
        if data["page"].as_u64()!=Some(n){bail!("Unexpected registry page {n}")}
        for node in data["nodes"].as_array().context("Missing registry entries")?{
            let pkg=item(node)?;found.insert(pkg["id"].as_str().unwrap().to_owned(),pkg);
        }
        Ok(())
    };
    append(&first,1)?;
    let mut jobs=tokio::task::JoinSet::new();
    let mut next=2;
    while next<=pages||!jobs.is_empty(){
        while next<=pages&&jobs.len()<4{let c=client.clone();let n=next;next+=1;jobs.spawn(async move{(n,page(&c,n).await)});}
        if let Some(result)=jobs.join_next().await{let (n,data)=result?;append(&data?,n)?;}
    }
    if found.len() as u64!=expected{bail!("Registry changed during refresh ({} of {expected}); retry to get a complete snapshot",found.len())}
    let mut packages:Vec<_>=found.into_values().collect();
    packages.sort_by(|a,b|b["downloads"].as_u64().cmp(&a["downloads"].as_u64()).then(a["slug"].as_str().cmp(&b["slug"].as_str())));
    let count=packages.len();let saved=Index{updated:now,packages};
    let mut value=serde_json::to_value(&saved)?;value["id"]=json!("comfy");
    let mut store=app.store.lock().unwrap();
    let rev=store.get("extension_registry","comfy").ok().and_then(|v|v["revision"].as_i64());
    store.save("extension_registry",value,rev)?;
    *index(app).write().unwrap()=Arc::new(saved);
    Ok(json!({"updated":now,"count":count}))
}
pub fn remote(app:&App,id:&str)->Option<Value>{
    index(app).read().unwrap().packages.iter().find(|v|v["id"]==id).cloned()
}
fn registry_card(v:&Value)->Value {
    let slug=v["slug"].as_str().unwrap_or("");
    let mut source=reqwest::Url::parse("https://registry.comfy.org/nodes/").unwrap();
    source.path_segments_mut().unwrap().pop_if_empty().push(slug);
    let mut card=json!({"id":v["id"],"name":v["name"],"source":source.as_str(),"backend":"ComfyUI","families":v["families"].as_array().cloned().unwrap_or_default(),
        "ru":v["description"],"en":v["description"],"publisher":v["publisher"],"available_version":v["version"],
        "requires_ru":"Граф ComfyUI и зависимости автора. Проверьте требования модели в документации.",
        "requires_en":"Requires ComfyUI and the publisher's dependencies. See model requirements in the package documentation."});
    if v["installable"]==true{card["registry"]=json!({"id":slug,"version":v["version"]})}
    card
}
pub fn browse(app:&App,p:&Value)->Result<Value>{
    let cache=index(app).read().unwrap().clone();
    let local=comfy_extensions::installed_nodes(app,&comfy_extensions::settings(app))?;
    let native=app.list("media_extension")?;
    let mut installed:HashMap<String,Value>=local.into_iter().map(|v|(v["id"].as_str().unwrap().into(),v)).collect();
    for e in native.as_array().into_iter().flatten(){installed.insert(e["id"].as_str().unwrap().into(),e.clone());}
    let requested=p["section"].as_str().unwrap_or("available");
    if !matches!(requested,"installed"|"available"){bail!("Invalid market section")}
    let mut shown=Vec::new();let mut seen=HashSet::new();let mut total_installed=0;let mut available=0;
    let q=p["query"].as_str().unwrap_or("").trim().to_lowercase();let family=p["family"].as_str().unwrap_or("");
    let mut consider=|mut card:Value,rec:Option<&Value>|{
        if rec.is_some(){total_installed+=1}else{available+=1}
        if (requested=="installed")!=rec.is_some(){return}
        let matches_family=family.is_empty()||card["families"].as_array().is_some_and(|fs|(fs.is_empty()&&family=="unspecified")||fs.iter().any(|f|f==family));
        if !matches_family{return}
        if !q.is_empty()&&!["name","id","registry","ru","en","publisher","families"].iter().any(|k|card[*k].to_string().to_lowercase().contains(&q)){return}
        if let Some(r)=rec{card["installed_record"]=r.clone();}
        shown.push(card);
    };
    let online:HashMap<_,_>=cache.packages.iter().map(|r|(r["id"].as_str().unwrap(),r)).collect();
    for c in builtin_catalog(){
        let id=c["id"].as_str().unwrap();seen.insert(id.to_string());let mut card=c.clone();
        if let Some(v)=online.get(id){card["available_version"]=v["version"].clone();card["publisher"]=v["publisher"].clone();
            if v["installable"]==true{card["registry"]["version"]=v["version"].clone()}else{card["registry"]=Value::Null}}
        consider(card,installed.get(id));
    }
    for v in &cache.packages{let id=v["id"].as_str().unwrap();if seen.insert(id.to_owned()){consider(registry_card(v),installed.get(id))}}
    for (id,r) in &installed{if seen.contains(id){continue}let mut card=json!({"id":id,"name":r["name"].as_str().or(r["registry_id"].as_str()).unwrap_or(id),"families":[],
        "backend":"ComfyUI","source":"https://registry.comfy.org/","ru":"Установлено в выбранном каталоге ComfyUI.","en":"Installed in the selected ComfyUI workspace."});
        if r["kind"]=="lora"{card["backend"]=json!("local");card["families"]=json!([r["family"]]);card["ru"]=r["description"].clone();card["en"]=r["description"].clone();}
        consider(card,Some(r));
    }
    let total=shown.len();let limit=p["limit"].as_u64().unwrap_or(24).clamp(1,100) as usize;
    let offset=(p["offset"].as_u64().unwrap_or(0) as usize).min(total.saturating_sub(1))/limit*limit;
    Ok(json!({"items":shown.into_iter().skip(offset).take(limit).collect::<Vec<_>>(),"offset":offset,"total":total,
        "installed":total_installed,"available":available,"updated":cache.updated,"registry_count":cache.packages.len()}))
}
