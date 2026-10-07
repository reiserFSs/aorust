//! DS1006d49a: ABIFF connector names -> AttractorEffectData_t.
use crate::archive::Archive;
use anyhow::{ensure,Context,Result};

#[derive(Clone,Debug,PartialEq)]
pub struct MeshEffectAttr {
    /// Depth-first RRefFrame index, shared with NodeRig::frame_transform.
    pub frame:usize,
    /// Rest-pose connector matrix, RH column-vector scene convention.
    pub transform:[[f32;4];4],
    pub effect:i32,
    pub color:Option<[f32;4]>, // RGBA; native flags bit0
    pub size:Option<f32>, // native flags bit1
}
const NAMES:[(&str,i32);31]=[
    ("muzzleflash",2000),("explosion",2100),("fire02",2200),("fire0*",2200),
    ("fire1*",2201),("fire2*",2202),("fire3*",2203),("fire4*",2204),
    ("fire5*",2205),("fire6*",2206),("fire7*",2207),("fire8*",2208),("fire9*",2209),
    ("smoke1",2301),("smoke2",2302),("smoke*",2300),("halo_white",7000),
    ("halo_red",7001),("halo_green",7002),("halo_blue",7003),("billboard",7100),
    ("geyshir*",12100),("waterdrip*",12110),("halo_S*",7006),("torch",12124),
    ("lightsabre*",90000),("halo_M*",7004),("halo_L*",7005),
    ("haloS*",7006),("haloM*",7004),("haloL*",7005),
];
/// Native rendering-only eff_hologram/space/clearzbuffer do not create children.
pub fn parse_effect_name(name:&str,frame:usize)->Result<Option<MeshEffectAttr>> {
    let Some(name)=name.strip_prefix("eff_") else {return Ok(None)};
    let mut out=MeshEffectAttr {frame,transform:ao_scene::IDENTITY,effect:7001,color:None,size:None};
    if let Some(id)=name.strip_prefix("universal_").or_else(||name.strip_prefix("universal")) {
        // sscanf %d accepts a decimal prefix, not an arbitrary trailing suffix.
        let end=id.char_indices().skip(usize::from(id.starts_with(['+','-']))).find(|(_,c)|!c.is_ascii_digit()).map_or(id.len(),|(i,_)|i);
        out.effect=id[..end].parse().context("invalid universal effect id")?;
        return Ok((out.effect!=0).then_some(out));
    }
    if name.starts_with("hologram") || name.starts_with("space") || name.starts_with("clear") {return Ok(None)}
    for (pattern,effect) in NAMES {
        if let Some(prefix)=pattern.strip_suffix('*') {
            if let Some(suffix)=name.strip_prefix(prefix) {
                out.effect=effect;
                let bytes=suffix.as_bytes();let mut values=[0.0;5];let mut count=0;
                while count<5 && count*2<bytes.len() {
                    let at=count*2;let end=(at+2).min(bytes.len());
                    let token=std::str::from_utf8(&bytes[at..end])?;
                    let value=u8::from_str_radix(token,16).context("invalid mesh effect hexadecimal override")?;
                    values[count]=value as f32/255.0;count+=1;
                }
                if count==1 || count==5 {out.size=Some(values[count-1]*4.0)}
                if count>=4 {out.color=Some([values[0],values[1],values[2],values[3]])}
                return Ok(Some(out));
            }
        } else if name==pattern {out.effect=effect;return Ok(Some(out))}
    }
    Ok(Some(out)) // DS native unknown eff_ name fallback is red halo7001.
}
/// Retains hierarchy indices even for connector frames without geometry.
pub fn mesh_effect_attrs(bytes:&[u8])->Result<Vec<MeshEffectAttr>> {
    let ar=Archive::parse(bytes)?;let mut attrs=Vec::new();let mut seen=vec![false;ar.objects.len()];let mut frame=0;
    fn walk(ar:&Archive,index:usize,depth:usize,parent:&super::Mat,seen:&mut[bool],frame:&mut usize,out:&mut Vec<MeshEffectAttr>)->Result<()> {
        ensure!(depth<=64,"mesh effect hierarchy too deep");
        let node=ar.objects.get(index).context("dangling mesh effect frame")?;
        ensure!(!seen[index],"repeated mesh effect hierarchy frame");seen[index]=true;
        let current=*frame;*frame+=1;
        let matrix=super::mul(&super::local_matrix(node),parent);
        ensure!(matrix.iter().flatten().all(|v|v.is_finite()),"nonfinite mesh effect transform");
        let connector=node.ref1("conn").map(|i|ar.objects.get(i).context("dangling mesh connector")).transpose()?;
        if let Some(bytes)=connector.and_then(|c|c.blob("name")) {
            let bytes=bytes.strip_suffix(&[0]).unwrap_or(bytes);
            let name=std::str::from_utf8(bytes).context("invalid connector name")?;
            if let Some(mut attr)=parse_effect_name(name,current)? {
                attr.transform=std::array::from_fn(|i|std::array::from_fn(|j|matrix[i][j]*if (i==2) != (j==2) {-1.0}else{1.0}));
                out.push(attr);
            }
        }
        for child in node.refs("chld") {walk(ar,child,depth+1,&matrix,seen,frame,out)?}
        Ok(())
    }
    walk(&ar,ar.root,0,&super::IDENTITY_MAT,&mut seen,&mut frame,&mut attrs)?;Ok(attrs)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_mesh_effect_name_table() {
        let halo=parse_effect_name("eff_haloSd47d00ff40",7).unwrap().unwrap();
        assert_eq!(halo.effect,7006);assert_eq!(halo.frame,7);
        assert_eq!(halo.color,Some([212.0/255.0,125.0/255.0,0.0,1.0]));assert_eq!(halo.size,Some(64.0/255.0*4.0));
        assert_eq!(parse_effect_name("eff_universal_72106",0).unwrap().unwrap().effect,72106);
        assert_eq!(parse_effect_name("eff_smoke",0).unwrap().unwrap().effect,2300);
        assert!(parse_effect_name("eff_hologram",0).unwrap().is_none());
        assert!(parse_effect_name("eff_fire2gg",0).is_err());
    }
    #[test]
    #[ignore="installed retail ABIFF assets"]
    fn installed_effect_connector_metadata()->Result<()> {
        let root=std::env::var_os("AO_CLIENT_DIR").map(std::path::PathBuf::from).unwrap_or_else(||std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join("Games/ProjectRubiKa/client"));
        let store=ao_rdb::RecordStore::open(&root)?;
        let attrs=mesh_effect_attrs(&store.get(super::super::MESH_TYPE,32247)?.context("mesh32247")?)?;
        ensure!(attrs.iter().any(|a|a.effect==7006 && a.color.is_some() && a.size.is_some()),"missing authored halo override");
        ensure!(attrs.iter().filter(|a|a.effect==2300).count()==4,"missing authored smoke connectors");Ok(())
    }
}
