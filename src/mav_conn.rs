use crate::mavlink::{open, MavError, Receiver, Sender};
use crossbeam_channel::{bounded, Receiver as CbReceiver, Sender as CbSender};
use mavlink::common::{
    MavAutopilot, MavMessage, MavModeFlag, MavState, MavType, HEARTBEAT_DATA,
    REQUEST_DATA_STREAM_DATA,
};
use mavlink::MavHeader;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

// ========== 实例锁（用于自动分配ID）==========

/// 实例锁，用于自动分配ID
pub struct InstanceLock {
    pub id: u8,
    _file: File,
    _path: PathBuf,
}

impl InstanceLock {
    pub fn acquire(app_name: &str) -> Result<Self, String> {
        let lock_dir = std::env::temp_dir().join(format!("{}_locks", app_name));
        std::fs::create_dir_all(&lock_dir).map_err(|e| format!("创建锁目录失败: {}", e))?;

        for id in 1..=255u8 {
            let lock_path = lock_dir.join(format!("instance_{}.lock", id));

            match OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&lock_path)
            {
                Ok(file) => {
                    if try_lock_exclusive(&file) {
                        use std::io::Write;
                        let mut f = &file;
                        let _ = writeln!(f, "{}", std::process::id());

                        return Ok(InstanceLock {
                            id,
                            _file: file,
                            _path: lock_path,
                        });
                    }
                }
                Err(_) => continue,
            }
        }

        Err("无法获取实例ID，已达到最大实例数(255)".to_string())
    }
}

#[cfg(unix)]
fn try_lock_exclusive(file: &File) -> bool {
    use std::os::unix::io::AsRawFd;
    let fd = file.as_raw_fd();
    let mut flock = libc::flock {
        l_type: libc::F_WRLCK as i16,
        l_whence: libc::SEEK_SET as i16,
        l_start: 0,
        l_len: 0,
        #[cfg(target_os = "linux")]
        l_pid: 0,
        #[cfg(target_os = "macos")]
        l_pid: 0,
    };
    unsafe { libc::fcntl(fd, libc::F_SETLK, &mut flock) == 0 }
}

#[cfg(windows)]
fn try_lock_exclusive(file: &File) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };

    let handle = file.as_raw_handle() as HANDLE;
    let mut overlapped = unsafe { std::mem::zeroed() };

    unsafe {
        LockFileEx(
            handle,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &mut overlapped,
        ) != 0
    }
}

// ========== 代理模式类型 ==========

/// 发送回调函数类型
pub type ProxyTxFn = Arc<dyn Fn(MavHeader, &MavMessage) + Send + Sync + 'static>;

/// 接收代理 Sender - 外部往这里塞消息
pub type ProxyRxSender = CbSender<(MavHeader, MavMessage)>;

// ========== 全局代理配置 ==========

static PROXY_TX_CB: OnceCell<ProxyTxFn> = OnceCell::new();
static PROXY_SEQ: AtomicU8 = AtomicU8::new(0);
/// 直接给内部程序用的 recv_out sender
static PROXY_RECV_OUT: OnceCell<CbSender<(MavHeader, MavMessage)>> = OnceCell::new();
/// 直接给内部程序用的 send_queue receiver
static PROXY_SEND_QUEUE: OnceCell<CbReceiver<SendItem>> = OnceCell::new();
/// target 信息
static PROXY_TARGET: OnceCell<TargetInfo> = OnceCell::new();
/// 代理模式的 self_system_id（新增）
static PROXY_SELF_SYSTEM_ID: AtomicU8 = AtomicU8::new(255);
/// 代理模式的 self_component_id（新增）
static PROXY_SELF_COMPONENT_ID: AtomicU8 = AtomicU8::new(0);

/// 设置代理发送回调（在 connect 之前调用）
pub fn set_proxy_tx_callback<F>(f: F)
where
    F: Fn(MavHeader, &MavMessage) + Send + Sync + 'static,
{
    let _ = PROXY_TX_CB.set(Arc::new(f));
}

/// 代理模式：外部调用这个函数塞收到的消息（直接转发，无中间层）
pub fn proxy_push_message(header: MavHeader, msg: MavMessage) {
    // 学习 target
    if let MavMessage::HEARTBEAT(hb) = &msg {
        if hb.mavtype != MavType::MAV_TYPE_GCS {
            if let Some(target) = PROXY_TARGET.get() {
                target.update(header.system_id, header.component_id);
            }
        }
    }

    // 直接塞到 recv_out，无中间 channel
    if let Some(sender) = PROXY_RECV_OUT.get() {
        let _ = sender.try_send((header, msg));
    }
}

/// 代理模式：启动发送处理线程（处理内部程序发出的消息）
fn start_proxy_send_loop() {
    std::thread::Builder::new()
        .name("mav-proxy-tx".to_string())
        .spawn(|| {
            let send_queue = PROXY_SEND_QUEUE.get().expect("send queue");
            let tx_callback = PROXY_TX_CB.get().expect("tx callback");

            loop {
                // 阻塞等待，不用 sleep
                match send_queue.recv() {
                    Ok(item) => {
                        let (header, msg) = match item {
                            SendItem::WithHeader(h, m) => (h, m),
                            SendItem::Message(m) => {
                                let seq = PROXY_SEQ.fetch_add(1, Ordering::Relaxed);
                                let sys = PROXY_SELF_SYSTEM_ID.load(Ordering::Relaxed);
                                let comp = PROXY_SELF_COMPONENT_ID.load(Ordering::Relaxed);
                                (
                                    MavHeader {
                                        system_id: sys,
                                        component_id: comp,
                                        sequence: seq,
                                    },
                                    m,
                                )
                            }
                        };
                        tx_callback(header, &msg);
                    }
                    Err(_) => break,
                }
            }
        })
        .expect("spawn proxy tx thread");
}

// ========== 发送项（支持带 header 或不带）==========

#[derive(Clone)]
enum SendItem {
    Message(MavMessage),
    WithHeader(MavHeader, MavMessage),
}

// ========== 数据流订阅配置 ==========

#[derive(Debug, Clone, Deserialize)]
pub struct StreamSubscription {
    #[serde(default)]
    pub stream_id: u8,
    #[serde(default = "default_rate")]
    pub rate_hz: u16,
}

fn default_rate() -> u16 {
    4
}

impl Default for StreamSubscription {
    fn default() -> Self {
        Self {
            stream_id: 0,
            rate_hz: 4,
        }
    }
}

impl StreamSubscription {
    pub fn all(rate_hz: u16) -> Self {
        Self {
            stream_id: 0,
            rate_hz,
        }
    }
    pub fn raw_sensors(rate_hz: u16) -> Self {
        Self {
            stream_id: 1,
            rate_hz,
        }
    }
    pub fn extended_status(rate_hz: u16) -> Self {
        Self {
            stream_id: 2,
            rate_hz,
        }
    }
    pub fn rc_channels(rate_hz: u16) -> Self {
        Self {
            stream_id: 3,
            rate_hz,
        }
    }
    pub fn position(rate_hz: u16) -> Self {
        Self {
            stream_id: 6,
            rate_hz,
        }
    }
    pub fn extra1(rate_hz: u16) -> Self {
        Self {
            stream_id: 10,
            rate_hz,
        }
    }
    pub fn extra2(rate_hz: u16) -> Self {
        Self {
            stream_id: 11,
            rate_hz,
        }
    }
    pub fn extra3(rate_hz: u16) -> Self {
        Self {
            stream_id: 12,
            rate_hz,
        }
    }
}

// ========== 配置 ==========

/// 连接模式
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ConnMode {
    #[default]
    Direct, // 直连串口/UDP/TCP
    Proxy, // 代理模式
}

#[derive(Debug, Clone, Deserialize)]
pub struct MavConfig {
    /// 实例ID（None表示自动分配）
    #[serde(default)]
    pub id: Option<u8>,
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(default)]
    pub addr1: String,
    #[serde(default)]
    pub addr2: String,
    /// 连接模式: "direct" 或 "proxy"
    #[serde(default)]
    pub mode: ConnMode,
    /// 重连间隔 (ms) - 仅在 socket 出错时使用
    #[serde(default = "default_reconnect")]
    pub reconnect_ms: u64,
    /// 心跳发送间隔 (ms)
    #[serde(default = "default_heartbeat")]
    pub heartbeat_ms: u64,
    /// 本机 system_id
    #[serde(default = "default_self_system_id")]
    pub self_system_id: u8,
    /// 本机 component_id
    #[serde(default = "default_self_component_id")]
    pub self_component_id: u8,
    /// 数据流订阅
    #[serde(default = "default_subscriptions")]
    pub subscriptions: Vec<StreamSubscription>,
    /// 订阅发送间隔 (ms) - 周期性发送，保证 PX4 重启后自动恢复
    #[serde(default = "default_subscription_interval")]
    pub subscription_interval_ms: u64,
}

fn default_name() -> String {
    "MAV".to_string()
}
fn default_reconnect() -> u64 {
    1000
}
fn default_heartbeat() -> u64 {
    1000
}
fn default_self_system_id() -> u8 {
    255
}
fn default_self_component_id() -> u8 {
    0
}
fn default_subscriptions() -> Vec<StreamSubscription> {
    vec![StreamSubscription::all(4)]
}
fn default_subscription_interval() -> u64 {
    2000
}

// ========== 传输类型 ==========

#[derive(Debug, Clone, Copy, PartialEq)]
enum TransportType {
    Serial,
    Udp,
    Tcp,
}

/// 判断地址是否是 UDP 类型
fn is_udp_addr(addr: &str) -> bool {
    addr.starts_with("udp") // 匹配 udp:, udpout:, udpin:
}

/// 判断地址是否是 TCP 类型
fn is_tcp_addr(addr: &str) -> bool {
    addr.starts_with("tcp") // 匹配 tcp:, tcpout:, tcpin:
}

/// 检测传输类型
fn detect_transport_type(addr: &str) -> TransportType {
    if addr.starts_with("serial:") {
        TransportType::Serial
    } else if is_udp_addr(addr) {
        TransportType::Udp
    } else if is_tcp_addr(addr) {
        TransportType::Tcp
    } else {
        TransportType::Serial // 默认按串口处理
    }
}

/// 给 UDP 地址的本地端口和远程端口都加上偏移量
/// 支持格式: "udp:ip:remote_port-local_port"
fn offset_udp_ports(addr: &str, offset: u16) -> String {
    // 只处理 udp 开头的地址
    if !addr.starts_with("udp:") {
        return addr.to_string();
    }

    // 格式: udp:ip:remote_port-local_port
    // 例如: udp:10.1.2.91:1030-1030
    if let Some(dash_pos) = addr.rfind('-') {
        let before_dash = &addr[..dash_pos];
        let local_port_str = &addr[dash_pos + 1..];

        // 解析 local_port
        if let Ok(local_port) = local_port_str.parse::<u16>() {
            // 找 remote_port (最后一个冒号后面的部分)
            if let Some(colon_pos) = before_dash.rfind(':') {
                let prefix = &before_dash[..colon_pos + 1];
                let remote_port_str = &before_dash[colon_pos + 1..];

                if let Ok(remote_port) = remote_port_str.parse::<u16>() {
                    return format!("{}{}-{}", prefix, remote_port + offset, local_port + offset);
                }
            }
        }
    }

    addr.to_string()
}

/// 给 TCP 地址的端口加上偏移量
/// 支持格式: "tcpout:ip:port" 或 "tcpin:port" 或 "tcp:port"
fn offset_tcp_ports(addr: &str, offset: u16) -> String {
    // 只处理 tcp 开头的地址
    if !addr.starts_with("tcp") {
        return addr.to_string();
    }

    // 提取协议部分和参数部分
    let parts: Vec<&str> = addr.splitn(2, ':').collect();
    if parts.len() < 2 {
        return addr.to_string();
    }

    let protocol = parts[0]; // tcp/tcpout/tcpin
    let params = parts[1];

    match protocol {
        "tcpout" => {
            // 格式: tcpout:ip:port
            // 例如: tcpout:127.0.0.1:5760
            let param_parts: Vec<&str> = params.rsplitn(2, ':').collect();
            if param_parts.len() == 2 {
                if let Ok(port) = param_parts[0].parse::<u16>() {
                    return format!("tcpout:{}:{}", param_parts[1], port + offset);
                }
            }
        }
        "tcpin" | "tcp" => {
            // 格式: tcpin:port 或 tcp:port
            // 例如: tcpin:5760 或 tcp:5760
            if let Ok(port) = params.parse::<u16>() {
                return format!("{}:{}", protocol, port + offset);
            }
        }
        _ => {}
    }

    addr.to_string()
}

impl MavConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let s = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        toml::from_str(&s).map_err(|e| e.to_string())
    }

    pub fn new(
        name: impl Into<String>,
        addr1: impl Into<String>,
        addr2: impl Into<String>,
    ) -> Self {
        Self {
            id: None,
            name: name.into(),
            addr1: addr1.into(),
            addr2: addr2.into(),
            mode: ConnMode::Direct,
            reconnect_ms: default_reconnect(),
            heartbeat_ms: default_heartbeat(),
            self_system_id: default_self_system_id(),
            self_component_id: default_self_component_id(),
            subscriptions: default_subscriptions(),
            subscription_interval_ms: default_subscription_interval(),
        }
    }

    /// 根据实例ID递增 self_system_id 和端口（UDP/TCP）
    /// 实例1: self_system_id 不变，端口不变
    /// 实例2: self_system_id + 1，端口 + 1
    /// 实例N: self_system_id + (N-1)，端口 + (N-1)
    pub fn with_instance_offset(&mut self, instance_id: u8) {
        let offset = (instance_id - 1) as u8;

        // 递增 self_system_id
        if let Some(new_id) = self.self_system_id.checked_add(offset) {
            self.self_system_id = new_id;
        }

        // 递增 UDP 和 TCP 端口
        if offset > 0 {
            self.addr1 = offset_udp_ports(&self.addr1, offset as u16);
            self.addr1 = offset_tcp_ports(&self.addr1, offset as u16);

            self.addr2 = offset_udp_ports(&self.addr2, offset as u16);
            self.addr2 = offset_tcp_ports(&self.addr2, offset as u16);
        }
    }

    pub fn with_reconnect(mut self, ms: u64) -> Self {
        self.reconnect_ms = ms;
        self
    }
    pub fn with_heartbeat(mut self, ms: u64) -> Self {
        self.heartbeat_ms = ms;
        self
    }
    pub fn with_mode(mut self, mode: ConnMode) -> Self {
        self.mode = mode;
        self
    }
    pub fn with_self_id(mut self, sys: u8, comp: u8) -> Self {
        self.self_system_id = sys;
        self.self_component_id = comp;
        self
    }
    pub fn with_subscriptions(mut self, subs: Vec<StreamSubscription>) -> Self {
        self.subscriptions = subs;
        self
    }
    pub fn with_subscription_interval(mut self, ms: u64) -> Self {
        self.subscription_interval_ms = ms;
        self
    }
}

// ========== 目标信息 ==========

#[derive(Clone)]
pub struct TargetInfo {
    pub system_id: Arc<AtomicU8>,
    pub component_id: Arc<AtomicU8>,
}

impl TargetInfo {
    fn new() -> Self {
        Self {
            system_id: Arc::new(AtomicU8::new(1)),
            component_id: Arc::new(AtomicU8::new(1)),
        }
    }

    pub fn get(&self) -> (u8, u8) {
        (
            self.system_id.load(Ordering::Relaxed),
            self.component_id.load(Ordering::Relaxed),
        )
    }

    fn update(&self, sys: u8, comp: u8) {
        self.system_id.store(sys, Ordering::Relaxed);
        self.component_id.store(comp, Ordering::Relaxed);
    }
}

// ========== 心跳 ==========

fn make_heartbeat(sys: u8, comp: u8) -> (MavHeader, MavMessage) {
    (
        MavHeader {
            system_id: sys,
            component_id: comp,
            sequence: 0,
        },
        MavMessage::HEARTBEAT(HEARTBEAT_DATA {
            custom_mode: 0,
            mavtype: MavType::MAV_TYPE_GCS,
            autopilot: MavAutopilot::MAV_AUTOPILOT_INVALID,
            base_mode: MavModeFlag::empty(),
            system_status: MavState::MAV_STATE_ACTIVE,
            mavlink_version: 3,
        }),
    )
}

// ========== 订阅请求 ==========

async fn send_subscriptions(
    tx: &mut Sender<MavMessage>,
    subs: &[StreamSubscription],
    target: &TargetInfo,
    name: &str,
) {
    let (sys, comp) = target.get();
    for sub in subs {
        let msg = MavMessage::REQUEST_DATA_STREAM(REQUEST_DATA_STREAM_DATA {
            target_system: sys,
            target_component: comp,
            req_stream_id: sub.stream_id,
            req_message_rate: sub.rate_hz,
            start_stop: 1,
        });
        if let Err(e) = tx.send(&msg).await {
            println!("[{}] 订阅发送失败: {}", name, e);
            return;
        }
    }
}

// ========== 发送/接收端 ==========

#[derive(Clone)]
pub struct MavTx {
    tx: CbSender<SendItem>,
    connected: Arc<AtomicBool>,
    running: Arc<AtomicBool>,  // 控制connection_loop退出
    target: TargetInfo,
}

impl MavTx {
    /// 发送消息（使用默认 header）
    pub fn send(&self, msg: MavMessage) -> Result<(), &'static str> {
        self.tx
            .send(SendItem::Message(msg))
            .map_err(|_| "channel closed")
    }

    /// 原样发送（与 send 相同，保留兼容）
    pub fn send_raw(&self, msg: MavMessage) -> Result<(), &'static str> {
        self.tx
            .send(SendItem::Message(msg))
            .map_err(|_| "channel closed")
    }

    /// 带自定义 header 发送
    pub fn send_with_header(&self, header: MavHeader, msg: MavMessage) -> Result<(), &'static str> {
        self.tx
            .send(SendItem::WithHeader(header, msg))
            .map_err(|_| "channel closed")
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    pub fn target(&self) -> (u8, u8) {
        self.target.get()
    }

    /// 关闭连接，停止connection_loop线程
    pub fn shutdown(&self) {
        self.running.store(false, Ordering::Relaxed);
        self.connected.store(false, Ordering::Relaxed);
    }
}

pub struct MavRx {
    rx: CbReceiver<(MavHeader, MavMessage)>,
    connected: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
}

impl MavRx {
    pub fn recv(&self) -> Option<(MavHeader, MavMessage)> {
        self.rx.recv().ok()
    }

    pub fn try_recv(&self) -> Option<(MavHeader, MavMessage)> {
        self.rx.try_recv().ok()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Option<(MavHeader, MavMessage)> {
        self.rx.recv_timeout(timeout).ok()
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// 关闭连接，停止connection_loop线程
    pub fn shutdown(&self) {
        self.running.store(false, Ordering::Relaxed);
        self.connected.store(false, Ordering::Relaxed);
    }
}

// ========== 连接 ==========

/// 创建 MAVLink 连接
///
/// 根据配置的 mode 自动选择连接方式：
/// - Direct: 直连串口/UDP/TCP
/// - Proxy: 通过全局回调代理（需先调用 set_proxy_tx_callback）
pub fn connect(cfg: MavConfig) -> (MavTx, MavRx) {
    match cfg.mode {
        ConnMode::Direct => connect_direct(cfg),
        ConnMode::Proxy => connect_proxy_internal(cfg),
    }
}

fn connect_direct(cfg: MavConfig) -> (MavTx, MavRx) {
    // unbounded channel - 在接收端处理堆积问题
    let (out_tx, out_rx) = crossbeam_channel::unbounded::<(MavHeader, MavMessage)>();
    let (in_tx, in_rx) = bounded::<SendItem>(16);
    let connected = Arc::new(AtomicBool::new(false));
    let running = Arc::new(AtomicBool::new(true));
    let target = TargetInfo::new();

    let conn = connected.clone();
    let run = running.clone();
    let tgt = target.clone();

    std::thread::Builder::new()
        .name(format!("mav-{}", cfg.name))
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(connection_loop(cfg, in_rx, out_tx, conn, run, tgt));
        })
        .expect("spawn thread");

    (
        MavTx {
            tx: in_tx,
            connected: connected.clone(),
            running: running.clone(),
            target: target.clone(),
        },
        MavRx {
            rx: out_rx,
            connected,
            running,
        },
    )
}

/// 连接循环
///
/// 简单逻辑：
/// 1. 打开 socket
/// 2. 周期性发心跳 + 周期性发订阅
/// 3. PX4 无论何时重启，几秒内自动恢复
/// 4. 只有 socket 出错才重连
/// 5. running为false时退出
async fn connection_loop(
    cfg: MavConfig,
    send_queue: CbReceiver<SendItem>,
    recv_out: CbSender<(MavHeader, MavMessage)>,
    connected: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    target: TargetInfo,
) {
    let addrs = [&cfg.addr1, &cfg.addr2];
    let name = &cfg.name;
    let mut idx = 0;

    while running.load(Ordering::Relaxed) {
        let addr = addrs[idx];
        println!("[{}] 连接 {}", name, addr);

        match open::<MavMessage>(addr).await {
            Ok((mut tx, mut rx)) => {
                connected.store(true, Ordering::Relaxed);
                println!("[{}] 已连接", name);

                // 关键修复：设置 self_system_id 和 self_component_id
                tx.set_self_id(cfg.self_system_id, cfg.self_component_id);
                println!(
                    "[{}] 设置 self_id: system={}, component={}",
                    name, cfg.self_system_id, cfg.self_component_id
                );

                // 检测传输类型
                let transport = detect_transport_type(addr);
                run_io(
                    &mut tx,
                    &mut rx,
                    &send_queue,
                    &recv_out,
                    &cfg,
                    name,
                    &target,
                    transport,
                    &running,
                )
                    .await;

                // run_io 返回说明 socket 出错或被关闭
                connected.store(false, Ordering::Relaxed);

                // 如果是主动关闭，直接退出，不打印"连接断开"
                if !running.load(Ordering::Relaxed) {
                    println!("[{}] 连接已关闭", name);
                    break;
                }

                println!("[{}] 连接断开", name);

                // 显式 drop，释放串口资源
                drop(tx);
                drop(rx);

                // 只有串口才需要等待 USB 重新枚举
                if transport == TransportType::Serial {
                    println!("[{}] 等待设备重新枚举...", name);
                    tokio::time::sleep(Duration::from_millis(6000)).await;
                }
            }
            Err(e) => {
                println!("[{}] 地址{}连接失败: {}", name, idx, e);
                idx = (idx + 1) % 2;
            }
        }

        tokio::time::sleep(Duration::from_millis(cfg.reconnect_ms)).await;
    }
}

/// 通信循环
///
/// 核心逻辑：
/// - 周期性发心跳（让 PX4 知道我在）
/// - 周期性发订阅（让 PX4 知道给我发什么）
/// - PX4 重启？没关系，几秒内自动收到订阅请求，恢复数据流
/// - 串口/TCP：socket 出错才返回
/// - UDP：永不返回，IO 错误只是对端未响应
/// - running为false时退出
async fn run_io(
    tx: &mut Sender<MavMessage>,
    rx: &mut Receiver<MavMessage>,
    send_queue: &CbReceiver<SendItem>,
    recv_out: &CbSender<(MavHeader, MavMessage)>,
    cfg: &MavConfig,
    name: &str,
    target: &TargetInfo,
    transport: TransportType,
    running: &Arc<AtomicBool>,
) {
    let mut last_heartbeat = Instant::now() - Duration::from_secs(10); // 立即发第一个心跳
    let mut last_subscription = Instant::now(); // 订阅不要立即发
    let mut target_learned = false; // 是否已学习到目标
    let mut peer_responding = true; // UDP：对端是否在响应（边沿触发用）

    // UDP 是无连接协议，其他都是面向连接的
    let is_connectionless = transport == TransportType::Udp;

    while running.load(Ordering::Relaxed) {
        // 周期性发心跳（heartbeat_ms > 0时才发送）
        if cfg.heartbeat_ms > 0 && last_heartbeat.elapsed().as_millis() as u64 >= cfg.heartbeat_ms {
            let (h, m) = make_heartbeat(cfg.self_system_id, cfg.self_component_id);
            if let Err(e) = tx.send_with_header(&h, &m).await {
                if is_connectionless {
                    // UDP 发送失败也继续，不退出
                    if peer_responding {
                        println!("[{}] 心跳发送失败: {} (UDP继续)", name, e);
                        peer_responding = false;
                    }
                } else {
                    // 串口/TCP 发送失败需要重连
                    println!("[{}] 心跳发送失败: {}", name, e);
                    return;
                }
            }
            last_heartbeat = Instant::now();
        }

        // 周期性发订阅
        // 关键：必须先收到飞控心跳（学习到 target）后才发订阅
        if target_learned
            && cfg.subscription_interval_ms > 0
            && last_subscription.elapsed().as_millis() as u64 >= cfg.subscription_interval_ms
        {
            send_subscriptions(tx, &cfg.subscriptions, target, name).await;
            last_subscription = Instant::now();
        }

        // 发送用户消息
        // 检测send_queue是否已关闭（发送端被drop）
        loop {
            match send_queue.try_recv() {
                Ok(item) => {
                    let result = match item {
                        SendItem::Message(msg) => tx.send(&msg).await,
                        SendItem::WithHeader(header, msg) => tx.send_with_header(&header, &msg).await,
                    };
                    if let Err(e) = result {
                        if is_connectionless {
                            // UDP 发送失败继续
                            if peer_responding {
                                println!("[{}] 发送失败: {} (UDP继续)", name, e);
                                peer_responding = false;
                            }
                        } else {
                            println!("[{}] 发送失败: {}", name, e);
                            return;
                        }
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    // 队列为空，正常情况，退出内层循环继续接收
                    break;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    // 发送端被drop，说明用户断开了连接，退出connection_loop
                    println!("[{}] 发送队列已关闭，断开连接", name);
                    return;
                }
            }
        }

        // 接收
        match rx.recv_timeout(50).await {
            Ok((h, m)) => {
                // 收到消息，对端在响应
                if !peer_responding {
                    println!("[{}] 连接恢复", name);
                    peer_responding = true;
                }

                // 从飞控心跳学习目标 ID
                if let MavMessage::HEARTBEAT(hb) = &m {
                    if hb.mavtype != MavType::MAV_TYPE_GCS {
                        let old = target.get();
                        if old.0 != h.system_id || old.1 != h.component_id {
                            println!("[{}] 目标: {}:{}", name, h.system_id, h.component_id);
                            target.update(h.system_id, h.component_id);
                        }

                        // 首次收到飞控心跳，立即发订阅
                        if !target_learned {
                            target_learned = true;
                            println!("[{}] 首次收到飞控心跳，发送订阅", name);
                            send_subscriptions(tx, &cfg.subscriptions, target, name).await;
                            last_subscription = Instant::now();
                        }
                    }
                }

                // unbounded channel，直接发送
                let _ = recv_out.send((h, m));
            }
            Err(MavError::Timeout) => {
                // 没收到包，继续
            }
            Err(MavError::Parse(_e)) => {
                // 解析错误 - mavlink 库不认识某些新消息，忽略继续
                // 不要断开连接
                // println!("mavlink解析错误:{}",_e);
            }
            Err(MavError::Io(e)) => {
                if is_connectionless {
                    // UDP 的 ICMP 错误（如 "远程主机强迫关闭了一个现有的连接"）不应该断开
                    // 边沿触发：只在状态变化时打印
                    if peer_responding {
                        println!("[{}] 对端未响应: {}", name, e);
                        peer_responding = false;
                    }
                    // 继续循环，不 return
                } else {
                    // 串口/TCP 真的需要重连
                    println!("[{}] IO错误: {}", name, e);
                    return;
                }
            }
        }
    }
}

// ========== 便捷构造 ==========

impl MavConfig {
    pub fn px4_sitl(name: impl Into<String>) -> Self {
        Self::new(name, "udpout:127.0.0.1:14540", "udpout:127.0.0.1:14540")
    }

    pub fn serial(name: impl Into<String>, port: &str, baud: u32) -> Self {
        let addr = format!("serial:{}:{}", port, baud);
        Self::new(name, &addr, &addr)
    }

    /// 创建 TCP 客户端配置
    pub fn tcp_client(name: impl Into<String>, host: &str, port: u16) -> Self {
        let addr = format!("tcpout:{}:{}", host, port);
        Self::new(name, &addr, &addr)
    }

    /// 创建 TCP 服务端配置
    pub fn tcp_server(name: impl Into<String>, port: u16) -> Self {
        let addr = format!("tcpin:{}", port);
        Self::new(name, &addr, &addr)
    }

    /// 创建代理模式配置（不需要地址）
    pub fn proxy(name: impl Into<String>) -> Self {
        Self {
            id: None,
            name: name.into(),
            addr1: String::new(),
            addr2: String::new(),
            mode: ConnMode::Proxy,
            reconnect_ms: default_reconnect(),
            heartbeat_ms: default_heartbeat(),
            self_system_id: default_self_system_id(),
            self_component_id: default_self_component_id(),
            subscriptions: default_subscriptions(),
            subscription_interval_ms: default_subscription_interval(),
        }
    }
}

// ========== 代理模式 ==========

/// 代理模式内部连接（无中间线程，直接转发）
fn connect_proxy_internal(cfg: MavConfig) -> (MavTx, MavRx) {
    // 创建 channel
    let (out_tx, out_rx) = bounded::<(MavHeader, MavMessage)>(64);
    let (in_tx, in_rx) = bounded::<SendItem>(16);
    let connected = Arc::new(AtomicBool::new(true));
    let running = Arc::new(AtomicBool::new(true));
    let target = TargetInfo::new();

    // 关键修复：设置代理模式的 self_system_id 和 self_component_id
    PROXY_SELF_SYSTEM_ID.store(cfg.self_system_id, Ordering::Relaxed);
    PROXY_SELF_COMPONENT_ID.store(cfg.self_component_id, Ordering::Relaxed);
    println!(
        "[{}] 代理模式设置 self_id: system={}, component={}",
        cfg.name, cfg.self_system_id, cfg.self_component_id
    );

    // 设置全局变量
    let _ = PROXY_RECV_OUT.set(out_tx);
    let _ = PROXY_SEND_QUEUE.set(in_rx);
    let _ = PROXY_TARGET.set(target.clone());

    // 启动发送处理线程
    start_proxy_send_loop();

    println!("[{}] 代理模式启动（直接转发）", cfg.name);

    (
        MavTx {
            tx: in_tx,
            connected: connected.clone(),
            running: running.clone(),
            target: target.clone(),
        },
        MavRx {
            rx: out_rx,
            connected,
            running,
        },
    )
}

// ============ MavConn 封装结构体（类似 PadConn）============

pub struct MavConn {
    id: u8,
    pub config: MavConfig,
    tx: MavTx,
    rx: MavRx,
    _instance_lock: Option<InstanceLock>,
}

impl MavConn {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self::new_with_app_name(path, "mav_app")
    }

    pub fn new_with_app_name(path: impl AsRef<Path>, app_name: &str) -> Self {
        let mut config = MavConfig::load(path).unwrap();

        let (id, instance_lock) = match config.id {
            Some(configured_id) => (configured_id, None),
            None => {
                let lock = InstanceLock::acquire(app_name).expect("获取实例ID失败");
                let id = lock.id;
                // 自动递增 self_system_id 和 UDP/TCP 端口
                config.with_instance_offset(id);
                println!(
                    "[MAV] 实例#{} | self_system_id:{} | addr:{}",
                    id, config.self_system_id, config.addr1
                );
                (id, Some(lock))
            }
        };

        let (tx, rx) = connect(config.clone());
        Self {
            id,
            config,
            tx,
            rx,
            _instance_lock: instance_lock,
        }
    }

    /// 从配置创建，指定应用名称和基准 system_id
    pub fn new_with_base_system_id(
        path: impl AsRef<Path>,
        app_name: &str,
        base_system_id: u8,
    ) -> Self {
        let mut config = MavConfig::load(path).unwrap();
        config.self_system_id = base_system_id;

        let (id, instance_lock) = match config.id {
            Some(configured_id) => (configured_id, None),
            None => {
                let lock = InstanceLock::acquire(app_name).expect("获取实例ID失败");
                let id = lock.id;
                config.with_instance_offset(id);
                println!(
                    "[MAV] 实例#{} | self_system_id:{} | addr:{}",
                    id, config.self_system_id, config.addr1
                );
                (id, Some(lock))
            }
        };

        let (tx, rx) = connect(config.clone());
        Self {
            id,
            config,
            tx,
            rx,
            _instance_lock: instance_lock,
        }
    }

    pub fn id(&self) -> u8 {
        self.id
    }

    pub fn config(&self) -> &MavConfig {
        &self.config
    }

    pub fn tx(&self) -> MavTx {
        self.tx.clone()
    }

    pub fn recv(&self) -> Option<(MavHeader, MavMessage)> {
        self.rx.recv()
    }

    pub fn try_recv(&self) -> Option<(MavHeader, MavMessage)> {
        self.rx.try_recv()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Option<(MavHeader, MavMessage)> {
        self.rx.recv_timeout(timeout)
    }

    pub fn send(&self, msg: MavMessage) -> Result<(), &'static str> {
        self.tx.send(msg)
    }

    pub fn send_raw(&self, msg: MavMessage) -> Result<(), &'static str> {
        self.tx.send_raw(msg)
    }

    pub fn send_with_header(&self, header: MavHeader, msg: MavMessage) -> Result<(), &'static str> {
        self.tx.send_with_header(header, msg)
    }

    pub fn is_connected(&self) -> bool {
        self.tx.is_connected()
    }

    pub fn target(&self) -> (u8, u8) {
        self.tx.target()
    }

    /// 获取当前 self_system_id
    pub fn self_system_id(&self) -> u8 {
        self.config.self_system_id
    }

    /// 获取当前 self_component_id
    pub fn self_component_id(&self) -> u8 {
        self.config.self_component_id
    }

    /// 关闭连接
    pub fn shutdown(&self) {
        self.tx.shutdown();
    }
}