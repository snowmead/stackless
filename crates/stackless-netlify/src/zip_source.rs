//! Zip a shallow checkout for Netlify's build API.

use base64::Engine as _;
use std::io::Write;
use std::path::Path;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::error::NetlifyError;

pub fn zip_directory(root: &Path) -> Result<Vec<u8>, NetlifyError> {
    zip_directory_beneath(root, Path::new("."))
}

pub fn zip_directory_beneath(root: &Path, relative: &Path) -> Result<Vec<u8>, NetlifyError> {
    let archive = stackless_core::source_archive::SourceArchive::capture_beneath(root, relative)
        .map_err(|e| provision(e.to_string()))?;
    zip_archive(archive)
}

pub fn zip_archive(
    archive: stackless_core::source_archive::SourceArchive,
) -> Result<Vec<u8>, NetlifyError> {
    if archive.files.is_empty() {
        return Err(provision("zip archive is empty".into()));
    }
    let mut buffer = Vec::new();
    {
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .last_modified_time(zip::DateTime::default());
        for file in archive.files {
            let data = base64::engine::general_purpose::STANDARD
                .decode(file.contents)
                .map_err(|e| provision(e.to_string()))?;
            let permissions = if file.executable { 0o755 } else { 0o644 };
            zip.start_file(file.path, options.unix_permissions(permissions))
                .map_err(|e| provision(e.to_string()))?;
            zip.write_all(&data).map_err(|e| provision(e.to_string()))?;
        }
        zip.finish().map_err(|e| provision(e.to_string()))?;
    }
    Ok(buffer)
}

fn provision(detail: String) -> NetlifyError {
    NetlifyError::ProvisionFailed {
        resource: "netlify-build".into(),
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zips_files_under_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), b"ok").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/a.txt"), b"a").unwrap();
        let bytes = zip_directory(dir.path()).unwrap();
        assert!(bytes.len() > 20);
        // ZIP local file header magic
        assert_eq!(&bytes[..2], b"PK");
    }
}
