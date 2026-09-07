//! All public lifecycle calls cross the controller socket.

use std::path::PathBuf;
use std::time::Duration;

use serde::de::DeserializeOwned;
use stackless_core::engine::{ProgressSink, StepProgress};
use stackless_core::fault::Report;
use stackless_core::state::new_operation_id;
use stackless_daemon::rpc::{Request as DaemonRequest, ResponseBody};

use super::*;
use crate::controller::{Command, Reply, Request};

fn invalid(detail: impl Into<String>) -> Error {
    Error::BadArgument {
        argument: "controller response".into(),
        detail: detail.into(),
    }
}

impl Client {
    pub fn controller_info(&self) -> Result<ControllerInfo, Error> {
        self.control(Request::Controller)
    }

    fn control<T: DeserializeOwned>(&self, request: Request) -> Result<T, Error> {
        if let Some(remote) = &self.inner.remote {
            return remote.call(request);
        }
        let request = serde_json::to_value(request).map_err(|err| invalid(err.to_string()))?;
        let mut connection = self.ensure_daemon()?;
        let body = connection.call(DaemonRequest::Control { request })?;
        let ResponseBody::Control { response } = body else {
            return Err(invalid("daemon did not return a controller response"));
        };
        match serde_json::from_value::<Reply>(response)
            .map_err(|_| invalid("incompatible controller reply"))?
        {
            Reply::Ok { value } => {
                serde_json::from_value(value).map_err(|_| invalid("incompatible controller result"))
            }
            Reply::Err { error } => Err(Error::Controller(error)),
        }
    }

    fn prepare_up_command(&self, mut args: UpArgs) -> Result<Command, Error> {
        let cwd = std::env::current_dir().map_err(Error::Runtime)?;
        if let Some(path) = &mut args.file
            && path.is_relative()
        {
            *path = cwd.join(&*path);
        }
        let path = args
            .file
            .clone()
            .unwrap_or_else(|| cwd.join("stackless.toml"));
        let definition = match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(error) if args.file.is_none() && error.kind() == std::io::ErrorKind::NotFound => {
                None
            }
            Err(source) => {
                return Err(Error::FileRead {
                    path: path.display().to_string(),
                    source,
                });
            }
        };
        args.sources = parse_sources(&args.sources)?
            .into_iter()
            .map(|(service, path)| {
                let path = PathBuf::from(path);
                let path = if path.is_absolute() {
                    path
                } else {
                    cwd.join(path)
                };
                format!("{service}={}", path.display())
            })
            .collect();
        Ok(Command::Up {
            args,
            definition,
            cwd,
        })
    }

    fn submit(&self, command: Command) -> Result<Operation, Error> {
        self.submit_with_id(&new_operation_id(), command)
    }

    fn submit_with_id(&self, id: &str, command: Command) -> Result<Operation, Error> {
        for attempt in 0..2 {
            match self.control(Request::Submit {
                id: id.into(),
                command: command.clone(),
            }) {
                Ok(operation) => return Ok(operation),
                Err(Error::Daemon(_)) if attempt == 0 => continue,
                Err(error) => {
                    let mut report = Report::from_fault(&error);
                    report.message = format!("{}; submission ID {id}", report.message);
                    return Err(Error::Controller(Box::new(report)));
                }
            }
        }
        Err(invalid("submission did not produce a response"))
    }

    pub(crate) fn submit_up_args(&self, args: UpArgs) -> Result<Operation, Error> {
        self.submit(self.prepare_up_command(args)?)
    }

    pub(crate) fn up_from_args_with_progress(
        &self,
        args: UpArgs,
        progress: Option<&mut dyn ProgressSink>,
    ) -> Result<UpOutcome, Error> {
        let operation = self.submit(self.prepare_up_command(args)?)?;
        self.wait_operation(&operation.id, progress)
    }

    /// Submit without waiting. The returned ID can be observed from a new client.
    pub fn submit_up(&self, request: UpRequest) -> Result<Operation, Error> {
        self.submit_up_with_id(&new_operation_id(), request)
    }

    pub fn submit_up_with_id(&self, id: &str, request: UpRequest) -> Result<Operation, Error> {
        let args = match request {
            UpRequest::Create(create) => UpArgs {
                name: create.name,
                file: create.file,
                on: Some(create.on),
                sources: create.sources,
                dirty: create.dirty,
                allow_host_execution: create.allow_host_execution,
                lease: create.lease,
                confirm_paid: create.paid.as_confirm_paid(),
            },
            UpRequest::Resume(resume) => UpArgs {
                name: Some(resume.name),
                file: resume.file,
                on: None,
                sources: resume.sources,
                dirty: resume.dirty,
                allow_host_execution: resume.allow_host_execution,
                lease: resume.lease,
                confirm_paid: false,
            },
        };
        self.submit_with_id(id, self.prepare_up_command(args)?)
    }

    pub fn submit_down(&self, name: &str) -> Result<Operation, Error> {
        self.submit(Command::Down { name: name.into() })
    }

    pub fn operation(&self, id: &str, after: i64) -> Result<OperationPage, Error> {
        self.control(Request::Operation {
            id: id.into(),
            after,
        })
    }

    pub fn operations(&self, instance: Option<&str>) -> Result<Vec<Operation>, Error> {
        self.control(Request::Operations {
            instance: instance.map(str::to_owned),
        })
    }

    pub fn cancel_operation(&self, id: &str) -> Result<Operation, Error> {
        self.control(Request::Cancel { id: id.into() })
    }

    pub fn wait_operation<T: DeserializeOwned>(
        &self,
        id: &str,
        mut progress: Option<&mut dyn ProgressSink>,
    ) -> Result<T, Error> {
        let mut cursor = 0;
        loop {
            let page = self.operation(id, cursor).map_err(|err| {
                let mut report = Report::from_fault(&err);
                report.message = format!("{}; operation {id} can be reconnected", report.message);
                Error::Controller(Box::new(report))
            })?;
            let event_count = page.events.len();
            for event in page.events {
                cursor = event.sequence;
                if let Some(sink) = progress.as_deref_mut() {
                    let event: StepProgress = serde_json::from_value(event.event)
                        .map_err(|err| invalid(err.to_string()))?;
                    sink.on_step(event);
                }
            }
            if page.operation.status.terminal() && event_count < 256 {
                if let Some(error) = page.operation.error {
                    let report: Report =
                        serde_json::from_value(error).map_err(|err| invalid(err.to_string()))?;
                    return Err(Error::Controller(Box::new(report)));
                }
                if page.operation.status != OperationStatus::Succeeded {
                    let code = match page.operation.status {
                        OperationStatus::Cancelled => {
                            stackless_core::fault::codes::OPERATION_CANCELLED
                        }
                        OperationStatus::Interrupted => {
                            stackless_core::fault::codes::OPERATION_INTERRUPTED
                        }
                        _ => stackless_core::fault::codes::OPERATION_RESULT_MISSING,
                    };
                    return Err(Error::Controller(Box::new(Report {
                        schema_version: 2, code: code.into(),
                        message: format!("operation {id} is {:?}", page.operation.status),
                        step: None, instance: Some(page.operation.instance),
                        remediation: "inspect the operation; run up to reconcile, verify to retry a proof, or down to clean up".into(),
                        context: Default::default(),
                    })));
                }
                return serde_json::from_value(
                    page.operation.result.unwrap_or(serde_json::Value::Null),
                )
                .map_err(|err| invalid(err.to_string()));
            }
            std::thread::sleep(Duration::from_millis(if self.inner.remote.is_some() {
                500
            } else {
                100
            }));
        }
    }

    pub fn down(&self, name: &str) -> Result<DownOutcome, Error> {
        let operation = self.submit_down(name)?;
        self.wait_operation(&operation.id, None)
    }
    pub fn submit_verify(&self, name: &str, tier: Option<&str>) -> Result<Operation, Error> {
        self.submit(Command::Verify {
            name: name.into(),
            tier: tier.map(str::to_owned),
        })
    }
    pub fn verify(&self, name: &str, tier: Option<&str>) -> Result<VerifyOutcome, Error> {
        let operation = self.submit_verify(name, tier)?;
        self.wait_operation(&operation.id, None)
    }
    pub fn status(&self, name: &str) -> Result<InstanceReport, Error> {
        self.control(Request::Status { name: name.into() })
    }
    pub fn list(&self) -> Result<Vec<InstanceReport>, Error> {
        self.control(Request::List)
    }
    pub fn logs(
        &self,
        name: &str,
        service: Option<&str>,
        tail: usize,
    ) -> Result<LogsOutcome, Error> {
        self.control(Request::Logs {
            name: name.into(),
            service: service.map(str::to_owned),
            tail,
        })
    }
}
