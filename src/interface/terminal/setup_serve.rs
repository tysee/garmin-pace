//! src/interface/terminal/setup_serve.rs runs onboarding step 10: keep MCP serving.
//! App::start_mcp checks once whether Pace already answers on the server config's listener,
//! then follows the supervisor: systemd status with a sudo start on Linux hosts, the
//! detached-container plus `podman exec` menu scheme in Podman, or foreground serving.

use super::{
    App,
    view::{Choice, Selection, Style},
};
use crate::{
    config::Settings,
    installation::{self, ServiceProbe, Supervisor, UNIT_NAME},
    interface::users,
};
use anyhow::{Result, bail};
use std::{path::Path, time::Duration};

/// How long a freshly started service may take before its listener answers.
const START_PROBES: u32 = 20;
const START_PROBE_INTERVAL: Duration = Duration::from_millis(500);

impl App {
    pub(super) fn start_mcp(&mut self, server: &Path) -> Result<()> {
        let settings = Settings::load(server)?;
        users::service(&settings)?;
        self.view.frame(
            "Start MCP",
            &[("Checking the MCP listener...".into(), Style::Plain)],
            "Local check",
        )?;
        let probe = installation::probe_service(&settings)?;
        let bind = settings.http.bind;
        if probe == ServiceProbe::OtherProgram {
            bail!(
                "another program answers on {bind}; stop it or change http.bind in {}",
                server.display()
            );
        }
        let running = probe == ServiceProbe::Running;
        match installation::detect_supervisor()? {
            Supervisor::Container { image } if running => {
                self.view.result("Start MCP", &format!("√ MCP is running in this container (listener {bind}).\nClosing this menu does not stop it.\n\nClients on this computer: http://127.0.0.1:PORT/api/v1/mcp with the header Authorization: Bearer <key>, where PORT is the host side of -p in your podman run command.\nImage: {}\nLogs: podman logs -f garmin-pace-dev\nStop: podman stop garmin-pace-dev", image.as_deref().unwrap_or("not recorded by Podman")))?;
            }
            Supervisor::Container { image } => self.container_scheme(server, &settings, image)?,
            Supervisor::Systemd => self.systemd_service(server, &settings, running)?,
            Supervisor::None if running => {
                self.view.result("Start MCP", &format!("√ Another Pace process already serves MCP on {bind}.\nNothing to start. Clients use http://{bind}/api/v1/mcp with Authorization: Bearer <key>."))?;
            }
            Supervisor::None => self.foreground(server, &settings)?,
        }
        Ok(())
    }

    fn foreground(&mut self, server: &Path, settings: &Settings) -> Result<()> {
        let details = format!(
            "Config: {}\nHTTP listener: {}\nMCP path: /api/v1/mcp\n\nThe menu will close. This process will serve until Ctrl-C. A valid issued key is required. Public access needs HTTPS; this action does not configure a proxy or install a service.\nFor serving without a terminal on Linux, install the service with deploy/install.sh.",
            server.display(),
            settings.http.bind
        );
        if self.confirm("Start MCP", "Close the menu and start HTTP MCP?", &details)? {
            self.pending_server = Some(server.to_path_buf());
        }
        Ok(())
    }

    /// In Podman the menu may be the container's main process, which takes MCP down with it.
    fn container_scheme(
        &mut self,
        server: &Path,
        settings: &Settings,
        image: Option<String>,
    ) -> Result<()> {
        let port = settings.http.bind.port();
        let image = image.unwrap_or_else(|| "localhost/garmin-pace:dev".into());
        let text = format!(
            "× MCP is not running in this container.\n\nThis menu is probably the container's main process: a container started with --rm -it stops, with everything in it, when the menu closes. Run the server in its own detached container and open this menu inside it instead.\n\n1. Leave this menu (Ctrl-C), then start the server; replace VOLUME with the volume from your current -v VOLUME:/data:\n\npodman run -d --name garmin-pace-dev \\\n  --read-only --cap-drop all \\\n  --security-opt no-new-privileges \\\n  --memory 128m --pids-limit 64 \\\n  -p 127.0.0.1:{port}:{port} \\\n  -v VOLUME:/data \\\n  {image} serve --transport http\n\n2. Open this menu in that container:\n\npodman exec -it garmin-pace-dev \\\n  /app/pace --config {} setup\n\nPrefix podman with `mise exec --` when Podman comes from mise. With the default volume, `mise run container:run` starts the server and `mise run container:menu` opens the main menu there, with Onboarding first. Keys issued in the menu work at once; closing the menu leaves MCP running. 127.0.0.1 keeps it private to this computer.",
            server.display()
        );
        self.view.result("Start MCP", &text)?;
        Ok(())
    }

    fn systemd_service(&mut self, server: &Path, settings: &Settings, running: bool) -> Result<()> {
        let state = installation::service_state(UNIT_NAME)?;
        let bind = settings.http.bind;
        let summary = format!("{UNIT_NAME}: {}, {} at boot", state.active, state.enabled);
        if running {
            self.view.result("Start MCP", &format!("√ MCP is running on {bind}.\n{summary}\nClosing this menu does not stop it.\n\nLogs: journalctl -u {UNIT_NAME} -f\nStop: sudo systemctl stop {UNIT_NAME}"))?;
            return Ok(());
        }
        let choices = [
            Choice::new(
                "Start the service with sudo",
                "Enable at boot and start now; sudo may ask for your password",
            ),
            Choice::new(
                "Run here until Ctrl-C",
                "Serve in this terminal instead; stops when it closes",
            ),
        ];
        if !self.view.result(
            "Start MCP",
            &format!("× MCP is not answering on {bind}.\n{summary}\n\nThe service keeps MCP running without a terminal and restarts it at boot.\n\nEnter: choose; Esc: back"),
        )? {
            return Ok(());
        }
        match self
            .view
            .select("Start MCP", &choices, &mut Selection::default(), false)?
        {
            Some(0) => self.start_with_sudo(settings, &summary),
            Some(1) => self.foreground(server, settings),
            _ => Ok(()),
        }
    }

    fn start_with_sudo(&mut self, settings: &Settings, before: &str) -> Result<()> {
        self.view.terminal.suspend()?;
        println!("Starting {UNIT_NAME} with sudo (it may ask for your password)...");
        let started = installation::start_service();
        self.view.terminal.resume()?;
        self.view.invalidate();
        started?;
        self.view.frame(
            "Start MCP",
            &[("Waiting for MCP to answer...".into(), Style::Plain)],
            "Local check",
        )?;
        for _ in 0..START_PROBES {
            if installation::probe_service(settings)? == ServiceProbe::Running {
                let state = installation::service_state(UNIT_NAME)?;
                self.view.result("Start MCP", &format!("√ MCP is running on {}.\n{UNIT_NAME}: {}, {} at boot (was: {before}).\nClosing this menu does not stop it.\n\nLogs: journalctl -u {UNIT_NAME} -f", settings.http.bind, state.active, state.enabled))?;
                return Ok(());
            }
            std::thread::sleep(START_PROBE_INTERVAL);
        }
        bail!(
            "{UNIT_NAME} was started but MCP does not answer on {}; inspect journalctl -u {UNIT_NAME}",
            settings.http.bind
        )
    }
}
