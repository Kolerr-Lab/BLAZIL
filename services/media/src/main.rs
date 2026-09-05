use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod codec;
mod config;
mod error;
mod server;
mod session;
mod stt;
mod tts;
mod turn;
mod turn_detector;
mod twilio;
mod vad;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "blazil_media=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cfg = config::Config::from_env()?;
    tracing::info!("Starting Blazil Media Plane on {}", cfg.bind_addr);

    server::serve(cfg).await?;

    Ok(())
}
