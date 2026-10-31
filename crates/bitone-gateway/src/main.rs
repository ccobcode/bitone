//! 用法:bitone-gateway [--config 文件] [--listen 地址] [--web-dir 目录] [--trusted-proxy IP]
//! 配置文件默认是当前目录的 gateway.toml,命令行的值优先.

use bitone_gateway::config::{Config, Options};
use std::error::Error;
use std::process::ExitCode;
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};

async fn run() -> Result<(), Box<dyn Error>> {
    let config = Config::load(Options::parse(std::env::args().skip(1))?)?;
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
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
