//! Optional development sources; shipping builds use the embedded bytes unchanged.
use std::{borrow::Cow, path::Path};

pub(crate) fn load(name: &str, embedded: &'static str) -> Cow<'static, str> {
    let directory = std::env::var_os("AOMAC_SHADER_DIR");
    load_from(directory.as_deref().map(Path::new), name, embedded)
        .unwrap_or_else(|error| panic!("{error}"))
}

fn load_from(directory: Option<&Path>, name: &str, embedded: &'static str) -> std::io::Result<Cow<'static, str>> {
    let Some(directory) = directory else { return Ok(Cow::Borrowed(embedded)); };
    let path = directory.join(name);
    std::fs::read_to_string(&path).map(Cow::Owned).map_err(|error| {
        std::io::Error::new(error.kind(), format!("AOMAC_SHADER_DIR: cannot read shader {}: {error}; unset AOMAC_SHADER_DIR to use embedded sources", path.display()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_or_explicit_directory_without_environment_mutation() {
        let embedded = load_from(None, "shader.wgsl", "embedded").unwrap();
        assert!(matches!(embedded, Cow::Borrowed("embedded")));
        let directory = std::env::temp_dir().join(format!("aomac-shader-source-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("shader.wgsl");
        std::fs::write(&path, "override").unwrap();
        assert_eq!(load_from(Some(&directory), "shader.wgsl", "embedded").unwrap(), "override");
        std::fs::write(&path, [0xff]).unwrap();
        assert_eq!(load_from(Some(&directory), "shader.wgsl", "embedded").unwrap_err().kind(), std::io::ErrorKind::InvalidData);
        std::fs::remove_file(&path).unwrap();
        let error = load_from(Some(&directory), "shader.wgsl", "embedded").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert!(error.to_string().contains("AOMAC_SHADER_DIR"));
        assert!(error.to_string().contains(&path.display().to_string()));
        std::fs::remove_dir(directory).unwrap();
    }
}
