//! User-authored 3D .cube LUTs. Red changes fastest, as specified by the .cube format.
use std::io::Read;

pub(super) const MAX_BYTES: usize = 16 * 1024 * 1024;
#[derive(Clone, Debug)]
pub(super) struct CubeLut {
    pub size: u32,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub values: Vec<[f32; 4]>,
}
impl CubeLut {
    pub fn identity() -> Self {
        Self { size: 2, min: [0.0; 3], max: [1.0; 3], values: (0..8).map(|i| [(i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32, 1.0]).collect() }
    }
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > MAX_BYTES { return Err(".cube exceeds 16 MiB".into()); }
        let mut lut = Self { size: 0, min: [0.0; 3], max: [1.0; 3], values: Vec::new() };
        let (mut has_min, mut has_max) = (false, false);
        for (line_no, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() { continue; }
            let fail = || format!("invalid .cube line {}", line_no + 1);
            let mut parts = line.split_whitespace();
            let first = parts.next().ok_or_else(fail)?;
            if first == "TITLE" {
                if !lut.values.is_empty() || !line[5..].trim().starts_with('"') || !line.ends_with('"') { return Err(fail()); }
                continue;
            }
            if first == "LUT_3D_SIZE" {
                if lut.size != 0 || !lut.values.is_empty() { return Err(fail()); }
                lut.size = parts.next().ok_or_else(fail)?.parse().map_err(|_| fail())?;
                if !(2..=65).contains(&lut.size) || parts.next().is_some() { return Err(fail()); }
                lut.values.reserve((lut.size as usize).pow(3));
                continue;
            }
            if first == "DOMAIN_MIN" || first == "DOMAIN_MAX" {
                if !lut.values.is_empty() { return Err(fail()); }
                let seen = if first == "DOMAIN_MIN" { &mut has_min } else { &mut has_max };
                if *seen { return Err(fail()); }
                *seen = true;
                let mut value = [0.0; 3];
                for v in &mut value { *v = parts.next().ok_or_else(fail)?.parse::<f32>().map_err(|_| fail())?; if !v.is_finite() { return Err(fail()); } }
                if parts.next().is_some() { return Err(fail()); }
                if first == "DOMAIN_MIN" { lut.min = value; } else { lut.max = value; }
                continue;
            }
            if lut.size == 0 || lut.values.len() >= (lut.size as usize).pow(3) { return Err(fail()); }
            let mut value = [0.0, 0.0, 0.0, 1.0];
            value[0] = first.parse::<f32>().map_err(|_| fail())?;
            for v in &mut value[1..3] { *v = parts.next().ok_or_else(fail)?.parse::<f32>().map_err(|_| fail())?; }
            if parts.next().is_some() || value[..3].iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v)) { return Err(fail()); }
            lut.values.push(value);
        }
        if lut.size == 0 || lut.values.len() != (lut.size as usize).pow(3) || (0..3).any(|i| lut.max[i] <= lut.min[i] || !(lut.max[i] - lut.min[i]).is_finite()) { return Err("invalid .cube domain or sample count".into()); }
        Ok(lut)
    }
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut text = String::new();
        file.take((MAX_BYTES + 1) as u64).read_to_string(&mut text).map_err(|e| e.to_string())?;
        Self::parse(&text)
    }

    pub fn for_zone(root: &std::path::Path, zone: Option<u32>, phase: &str) -> Result<Self, String> {
        let zone_path = zone.map(|id| root.join("luts").join(format!("{id}-{phase}.cube")));
        let path = match zone_path {
            Some(path) if path.try_exists().map_err(|e| e.to_string())? => path,
            _ => root.join(format!("randy-vk-{phase}.cube")),
        };
        if !path.try_exists().map_err(|e| e.to_string())? { return Ok(Self::identity()); }
        Self::load(&path).map_err(|error| format!("{}: {error}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cube_identity_order_and_rejection() {
        let identity = CubeLut::identity();
        let mut text = String::from("TITLE \"Identity\"\nLUT_3D_SIZE 2\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 1 1 1\n");
        for v in &identity.values { text += &format!("{} {} {}\n", v[0], v[1], v[2]); }
        assert_eq!(CubeLut::parse(&text).unwrap().values, identity.values);
        for bad in ["LUT_3D_SIZE 1", "LUT_3D_SIZE 66", "LUT_3D_SIZE 2\nNaN 0 0", "LUT_1D_SIZE 2", "LUT_3D_SIZE 2\n0 0 0 0", "LUT_3D_SIZE 2\n2 0 0"] { assert!(CubeLut::parse(bad).is_err(), "{bad}"); }
        assert!(CubeLut::parse(&(text.clone() + "0 0 0\n")).is_err());
        assert!(CubeLut::parse(&text.replace("DOMAIN_MAX 1 1 1", "DOMAIN_MAX 0 1 1")).is_err());
        assert!(CubeLut::parse(&text.replace("DOMAIN_MIN 0 0 0", "DOMAIN_MIN NaN 0 0")).is_err());
        assert!(CubeLut::parse(&" ".repeat(MAX_BYTES + 1)).is_err());
    }

    #[test]
    fn zone_phase_override_missing_identity_and_bounded_file_loading() {
        let root = std::env::temp_dir().join(format!("aomac-cube-zone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("luts")).unwrap();
        let constant = |rgb: &str| "LUT_3D_SIZE 2\n".to_owned() + &format!("{rgb}\n").repeat(8);
        std::fs::write(root.join("randy-vk-day.cube"), constant("1 0 0")).unwrap();
        std::fs::write(root.join("luts/42-day.cube"), constant("0 1 0")).unwrap();
        assert_eq!(CubeLut::for_zone(&root, Some(42), "day").unwrap().values[0], [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(CubeLut::for_zone(&root, Some(43), "day").unwrap().values[0], [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(CubeLut::for_zone(&root, Some(42), "night").unwrap().values, CubeLut::identity().values);
        std::fs::write(root.join("luts/42-day.cube"), "LUT_3D_SIZE 99").unwrap();
        assert!(CubeLut::for_zone(&root, Some(42), "day").is_err(), "bad override must not retain another zone's LUT");
        std::fs::write(root.join("randy-vk-night.cube"), " ".repeat(MAX_BYTES + 1)).unwrap();
        assert!(CubeLut::for_zone(&root, None, "night").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
