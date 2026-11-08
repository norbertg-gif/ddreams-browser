use serde::Serialize;

#[derive(Clone, Copy, Debug, Serialize)]
pub struct ColorInfo { pub value: &'static str, pub name: &'static str, pub hex: &'static str }

pub const PALETTE: [ColorInfo;13] = [
    ColorInfo{value:"červená",name:"Červená",hex:"#d33"}, ColorInfo{value:"oranžová",name:"Oranžová",hex:"#e8822a"},
    ColorInfo{value:"hnedá",name:"Hnedá",hex:"#8b5a2b"}, ColorInfo{value:"žltá",name:"Žltá",hex:"#e6c229"},
    ColorInfo{value:"zelená",name:"Zelená",hex:"#3a9d3a"}, ColorInfo{value:"tyrkysová",name:"Tyrkysová",hex:"#2bb3b1"},
    ColorInfo{value:"modrá",name:"Modrá",hex:"#2f6fd6"}, ColorInfo{value:"fialová",name:"Fialová",hex:"#8e44ad"},
    ColorInfo{value:"ružová",name:"Ružová",hex:"#e86fb0"}, ColorInfo{value:"biela",name:"Biela",hex:"#f2f2f2"},
    ColorInfo{value:"sivá",name:"Sivá",hex:"#8a8f96"}, ColorInfo{value:"čierna",name:"Čierna",hex:"#111"},
    ColorInfo{value:"čiernobiela",name:"Čiernobiela",hex:"linear-gradient(90deg,#111 50%,#f2f2f2 50%)"},
];

pub fn effective_colors(auto: &[String], overrides: &[(String,i64)]) -> Vec<String> {
    let mut result=auto.iter().filter(|color| overrides.iter().find(|(name,_)| name==*color).map_or(true,|(_,state)|*state!=0)).cloned().collect::<Vec<_>>();
    for (color,state) in overrides { if *state==1 && !result.iter().any(|value|value==color) { result.push(color.clone()); } }
    result
}

pub fn unique_name_match(name:&str,candidates:&[(i64,String)])->Result<Option<i64>,()>{
    let key=std::path::Path::new(name).file_name().and_then(|v|v.to_str()).unwrap_or(name).to_lowercase();
    let mut found=candidates.iter().filter(|(_,candidate)|candidate.to_lowercase()==key).map(|(id,_)|*id);
    let first=found.next();if found.next().is_some(){Err(())}else{Ok(first)}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn effective_auto_add_remove_and_clear_semantics() {
        let auto=vec!["červená".to_string(),"zelená".to_string()];
        assert_eq!(effective_colors(&auto,&[("zelená".into(),0),("modrá".into(),1)]),vec!["červená","modrá"]);
        assert_eq!(effective_colors(&auto,&[]),auto);
    }
    #[test]
    fn photosort_import_only_accepts_unique_case_insensitive_names(){
        let rows=vec![(1,"Flower.JPG".into()),(2,"Other.jpg".into()),(3,"flower.jpg".into())];
        assert_eq!(unique_name_match("other.JPG",&rows),Ok(Some(2)));
        assert_eq!(unique_name_match("missing.jpg",&rows),Ok(None));
        assert_eq!(unique_name_match("FLOWER.jpg",&rows),Err(()));
    }
}
