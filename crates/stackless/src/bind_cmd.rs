//! `stackless bind` — thin adapter over `stackless-idl`.

use std::path::{Path, PathBuf};

use stackless_idl::{EmitTarget, IdlError, check_bytes, compile_source, emit_for, write_atomic};

use crate::error::Error;
use crate::output::Output;

#[derive(Debug)]
pub struct BindArgs {
    pub file: PathBuf,
    pub idl: PathBuf,
    /// `(language, path)` pairs from `--emit LANG=PATH` plus `--rs` / `--ts` aliases.
    pub emits: Vec<(String, PathBuf)>,
    pub go_package: String,
    pub check: bool,
}

pub fn bind(args: BindArgs, output: &Output) -> Result<(), Error> {
    let text = std::fs::read_to_string(&args.file).map_err(|source| Error::FileRead {
        path: args.file.display().to_string(),
        source,
    })?;
    let known = crate::substrates::known_names();
    let compiled = compile_source(&text, &known).map_err(map_idl)?;
    let idl_json = compiled.pretty_json;
    let idl = compiled.idl;

    let mut planned: Vec<(PathBuf, String)> = Vec::new();
    planned.push((args.idl.clone(), idl_json));

    for (language, path) in &args.emits {
        let target = EmitTarget::parse(language, &args.go_package).map_err(map_idl)?;
        let body = emit_for(&target, &idl).map_err(map_idl)?;
        planned.push((path.clone(), body));
    }

    if args.check {
        for (path, contents) in &planned {
            check_bytes(path, contents).map_err(map_idl)?;
        }
        output.message(&format!("bind check ok ({} outputs)", planned.len()));
        return Ok(());
    }

    for (path, contents) in &planned {
        write_atomic(path, contents).map_err(|err| map_write(path, err))?;
        output.message(&format!("wrote {}", path.display()));
    }
    Ok(())
}

/// Parse `--emit LANG=PATH` values into language/path pairs.
pub fn parse_emit_specs(specs: &[String]) -> Result<Vec<(String, PathBuf)>, Error> {
    let mut out = Vec::new();
    for spec in specs {
        let Some((lang, path)) = spec.split_once('=') else {
            return Err(Error::BadArgument {
                argument: "bind --emit".into(),
                detail: format!("expected LANG=PATH, got {spec:?}"),
            });
        };
        if lang.is_empty() || path.is_empty() {
            return Err(Error::BadArgument {
                argument: "bind --emit".into(),
                detail: format!("expected LANG=PATH, got {spec:?}"),
            });
        }
        out.push((lang.to_owned(), PathBuf::from(path)));
    }
    Ok(out)
}

fn map_idl(err: IdlError) -> Error {
    match err {
        IdlError::IoRead { path, source } => Error::FileRead {
            path: path.display().to_string(),
            source,
        },
        IdlError::IoWrite { path, source } => Error::FileWrite {
            path: path.display().to_string(),
            source,
        },
        IdlError::Stale { path } | IdlError::Missing { path } => Error::BadArgument {
            argument: "bind --check".into(),
            detail: format!("{} is stale or missing", path.display()),
        },
        other => Error::BadArgument {
            argument: "bind".into(),
            detail: other.to_string(),
        },
    }
}

fn map_write(path: &Path, err: IdlError) -> Error {
    match err {
        IdlError::IoWrite { path, source } => Error::FileWrite {
            path: path.display().to_string(),
            source,
        },
        other => Error::FileWrite {
            path: path.display().to_string(),
            source: std::io::Error::other(other.to_string()),
        },
    }
}
