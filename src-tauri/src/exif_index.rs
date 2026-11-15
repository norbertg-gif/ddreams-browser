use crate::{db::Db, formats::FormatTable};
use rusqlite::{Connection, params};
use serde::Serialize;
use serde_json::Value;
use std::{collections::HashMap, io::Write, path::{Path, PathBuf}, process::{Command, Stdio}, sync::{Arc, Mutex}, thread, time::Duration};

#[derive(Clone)]
struct Asset { id:i64, path:String, size:Option<f64>, modified:Option<String>, ext:String }
#[derive(Default,Clone,Serialize)]
pub struct Progress { pub done:usize, pub total:usize, pub running:bool, pub error:Option<String> }
#[derive(Serialize)]
pub struct Group { pub key:String, pub name:String, pub values:Vec<GroupValue> }
#[derive(Serialize)]
pub struct GroupValue { pub value:String, pub label:String, pub count:i64 }
pub struct ExifIndex { path:PathBuf, catalog:PathBuf, tool:PathBuf, formats:Arc<FormatTable>, progress:Mutex<Progress> }

const SCHEMA:&str="CREATE TABLE IF NOT EXISTS exif_index(asset_id INTEGER PRIMARY KEY,path TEXT NOT NULL,size REAL,modified TEXT,camera TEXT,lens TEXT,iso INTEGER,aperture REAL,exposure REAL,focal INTEGER,ext TEXT,bucket INTEGER,scanned_at TEXT DEFAULT CURRENT_TIMESTAMP);";
const GROUPS:[(&str,&str,&str,&str);8]=[("camera","Fotoaparát","camera","camera COLLATE NOCASE"),("lens","Objektív","lens","lens COLLATE NOCASE"),("iso","ISO","iso","iso"),("aperture","Clona","aperture","aperture"),("exposure","Expozičný čas","exposure","exposure"),("focal","Ohnisková vzdialenosť","focal","focal"),("ext","Typ súboru","ext","ext COLLATE NOCASE"),("bucket","Veľkosť súboru","bucket","bucket")];
fn camera(make:Option<&str>,model:Option<&str>)->Option<String>{let make=make.unwrap_or("").trim();let model=model.unwrap_or("").trim();if model.is_empty(){return (!make.is_empty()).then(||make.to_string())}if make.is_empty()||model.to_lowercase().starts_with(&make.to_lowercase()){Some(model.to_string())}else{Some(format!("{make} {model}"))}}
/// Cameras fill unknown lens fields with dashes, zeros or "Unknown"; those are not lenses.
fn placeholder(s:&str)->bool{let t=s.trim();t.chars().all(|c|c=='-'||c=='0'||c==' '||c=='.')||matches!(t.to_ascii_lowercase().as_str(),"unknown"|"n/a"|"none")}
fn aperture(v:f64)->f64{(v*10.0).round()/10.0}
fn exposure(v:f64)->String{if v<1.0{format!("1/{} s",(1.0/v).round().max(1.0) as u64)}else{format!("{v} s")}}
fn bucket(v:f64)->i64{if v<1_000_000.0{0}else if v<5_000_000.0{1}else if v<20_000_000.0{2}else if v<50_000_000.0{3}else{4}}
fn bucket_label(v:i64)->&'static str{["< 1 MB","1–5 MB","5–20 MB","20–50 MB","> 50 MB"].get(v as usize).copied().unwrap_or("")}
fn same(a:&Asset,old:Option<(String,Option<f64>,Option<String>)>)->bool{old.is_some_and(|(p,s,m)|p==a.path&&s==a.size&&m==a.modified)}
fn string(v:&Value,k:&str)->Option<String>{v.get(k).and_then(Value::as_str).map(str::trim).filter(|s|!s.is_empty()).map(str::to_owned)}
fn number(v:&Value,k:&str)->Option<f64>{v.get(k).and_then(|x|x.as_f64().or_else(||x.as_str()?.parse().ok())).filter(|x|x.is_finite()&&*x>0.0)}
fn index_open(path:&Path)->Result<Connection,String>{let conn=Connection::open(path).map_err(|e|e.to_string())?;conn.busy_timeout(Duration::from_secs(3)).map_err(|e|e.to_string())?;conn.execute_batch("PRAGMA journal_mode=WAL;").map_err(|e|e.to_string())?;conn.execute_batch(SCHEMA).map_err(|e|e.to_string())?;Ok(conn)}
impl ExifIndex {
    pub fn new(catalog:PathBuf,tool:PathBuf,formats:Arc<FormatTable>,db:&Db)->Arc<Self>{
        let path=catalog.parent().unwrap_or(Path::new(".")).join("exif.sqlite");
        let service=Arc::new(Self{path:path.clone(),catalog,tool,formats,progress:Mutex::new(Progress::default())});
        match index_open(&path).and_then(|_|db.lock().map_err(|e|e.to_string())?.execute("ATTACH DATABASE ? AS auto_exif",[path.to_string_lossy().as_ref()]).map(|_|()).map_err(|e|e.to_string())){Ok(())=>{let worker=service.clone();thread::spawn(move||{thread::sleep(Duration::from_secs(5));loop{if let Err(e)=worker.scan(){if let Ok(mut p)=worker.progress.lock(){p.running=false;p.error=Some(e)}}thread::sleep(Duration::from_secs(60));}});},Err(e)=>service.progress.lock().unwrap().error=Some(e)}
        service
    }
    pub fn snapshot(&self)->Result<(Vec<Group>,Progress),String>{let conn=Connection::open_with_flags(&self.path,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e|e.to_string())?;let groups=group_query(&conn)?;Ok((groups,self.progress.lock().map_err(|e|e.to_string())?.clone()))}
    fn scan(&self)->Result<(),String>{
        let catalog=Connection::open_with_flags(&self.catalog,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e|e.to_string())?;
        let mut stmt=catalog.prepare("SELECT id,path,size,file_modified,coalesce(ext_lower,ext,'') FROM assets WHERE is_listed=1 AND path IS NOT NULL AND path<>''").map_err(|e|e.to_string())?;
        let assets=stmt.query_map([],|r|Ok(Asset{id:r.get(0)?,path:r.get(1)?,size:r.get(2)?,modified:r.get(3)?,ext:r.get(4)?})).map_err(|e|e.to_string())?.filter_map(Result::ok).filter(|a|matches!(self.formats.group(&a.ext),Some("image"|"raw"))).collect::<Vec<_>>();
        drop(stmt);let mut conn=index_open(&self.path)?;
        let old={let mut s=conn.prepare("SELECT asset_id,path,size,modified FROM exif_index").map_err(|e|e.to_string())?;let rows=s.query_map([],|r|Ok((r.get::<_,i64>(0)?,(r.get::<_,String>(1)?,r.get::<_,Option<f64>>(2)?,r.get::<_,Option<String>>(3)?)))).map_err(|e|e.to_string())?;rows.filter_map(Result::ok).collect::<HashMap<_,_>>()};
        let ids=assets.iter().map(|a|a.id).collect::<std::collections::HashSet<_>>();
        {let tx=conn.transaction().map_err(|e|e.to_string())?;for id in old.keys().filter(|id|!ids.contains(id)){tx.execute("DELETE FROM exif_index WHERE asset_id=?",[id]).map_err(|e|e.to_string())?;}tx.commit().map_err(|e|e.to_string())?;}
        let mut todo=Vec::new();let mut done=0;let mut stale=Vec::new();for a in assets.iter(){if same(a,old.get(&a.id).cloned()){done+=1}else {if old.contains_key(&a.id){stale.push(a.id)}if Path::new(&a.path).is_file(){todo.push(a.clone())}}}
        {let tx=conn.transaction().map_err(|e|e.to_string())?;for id in stale{tx.execute("DELETE FROM exif_index WHERE asset_id=?",[id]).map_err(|e|e.to_string())?;}tx.commit().map_err(|e|e.to_string())?;}
        *self.progress.lock().unwrap()=Progress{done,total:assets.len(),running:!todo.is_empty(),error:None};
        for batch in todo.chunks(200){let values=read_batch(&self.tool,batch)?;let tx=conn.transaction().map_err(|e|e.to_string())?;let mut count=0;
            // Every file of the batch gets a row, even without EXIF, so it is not re-read on every pass.
            for a in batch{let v=values.get(&a.path.to_lowercase().replace('/',"\\")).filter(|v|v.get("Error").is_none()).unwrap_or(&Value::Null);{let make=string(v,"Make");let model=string(v,"Model");let lens=[string(v,"LensModel"),string(v,"LensID"),string(v,"Lens")].into_iter().flatten().find(|s|!placeholder(s));
                tx.execute("INSERT OR REPLACE INTO exif_index(asset_id,path,size,modified,camera,lens,iso,aperture,exposure,focal,ext,bucket) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",params![a.id,a.path,a.size,a.modified,camera(make.as_deref(),model.as_deref()),lens,number(v,"ISO").map(|x|x.round() as i64),number(v,"FNumber").or_else(||number(v,"Aperture")).map(aperture),number(v,"ExposureTime").map(|x|if x<1.0 {1.0/(1.0/x).round().max(1.0)} else {x}),number(v,"FocalLength").map(|x|x.round() as i64),a.ext.to_lowercase(),a.size.filter(|x|*x>=0.0).map(bucket)]).map_err(|e|e.to_string())?;count+=1;}}
            tx.commit().map_err(|e|e.to_string())?;self.progress.lock().unwrap().done+=count;thread::sleep(Duration::from_millis(250));
        }
        self.progress.lock().unwrap().running=false;Ok(())
    }
}
fn read_batch(tool:&Path,assets:&[Asset])->Result<HashMap<String,Value>,String>{
    let mut cmd=Command::new(tool);cmd.args(["-charset","filename=UTF8","-j","-n","-fast2","-Make","-Model","-LensModel","-LensID","-Lens","-ISO","-FNumber","-Aperture","-ExposureTime","-FocalLength","-@","-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)] {use std::os::windows::process::CommandExt;cmd.creation_flags(0x0800_0000|0x0000_4000);}
    let mut child=cmd.spawn().map_err(|e|e.to_string())?;let mut input=child.stdin.take().ok_or("ExifTool stdin")?;
    let paths=assets.iter().map(|a|a.path.clone()).collect::<Vec<_>>();
    let writer=thread::spawn(move||->std::io::Result<()>{for path in paths{if path.contains(['\n','\r']){continue}writeln!(input,"{path}")?;}Ok(())});
    let output=child.wait_with_output().map_err(|e|e.to_string())?;writer.join().map_err(|_|"ExifTool input thread failed")?.map_err(|e|e.to_string())?;if output.stdout.iter().all(u8::is_ascii_whitespace){return Ok(HashMap::new())}let values:Vec<Value>=serde_json::from_slice(&output.stdout).map_err(|e|format!("ExifTool: {e}"))?;
    Ok(values.into_iter().filter_map(|v|v.get("SourceFile").and_then(Value::as_str).map(|p|(p.to_lowercase().replace('/',"\\"),v.clone()))).collect())
}
pub fn field(group:&str)->Option<&'static str>{GROUPS.iter().find(|g|g.0==group).map(|g|g.2)}
fn group_query(conn:&Connection)->Result<Vec<Group>,String>{let mut groups=Vec::new();for (key,name,field,order) in GROUPS{let sql=format!("SELECT {field},count(*) FROM exif_index WHERE {field} IS NOT NULL AND CAST({field} AS TEXT)<>'' GROUP BY {field} ORDER BY {order}");let mut stmt=conn.prepare(&sql).map_err(|e|e.to_string())?;let mut rows=stmt.query([]).map_err(|e|e.to_string())?;let mut values=Vec::new();while let Some(r)=rows.next().map_err(|e|e.to_string())?{let raw=r.get_ref(0).map_err(|e|e.to_string())?;let label=match key{"aperture"=>format!("f/{:.1}",r.get::<_,f64>(0).unwrap()),"exposure"=>exposure(r.get::<_,f64>(0).unwrap()),"focal"=>format!("{} mm",r.get::<_,i64>(0).unwrap()),"bucket"=>bucket_label(r.get::<_,i64>(0).unwrap()).to_string(),_=>match raw{rusqlite::types::ValueRef::Text(t)=>String::from_utf8_lossy(t).into_owned(),rusqlite::types::ValueRef::Integer(n)=>n.to_string(),_=>String::new()}};let value=match raw{rusqlite::types::ValueRef::Real(n)=>n.to_string(),rusqlite::types::ValueRef::Integer(n)=>n.to_string(),rusqlite::types::ValueRef::Text(t)=>String::from_utf8_lossy(t).into_owned(),_=>String::new()};values.push(GroupValue{value,label,count:r.get(1).map_err(|e|e.to_string())?});}if !values.is_empty(){groups.push(Group{key:key.into(),name:name.into(),values});}}Ok(groups)}

#[cfg(test)] mod tests{use super::*; use rusqlite::OptionalExtension;
#[test]fn normalization(){assert_eq!(camera(Some(" Canon "),Some("Canon EOS R6")).as_deref(),Some("Canon EOS R6"));assert_eq!(camera(Some("Canon"),Some("EOS R6")).as_deref(),Some("Canon EOS R6"));assert_eq!(aperture(2.84),2.8);assert!(placeholder("----")&&placeholder("0.0 mm")==false&&placeholder("Unknown")&&!placeholder("E 28-75mm F2.8"));assert_eq!(exposure(0.004),"1/250 s");assert_eq!(exposure(2.0),"2 s");assert_eq!([bucket(0.0),bucket(1_000_000.0),bucket(5_000_000.0),bucket(20_000_000.0),bucket(50_000_000.0)],[0,1,2,3,4]);}
#[test]fn skip_reindex(){let c=Connection::open_in_memory().unwrap();c.execute_batch(SCHEMA).unwrap();c.execute("INSERT INTO exif_index(asset_id,path,size,modified) VALUES(1,'a',10,'x')",[]).unwrap();let old=c.query_row("SELECT path,size,modified FROM exif_index WHERE asset_id=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().unwrap();let mut a=Asset{id:1,path:"a".into(),size:Some(10.0),modified:Some("x".into()),ext:"jpg".into()};assert!(same(&a,old.clone()));a.modified=Some("y".into());assert!(!same(&a,old));}
#[test]fn grouping(){let c=Connection::open_in_memory().unwrap();c.execute_batch(SCHEMA).unwrap();c.execute_batch("INSERT INTO exif_index(asset_id,path,camera,iso) VALUES(1,'a','Canon',100),(2,'b','Canon',200),(3,'c','Nikon',100)").unwrap();let g=group_query(&c).unwrap();assert_eq!(g[0].values[0].count,2);assert_eq!(g[1].values[0].count,2);}}
