use std::path::PathBuf;
use std::sync::Arc;

use codeg_lib::app_state::AppState;
use codeg_lib::web::event_bridge::{EventEmitter, WebEventBroadcaster};
use codeg_lib::web::shutdown::ShutdownSignal;
use codeg_lib::web::{
    find_static_dir_standalone, generate_random_token, get_local_addresses, WebServerState,
};

#[derive(Debug, Default)]
struct CliArgs {
    /// --listen <host:port>: overrides CODEG_HOST / CODEG_PORT.
    listen: Option<String>,
    /// --bootstrap-stdio: daemon mode (handshake JSON on stdout, stdin EOF
    /// triggers graceful shutdown, chat_channel background tasks skipped).
    bootstrap_stdio: bool,
    /// --version / -V
    print_version: bool,
    /// --help / -h
    print_help: bool,
}

const HELP_TEXT: &str = r#"codeg-server — codeg backend (self-hosted or remote daemon)

USAGE:
    codeg-server [OPTIONS]

OPTIONS:
    --listen <host:port>     Bind address. Overrides CODEG_HOST / CODEG_PORT.
                             Use 127.0.0.1:0 for OS-assigned port (daemon mode).
    --bootstrap-stdio        Daemon mode: write a one-line JSON handshake to
                             stdout after bind, then watch stdin for EOF and
                             exit gracefully when the parent closes the pipe.
                             Implies a random token (CODEG_TOKEN ignored).
    --version, -V            Print version and exit.
    --help, -h               Print this help and exit.

ENV (used when corresponding CLI flag is absent):
    CODEG_PORT               Listen port (default 3080)
    CODEG_HOST               Listen host (default 0.0.0.0)
    CODEG_TOKEN              Bearer token (default: random; ignored in daemon mode)
    CODEG_DATA_DIR           SQLite + cache directory
    CODEG_STATIC_DIR         Frontend static export root
"#;

fn parse_args(argv: &[String]) -> Result<CliArgs, String> {
    let mut cli = CliArgs::default();
    let mut i = 1;
    while i < argv.len() {
        let arg = &argv[i];
        match arg.as_str() {
            "--version" | "-V" => cli.print_version = true,
            "--help" | "-h" => cli.print_help = true,
            "--bootstrap-stdio" => cli.bootstrap_stdio = true,
            "--listen" => {
                i += 1;
                if i >= argv.len() {
                    return Err("--listen requires <host:port> argument".into());
                }
                cli.listen = Some(argv[i].clone());
            }
            other if other.starts_with("--listen=") => {
                cli.listen = Some(other.trim_start_matches("--listen=").to_string());
            }
            other => return Err(format!("Unknown argument: {}", other)),
        }
        i += 1;
    }
    Ok(cli)
}

fn parse_listen_spec(spec: &str) -> Result<(String, u16), String> {
    let (host, port_str) = if let Some(stripped) = spec.strip_prefix('[') {
        let end = stripped
            .find(']')
            .ok_or("missing closing ']' in IPv6 literal")?;
        let host = format!("[{}]", &stripped[..end]);
        let rest = &stripped[end + 1..];
        let port_str = rest
            .strip_prefix(':')
            .ok_or("missing ':port' after IPv6 literal")?;
        (host, port_str.to_string())
    } else {
        let (h, p) = spec
            .rsplit_once(':')
            .ok_or("missing ':' in listen spec")?;
        (h.to_string(), p.to_string())
    };
    let port: u16 = port_str
        .parse()
        .map_err(|e| format!("invalid port: {}", e))?;
    Ok((host, port))
}

fn resolve_listen_config(cli: &CliArgs) -> (String, u16, String) {
    let (host, port) = if let Some(spec) = &cli.listen {
        parse_listen_spec(spec).unwrap_or_else(|e| {
            eprintln!("error: invalid --listen value '{}': {}", spec, e);
            std::process::exit(2);
        })
    } else {
        let host = std::env::var("CODEG_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
        let port: u16 = std::env::var("CODEG_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3080);
        (host, port)
    };

    // Daemon mode forces a fresh random token to avoid CODEG_TOKEN leaking
    // through SSH env into stderr or process listings.
    let token = if cli.bootstrap_stdio {
        generate_random_token()
    } else {
        std::env::var("CODEG_TOKEN").unwrap_or_else(|_| generate_random_token())
    };

    (host, port, token)
}

fn spawn_stdin_watchdog(shutdown: Arc<ShutdownSignal>) {
    use tokio::io::{AsyncReadExt, BufReader};
    tokio::spawn(async move {
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut buf = [0u8; 1024];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) => {
                    eprintln!("[DAEMON] stdin EOF, initiating graceful shutdown");
                    shutdown.trigger();
                    return;
                }
                Ok(_) => {
                    // Tolerate any data the parent writes; daemon does not
                    // expect stdin commands today.
                    continue;
                }
                Err(e) => {
                    eprintln!("[DAEMON] stdin read error: {}, shutting down", e);
                    shutdown.trigger();
                    return;
                }
            }
        }
    });
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let cli = match parse_args(&argv) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {}", e);
            eprintln!("\n{}", HELP_TEXT);
            std::process::exit(2);
        }
    };

    if cli.print_help {
        println!("{}", HELP_TEXT);
        return;
    }
    if cli.print_version {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return;
    }

    // PATH initialisation MUST happen before the tokio runtime is created.
    // std::env::set_var is not thread-safe (unsafe in Rust edition 2024);
    // #[tokio::main] would spawn worker threads before we reach this point.
    codeg_lib::process::ensure_node_in_path();
    codeg_lib::process::ensure_user_npm_prefix_in_path();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build tokio runtime")
        .block_on(async_main(cli));
}

async fn async_main(cli: CliArgs) {
    let bootstrap_mode = cli.bootstrap_stdio;

    // Sweep stale ACP binary cache trash (rename-aside fallback artifacts).
    // Detached OS thread: cannot block startup, panics are caught and dropped,
    // errors are silenced, no subprocesses spawned.
    std::thread::spawn(|| {
        let _ = std::panic::catch_unwind(|| {
            codeg_lib::sweep_acp_binary_trash();
        });
    });

    let (host, port, token) = resolve_listen_config(&cli);
    let data_dir = std::env::var("CODEG_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_data_dir());
    let static_dir_env = std::env::var("CODEG_STATIC_DIR").ok();

    let static_dir = find_static_dir_standalone(static_dir_env.as_deref());
    let app_version = env!("CARGO_PKG_VERSION");

    if bootstrap_mode {
        eprintln!(
            "[DAEMON] codeg-server v{} starting (bootstrap mode)",
            app_version
        );
    } else {
        eprintln!("[SERVER] codeg-server v{}", app_version);
        eprintln!("[SERVER] Data directory: {}", data_dir.display());
        eprintln!("[SERVER] Static directory: {}", static_dir.display());
    }

    // Initialize database
    let db = codeg_lib::db::init_database(&data_dir, app_version)
        .await
        .expect("Failed to initialize database");

    // Create shared broadcaster
    let broadcaster = Arc::new(WebEventBroadcaster::new());
    let emitter = EventEmitter::WebOnly(broadcaster.clone());

    // Build AppState
    let state = Arc::new(AppState {
        db,
        connection_manager: codeg_lib::app_state::default_connection_manager(),
        terminal_manager: codeg_lib::app_state::default_terminal_manager(),
        event_broadcaster: broadcaster,
        emitter,
        data_dir,
        web_server_state: WebServerState::new(),
        chat_channel_manager: codeg_lib::app_state::default_chat_channel_manager(),
        remote_connections: codeg_lib::app_state::default_remote_connection_manager(),
    });

    // Bootstrap mode skips chat_channel background tasks and bundled experts
    // installation: a remote daemon spawned over SSH should not own webhook
    // subscriptions, schedule chat messages, or write to the user's central
    // skills store. Those responsibilities stay with the user's desktop /
    // self-hosted server. See dev-design CG-002.3 §2.6.
    if !bootstrap_mode {
        // Install bundled expert skills into the central store
        // (`~/.codeg/skills/`). Runs in the background; failures are logged
        // but non-fatal.
        tokio::spawn(async move {
            let report = codeg_lib::commands::experts::ensure_central_experts_installed().await;
            if !report.errors.is_empty() {
                eprintln!(
                    "[Experts] install finished with {} error(s): {:?}",
                    report.errors.len(),
                    report.errors
                );
            } else {
                eprintln!(
                    "[Experts] install ok: installed={} updated={} pending_review={}",
                    report.installed_count,
                    report.updated_count,
                    report.pending_user_review.len()
                );
            }
        });

        // Start chat channel background tasks (event subscriber, command dispatcher,
        // scheduler, auto-connect)
        state
            .chat_channel_manager
            .start_background(
                state.event_broadcaster.clone(),
                state.db.conn.clone(),
                state.connection_manager.clone_ref(),
                state.emitter.clone(),
            )
            .await;
    }

    // Spawn the LifecycleSubscriber for cross-connection DB writes.
    tokio::spawn(codeg_lib::lifecycle_subscriber_task(
        state.db.conn.clone(),
        state.connection_manager.clone_ref(),
        state.event_broadcaster.clone(),
    ));

    // Spawn the idle sweep so connections abandoned without an explicit
    // disconnect (e.g. browser tab closed, panic survivors) are reaped.
    // Override the 60-second default via `CODEG_ACP_IDLE_TIMEOUT_SECS`
    // (set to `0` to disable).
    if let Some(idle_timeout) = codeg_lib::idle_timeout_from_env() {
        tokio::spawn(codeg_lib::idle_sweep_task(
            state.connection_manager.clone_ref(),
            idle_timeout,
            std::time::Duration::from_secs(codeg_lib::SWEEP_INTERVAL_SECS),
        ));
    }

    // Build router
    let shutdown_signal = state.web_server_state.shutdown_signal();
    let router = codeg_lib::web::router::build_router(
        state.clone(),
        token.clone(),
        static_dir,
        shutdown_signal.clone(),
    );

    // Bind
    let addr = format!("{}:{}", host, port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("[SERVER] Failed to bind {}: {}", addr, e);
            std::process::exit(1);
        });

    if let Err(e) = codeg_lib::web::socket_inherit::mark_listener_non_inheritable(&listener) {
        eprintln!(
            "[SERVER][WARN] failed to mark listener non-inheritable: {}",
            e
        );
    }

    let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);

    // Publish runtime state so the settings page (served by us) shows
    // the truth — running on `actual_port` with this token — instead of
    // the placeholder "stopped" that triggers the stale-port banner.
    state
        .web_server_state
        .mark_externally_running(actual_port, token.clone());

    if bootstrap_mode {
        let handshake = codeg_lib::web::bootstrap::BootstrapHandshake::new(
            app_version,
            actual_port,
            &token,
        );
        if let Err(e) = handshake.write_to_stdout() {
            eprintln!("[DAEMON] failed to write bootstrap handshake: {}", e);
            std::process::exit(1);
        }
        // NOTE: the literal token is intentionally NOT logged on stderr in
        // bootstrap mode. SSH parents commonly route stderr through journald
        // / sshd logs; a leaked token there would persist on disk.
        eprintln!(
            "[DAEMON] bootstrap sent on stdout, listening on 127.0.0.1:{}",
            actual_port
        );

        // Parent (SSH pipe) closing stdin → graceful shutdown. Daemon mode
        // implicitly opts in; self-hosted mode does not, so systemd-style
        // detached supervision still works there.
        spawn_stdin_watchdog(shutdown_signal.clone());
    } else {
        let addresses = get_local_addresses(actual_port);
        eprintln!("[SERVER] Token: {}", token);
        eprintln!("[SERVER] Listening on:");
        for a in &addresses {
            eprintln!("  {}", a);
        }
    }

    // Start serving with graceful shutdown so both modes can react to the
    // shared signal: in self-hosted mode the runtime triggers it on exit,
    // in daemon mode the stdin watchdog triggers it on EOF.
    let serve_shutdown = shutdown_signal.clone();
    if let Err(e) = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            serve_shutdown.wait().await;
        })
        .await
    {
        eprintln!("[SERVER] Server error: {}", e);
        std::process::exit(1);
    }
}

fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .map(|d| d.join("codeg"))
        .unwrap_or_else(|| PathBuf::from(".codeg-data"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_listen_ipv4() {
        let (h, p) = parse_listen_spec("127.0.0.1:0").unwrap();
        assert_eq!(h, "127.0.0.1");
        assert_eq!(p, 0);
    }

    #[test]
    fn parse_listen_ipv4_real_port() {
        let (h, p) = parse_listen_spec("0.0.0.0:3080").unwrap();
        assert_eq!(h, "0.0.0.0");
        assert_eq!(p, 3080);
    }

    #[test]
    fn parse_listen_ipv6() {
        let (h, p) = parse_listen_spec("[::1]:8080").unwrap();
        assert_eq!(h, "[::1]");
        assert_eq!(p, 8080);
    }

    #[test]
    fn parse_listen_rejects_missing_port() {
        assert!(parse_listen_spec("127.0.0.1").is_err());
    }

    #[test]
    fn parse_listen_rejects_bad_port() {
        assert!(parse_listen_spec("127.0.0.1:abc").is_err());
        assert!(parse_listen_spec("127.0.0.1:99999").is_err());
    }

    #[test]
    fn parse_listen_rejects_unclosed_ipv6() {
        assert!(parse_listen_spec("[::1:8080").is_err());
    }

    #[test]
    fn parse_args_defaults_empty() {
        let cli = parse_args(&args(&["codeg-server"])).unwrap();
        assert!(!cli.bootstrap_stdio);
        assert!(!cli.print_version);
        assert!(!cli.print_help);
        assert!(cli.listen.is_none());
    }

    #[test]
    fn parse_args_combo() {
        let cli = parse_args(&args(&[
            "codeg-server",
            "--listen",
            "127.0.0.1:0",
            "--bootstrap-stdio",
        ]))
        .unwrap();
        assert_eq!(cli.listen.as_deref(), Some("127.0.0.1:0"));
        assert!(cli.bootstrap_stdio);
    }

    #[test]
    fn parse_args_listen_eq_form() {
        let cli = parse_args(&args(&["codeg-server", "--listen=127.0.0.1:9999"])).unwrap();
        assert_eq!(cli.listen.as_deref(), Some("127.0.0.1:9999"));
    }

    #[test]
    fn parse_args_version_short_long() {
        assert!(parse_args(&args(&["codeg-server", "-V"])).unwrap().print_version);
        assert!(parse_args(&args(&["codeg-server", "--version"])).unwrap().print_version);
    }

    #[test]
    fn parse_args_help_short_long() {
        assert!(parse_args(&args(&["codeg-server", "-h"])).unwrap().print_help);
        assert!(parse_args(&args(&["codeg-server", "--help"])).unwrap().print_help);
    }

    #[test]
    fn parse_args_unknown_fails() {
        assert!(parse_args(&args(&["codeg-server", "--unknown"])).is_err());
    }

    #[test]
    fn parse_args_listen_without_value_fails() {
        assert!(parse_args(&args(&["codeg-server", "--listen"])).is_err());
    }
}
