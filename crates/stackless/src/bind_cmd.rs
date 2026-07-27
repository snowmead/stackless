//! `stackless bind` — thin adapter over `stackless-idl`.

use std::path::{Path, PathBuf};

use stackless_idl::{
    IdlError, check_bytes, compile_source, emit_rust_from_idl, emit_typescript_from_idl,
    write_atomic,
};

use crate::error::Error;
use crate::output::Output;

#[derive(Debug)]
pub struct BindArgs {
    pub file: PathBuf,
    pub idl: PathBuf,
    pub ts: Option<PathBuf>,
    pub rs: Option<PathBuf>,
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

    if let Some(path) = &args.rs {
        let rust = emit_rust_from_idl(&idl).map_err(map_idl)?;
        planned.push((path.clone(), rust));
    }
    if let Some(path) = &args.ts {
        let ts = emit_typescript_from_idl(&idl).map_err(map_idl)?;
        planned.push((path.clone(), ts));
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
