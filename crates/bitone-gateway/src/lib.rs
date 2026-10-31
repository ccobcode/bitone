//! BitOne 网关.无状态:不存储用户数据,不鉴权,只对接链上数据.
//! 接口在 api,各条链的适配在 chains,上游连接在 upstream.
//!
//! 隐私约束:这里不依赖任何日志框架,只有 metrics 模块的聚合行和启动提示会写到 stderr.
//! 新增日志时不要写入地址,路径,IP,请求体和上游错误文本.

mod api;
mod cache;
mod chains;
pub mod config;
mod error;
mod follow;
mod prices;
mod upstream;

use api::AppState;
use config::Config;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::time::{sleep, timeout};

const METRICS_PERIOD: Duration = Duration::from_secs(60);
/// 退出时等待在途请求和 WebSocket 收尾的上限,分别计时.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// 运行网关,直到 `shutdown` 完成,然后优雅退出.
pub async fn serve(
    config: Config,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> io::Result<()> {
    let state = Arc::new(AppState::new(&config)?);
    let app = api::router(state.clone(), config.web_dir.as_deref());

    let metrics = state.clone();
    let flush = tokio::spawn(async move {
        loop {
            sleep(METRICS_PERIOD).await;
            metrics.metrics.flush();
        }
    });

    let mut closing = state.closing.subscribe();
    let signal = {
        let state = state.clone();
        async move {
            shutdown.await;
            state.closing.send_replace(true);
        }
    };
    let server =
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).with_graceful_shutdown(signal);
    tokio::select! {
        result = server.into_future() => result?,
        // 卡住的连接不能阻止退出.
        _ = async {
            let _ = closing.wait_for(|closing| *closing).await;
            sleep(SHUTDOWN_GRACE).await;
        } => {}
    }
    // axum 不管已升级的连接,这里等 WebSocket 发完关闭帧.
    drop(closing);
    let _ = timeout(SHUTDOWN_GRACE, state.closing.closed()).await;
    flush.abort();
    state.metrics.flush();
    Ok(())
}
