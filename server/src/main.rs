//! Local HTTP entrypoint for `taskveil-server`.
//!
//! Lambda-specific adapters stay outside this binary. The reusable router and
//! services live in the library crate.

use taskveil_server::{build_router_with_realtime, config::RuntimeConfig, db, AppState};
use tokio::signal;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    if run().await.is_err() {
        tracing::error!(event = "server_startup_failed");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), ()> {
    let config = RuntimeConfig::load().await.map_err(|_| ())?;
    let pool = db::connect_application(&config.database_url)
        .await
        .map_err(|_| ())?;

    let app = build_router_with_realtime(
        AppState {
            pool,
            billing: config.billing,
            auth_issuer: config.auth_issuer,
        },
        config.realtime,
    );
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|_| ())?;

    tracing::info!("taskveil-server listening on port {port}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|_| ())?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("shutdown signal received, shutting down gracefully");
}
