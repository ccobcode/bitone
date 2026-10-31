//! 用法：bitone-gateway [配置文件]，默认读当前目录的 gateway.toml。

use bitone_gateway::config::Config;
use std::error::Error;
use std::path::Path;
use std::process::ExitCode;
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};

async fn run(path: &str) -> Result<(), Box<dyn Error>> {
    let config = Config::load(Path::new(path))?;
    let mut terminate = signal(SignalKind::terminate())?;
    let listener = TcpListener::bind(config.listen).await?;
    eprintln!("bitone-gateway 监听 {}", listener.local_addr()?);
    let shutdown = async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    };
    bitone_gateway::serve(config, listener, shutdown).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let path = match args.next().as_deref() {
        Some("--config" | "-c") => args.next(),
        other => other.map(String::from),
    };
    match run(path.as_deref().unwrap_or("gateway.toml")).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
