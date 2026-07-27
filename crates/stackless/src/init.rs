//! `stackless init` — scaffold a minimal valid `stackless.toml`.

use std::path::PathBuf;

use crate::authoring::{
    self, default_output_path, default_source, definition_dir, static_web_template,
};
use crate::error::Error;
use crate::output::Output;

pub struct InitArgs {
    pub name: Option<String>,
    pub file: Option<PathBuf>,
    pub force: bool,
}

pub fn init(args: InitArgs, output: &Output) -> Result<(), Error> {
    let file = args.file.unwrap_or_else(default_output_path);
    if file.exists() && !args.force {
        return Err(Error::InitExists {
            path: file.display().to_string(),
        });
    }
    let dir = definition_dir(&file);
    let stack = authoring::resolve_stack_name(args.name.as_deref(), &dir)?;
    let source = default_source(&dir);
    let text = static_web_template(&stack, &source);
    stackless_core::def::StackDef::parse(&text)?;
    std::fs::write(&file, &text).map_err(|source| Error::FileWrite {
        path: file.display().to_string(),
        source,
    })?;
    output.init_ok(file.display().to_string(), &stack);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static CWD_LOCK: Mutex<()> = Mutex::new(());

    fn with_cwd<F: FnOnce()>(dir: &std::path::Path, f: F) {
        let _guard = CWD_LOCK.lock().unwrap();
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir).unwrap();
        f();
        let _ = std::env::set_current_dir(previous);
    }

    #[test]
    fn init_writes_valid_definition() {
        let dir = tempfile::tempdir().unwrap();
        with_cwd(dir.path(), || {
            let file = dir.path().join("stackless.toml");
            init(
                InitArgs {
                    name: Some("demo".into()),
                    file: Some(file.clone()),
                    force: false,
                },
                &Output::new(false),
            )
            .unwrap();
            let text = std::fs::read_to_string(&file).unwrap();
            stackless_core::def::StackDef::parse(&text).unwrap();
            assert!(text.contains("name = \"demo\""));
        });
    }

    #[test]
    fn init_refuses_existing_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("stackless.toml");
        std::fs::write(&file, "existing").unwrap();
        let err = init(
            InitArgs {
                name: None,
                file: Some(file),
                force: false,
            },
            &Output::new(false),
        )
        .unwrap_err();
        assert!(matches!(err, Error::InitExists { .. }));
    }
}
