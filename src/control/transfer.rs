//! 文件传输连接：在调用方的任务上用一条临时连接完成一个请求（归档、恢复、升级、作业与回放数据的上传下载）。
//!
//! 半双工往返，不需要 FIFO 或读任务：连接、写出、读到响应、关闭。不走主连接的原因：传输几百 MB 期间主连接上的
//! 其他请求与通知都要排队；服务端的响应队列有 100 MB 上限，超大响应若被丢弃，主连接的 FIFO 随之错位。另开连接与
//! 提前关闭连接分别是协议假设 T1、T2。

use std::io::ErrorKind;
use std::time::Duration;

use super::client::{CONNECT_TIMEOUT, Client, encode};
use super::{HEADER_LEN, Inbound, Request, Response};
use crate::error::{self, DisconnectReason, Error, Result};
use crate::frame::{FrameError, FrameReader};
use crate::net::{self, MAX_FRAME};

/// 文件连接的帧长上限：64 位 `u32::MAX`（带回放数据的备份可能超过 1 GiB，服务端只拒绝超过 `u32::MAX` 的帧），
/// 32 位与其他连接相同。
const MAX_TRANSFER: u32 = if usize::BITS >= 64 {
    u32::MAX
} else {
    MAX_FRAME
};

impl Client {
    /// 在新连接上发送 `request` 并等待响应。`idle` 是连续没有收到任何字节的时长上限，不限制总时长。
    ///
    /// 下载的 `Bin` 直接引用响应帧，峰值内存约为文件大小；上传的 `Bin` 编码时复制进帧，峰值约为两倍，写出之后即
    /// 释放（服务端处理恢复、升级可能要几分钟，等待期间不占着上传的数据）。超时或连接出错时连接即丢弃，不影响主连接。
    /// 变更请求（恢复、升级、上传）结束后计入连接的写计数（资源缓存据此失效），无论成败：服务端可能已经执行。
    pub async fn transfer(&self, request: Request, idle: Duration) -> Result<Response> {
        let method = request.method;
        let result = self.exchange(request, idle).await;
        self.shared.count_write(method);
        result
    }

    /// 连接、写出 `request`、读到第一条响应。
    async fn exchange(&self, request: Request, idle: Duration) -> Result<Response> {
        let frame = encode(&request)?;
        let Request {
            method,
            path,
            payload,
            ..
        } = request;
        drop(payload);
        let mut stream = net::connect(self.addr(), CONNECT_TIMEOUT).await?;
        let disconnected = |reason: DisconnectReason| Error::Disconnected(reason.into());
        net::write_all(&mut stream, &frame)
            .await
            .map_err(|e| disconnected(DisconnectReason::Write(e)))?;
        drop(frame);
        let mut frames = FrameReader::new(HEADER_LEN, MAX_TRANSFER);
        loop {
            let frame = match net::read_frame(&mut stream, &mut frames, idle).await {
                Ok(Some(frame)) => frame,
                Ok(None) => return Err(disconnected(DisconnectReason::Eof)),
                Err(FrameError::Io(e)) if e.kind() == ErrorKind::TimedOut => {
                    return Err(Error::Timeout {
                        method,
                        path,
                        after: idle,
                    });
                }
                Err(e) => return Err(disconnected(e.into())),
            };
            // 这条连接上没有订阅与流，第一条响应就是应答。
            match Inbound::decode(frame) {
                Ok(Inbound::Response(response)) => return error::check(method, path, response),
                Ok(_) => {}
                Err(e) => return Err(disconnected(e.into())),
            }
        }
    }
}
