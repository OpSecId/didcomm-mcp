//! Windows: a Windows service, run as LocalSystem. Its directory under `%ProgramData%`
//! (configuration with the bearer token, identity, log) is restricted to SYSTEM and
//! Administrators.
//!
//! The service manager starts `didcomm-mcp --windows-service --config <path>`, which
//! talks the service control protocol and serves `--http` until it's stopped.

use std::ffi::OsString;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Context;
use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept, ServiceErrorControl,
    ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod, ServiceInfo, ServiceStartType, ServiceState,
    ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

use super::NAME;

/// The (hidden) argument the service manager starts the binary with.
pub const RUN_AS_SERVICE: &str = "--windows-service";

/// The arguments after [`RUN_AS_SERVICE`], for [`service_main`].
static ARGS: OnceLock<Vec<String>> = OnceLock::new();

/// Run as the service. Only the service manager starts the binary this way.
pub fn run(args: &[String]) -> anyhow::Result<()> {
    let _ = ARGS.set(args.to_vec());
    service_dispatcher::start(NAME, ffi_service_main).context(
        "this mode is for the Windows service manager; run `didcomm-mcp --http` to serve from a terminal",
    )?;
    Ok(())
}

define_windows_service!(ffi_service_main, service_main);

fn service_main(_arguments: Vec<OsString>) {
    if let Err(e) = serve() {
        tracing::error!("{e:#}");
    }
}

fn serve() -> anyhow::Result<()> {
    let mut args = crate::parse_args(ARGS.get().cloned().unwrap_or_default())?;
    args.http.get_or_insert(None);
    // The service has no console: log next to the configuration.
    if let Some(dir) = args.config.as_deref().and_then(Path::parent) {
        if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("didcomm-mcp.log")) {
            crate::init_logging(Mutex::new(file));
        }
    }

    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let stop = Mutex::new(Some(stop));
    let status = service_control_handler::register(NAME, move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            if let Some(stop) = stop.lock().unwrap().take() {
                let _ = stop.send(());
            }
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    let report = |state: ServiceState, exit_code: ServiceExitCode| {
        status.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: if state == ServiceState::Running {
                ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
            } else {
                ServiceControlAccept::empty()
            },
            exit_code,
            checkpoint: 0,
            wait_hint: Duration::from_secs(10),
            process_id: None,
        })
    };
    report(ServiceState::Running, ServiceExitCode::Win32(0))?;
    let result = tokio::runtime::Runtime::new()?.block_on(crate::run(args, async {
        let _ = stopped.await;
    }));
    if let Err(e) = &result {
        tracing::error!("{e:#}");
    }
    // A non-zero exit lets the failure actions restart it.
    let code = if result.is_ok() { ServiceExitCode::Win32(0) } else { ServiceExitCode::ServiceSpecific(1) };
    report(ServiceState::Stopped, code)?;
    result
}

/// Access denied means the terminal isn't elevated; say so.
fn explain(error: windows_service::Error, doing: &str) -> anyhow::Error {
    let denied = match &error {
        windows_service::Error::Winapi(io) => io.raw_os_error() == Some(5),
        _ => false,
    };
    if denied {
        anyhow::anyhow!("{doing}: access denied. Run this from a terminal opened as Administrator.")
    } else {
        anyhow::Error::new(error).context(doing.to_string())
    }
}

/// Install (or update) and (re)start the service; returns how to manage it.
pub fn install(exe: &Path, config: &Path) -> anyhow::Result<String> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)
        .map_err(|e| explain(e, "opening the service manager"))?;
    let info = ServiceInfo {
        name: OsString::from(NAME),
        display_name: OsString::from("DIDComm MCP server"),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe.to_path_buf(),
        launch_arguments: vec![RUN_AS_SERVICE.into(), "--config".into(), config.as_os_str().to_owned()],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };
    let access = ServiceAccess::QUERY_STATUS | ServiceAccess::START | ServiceAccess::STOP | ServiceAccess::CHANGE_CONFIG;
    let service = match manager.open_service(NAME, access) {
        Ok(existing) => {
            existing.change_config(&info).map_err(|e| explain(e, "updating the service"))?;
            stop_and_wait(&existing)?;
            existing
        }
        Err(_) => manager.create_service(&info, access).map_err(|e| explain(e, "creating the service"))?,
    };
    service
        .set_description("Serves the DIDComm MCP server over HTTP (https://github.com/wyvrn-cloud/mcp).")
        .map_err(|e| explain(e, "describing the service"))?;
    let restart = |secs| ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(secs) };
    service
        .update_failure_actions(ServiceFailureActions {
            reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(24 * 60 * 60)),
            reboot_msg: None,
            command: None,
            actions: Some(vec![restart(5), restart(30), restart(60)]),
        })
        .map_err(|e| explain(e, "setting the service's restart policy"))?;
    service
        .set_failure_actions_on_non_crash_failures(true)
        .map_err(|e| explain(e, "setting the service's restart policy"))?;
    service.start::<&str>(&[]).map_err(|e| explain(e, "starting the service"))?;

    let log = config.parent().map(|d| d.join("didcomm-mcp.log")).unwrap_or_default();
    Ok(format!(
        "Manage it from an Administrator PowerShell (or services.msc):\n\n  Get-Service {NAME}\n  Restart-Service {NAME}    # after editing the configuration\n  Get-Content -Wait \"{}\"    # logs",
        log.display()
    ))
}

fn stop_and_wait(service: &windows_service::service::Service) -> anyhow::Result<()> {
    let state = service.query_status().map_err(|e| explain(e, "querying the service"))?.current_state;
    if state == ServiceState::Stopped {
        return Ok(());
    }
    if state != ServiceState::StopPending {
        service.stop().map_err(|e| explain(e, "stopping the service"))?;
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while service.query_status().map_err(|e| explain(e, "querying the service"))?.current_state != ServiceState::Stopped {
        if Instant::now() > deadline {
            anyhow::bail!("the service didn't stop within 30 seconds");
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(())
}

pub fn uninstall() -> anyhow::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|e| explain(e, "opening the service manager"))?;
    let service = manager
        .open_service(NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE)
        .map_err(|e| explain(e, "opening the didcomm-mcp service (is it installed?)"))?;
    stop_and_wait(&service)?;
    service.delete().map_err(|e| explain(e, "removing the service"))?;
    Ok(())
}

/// Let only SYSTEM and Administrators into `dir` (it will hold the bearer token and the
/// agent's keys); `%ProgramData%` lets every user read by default.
pub fn restrict_to_administrators(dir: &Path) -> anyhow::Result<()> {
    // Well-known SIDs, so this works whatever the system language.
    let mut command = std::process::Command::new("icacls");
    command.arg(dir).args(["/inheritance:r", "/grant:r", "*S-1-5-18:(OI)(CI)F", "/grant:r", "*S-1-5-32-544:(OI)(CI)F"]);
    super::run(&mut command).context("restricting the configuration directory to Administrators")
}
